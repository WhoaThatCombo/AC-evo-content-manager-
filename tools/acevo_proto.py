"""Load the recovered AC EVO protobuf schemas into a usable message factory.

dump_protos.py writes raw FileDescriptorProto blobs pulled straight out of the
client. Feeding those into a DescriptorPool gives working message classes with
no protoc step and no hand-written .proto files.

    import acevo_proto
    m = acevo_proto.new("RegisterResponse")
    m.is_registered = True
"""
import glob
import os

from google.protobuf import descriptor_pb2, descriptor_pool, message_factory

# ACECM_PROTOS lets a packaged build point at schemas extracted on the
# user's own machine, since we do not redistribute Kunos' descriptors.
PROTOS = os.environ.get("ACECM_PROTOS") or os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "protos")

pool = descriptor_pool.DescriptorPool()
loaded, failed = [], {}


def _load_all():
    blobs = {}
    for path in glob.glob(os.path.join(PROTOS, "*.desc")):
        fdp = descriptor_pb2.FileDescriptorProto()
        fdp.ParseFromString(open(path, "rb").read())
        blobs[fdp.name] = fdp

    # Fixed point: keep trying everything still pending until a whole pass adds
    # nothing. Simpler and more robust than computing dependency order by hand,
    # and it tolerates descriptors we couldn't recover.
    pending = dict(blobs)
    while pending:
        progressed = False
        for name in list(pending):
            try:
                pool.Add(pending[name])
                loaded.append(name)
                del pending[name]
                progressed = True
            except Exception as ex:
                failed[name] = str(ex)[:140]
        if not progressed:
            break
    for name in pending:
        failed.setdefault(name, "unresolved dependencies")
    for name in loaded:
        failed.pop(name, None)


_load_all()

# protobuf >= 5 still HAS MessageFactory, it just dropped GetPrototype - so
# test for the function rather than for the class, or the wrong branch is taken.
if hasattr(message_factory, "GetMessageClass"):
    def _cls(desc):
        return message_factory.GetMessageClass(desc)
else:
    _factory = message_factory.MessageFactory(pool)
    def _cls(desc):
        return _factory.GetPrototype(desc)


def new(type_name):
    """Instantiate a message by name, e.g. new('RegisterResponse')."""
    return _cls(pool.FindMessageTypeByName(type_name))()


def cls(type_name):
    return _cls(pool.FindMessageTypeByName(type_name))


def has(type_name):
    try:
        pool.FindMessageTypeByName(type_name)
        return True
    except KeyError:
        return False


if __name__ == "__main__":
    print(f"loaded {len(loaded)} / {len(loaded) + len(failed)} descriptors")
    if failed:
        print("\nnot loaded:")
        for k, v in list(failed.items())[:10]:
            print(f"  {k:42s} {v[:90]}")
    print("\nmessages needed for a backend:")
    for t in ("BackendMessage", "BackendResponse", "RegisterRequest", "RegisterResponse",
              "MultiplayerServerListRequestServerList",
              "MultiplayerServerListResponseServerList",
              "MultiplayerServerListRequestServerEntry",
              "MultiplayerServerListResponseServerEntry",
              "MultiplayerServerListRequestConnectToServer",
              "MultiplayerServerListResponseConnectToServer",
              "MultiplayerServerListEntry"):
        print(f"  {'OK ' if has(t) else '-- '}{t}")

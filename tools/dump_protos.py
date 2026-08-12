"""Recover .proto definitions from the AC EVO binaries.

Generated protobuf C++ embeds a serialised FileDescriptorProto for every .proto
it compiles. Finding those blobs gives exact message definitions - field names,
numbers and types - instead of guessing them from the wire.

A blob starts with field 1 (name) of the FileDescriptorProto:
    0x0a <varint len> "<Something>.proto"
The end isn't marked, so the length is found by growing the slice until
FileDescriptorProto.ParseFromString stops improving - descriptors parse
greedily and trailing junk makes it throw.

    python dump_protos.py                 dump every descriptor found
    python dump_protos.py Backend         only those whose name matches
"""
import os
import re
import sys

from google.protobuf import descriptor_pb2

EXE = os.environ.get(
    "TARGET",
    r"C:\Program Files (x86)\Steam\steamapps\common\Assetto Corsa EVO\AssettoCorsaEVO.exe")
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "protos")


def find_blobs(d):
    """Yield (offset, name) for every embedded FileDescriptorProto."""
    # [\s\S] rather than . : a name of exactly 10 characters has a length byte
    # of 0x0a, which IS a newline, and "." does not match it. That silently hid
    # Math.proto - and with it every descriptor that depends on Math.proto.
    for m in re.finditer(rb"\x0a([\s\S])([A-Za-z0-9_/]{3,60}\.proto)", d):
        ln = m.group(1)[0]
        name = m.group(2)
        if ln == len(name):
            yield m.start(), name.decode()


def _varint(d, i):
    r = s = 0
    while True:
        b = d[i]; i += 1
        r |= (b & 0x7F) << s
        if not b & 0x80:
            return r, i
        s += 7


def blob_len(d, off, limit=1 << 22):
    """Exact length, by walking top-level fields until the wire stops making
    sense. Probing by powers overshoots the true size and the parse then fails,
    which is why guessing sizes missed most descriptors."""
    i, end = off, min(len(d), off + limit)
    known = {1, 2, 3, 4, 5, 6, 7, 8, 9, 12}      # FileDescriptorProto fields
    while i < end:
        start = i
        try:
            key, i = _varint(d, i)
        except Exception:
            return start - off
        fn, wt = key >> 3, key & 7
        if fn not in known or wt not in (0, 2):
            return start - off
        if wt == 0:
            _, i = _varint(d, i)
        else:
            ln, i = _varint(d, i)
            i += ln
        if i > end:
            return start - off
    return end - off


def extract(d, off):
    """Parse the descriptor using its exact on-wire length."""
    n = blob_len(d, off)
    for size in (n, n - 1, n - 2):               # tolerate a trailing byte
        if size < 8:
            continue
        fdp = descriptor_pb2.FileDescriptorProto()
        try:
            fdp.ParseFromString(bytes(d[off:off + size]))
            if fdp.name:
                return size, fdp
        except Exception:
            pass
    return None


def main():
    filt = sys.argv[1] if len(sys.argv) > 1 else ""
    d = open(EXE, "rb").read()
    os.makedirs(OUT, exist_ok=True)
    seen, ok = set(), 0
    for off, name in find_blobs(d):
        if name in seen or (filt and filt.lower() not in name.lower()):
            continue
        seen.add(name)
        got = extract(d, off)
        if not got:
            print(f"  {name:45s} at {off:#x}  FAILED to parse")
            continue
        size, fdp = got
        ok += 1
        msgs = [m.name for m in fdp.message_type]
        print(f"  {name:45s} {size:6d}B  {len(msgs):3d} messages")
        base = os.path.join(OUT, name.replace("/", "_"))
        with open(base + ".txt", "w", encoding="utf-8") as f:
            f.write(str(fdp))
        # raw bytes too: these feed a DescriptorPool directly, so no protoc and
        # no hand-written .proto files are needed to speak the protocol.
        with open(base + ".desc", "wb") as f:
            f.write(bytes(d[off:off + size]))
    print(f"\n{ok} descriptors written to {OUT}")


if __name__ == "__main__":
    main()

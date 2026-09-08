"""Name physics-struct offsets by matching loader source offsets to a protobuf
message layout.

Protobuf C++ lays scalar fields out in FIELD-NUMBER order, 4 bytes each,
starting at 0x10 (verified against CarAssistsValues and
RacingSettingsAssistPreset via their ByteSizeLong). So for an all-4-byte-scalar
message we know offset -> field name analytically.

A loader function copies `[proto + X] -> [physics + Y]`. Matching its set of X
values against a message's expected offsets identifies both the message and,
via the pairs, names every destination offset Y.

    python name_fields.py <exe> <outdir>
"""
import json, os, re, struct, sys
from collections import Counter
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

sys.path.insert(0, r'C:\Projects\acecm')
from acecm import protos                                  # noqa: E402
from google.protobuf.descriptor import FieldDescriptor as FD   # noqa: E402

EXE, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)

FOUR = {FD.TYPE_FLOAT, FD.TYPE_INT32, FD.TYPE_UINT32, FD.TYPE_ENUM,
        FD.TYPE_SINT32, FD.TYPE_FIXED32, FD.TYPE_SFIXED32}


def repeated(f):
    try:
        return f.is_repeated
    except AttributeError:
        return False


# ---- expected layouts for all-4-byte-scalar messages ----
layouts = {}
for n in protos.message_names():
    try:
        m = protos.new(n)
    except Exception:
        continue
    fs = list(m.DESCRIPTOR.fields)
    if not fs or any(f.message_type is not None or repeated(f) or
                     f.type not in FOUR for f in fs):
        continue
    fs.sort(key=lambda f: f.number)
    layouts[n] = {0x10 + 4 * i: (f.name, f.type) for i, f in enumerate(fs)}
print('messages with a computable all-4-byte layout: %d' % len(layouts))

# ---- loaders ----
d = open(EXE, 'rb').read()
pe = struct.unpack_from('<I', d, 0x3C)[0]
ns = struct.unpack_from('<H', d, pe + 6)[0]
osz = struct.unpack_from('<H', d, pe + 20)[0]
ib = struct.unpack_from('<Q', d, pe + 24 + 24)[0]
S = []
for i in range(ns):
    o = pe + 24 + osz + 40 * i
    nm = d[o:o+8].rstrip(b'\0').decode('latin1')
    vs, va, rs, raw = struct.unpack_from('<IIII', d, o + 8)
    S.append((nm, va, vs, raw, rs))


def va2off(va):
    r = va - ib
    for nm, sva, vs, raw, rs in S:
        if sva <= r < sva + max(vs, rs):
            x = r - sva
            if x < rs:
                return raw + x
    return None


pdt = [x for x in S if x[0] == '.pdata'][0]
_, pva, pvs, praw, prs = pdt
funcs = []
for i in range(prs // 12):
    b, e, u = struct.unpack_from('<III', d, praw + 12 * i)
    if b and e > b:
        funcs.append((b, e))

md = Cs(CS_ARCH_X86, CS_MODE_64)
md.detail = False
LD = re.compile(r'^(xmm[0-9]+), dword ptr \[(r[a-z0-9]+) \+ (0x[0-9a-f]+)\]$')
STO = re.compile(r'^dword ptr \[(r[a-z0-9]+) \+ (0x[0-9a-f]+)\], (xmm[0-9]+)$')

loaders = []
for b, e in funcs:
    size = e - b
    if size > 60000 or size < 40:
        continue
    off = va2off(ib + b)
    if off is None:
        continue
    pend, pairs = {}, []
    for x in md.disasm(d[off:off + size], ib + b):
        if x.mnemonic in ('vmovss', 'movss'):
            m = LD.match(x.op_str)
            if m:
                pend[m.group(1)] = (m.group(2), int(m.group(3), 16))
                continue
            m = STO.match(x.op_str)
            if m and m.group(3) in pend:
                sr, so = pend[m.group(3)]
                if sr != m.group(1):
                    pairs.append((sr, so, m.group(1), int(m.group(2), 16)))
    if len(pairs) >= 8:
        loaders.append((ib + b, size, pairs))
print('loader-shaped functions: %d' % len(loaders))

# ---- match ----
results = {}
for fva, size, pairs in loaders:
    c = Counter((p[0], p[2]) for p in pairs)
    for (sr, dr), _ in c.most_common(3):
        sel = [p for p in pairs if p[0] == sr and p[2] == dr]
        if len(sel) < 8:
            continue
        srcs = {p[1] for p in sel}
        for msg, lay in layouts.items():
            inter = srcs & set(lay)
            if len(inter) < 8:
                continue
            score = len(inter) / len(srcs)
            if score < 0.85:
                continue
            # the loader uses vmovss, so every offset it reads must be a FLOAT
            # field in the true message. The float/int pattern is the only
            # message-specific signal - offsets alone are identical for every
            # all-4-byte message and match everything.
            if any(lay[o][1] != FD.TYPE_FLOAT for o in inter):
                continue
            floats = {o for o, (nm2, t) in lay.items() if t == FD.TYPE_FLOAT}
            if len(inter) < 0.8 * len(floats):
                continue
            prev = results.get(msg)
            if prev and prev['matched'] >= len(inter):
                continue
            results[msg] = {
                'loader': hex(fva), 'src_reg': sr, 'dst_reg': dr,
                'matched': len(inter), 'of_src': len(srcs),
                'coverage_of_message': round(len(inter) / len(lay), 3),
                'fields': {hex(p[3]): lay[p[1]][0] for p in sel if p[1] in lay},
            }

print('messages matched to a loader: %d' % len(results))
for msg, r in sorted(results.items(), key=lambda kv: -kv[1]['matched'])[:20]:
    print('  %-34s %3d/%-3d src matched  cov %.2f  loader %s  -> %d named'
          % (msg, r['matched'], r['of_src'], r['coverage_of_message'],
             r['loader'], len(r['fields'])))
json.dump(results, open(os.path.join(OUT, 'named_fields.json'), 'w'), indent=1)
print('written ->', os.path.join(OUT, 'named_fields.json'))

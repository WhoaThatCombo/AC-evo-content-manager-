"""Find struct-copy / loader functions: read [srcReg+X] -> write [dstReg+Y].

A protobuf -> physics loader is a long run of such pairs. Those pairs are the
bridge that lets a named .proto field name a raw physics offset.

    python find_loaders.py <exe> [min_pairs]
"""
import re, struct, sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

EXE = sys.argv[1]
MINP = int(sys.argv[2]) if len(sys.argv) > 2 else 12
d = open(EXE, 'rb').read()
pe = struct.unpack_from('<I', d, 0x3C)[0]
ns = struct.unpack_from('<H', d, pe + 6)[0]
osz = struct.unpack_from('<H', d, pe + 20)[0]
ib = struct.unpack_from('<Q', d, pe + 24 + 24)[0]
S = []
for i in range(ns):
    o = pe + 24 + osz + 40 * i
    n = d[o:o+8].rstrip(b'\0').decode('latin1')
    vs, va, rs, raw = struct.unpack_from('<IIII', d, o + 8)
    S.append((n, va, vs, raw, rs))


def va2off(va):
    r = va - ib
    for n, sva, vs, raw, rs in S:
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

out = []
for b, e in funcs:
    size = e - b
    if size > 60000 or size < 40:
        continue
    off = va2off(ib + b)
    if off is None:
        continue
    ins = list(md.disasm(d[off:off + size], ib + b))
    pending = {}
    pairs = []
    for x in ins:
        if x.mnemonic in ('vmovss', 'movss'):
            m = LD.match(x.op_str)
            if m:
                pending[m.group(1)] = (m.group(2), int(m.group(3), 16))
                continue
            m = STO.match(x.op_str)
            if m and m.group(3) in pending:
                sreg, soff = pending[m.group(3)]
                if sreg != m.group(1):
                    pairs.append((sreg, soff, m.group(1), int(m.group(2), 16)))
    if len(pairs) >= MINP:
        # dominant (src,dst) register pairing
        from collections import Counter
        c = Counter((p[0], p[2]) for p in pairs)
        (sr, dr), n = c.most_common(1)[0]
        sel = [p for p in pairs if p[0] == sr and p[2] == dr]
        out.append((ib + b, size, sr, dr, len(pairs), len(sel), sel))

out.sort(key=lambda r: -r[5])
print('loader-shaped functions (>=%d pairs): %d' % (MINP, len(out)))
for fva, size, sr, dr, np_, ns_, sel in out[:25]:
    lo = min(p[3] for p in sel)
    hi = max(p[3] for p in sel)
    print('  %x size %-6d %s->%s pairs %-3d (dominant %-3d) dst 0x%x..0x%x'
          % (fva, size, sr, dr, np_, ns_, lo, hi))

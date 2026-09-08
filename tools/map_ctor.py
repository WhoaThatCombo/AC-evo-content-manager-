"""Map a C++ class's fields from its constructor.

Every `mov/vmovss [this + off], <imm>` in a ctor is a field with its default.
Run over PhysicsEngine::PhysicsEngine (1422941a0) to get the layout for free.

    python map_ctor.py <hex ctor VA> [this_reg]
"""
import re, struct, sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

EXE = r'C:/Program Files (x86)/Steam/steamapps/common/Assetto Corsa EVO/AssettoCorsaEVO.exe'
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


def cstr(va, limit=120):
    o = va2off(va)
    if o is None:
        return None
    e = d.find(b'\0', o, o + limit)
    if e < 0:
        return None
    s = d[o:e]
    if len(s) < 4 or not all(9 <= c < 127 for c in s):
        return None
    return s.decode('latin1')


import bisect
pdt = [x for x in S if x[0] == '.pdata'][0]
_, pva, pvs, praw, prs = pdt
st, en = [], []
for i in range(prs // 12):
    b, e, u = struct.unpack_from('<III', d, praw + 12 * i)
    if b and e > b:
        st.append(b); en.append(e)
o = sorted(range(len(st)), key=lambda i: st[i])
st = [st[i] for i in o]; en = [en[i] for i in o]

CTOR = int(sys.argv[1], 16)
THIS = sys.argv[2] if len(sys.argv) > 2 else 'rbx'
rva = CTOR - ib
i = bisect.bisect_right(st, rva) - 1
lo, hi = st[i], en[i]
print('ctor %x .. %x  (size %d), this-reg assumed %s'
      % (ib + lo, ib + hi, hi - lo, THIS))

md = Cs(CS_ARCH_X86, CS_MODE_64)
md.detail = False
off0 = va2off(ib + lo)
STORE = re.compile(r'^(?:[a-z]+ ptr )?\[' + THIS + r' \+ (0x[0-9a-f]+)\], (.+)$')
rows = []
for ins in md.disasm(d[off0:off0 + (hi - lo)], ib + lo):
    m = STORE.match(ins.op_str)
    if not m:
        continue
    off = int(m.group(1), 16)
    val = m.group(2)
    note = ''
    if val.startswith('0x'):
        try:
            iv = int(val, 16)
            if 0x30000000 < iv < 0x50000000:
                note = ' = %g (float)' % struct.unpack('<f', struct.pack('<I', iv))[0]
            else:
                note = ' = %d' % iv
        except ValueError:
            pass
    rows.append((off, ins.address, ins.mnemonic, val, note))

rows.sort()
print('%d field stores' % len(rows))
for off, a, mn, val, note in rows:
    print('  +0x%-5x  %-8s %-22s%s   @%x' % (off, mn, val, note, a))

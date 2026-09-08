"""Class atlas, take 2 - linear passes only.

v1 did a full-binary `find` per class (4667 x 101 MB). This does:
  pass A: numpy scan of data sections for TypeDescriptor RVAs  -> COLs
  pass B: numpy scan for pointers to those COLs                -> vftables
  pass C: one capstone sweep of all functions                  -> lea targets
  pass D: per class, ctor = function containing lea(vftable) + `mov [this], r`

    python atlas2.py <exe> <outdir>
"""
import bisect, json, os, re, struct, sys
import numpy as np
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

EXE, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
d = open(EXE, 'rb').read()
buf = np.frombuffer(d, dtype=np.uint8)
pe = struct.unpack_from('<I', d, 0x3C)[0]
ns = struct.unpack_from('<H', d, pe + 6)[0]
osz = struct.unpack_from('<H', d, pe + 20)[0]
ib = struct.unpack_from('<Q', d, pe + 24 + 24)[0]
SECS = []
for i in range(ns):
    o = pe + 24 + osz + 40 * i
    n = d[o:o+8].rstrip(b'\0').decode('latin1')
    vs, va, rs, raw = struct.unpack_from('<IIII', d, o + 8)
    SECS.append((n, va, vs, raw, rs))


def va2off(va):
    r = va - ib
    for n, sva, vs, raw, rs in SECS:
        if sva <= r < sva + max(vs, rs):
            x = r - sva
            if x < rs:
                return raw + x
    return None


def off2va(off):
    for n, sva, vs, raw, rs in SECS:
        if raw <= off < raw + rs:
            return ib + sva + (off - raw)
    return None


# ---------- RTTI names ----------
names = {}
for m in re.finditer(rb'\.\?A[UV][A-Za-z0-9_@?$]{3,200}@@', d):
    va = off2va(m.start())
    if va is not None:
        names[va - 0x10] = m.group().decode('ascii')     # TypeDescriptor VA
print('rtti type descriptors: %d' % len(names))
td_rva = {(td - ib): sym for td, sym in names.items()}

DATA = [s for s in SECS if s[0] in ('.rdata', '.data')]

# ---------- pass A: COLs ----------
col_of = {}
for n, sva, vs, raw, rs in DATA:
    n4 = rs // 4
    words = buf[raw:raw + n4 * 4].view(np.uint32)
    keys = np.array(sorted(td_rva.keys()), dtype=np.uint32)
    hit = np.isin(words, keys)
    for idx in np.nonzero(hit)[0]:
        off = raw + int(idx) * 4
        va = off2va(off)
        if va is None:
            continue
        col = va - 12
        co = va2off(col)
        if co is None or co + 4 > len(d):
            continue
        if struct.unpack_from('<I', d, co)[0] not in (0, 1):
            continue
        col_of[col] = td_rva[int(words[idx])]
print('complete object locators: %d' % len(col_of))

# ---------- pass B: vftables ----------
vft_of = {}
colkeys = np.array(sorted(col_of.keys()), dtype=np.uint64)
for n, sva, vs, raw, rs in DATA:
    n8 = rs // 8
    q = buf[raw:raw + n8 * 8].view(np.uint64)
    hit = np.isin(q, colkeys)
    for idx in np.nonzero(hit)[0]:
        off = raw + int(idx) * 8
        va = off2va(off)
        if va is None:
            continue
        vft_of.setdefault(col_of[int(q[idx])], []).append(va + 8)
print('classes with a vftable: %d' % len(vft_of))

# ---------- function bounds ----------
pdt = [x for x in SECS if x[0] == '.pdata'][0]
_, pva, pvs, praw, prs = pdt
st, en = [], []
for i in range(prs // 12):
    b, e, u = struct.unpack_from('<III', d, praw + 12 * i)
    if b and e > b:
        st.append(b); en.append(e)
o = sorted(range(len(st)), key=lambda i: st[i])
st = [st[i] for i in o]; en = [en[i] for i in o]


def func_of(va):
    r = va - ib
    i = bisect.bisect_right(st, r) - 1
    if i < 0 or r >= en[i]:
        return None
    return ib + st[i], en[i] - st[i]


# ---------- pass C: lea targets ----------
md = Cs(CS_ARCH_X86, CS_MODE_64)
md.detail = False
LEA = re.compile(r'^(r[a-z0-9]+), \[rip ([+-]) (0x[0-9a-f]+)\]$')
want = set()
for v in vft_of.values():
    want.update(v)
lea_sites = {}
for k in range(len(st)):
    va = ib + st[k]
    size = en[k] - st[k]
    if size > 60000:
        continue
    off = va2off(va)
    if off is None:
        continue
    for ins in md.disasm(d[off:off + size], va):
        if ins.mnemonic != 'lea':
            continue
        m = LEA.match(ins.op_str)
        if not m:
            continue
        disp = int(m.group(3), 16) * (1 if m.group(2) == '+' else -1)
        tgt = ins.address + ins.size + disp
        if tgt in want:
            lea_sites.setdefault(tgt, []).append((ins.address, m.group(1)))
print('vftable lea sites: %d' % sum(len(v) for v in lea_sites.values()))

# ---------- pass D: ctors + field maps ----------
STORE = re.compile(r'^(?:[a-z]+ ptr )?\[(r[a-z0-9]+) \+ (0x[0-9a-f]+)\], (.+)$')
VSTORE = re.compile(r'^qword ptr \[(r[a-z0-9]+)\]$')


MOVRR = re.compile(r'^(r[a-z0-9]+), (r[a-z0-9]+)$')


def fieldmap(fva, fsize, this_reg):
    """Track register aliases of `this` - ctors routinely do `mov rbx, rcx`
    and then write every field through rbx, so following only the register the
    vftable was stored through misses almost the whole object."""
    off = va2off(fva)
    rows = {}
    alias = {this_reg}
    for ins in md.disasm(d[off:off + fsize], fva):
        if ins.mnemonic == 'mov':
            mm = MOVRR.match(ins.op_str)
            if mm:
                if mm.group(2) in alias:
                    alias.add(mm.group(1))
                elif mm.group(1) in alias:
                    alias.discard(mm.group(1))
        m = STORE.match(ins.op_str)
        if not m or m.group(1) not in alias:
            continue
        o2 = int(m.group(2), 16)
        val = m.group(3)
        e = {'op': ins.mnemonic, 'value': val}
        if val.startswith('0x'):
            try:
                iv = int(val, 16)
                e['int'] = iv
                if 0x30000000 < iv < 0x50000000:
                    e['float'] = round(struct.unpack('<f', struct.pack('<I', iv))[0], 6)
            except ValueError:
                pass
        rows.setdefault(o2, e)
    return rows


atlas = {}
for sym, vfts in vft_of.items():
    best = None
    for vft in set(vfts):
        for (addr, reg) in lea_sites.get(vft, []):
            f = func_of(addr)
            if not f:
                continue
            fva, fsize = f
            o = va2off(addr)
            this_reg = None
            for ins in md.disasm(d[o:o + 24], addr):
                if ins.mnemonic == 'mov' and ins.op_str.endswith(', ' + reg):
                    mm = VSTORE.match(ins.op_str.split(',')[0].strip())
                    if mm:
                        this_reg = mm.group(1)
                        break
            if this_reg is None:
                continue
            fm = fieldmap(fva, fsize, this_reg)
            if best is None or len(fm) > len(best['fields']):
                best = {'vftable': hex(vft), 'ctor': hex(fva), 'ctor_size': fsize,
                        'this': this_reg, 'n_fields': len(fm),
                        'fields': {hex(k): v for k, v in sorted(fm.items())}}
    if best and best['fields']:
        atlas[sym] = best

print('classes with a mapped ctor: %d' % len(atlas))
p = os.path.join(OUT, 'class_atlas.json')
json.dump(atlas, open(p, 'w'), indent=1)
print('written ->', p)
tot = sum(v['n_fields'] for v in atlas.values())
print('total mapped fields: %d' % tot)

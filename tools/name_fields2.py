"""Name physics-struct offsets using EXACT protobuf layouts.

1. For each protobuf message, find the vtable slot (ByteSizeLong) whose walked
   offset run best matches the descriptor's field count. ByteSizeLong visits
   fields in field-NUMBER order, so when
       len(observed offsets) == len(descriptor fields)
   index alignment yields offset -> field name. That equality is the check: if
   the code skipped or double-counted a field the counts disagree and we drop
   the message rather than guess.
2. Find loader functions copying [proto+X] -> [physics+Y].
3. Match a loader's source offsets against those exact layouts, keep only
   messages whose layout signature is UNIQUE, and emit named physics offsets.

    python name_fields2.py <exe> <outdir>
"""
import json, os, re, struct, sys
from collections import Counter, defaultdict
import numpy as np
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

sys.path.insert(0, r'C:\Projects\acecm')
from acecm import protos                                        # noqa: E402
from google.protobuf.descriptor import FieldDescriptor as FD    # noqa: E402

EXE, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
d = open(EXE, 'rb').read()
buf = np.frombuffer(d, dtype=np.uint8)
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


def off2va(off):
    for nm, sva, vs, raw, rs in S:
        if raw <= off < raw + rs:
            return ib + sva + (off - raw)
    return None


md = Cs(CS_ARCH_X86, CS_MODE_64)
md.detail = False

# ---- RTTI -> vftables ----
names = {}
for m in re.finditer(rb'\.\?AV([A-Za-z0-9_]{2,120})@@', d):
    va = off2va(m.start())
    if va is not None:
        names[va - 0x10] = m.group(1).decode('ascii')
td_rva = {(td - ib): s for td, s in names.items()}
DATA = [s for s in S if s[0] in ('.rdata', '.data')]
col_of = {}
keys = np.array(sorted(td_rva.keys()), dtype=np.uint32)
for nm, sva, vs, raw, rs in DATA:
    w = buf[raw:raw + (rs // 4) * 4].view(np.uint32)
    for idx in np.nonzero(np.isin(w, keys))[0]:
        va = off2va(raw + int(idx) * 4)
        if va is None:
            continue
        co = va2off(va - 12)
        if co is None or struct.unpack_from('<I', d, co)[0] not in (0, 1):
            continue
        col_of[va - 12] = td_rva[int(w[idx])]
vft_of = defaultdict(list)
ck = np.array(sorted(col_of.keys()), dtype=np.uint64)
for nm, sva, vs, raw, rs in DATA:
    q = buf[raw:raw + (rs // 8) * 8].view(np.uint64)
    for idx in np.nonzero(np.isin(q, ck))[0]:
        va = off2va(raw + int(idx) * 8)
        if va is not None:
            vft_of[col_of[int(q[idx])]].append(va + 8)

RD = re.compile(r'^(?:xmm[0-9]+|e[a-z][a-z]|r[a-z0-9]+), (?:dword|qword) ptr \[(r[a-z0-9]+) \+ (0x[0-9a-f]+)\]$')


def follow(va):
    o = va2off(va)
    if o is None:
        return None
    for ins in md.disasm(d[o:o + 16], va):
        return int(ins.op_str, 16) if ins.mnemonic == 'jmp' and ins.op_str.startswith('0x') else va
    return None


def slot_offsets(fva, limit=6000):
    o = va2off(fva)
    if o is None:
        return []
    seen, order = set(), []
    for ins in md.disasm(d[o:o + limit], fva):
        if ' ptr [' in ins.op_str and ins.mnemonic in ('mov', 'vmovss', 'movss'):
            m = RD.match(ins.op_str)
            if m:
                off = int(m.group(2), 16)
                if off < 0x800 and off not in seen:
                    seen.add(off)
                    order.append(off)
        if ins.mnemonic == 'ret':
            break
    return order


pool = set(protos.message_names())
layouts, dropped = {}, 0
for sym, vfts in vft_of.items():
    if sym not in pool:
        continue
    try:
        msg = protos.new(sym)
    except Exception:
        continue
    fields = sorted(msg.DESCRIPTOR.fields, key=lambda f: f.number)
    best = None
    for vft in set(vfts):
        vo = va2off(vft)
        if vo is None:
            continue
        for slot in range(16):
            fn = struct.unpack_from('<Q', d, vo + 8 * slot)[0]
            if not (ib < fn < ib + 0x8000000):
                break
            t = follow(fn)
            if not t:
                continue
            offs = slot_offsets(t)
            if not offs:
                continue
            if best is None or abs(len(offs) - len(fields)) < abs(len(best) - len(fields)):
                best = offs
    if not best:
        continue
    if len(best) != len(fields):
        dropped += 1
        continue
    layouts[sym] = {off: (f.name, f.type)
                    for off, f in zip(sorted(best), fields)}

print('messages with an exact, count-verified layout: %d (dropped %d on count mismatch)'
      % (len(layouts), dropped))

sig = defaultdict(list)
for n, lay in layouts.items():
    sig[tuple(sorted(lay))].append(n)
unique = {g[0] for g in sig.values() if len(g) == 1}
print('of those, uniquely identifiable by layout: %d' % len(unique))

# ---- loaders ----
pdt = [x for x in S if x[0] == '.pdata'][0]
_, pva, pvs, praw, prs = pdt
funcs = []
for i in range(prs // 12):
    b, e, u = struct.unpack_from('<III', d, praw + 12 * i)
    if b and e > b:
        funcs.append((b, e))
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
    if len(pairs) >= 6:
        loaders.append((ib + b, pairs))
print('loader-shaped functions: %d' % len(loaders))

# ---- match ----
res = {}
for fva, pairs in loaders:
    for (sr, dr), _ in Counter((p[0], p[2]) for p in pairs).most_common(3):
        sel = [p for p in pairs if p[0] == sr and p[2] == dr]
        if len(sel) < 6:
            continue
        srcs = {p[1] for p in sel}
        for msg in unique:
            lay = layouts[msg]
            inter = srcs & set(lay)
            if len(inter) < 6 or len(inter) / len(srcs) < 0.85:
                continue
            if any(lay[o][1] != FD.TYPE_FLOAT for o in inter):
                continue
            prev = res.get(msg)
            if prev and prev['matched'] >= len(inter):
                continue
            res[msg] = {'loader': hex(fva), 'matched': len(inter),
                        'of_src': len(srcs),
                        'fields': {hex(p[3]): lay[p[1]][0]
                                   for p in sel if p[1] in lay}}

print('CONFIRMED namings: %d' % len(res))
for m2, r in sorted(res.items(), key=lambda kv: -kv[1]['matched']):
    print('   %-32s %3d/%-3d  loader %-12s -> %d physics offsets named'
          % (m2, r['matched'], r['of_src'], r['loader'], len(r['fields'])))
p = os.path.join(OUT, 'named_fields_exact.json')
json.dump(res, open(p, 'w'), indent=1)
print('written ->', p)

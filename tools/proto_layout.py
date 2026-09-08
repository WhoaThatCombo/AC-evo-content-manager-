"""Extract EXACT protobuf field offsets from each message's ByteSizeLong.

Works for mixed messages (submessages/strings/pointers) where the
`0x10 + 4*index` shortcut does not hold.

ByteSizeLong is the vtable slot that walks a run of consecutive small offsets
and adds a constant per present field:
    float  -> vmovss [this+off] ; vcomiss vs 0 ; add rbx, N
    int32  -> mov eax,[this+off] ; bsr/shr 6 varint-size math
and N = tag_size + 4 for a fixed32, so N tells us the field number's tag width
(5 => field < 16, 6 => field >= 16). Aligning the observed order against the
descriptor's fields in field-NUMBER order pins every offset.

    python proto_layout.py <exe> <out.json> [MessageName ...]
"""
import json, re, struct, sys
import numpy as np
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

sys.path.insert(0, r'C:\Projects\acecm')
from acecm import protos                                        # noqa: E402
from google.protobuf.descriptor import FieldDescriptor as FD    # noqa: E402

EXE, OUTJSON = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])

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


# ---- RTTI -> vftable (same pipeline as atlas2) ----
names = {}
for m in re.finditer(rb'\.\?AV([A-Za-z0-9_]{2,120})@@', d):
    va = off2va(m.start())
    if va is not None:
        names[va - 0x10] = m.group(1).decode('ascii')
td_rva = {(td - ib): sym for td, sym in names.items()}
DATA = [s for s in S if s[0] in ('.rdata', '.data')]
col_of = {}
keys = np.array(sorted(td_rva.keys()), dtype=np.uint32)
for nm, sva, vs, raw, rs in DATA:
    w = buf[raw:raw + (rs // 4) * 4].view(np.uint32)
    for idx in np.nonzero(np.isin(w, keys))[0]:
        va = off2va(raw + int(idx) * 4)
        if va is None:
            continue
        col = va - 12
        co = va2off(col)
        if co is None:
            continue
        if struct.unpack_from('<I', d, co)[0] not in (0, 1):
            continue
        col_of[col] = td_rva[int(w[idx])]
vft_of = {}
ck = np.array(sorted(col_of.keys()), dtype=np.uint64)
for nm, sva, vs, raw, rs in DATA:
    q = buf[raw:raw + (rs // 8) * 8].view(np.uint64)
    for idx in np.nonzero(np.isin(q, ck))[0]:
        va = off2va(raw + int(idx) * 8)
        if va is not None:
            vft_of.setdefault(col_of[int(q[idx])], []).append(va + 8)

md = Cs(CS_ARCH_X86, CS_MODE_64)
md.detail = False
RD = re.compile(r'^(?:xmm[0-9]+|e[a-z][a-z]|r[a-z0-9]+), (?:dword|qword) ptr \[(r[a-z0-9]+) \+ (0x[0-9a-f]+)\]$')
ADD = re.compile(r'^(r[a-z0-9]+), (0x[0-9a-f]+|[0-9]+)$')


def follow(va):
    o = va2off(va)
    if o is None:
        return None
    for ins in md.disasm(d[o:o + 16], va):
        if ins.mnemonic == 'jmp':
            try:
                return int(ins.op_str, 16)
            except ValueError:
                return None
        return va
    return None


def scan_slot(fva, limit=4000):
    """Return ordered [(offset, add_const_or_None)] observed in the function."""
    o = va2off(fva)
    if o is None:
        return []
    seq = []
    last = None
    for ins in md.disasm(d[o:o + limit], fva):
        if ins.mnemonic in ('vmovss', 'movss', 'mov') and ' ptr [' in ins.op_str:
            m = RD.match(ins.op_str)
            if m:
                off = int(m.group(2), 16)
                if off < 0x400:
                    last = off
                    seq.append([off, None])
                continue
        if ins.mnemonic == 'add' and seq:
            m = ADD.match(ins.op_str)
            if m:
                try:
                    v = int(m.group(2), 16) if m.group(2).startswith('0x') else int(m.group(2))
                except ValueError:
                    continue
                if 1 <= v <= 12 and seq[-1][1] is None:
                    seq[-1][1] = v
        if ins.mnemonic == 'ret':
            break
    return [tuple(x) for x in seq]


out = {}
pool = set(protos.message_names())
for sym, vfts in vft_of.items():
    if sym not in pool:
        continue
    if ONLY and sym not in ONLY:
        continue
    try:
        msg = protos.new(sym)
    except Exception:
        continue
    fields = sorted(msg.DESCRIPTOR.fields, key=lambda f: f.number)
    best = []
    for vft in set(vfts):
        vo = va2off(vft)
        if vo is None:
            continue
        for slot in range(16):
            fn = struct.unpack_from('<Q', d, vo + 8 * slot)[0]
            if not (ib < fn < ib + 0x8000000):
                break
            tgt = follow(fn)
            if not tgt:
                continue
            seq = scan_slot(tgt)
            offs = sorted({s[0] for s in seq})
            if len(offs) > len(best):
                best = offs
    if len(best) >= 2:
        out[sym] = {'n_descriptor_fields': len(fields),
                    'observed_offsets': [hex(x) for x in best]}

print('messages with an observed offset run: %d' % len(out))
json.dump(out, open(OUTJSON, 'w'), indent=1)
print('written ->', OUTJSON)
for k in list(ONLY) or list(out)[:5]:
    if k in out:
        print('  %-30s %d fields, offsets %s'
              % (k, out[k]['n_descriptor_fields'],
                 ' '.join(out[k]['observed_offsets'][:14])))

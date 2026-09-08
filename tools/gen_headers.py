"""Emit source-shaped C++ headers from the class atlas + vtables + named fields.

Everything in the output is evidence from the binary:
  - class name            : RTTI
  - field offset          : a store in the constructor
  - field default         : the immediate that store writes
  - field name            : only where a confirmed protobuf mapping exists
  - virtual method slots  : the class vftable

Unknown gaps are emitted as explicit padding so offsets stay honest - this is a
layout reconstruction, NOT compilable source.

    python gen_headers.py <exe> <atlas.json> <named_FINAL.json> <outdir>
"""
import json, os, re, struct, sys

EXE, ATLAS, NAMED, OUT = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
os.makedirs(OUT, exist_ok=True)
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


atlas = json.load(open(ATLAS))
named = json.load(open(NAMED))

# ⚠ Field names from named_fields are relative to ONE destination struct, and
# we have not established which class that struct is. Applying them globally by
# offset stamps unrelated names on every class sharing an offset (it labelled
# PhysicsEngine+0x28 'second_places' from PrintableDriverStatistics). Until the
# loader's destination is tied to a class, emit offsets only.
name_by_off = {}


def pretty(sym):
    m = re.match(r'^\.\?A[UV](.+)@@$', sym)
    if not m:
        return sym
    return '::'.join(reversed([p for p in m.group(1).split('@') if p]))


def vslots(vft_hex, limit=64):
    o = va2off(int(vft_hex, 16))
    if o is None:
        return []
    out = []
    for i in range(limit):
        q = struct.unpack_from('<Q', d, o + 8 * i)[0]
        if not (ib < q < ib + 0x8000000):
            break
        out.append(q)
    return out


def ctype(e):
    if 'float' in e:
        return 'float'
    if 'int' in e:
        v = e['int']
        if v in (0, 1):
            return 'bool/int32'
        return 'int32'
    if e['op'] in ('vmovss', 'movss'):
        return 'float'
    if e['op'] in ('vmovups', 'movups', 'vmovaps', 'movaps'):
        return 'float[4]'
    return 'void*' if e['op'] == 'mov' else 'unknown'


def default_of(e):
    if 'float' in e:
        return '%g' % e['float']
    if 'int' in e:
        return str(e['int'])
    return e['value']


made = 0
skipped = 0
for sym, v in atlas.items():
    name = pretty(sym)
    if name.startswith(('std::', 'renoir::', 'cohtml::', 'Concurrency::')) \
       or sym.startswith(('.?AV?$', '.?AU?$')):
        skipped += 1
        continue
    if v['n_fields'] < 6:
        skipped += 1
        continue
    slots = vslots(v['vftable'])
    safe = re.sub(r'[^A-Za-z0-9_]', '_', name)[:90]
    lines = []
    lines.append('// %s' % name)
    lines.append('// reconstructed from RTTI + constructor %s (%d bytes)'
                 % (v['ctor'], v['ctor_size']))
    lines.append('// vftable %s, %d virtual slots' % (v['vftable'], len(slots)))
    lines.append('// %d fields observed; gaps are unobserved, not empty'
                 % v['n_fields'])
    lines.append('struct %s {' % safe)
    prev_end = 0
    for off_hex, e in sorted(v['fields'].items(), key=lambda kv: int(kv[0], 16)):
        off = int(off_hex, 16)
        if off > prev_end:
            lines.append('    char _pad_%x[0x%x];' % (prev_end, off - prev_end))
        t = ctype(e)
        nm2 = None
        if off in name_by_off:
            nm2, src = name_by_off[off]
        fld = nm2 if nm2 else 'field_%x' % off
        note = '  // +0x%x  = %s' % (off, default_of(e))
        if nm2:
            note += '   [name from %s]' % name_by_off[off][1]
        lines.append('    %-11s %s;%s' % (t, fld, note))
        prev_end = off + (16 if t == 'float[4]' else
                          8 if t == 'void*' else 4)
    lines.append('')
    for i, q in enumerate(slots[:24]):
        lines.append('    // vtable[%2d] = %x' % (i, q))
    lines.append('};')
    open(os.path.join(OUT, safe + '.h'), 'w', encoding='utf-8').write(
        '\n'.join(lines) + '\n')
    made += 1

print('headers written: %d  (skipped %d template/std/small)' % (made, skipped))
print('out ->', OUT)

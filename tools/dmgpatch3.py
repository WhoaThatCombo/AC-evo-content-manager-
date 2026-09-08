"""Damage patch v3 - change the DEFAULT damage multiplier from 1.0 to 0.0.

The object holding the multiplier (allocated 0x19e8 bytes, stored at
RealtimePhysicsEngineController + 0x2dc0) initialises it in its constructor:

    14229433a  mov dword ptr [rbx + 0x3b8], 0x3f800000     ; 1.0f

In singleplayer the CarAssistsChangeEvent handler overwrites it with
damage_rate + 1.0, so SP keeps working exactly as now (100% -> 1.0 -> damage,
0% -> 0.0 -> no damage). In multiplayer that handler never runs, so the field
keeps whatever the constructor put there - which is why the assist has no
effect online. Flipping the default to 0.0 gives no damage in MP while leaving
SP fully controllable.

One immediate, 4 bytes, same size, no code cave.

    python dmgpatch3.py             # dry run
    python dmgpatch3.py --apply
    python dmgpatch3.py --revert
"""
import os, shutil, struct, sys

EXE = (r'C:\Program Files (x86)\Steam\steamapps\common'
       r'\Assetto Corsa EVO\AssettoCorsaEVO.exe')
BAK = EXE + '.bak_dmgdefault'
SITE = 0x14229433a
FULL = bytes.fromhex('c783b803000000008 03f'.replace(' ', ''))   # mov [rbx+0x3b8],1.0f
IMM_OFF = 6                     # immediate starts 6 bytes in
ONE = bytes.fromhex('0000803f')
ZERO = bytes.fromhex('00000000')

data = bytearray(open(EXE, 'rb').read())
pe = struct.unpack_from('<I', data, 0x3C)[0]
ns = struct.unpack_from('<H', data, pe + 6)[0]
osz = struct.unpack_from('<H', data, pe + 20)[0]
ib = struct.unpack_from('<Q', data, pe + 24 + 24)[0]
SECS = []
for i in range(ns):
    o = pe + 24 + osz + 40 * i
    n = data[o:o+8].rstrip(b'\0').decode('latin1')
    vs, va, rs, raw = struct.unpack_from('<IIII', data, o + 8)
    SECS.append((n, va, vs, raw, rs))


def va2off(va):
    r = va - ib
    for n, sva, vs, raw, rs in SECS:
        if sva <= r < sva + max(vs, rs):
            x = r - sva
            if x < rs:
                return raw + x
    return None


off = va2off(SITE)
cur = bytes(data[off:off + 10])
print('site %x -> file offset %d' % (SITE, off))
print('bytes now : %s' % cur.hex(' '))
print('expected  : %s  (mov dword [rbx+0x3b8], 1.0f)' % FULL.hex(' '))
imm = cur[IMM_OFF:IMM_OFF + 4]
print('immediate : %s = %g' % (imm.hex(' '), struct.unpack('<f', imm)[0]))

if '--revert' in sys.argv:
    if not os.path.isfile(BAK):
        raise SystemExit('no backup at %s' % BAK)
    shutil.copyfile(BAK, EXE)
    print('reverted from', BAK)
    raise SystemExit
if '--apply' not in sys.argv:
    print()
    print('DRY RUN - re-run with --apply')
    raise SystemExit

if imm == ZERO:
    raise SystemExit('already patched')
if cur != FULL:
    raise SystemExit('ABORT: unexpected bytes at the site')

if not os.path.isfile(BAK):
    shutil.copyfile(EXE, BAK)
    print('backup ->', BAK)
data[off + IMM_OFF:off + IMM_OFF + 4] = ZERO
open(EXE, 'wb').write(bytes(data))
chk = open(EXE, 'rb').read()[off:off + 10]
print('now       : %s  (immediate = %g)'
      % (chk.hex(' '), struct.unpack('<f', chk[IMM_OFF:IMM_OFF + 4])[0]))
print('patched - default damage multiplier is now 0.0')

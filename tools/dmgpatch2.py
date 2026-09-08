"""Damage multiplier patch v2 - "sticky damage-off".

The multiplier lives on a SESSION-GLOBAL object:
    [controller + 0x2dc0] + 0x3b8 = damage_rate + 1.0
and every CarAssistsChangeEvent writes it, before any car is identified. Other
assists (gearbox/blip/clutch) are per-car; damage is not. So in multiplayer a
remote player's event (damage_rate 0 -> multiplier 1.0) overwrites your 0.

v1 forced the multiplier to 0 unconditionally and the game crashed. This instead
keeps the original arithmetic and simply SKIPS THE STORE when the computed
multiplier is exactly 1.0, so "full damage" events cannot clobber a 0 you set.
Set 0% in Assists as usual; nothing invents a value.

Redirects 23 bytes at 1414cff74 into a code cave in .text tail padding.

    python dmgpatch2.py            # dry run, shows the assembled cave
    python dmgpatch2.py --apply
    python dmgpatch2.py --revert
"""
import os, shutil, struct, sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

EXE = (r'C:\Program Files (x86)\Steam\steamapps\common'
       r'\Assetto Corsa EVO\AssettoCorsaEVO.exe')
BAK = EXE + '.bak_nodamage2'

SITE = 0x1414cff74        # vaddss xmm1, xmm0, [rip->1.0]
BACK = 0x1414cff8b        # instruction after the original store
CAVE = 0x14315c100        # inside the 7556-byte padding run at 14315c07c
ONE = 0x14316797c         # the 1.0f constant

ORIG = bytes.fromhex('c5fa580d')                      # vaddss ...
MOVRAX = bytes.fromhex('488b83c02d0000')              # mov rax,[rbx+0x2dc0]
STORE = bytes.fromhex('c5fa1188b8030000')             # vmovss [rax+0x3b8],xmm1

data = bytearray(open(EXE, 'rb').read())
pe = struct.unpack_from('<I', data, 0x3C)[0]
nsec = struct.unpack_from('<H', data, pe + 6)[0]
optsz = struct.unpack_from('<H', data, pe + 20)[0]
ib = struct.unpack_from('<Q', data, pe + 24 + 24)[0]
SECS = []
for i in range(nsec):
    o = pe + 24 + optsz + 40 * i
    n = data[o:o+8].rstrip(b'\0').decode('latin1')
    vs, va, rs, raw = struct.unpack_from('<IIII', data, o + 8)
    SECS.append((n, va, vs, raw, rs))


def va2off(va):
    r = va - ib
    for n, sva, vs, raw, rs in SECS:
        if sva <= r < sva + max(vs, rs):
            d = r - sva
            if d < rs:
                return raw + d
    return None


# ---- assemble the cave ----
def s32(v):
    return struct.pack('<i', v)


cave = bytearray()
cave += bytes.fromhex('c5fa580d') + s32(ONE - (CAVE + 8))        # vaddss xmm1,xmm0,[1.0]
cave += bytes.fromhex('c5f82e0d') + s32(ONE - (CAVE + 16))       # vucomiss xmm1,[1.0]
cave += bytes.fromhex('740f')                                    # je +15 -> skip store
cave += MOVRAX                                                   # mov rax,[rbx+0x2dc0]
cave += STORE                                                    # vmovss [rax+0x3b8],xmm1
cave += b'\xe9' + s32(BACK - (CAVE + len(cave) + 5))             # jmp back
assert len(cave) == 38, len(cave)

patch = bytearray(b'\xe9' + s32(CAVE - (SITE + 5)))
patch += b'\x90' * (BACK - SITE - len(patch))
assert len(patch) == BACK - SITE == 23, len(patch)

md = Cs(CS_ARCH_X86, CS_MODE_64)
print('=== cave @ %x ===' % CAVE)
for i in md.disasm(bytes(cave), CAVE):
    print('   %x  %-9s %s' % (i.address, i.mnemonic, i.op_str))
print('=== patch @ %x (%d bytes) ===' % (SITE, len(patch)))
for i in md.disasm(bytes(patch), SITE):
    print('   %x  %-9s %s' % (i.address, i.mnemonic, i.op_str))

site_off = va2off(SITE)
cave_off = va2off(CAVE)
cur = bytes(data[site_off:site_off + 4])
cave_now = bytes(data[cave_off:cave_off + len(cave)])
print()
print('site bytes now : %s' % cur.hex(' '))
print('cave is padding: %s' % all(c in (0xCC, 0x00) for c in cave_now))

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

if cur != ORIG:
    raise SystemExit('ABORT: site does not hold the stock vaddss (%s)' % cur.hex(' '))
if not all(c in (0xCC, 0x00) for c in cave_now):
    raise SystemExit('ABORT: cave is not padding')

if not os.path.isfile(BAK):
    shutil.copyfile(EXE, BAK)
    print('backup ->', BAK)
data[cave_off:cave_off + len(cave)] = cave
data[site_off:site_off + len(patch)] = patch
open(EXE, 'wb').write(bytes(data))
print('patched: sticky damage-off installed')

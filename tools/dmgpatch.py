"""TEST PATCH: force the damage multiplier to 0 in the AC EVO client.

Handler `RealtimePhysicsEngineController::onCarAssistsChanged` @ 1414cff30 does
    vmovss xmm0, [rax + 0x34]      ; damage_rate
    vaddss xmm1, xmm0, [rip->1.0]  ; + 1.0
    vmovss [physics_car + 0x3b8], xmm1
Replacing the vaddss with `vxorps xmm1,xmm1,xmm1` + NOPs makes the stored
multiplier always 0.0 = no damage, whatever the incoming assists say.

This is a DIAGNOSTIC: it tells us whether the multiplier is what governs damage
in multiplayer. Patches the EXE, not content, so car SHA digests are unaffected
and it cannot cause "Content unavailable or version mismatch".

    python dmgpatch.py            # show current bytes
    python dmgpatch.py --apply
    python dmgpatch.py --revert
"""
import os, shutil, struct, sys

EXE = (r'C:\Program Files (x86)\Steam\steamapps\common'
       r'\Assetto Corsa EVO\AssettoCorsaEVO.exe')
BAK = EXE + '.bak_nodamage'
VA = 0x1414cff74                      # the vaddss
ORIG = bytes.fromhex('c5fa580d')      # vaddss xmm1, xmm0, dword ptr [rip+...]
PATCH = bytes.fromhex('c5f057c9') + b'\x90' * 4   # vxorps xmm1,xmm1,xmm1 + nops

data = open(EXE, 'rb').read()
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


off = va2off(VA)
cur = data[off:off + 8]
print('VA %x -> file offset %d' % (VA, off))
print('current bytes: %s' % cur.hex(' '))
print('stock  starts: %s (vaddss xmm1,xmm0,[rip+..])' % ORIG.hex(' '))
print('patch  bytes : %s (vxorps xmm1,xmm1,xmm1 + nops)' % PATCH.hex(' '))

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

if cur == PATCH:
    raise SystemExit('already patched')
if cur[:4] != ORIG:
    raise SystemExit('ABORT: unexpected bytes %s - wrong address or build'
                     % cur.hex(' '))

if not os.path.isfile(BAK):
    shutil.copyfile(EXE, BAK)
    print('backup ->', BAK)
with open(EXE, 'r+b') as f:
    f.seek(off)
    f.write(PATCH)
with open(EXE, 'rb') as f:
    f.seek(off)
    print('now: %s' % f.read(8).hex(' '))
print('patched - damage multiplier forced to 0.0')

"""Zero collision-damage physics in the SERVER's content.kspkg only.

Path: CarData f2 (suspensions) -> f4 (damage) -> f2 gain, f3 maxDamage, floats.

⚠ BYTE-PATCH, never re-serialize: proto3 omits zero-valued scalars, so
re-serializing a zeroed float would DROP the field and change the entry size,
which would need a full index rewrite. Writing 0.0 over the existing float
keeps the field present, the entry the same size, and the archive untouched
apart from 8 bytes per car.

Entry bodies are XOR'd with the engine key at phase 0 relative to the START of
the entry, so plaintext byte i is p ^ KEY[i % 8].

    python srv_nodamage.py            # dry run
    python srv_nodamage.py --apply    # write
    python srv_nodamage.py --revert   # restore from the saved json
"""
import json, os, struct, sys

sys.path.insert(0, r'C:\Projects\AC_EVO_Car_Conversion_Toolkit\scripts')
import kspkg_extract as K

KS = (r'C:\Program Files (x86)\Steam\steamapps\common'
      r'\Assetto Corsa EVO Dedicated Server\content.kspkg')
KEY = bytes.fromhex('c135117da921979f')
SEP = chr(92)
REVERT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                      'srv_nodamage_revert.json')


def varint(b, i):
    r = s = 0
    while True:
        c = b[i]; i += 1
        r |= (c & 0x7F) << s
        if not c & 0x80:
            return r, i
        s += 7


def walk(buf, start, end):
    """Yield (field_number, wiretype, value_start, value_end) at this level."""
    i = start
    while i < end:
        key, i = varint(buf, i)
        fn, wt = key >> 3, key & 7
        if wt == 0:
            j = i
            _, i = varint(buf, i)
            yield fn, wt, j, i
        elif wt == 5:
            yield fn, wt, i, i + 4; i += 4
        elif wt == 1:
            yield fn, wt, i, i + 8; i += 8
        elif wt == 2:
            n, i = varint(buf, i)
            yield fn, wt, i, i + n; i += n
        else:
            raise ValueError('wiretype %d' % wt)


def descend(buf, start, end, fnum):
    for fn, wt, vs, ve in walk(buf, start, end):
        if fn == fnum and wt == 2:
            return vs, ve
    return None


def targets(buf):
    """Absolute offsets (within the entry) of gain and maxDamage floats."""
    sus = descend(buf, 0, len(buf), 2)              # CarData.suspensions
    if not sus:
        return None
    dmg = descend(buf, sus[0], sus[1], 4)           # .damage
    if not dmg:
        return None
    out = {}
    for fn, wt, vs, ve in walk(buf, dmg[0], dmg[1]):
        if fn in (2, 3) and wt == 5:
            out['gain' if fn == 2 else 'maxDamage'] = vs
    return out or None


def main():
    apply = '--apply' in sys.argv
    revert = '--revert' in sys.argv

    if revert:
        info = json.load(open(REVERT))
        with open(info['kspkg'], 'r+b') as f:
            for row in info['cars']:
                for name in ('gain', 'maxDamage'):
                    f.seek(row[name + '_abs'])
                    f.write(bytes.fromhex(row[name + '_orig']))
        print('reverted %d cars' % len(info['cars']))
        return

    ents = []
    for path, size, off in K.iter_entries(KS):
        lp = path.lower()
        if size and lp.endswith('.car') and lp.startswith(
                'content' + SEP + 'cars' + SEP):
            ents.append((path, size, off))
    print('%d .car entries' % len(ents))

    rows, skipped = [], []
    with open(KS, 'rb') as f:
        for path, size, off in sorted(ents):
            f.seek(off)
            enc = f.read(size)
            buf = K.decrypt(enc, 0)
            t = targets(buf)
            car = path.split(SEP)[2]
            if not t or 'gain' not in t or 'maxDamage' not in t:
                skipped.append(car)
                continue
            row = {'car': car}
            for name in ('gain', 'maxDamage'):
                rel = t[name]
                row[name] = struct.unpack('<f', buf[rel:rel + 4])[0]
                row[name + '_abs'] = off + rel
                row[name + '_orig'] = enc[rel:rel + 4].hex()
                row[name + '_zero'] = bytes(
                    0 ^ KEY[(rel + k) % 8] for k in range(4)).hex()
            rows.append(row)

    print('%d cars with damage fields, %d skipped' % (len(rows), len(skipped)))
    if skipped:
        print('   skipped:', ', '.join(skipped[:8]))
    nz = [r for r in rows if r['gain'] or r['maxDamage']]
    print('%d cars currently have NON-ZERO damage' % len(nz))
    for r in rows[:5]:
        print('   %-38s gain=%-10g maxDamage=%g' % (r['car'], r['gain'],
                                                    r['maxDamage']))

    if not apply:
        print()
        print('DRY RUN - nothing written. re-run with --apply')
        return

    json.dump({'kspkg': KS, 'cars': rows}, open(REVERT, 'w'), indent=1)
    print('revert data ->', REVERT)
    with open(KS, 'r+b') as f:
        for r in rows:
            for name in ('gain', 'maxDamage'):
                f.seek(r[name + '_abs'])
                f.write(bytes.fromhex(r[name + '_zero']))
    print('patched %d cars (%d bytes total)' % (len(rows), len(rows) * 8))

    # verify a sample by re-reading through the normal path
    bad = 0
    with open(KS, 'rb') as f:
        for path, size, off in sorted(ents):
            f.seek(off)
            buf = K.decrypt(f.read(size), 0)
            t = targets(buf)
            if not t:
                continue
            g = struct.unpack('<f', buf[t['gain']:t['gain'] + 4])[0]
            m = struct.unpack('<f', buf[t['maxDamage']:t['maxDamage'] + 4])[0]
            if g or m:
                bad += 1
    print('cars still showing non-zero damage after patch: %d' % bad)
    print('archive size:', os.path.getsize(KS))


if __name__ == '__main__':
    main()

"""The AC1 reference light of the track brightness model (src/track/look.rs).

Reads, from the AC1 install (argument 1, default the Steam path):
- system/cfg/colorCurves.ini: the SUN and AMBIENT colour curves (r, g, b in
  0..255 and a multiplier; LOW / HIGH ends);
- system/cfg/ppfilters/default.ini: the default tonemapping exposure;
- every content/tracks/**/data/lighting.ini: SUN_PITCH_ANGLE (the sun's
  elevation, degrees), as a histogram;
and, with --shaders, runs the Windows SDK fxc on the ksPerPixel and
ksMultilayer pixel shaders and prints the instructions after the shadow
lookup (the lighting sum), so the formula in look.rs can be re-checked.

python tools/ac1_lighting.py ["<AC1 dir>"] [--shaders] > stockstats/measure_ac1_lighting.txt
"""
import collections
import configparser
import glob
import os
import subprocess
import sys

args = [a for a in sys.argv[1:] if not a.startswith('--')]
ac = args[0] if args else r'C:\Program Files (x86)\Steam\steamapps\common\assettocorsa'


def ini(path):
    c = configparser.ConfigParser(strict=False, inline_comment_prefixes=(';',))
    c.optionxform = str
    with open(path, encoding='utf-8', errors='replace') as f:
        c.read_string(f.read())
    return c


def luma(r, g, b):
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


cc = ini(os.path.join(ac, 'system', 'cfg', 'colorCurves.ini'))
print('colorCurves.ini (r, g, b 0..255, multiplier) -> luma x multiplier / 255')
for sec in ('SUN', 'AMBIENT'):
    for end in ('LOW', 'HIGH'):
        v = [float(x) for x in cc[sec][end].split(',')]
        print(f'  {sec} {end}: {v} -> {luma(*v[:3]) * v[3] / 255:.4f}')

pp = ini(os.path.join(ac, 'system', 'cfg', 'ppfilters', 'default.ini'))
print(f"ppfilters/default.ini TONEMAPPING EXPOSURE = {pp['TONEMAPPING']['EXPOSURE']}")

hist = collections.Counter()
n = 0
for p in glob.glob(os.path.join(ac, 'content', 'tracks', '**', 'lighting.ini'), recursive=True):
    try:
        c = ini(p)
        v = c['LIGHTING']['SUN_PITCH_ANGLE']
    except Exception:
        continue
    hist[float(v)] += 1
    n += 1
print(f'SUN_PITCH_ANGLE over {n} track lighting.ini files (value: count):')
for v, k in sorted(hist.items(), key=lambda x: (-x[1], x[0])):
    print(f'  {v:g}: {k}')
print(f'  mode: {hist.most_common(1)[0][0]:g}')

if '--shaders' in sys.argv:
    kits = glob.glob(r'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\fxc.exe')
    fxc = sorted(kits)[-1]
    for s in ('ksPerPixel', 'ksMultilayer'):
        out = subprocess.run([fxc, '/nologo', '/dumpbin', os.path.join(ac, 'system', 'shaders', 'win', f'{s}_ps.fxo')], capture_output=True, text=True).stdout
        lines = out.splitlines()
        # the lighting sum follows the last shadow-map lookup
        last = max(i for i, l in enumerate(lines) if 'sample_c' in l or l.strip() == 'endif')
        print(f'--- {s}_ps.fxo after the shadow lookup:')
        for l in lines[last + 1:]:
            if l.strip() and not l.startswith('//'):
                print('  ' + l.strip())

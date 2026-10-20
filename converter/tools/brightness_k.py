"""RENDER_TO_ALBEDO (src/track/materials.rs) from the calibration pair.

Inputs:
1. the convert report (ui/convert_report.txt or the converter's console log) of
   ks_brands_hatch converted with RENDER_TO_ALBEDO = 1: its line
   "brightness: AC1 reference light, class fit ...: <class> rendered median <v> x<s>, ..."
2. stockstats/measure_albedo.txt (`aceconv albedo <content.kspkg>`): the
   stock brands_hatch line "brands_hatch: <class> <median> [..] n.., ...".

K = the geometric mean, over the classes both have (tree left out: AC1 tree
cards are replaced by the vegetation stage), of stock median / rendered median.

python tools/brightness_k.py <report> stockstats/measure_albedo.txt
"""
import math
import re
import sys

rep = open(sys.argv[1], encoding='utf-8', errors='replace').read()
m = re.search(r'brightness: AC1 reference light[^:]*: (.*)', rep)
rendered = {k: float(v) for k, v in re.findall(r'(\w+) rendered median ([0-9.]+)', m.group(1))}
stock = {}
for line in open(sys.argv[2], encoding='utf-8'):
    if line.startswith('brands_hatch:'):
        stock = {k: float(v) for k, v in re.findall(r'(\w+) ([0-9.]+) \[', line)}
classes = sorted(c for c in rendered if c in stock and c != 'tree')
logs = []
for c in classes:
    r = stock[c] / rendered[c]
    logs.append(math.log(r))
    print(f'{c}: stock brands_hatch {stock[c]:.3f} / converted ks_brands_hatch rendered {rendered[c]:.3f} = {r:.3f}')
print(f'K = geometric mean over {len(classes)} classes = {math.exp(sum(logs) / len(logs)):.3f}')

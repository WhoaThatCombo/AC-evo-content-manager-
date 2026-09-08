"""Render a class atlas JSON into readable markdown.

    python atlas_report.py <atlas.json> <outdir> [ClassName ...]
"""
import json, os, re, sys

src, outdir = sys.argv[1], sys.argv[2]
want = sys.argv[3:]
a = json.load(open(src))
os.makedirs(outdir, exist_ok=True)


def pretty(sym):
    m = re.match(r'^\.\?A[UV](.+)@@$', sym)
    if not m:
        return sym
    parts = [p for p in m.group(1).split('@') if p]
    return '::'.join(reversed(parts))


def is_game(sym):
    if sym.startswith('.?AV?$') or sym.startswith('.?AU?$'):
        return False
    n = pretty(sym)
    return not n.startswith(('std::', 'renoir::', 'cohtml::', 'Concurrency::'))


# index
rows = sorted(((v['n_fields'], pretty(k), v) for k, v in a.items() if is_game(k)),
              reverse=True)
with open(os.path.join(outdir, 'INDEX.md'), 'w', encoding='utf-8') as f:
    f.write('# AC EVO class atlas\n\n')
    f.write('%d classes with a mapped constructor, %d fields total.\n\n'
            % (len(a), sum(v['n_fields'] for v in a.values())))
    f.write('Derived from RTTI -> vftable -> constructor. Every row is a field\n'
            'the constructor writes, with the literal default where it is an\n'
            'immediate. Nothing here is inferred.\n\n')
    f.write('| class | fields | ctor | vftable |\n|---|--:|---|---|\n')
    for n, name, v in rows[:400]:
        f.write('| `%s` | %d | `%s` | `%s` |\n' % (name, n, v['ctor'], v['vftable']))
print('INDEX.md: %d game classes' % len(rows))


def dump(sym, v, path):
    with open(path, 'w', encoding='utf-8') as f:
        f.write('# %s\n\n' % pretty(sym))
        f.write('- ctor `%s` (%d bytes), `this` in `%s`\n'
                % (v['ctor'], v['ctor_size'], v['this']))
        f.write('- vftable `%s`\n- %d fields\n\n' % (v['vftable'], v['n_fields']))
        f.write('| offset | op | value | as float |\n|---|---|---|---|\n')
        for off, e in sorted(v['fields'].items(), key=lambda kv: int(kv[0], 16)):
            fl = ('%g' % e['float']) if 'float' in e else ''
            f.write('| `%s` | %s | `%s` | %s |\n' % (off, e['op'], e['value'], fl))


made = 0
for k, v in a.items():
    name = pretty(k)
    if want and not any(w.lower() in name.lower() for w in want):
        continue
    if not want and v['n_fields'] < 40:
        continue
    if not is_game(k):
        continue
    safe = re.sub(r'[^A-Za-z0-9_.-]', '_', name)[:80]
    dump(k, v, os.path.join(outdir, safe + '.md'))
    made += 1
print('per-class files: %d -> %s' % (made, outdir))

"""Render a converted car's sound events next to the AC1 original and a stock EVO car, offline.

  python tools/soundcmp.py <out dir> <ac1 car dir> <converted banks dir> <car id> <stock car id> [limiter rpm]

Writes <out dir>/<car id>/{conv,ac1,stock}_{ext,int}_{on,off}.wav (rpm sweep 1000 -> limiter over
8 s, throttle 1 / 0; exterior = chase camera 6 m behind the car, interior = 1 m), one WAV per other
converted role, and prints / appends table rows to <out dir>/table.tsv: RMS overall and per third of
the sweep, spectral centroid, and the mean absolute octave-band difference to the AC1 render.
Nothing is played on the speakers (tools/fmodrender.py renders through WAVWRITER_NRT)."""
import os
import subprocess
import sys
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
RENDER = os.path.join(HERE, 'fmodrender.py')
AC1 = os.environ.get('ACECONV_AC1_DIR', r'C:\Program Files (x86)\Steam\steamapps\common\assettocorsa')
EVO_SFX = os.environ.get('ACECONV_EVO_SFX')  # a folder with the game's content\sfx banks (unpacked)


def run(args):
    r = subprocess.run([sys.executable, '-I', RENDER] + args, capture_output=True, text=True)
    if r.returncode:
        raise RuntimeError(r.stderr.strip() or r.stdout.strip())
    return r.stdout


def load(p):
    w = wave.open(p)
    n, ch, sw, rate = w.getnframes(), w.getnchannels(), w.getsampwidth(), w.getframerate()
    b = w.readframes(n)
    x = np.frombuffer(b, '<i2').astype(np.float32) / 32768 if sw == 2 else np.frombuffer(b, '<f4')
    return x.reshape(-1, ch).mean(1), rate


def db(x):
    return 20 * np.log10(np.sqrt((x ** 2).mean()) + 1e-9)


EDGES = [63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]


def stats(p):
    m, r = load(p)
    thirds = np.array_split(m, 3)
    f = np.abs(np.fft.rfft(m * np.hanning(len(m)))) ** 2
    fr = np.fft.rfftfreq(len(m), 1 / r)
    tot = f.sum() + 1e-20
    bands = [10 * np.log10(f[(fr >= lo) & (fr < hi)].sum() / tot + 1e-12) for lo, hi in zip(EDGES, EDGES[1:])]
    cent = float((np.sqrt(f) * fr).sum() / (np.sqrt(f).sum() + 1e-9))
    return {'rms': db(m), 'thirds': [db(t) for t in thirds], 'bands': bands, 'centroid': cent}


def guids(path):
    m = {}
    if os.path.isfile(path):
        for line in open(path, encoding='utf-8', errors='replace'):
            p = line.split()
            if len(p) == 2:
                m[p[1]] = p[0]
    return m


def main():
    out, ac1car, conv, cid, stock = sys.argv[1:6]
    lim = float(sys.argv[6]) if len(sys.argv) > 6 else 7000.0
    d = os.path.join(out, cid)
    os.makedirs(d, exist_ok=True)
    sfx = EVO_SFX
    game = [os.path.join(sfx, 'master_bank.strings.bank'), os.path.join(sfx, 'master_bank.bank')]
    conv_banks = game + [os.path.join(conv, 'master_bank.strings.bank'), os.path.join(conv, 'master_bank.bank')]
    stock_banks = game + [os.path.join(sfx, stock + '.bank')]
    folder = os.path.basename(os.path.normpath(ac1car))
    ac1_banks = [os.path.join(AC1, 'content', 'sfx', 'common.strings.bank'), os.path.join(AC1, 'content', 'sfx', 'common.bank'),
                 os.path.join(ac1car, 'sfx', folder + '.bank')]
    names = guids(os.path.join(AC1, 'content', 'sfx', 'GUIDs.txt'))
    names.update(guids(os.path.join(ac1car, 'sfx', 'GUIDs.txt')))
    pos = {'ext': '0,0,6', 'int': '0,0,1'}
    sweep = ['--set', 'rpms=1000@0', '--set', 'rpms=%d@8' % lim]
    rows = []
    for side in ('ext', 'int'):
        for load_, thr in (('on', 1), ('off', 0)):
            tag = '%s_%s' % (side, load_)
            files = {}
            jobs = [('conv', conv_banks, 'event:/evo_cars/%s/engine_%s' % (cid, side), 'evo', 'throttle_position'),
                    ('stock', stock_banks, 'event:/evo_cars/%s/engine_%s' % (stock, side), 'evo', 'throttle_position'),
                    ('ac1', ac1_banks, names.get('event:/cars/%s/engine_%s' % (folder, side)), 'ac1', 'throttle')]
            for src, banks, ev, rt, tp in jobs:
                if not ev:
                    continue
                f = os.path.join(d, '%s_%s.wav' % (src, tag))
                extra = ['--set', 'ignition=2', '--set', 'starter=2'] if rt == 'evo' else []
                try:
                    run(['render', '--rt', rt] + banks + ['--event', ev, '--out', f, '--secs', '8', '--pos', pos[side]] + sweep + ['--set', '%s=%d' % (tp, thr)]
                        + extra)
                    files[src] = f
                except RuntimeError as e:
                    print('render %s %s: %s' % (src, tag, e), file=sys.stderr)
            st = {k: stats(v) for k, v in files.items()}
            for src, s in st.items():
                diff = ''
                if src != 'ac1' and 'ac1' in st:
                    diff = '%.1f' % np.mean([abs(a - b) for a, b in zip(s['bands'][1:7], st['ac1']['bands'][1:7])])
                rows.append([cid, tag, src, '%.1f' % s['rms']] + ['%.1f' % t for t in s['thirds']] + ['%.0f' % s['centroid'], diff])
    # the other converted roles: one render each, to show they play
    extra = [('gearbox', ['--set', 'gear_up=1']),  # the game starts the gearbox event per shift
             ('horn', ['--set', 'horn_sound=1@0', '--set', 'horn_sound=1@2', '--set', 'horn_sound=0@2.01']),
             ('turbo', ['--set', 'rpms=3000@0', '--set', 'rpms=%d@3' % lim, '--set', 'throttle_position=1@0', '--set', 'throttle_position=1@3',
                        '--set', 'throttle_position=0@3.05', '--set', 'boost=0@0', '--set', 'boost=1.2@3', '--set', 'boost=0@3.5', '--set', 'bov=0@0',
                        '--set', 'bov=0@3', '--set', 'bov=1@3.05']),
             ('backfire', ['--set', 'rpms=%d' % (lim * 0.8), '--set', 'throttle_position=1@0', '--set', 'throttle_position=1@1', '--set', 'throttle_position=0@1.1',
                           '--set', 'throttle_position=1@2', '--set', 'throttle_position=1@2.5', '--set', 'throttle_position=0@2.6']),
             ('transmission', ['--set', 'drivetrain_speed=0@0', '--set', 'drivetrain_speed=300@4', '--set', 'throttle_position=1'])]
    for role, sets in extra:
        f = os.path.join(d, 'conv_%s.wav' % role)
        try:
            run(['render'] + conv_banks + ['--event', 'event:/evo_cars/%s/%s' % (cid, role), '--out', f, '--secs', '4', '--pos', '0,0,6'] + sets)
            s = stats(f)
            rows.append([cid, role, 'conv', '%.1f' % s['rms']] + ['%.1f' % t for t in s['thirds']] + ['%.0f' % s['centroid'], ''])
        except RuntimeError as e:
            msg = str(e).splitlines()[-1]
            rows.append([cid, role, 'conv', 'n/a', '', '', '', '', msg[:60]])
    with open(os.path.join(out, 'table.tsv'), 'a', encoding='utf-8') as t:
        for r in rows:
            t.write('\t'.join(r) + '\n')
    for r in rows:
        print('\t'.join(r))


if __name__ == '__main__':
    main()

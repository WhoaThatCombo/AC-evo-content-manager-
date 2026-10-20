"""Fit the converter's AI physics hints (src/car/physics/ai.rs) and the
auto-shift ratios (src/car/physics/drivetrain.rs) to the stock cars.

Input: tools/aifit_stock.tsv, written by
    cargo run --release --example aifit -- <content.kspkg> > tools/aifit_stock.tsv
Output: tools/aifit_fit.tsv (per car: the model inputs, the shipped hint, the
chosen models' predictions and residuals) and tools/aifit_summary.txt (this
script's printout: every candidate form with its residuals, the chosen
parameters, the clamp bounds).

    python tools/aifit.py

Candidate forms are compared on the stock cars by the median absolute
residual (robust: a few stock hints are far off any model) and the RMS.
Forms are listed simplest first (fewest inputs / parameters); a more complex
form replaces the current choice only when it lowers one of the two by at
least IMPROVE and does not raise the other (OURS). Parameters are
least squares where the form is linear in them, else least absolute
deviation by a zooming grid search. Clamp bounds are the stock min / max
widened by MARGIN of the stock span at each end (OURS).
"""

import csv
import math
import os
import re
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
RHO = 1.22  # air density the converter's models use (check.rs RHO)
G = 9.81
MARGIN = 0.10  # OURS: clamp bounds = stock min / max -+ 10 % of the stock span
IMPROVE = 0.05  # OURS: a more complex form must cut a residual measure by 5 %

out_lines = []


def say(*a):
    s = re.sub(r"np\.float64\(([^)]*)\)", r"\1", " ".join(str(x) for x in a))
    out_lines.append(s)
    print(s)


def load():
    with open(os.path.join(HERE, "aifit_stock.tsv"), newline="") as fh:
        rows = list(csv.DictReader(fh, delimiter="\t"))
    cols = {k: np.array([float(r[k]) for r in rows]) for k in rows[0] if k != "id"}
    cols["id"] = [r["id"] for r in rows]
    return cols


def stats(res):
    res = np.asarray(res)
    return float(np.median(np.abs(res))), float(math.sqrt(np.mean(res ** 2)))


def lad_grid(f, x0, spans, rounds=12, n=41):
    """Least absolute deviation: zooming grid around x0 (one span per param)."""
    best = np.array(x0, float)
    spans = np.array(spans, float)
    cost = f(best)
    for _ in range(rounds):
        axes = [np.linspace(b - s, b + s, n) for b, s in zip(best, spans)]
        mesh = np.meshgrid(*axes, indexing="ij")
        for idx in np.ndindex(*[n] * len(best)):
            p = np.array([m[idx] for m in mesh])
            c = f(p)
            if c < cost:
                cost, best = c, p
        spans = spans * 4.0 / n
    return best, cost


def choose(cands):
    """cands: [(name, residuals)] simplest first -> the chosen name."""
    cur, cr = cands[0][0], stats(cands[0][1])
    for name, r in cands[1:]:
        m, q = stats(r)
        better = (m <= cr[0] * (1 - IMPROVE) and q <= cr[1]) or (q <= cr[1] * (1 - IMPROVE) and m <= cr[0])
        if better:
            cur, cr = name, (m, q)
    return cur


def bounds(v):
    lo, hi = float(np.min(v)), float(np.max(v))
    m = MARGIN * (hi - lo)
    return lo, hi, lo - m, hi + m


def main():
    d = load()
    n = len(d["id"])
    say(f"# {n} stock cars (tools/aifit_stock.tsv)")
    m = d["mass_static"]
    s = d["cl_area"] * 0.5 * RHO / (m * G)  # CL.A q / (m g) per (m/s)^2
    muy, mux = d["mu_y"], d["mu_x"]
    bt = d["brake_torque"] / (d["tyre_radius"] * m * G)
    wing_only = d["aero_map"] == 0
    hint, lat, brk = d["aero_hint"], d["lat_low"], d["braking"]
    pred = {}

    # ------------------------------------------------------------ aero_hint
    say("\n## aero_hint (physics_hints 1) vs s = CL.A rho / (2 m g)")
    say(f"fit on the {int(wing_only.sum())} cars without CZ aero maps (AeroData 7 / 8): the other "
        f"{int((~wing_only).sum())} make their downforce in maps the wings' CL.A does not see")
    w = wing_only
    say("scatter (wing-only cars, sorted by s):  s*1e4  aero_hint")
    for i in np.argsort(s[w]):
        say(f"  {s[w][i] * 1e4:8.3f}  {hint[w][i]:.4f}  {np.array(d['id'])[w][i]}")
    # A: linear, least squares
    A = np.vstack([np.ones(w.sum()), s[w]]).T
    (a_lin, b_lin), *_ = np.linalg.lstsq(A, hint[w], rcond=None)
    ra = hint[w] - (a_lin + b_lin * s[w])
    say(f"A  a + b s           a {a_lin!r} b {b_lin!r} s^2/m^2 (= x at {math.sqrt(max(b_lin, 0)):.2f} m/s)"
        f"  median|res| {stats(ra)[0]:.4f} rms {stats(ra)[1]:.4f}")
    # B: load-sensitive tyre, a (1 + s V^2)^n_y, V scanned
    ny = d["ny_f"] * d["cg_front"] + d["ny_r"] * (1 - d["cg_front"])
    best = None
    for V in np.arange(10.0, 100.0, 0.05):
        p = np.clip(1 + s[w] * V * V, 0.05, None) ** ny[w]
        a = float(np.sum(hint[w] * p) / np.sum(p * p))
        r = hint[w] - a * p
        c = stats(r)[1]
        if best is None or c < best[0]:
            best = (c, V, a, r)
    _, v_b, a_b, rb = best
    say(f"B  a (1 + s V^2)^n_y  a {a_b!r} V {v_b:.2f} m/s (least squares over V)"
        f"  median|res| {stats(rb)[0]:.4f} rms {stats(rb)[1]:.4f}")
    use_a = choose([("A", ra), ("B", rb)]) == "A"
    say(f"chosen: {'A' if use_a else 'B'}")
    lo, hi, clo, chi = bounds(hint)
    say(f"stock aero_hint min {lo!r} max {hi!r} (all {n} cars) -> clamp {clo!r} .. {chi!r}")
    pred["aero_hint"] = np.clip(a_lin + b_lin * s, clo, chi) if use_a else np.clip(a_b * np.clip(1 + s * v_b * v_b, 0.05, None) ** ny, clo, chi)
    r_all = hint - pred["aero_hint"]
    say(f"chosen model over the wing-only cars: median residual {np.median(r_all[w]):+.4f}, median|res| {stats(r_all[w])[0]:.4f}, "
        f"p10 / p90 residual {np.percentile(r_all[w], 10):+.4f} / {np.percentile(r_all[w], 90):+.4f}")
    say(f"  (aero-map cars, for reference: median residual {np.median(r_all[~w]):+.4f})")
    aero = dict(a=a_lin, b=b_lin, lo=clo, hi=chi)

    # ---------------------------------------------- low-speed lateral plateau
    say("\n## lateral_G_lowspeed_plateau_end (physics_hints 5.1) vs mu_y")
    say("mu_y: the default compound's static-load lateral mu D_y (Fz/Fz0)^(n_y - 1), averaged over the axles by weight")
    say("scatter (sorted by mu_y):  mu_y  plateau")
    for i in np.argsort(muy):
        say(f"  {muy[i]:.4f}  {lat[i]:.4f}  {d['id'][i]}")
    k = float(np.median(lat / muy))
    r1 = lat - k * muy
    say(f"A  k mu_y            k {k!r} (median ratio)  median|res| {stats(r1)[0]:.4f} rms {stats(r1)[1]:.4f}")
    (a2, b2), *_ = np.linalg.lstsq(np.vstack([np.ones(n), muy]).T, lat, rcond=None)
    r2 = lat - (a2 + b2 * muy)
    say(f"B  a + b mu_y        a {a2!r} b {b2!r}  median|res| {stats(r2)[0]:.4f} rms {stats(r2)[1]:.4f}")
    (lc, p3), *_ = np.linalg.lstsq(np.vstack([np.ones(n), np.log(muy)]).T, np.log(lat), rcond=None)
    c3 = math.exp(lc)
    r3 = lat - c3 * muy ** p3
    say(f"C  c mu_y^p          c {c3!r} p {p3!r} (least squares in logs)  median|res| {stats(r3)[0]:.4f} rms {stats(r3)[1]:.4f}")
    forms = {"A": ("k", k), "B": ("lin", a2, b2), "C": ("pow", c3, p3)}
    pick = choose([("A", r1), ("B", r2), ("C", r3)])
    say(f"chosen: {pick}")
    lo, hi, clo, chi = bounds(lat)
    say(f"stock plateau min {lo!r} max {hi!r} -> clamp {clo!r} .. {chi!r}")
    f = forms[pick]
    raw = k * muy if f[0] == "k" else (a2 + b2 * muy if f[0] == "lin" else c3 * muy ** p3)
    pred["lat_low"] = np.clip(raw, clo, chi)
    r = lat - pred["lat_low"]
    say(f"chosen model: median residual {np.median(r):+.4f}, median|res| {stats(r)[0]:.4f}, p10 / p90 {np.percentile(r, 10):+.4f} / {np.percentile(r, 90):+.4f}")
    latfit = dict(form=f, lo=clo, hi=chi)

    # the other plateau numbers, measured
    say("\n## plateau start / end and tyre slips (stock medians)")
    q = lambda v: float(np.median(v))
    ratios = {
        "low start / end g (4.1 / 5.1)": d["lat_low_start"] / lat,
        "high start / end g (6.1 / 7.1)": d["lat_high_start"] / d["lat_high"],
        "high end / low end / aero_hint (7.1 / 5.1 / 1)": d["lat_high"] / lat / hint,
        "slip low start / low end (4.2 / 5.2)": d["slip_low_start"] / d["slip_low"],
        "slip high start / low end (6.2 / 5.2)": d["slip_high_start"] / d["slip_low"],
        "slip high end / low end (7.2 / 5.2)": d["slip_high"] / d["slip_low"],
    }
    med = {}
    for k_, v in ratios.items():
        med[k_] = q(v)
        say(f"  {k_:48} median {q(v)!r}  p10 {np.percentile(v, 10):.4f}  p90 {np.percentile(v, 90):.4f}")
    # low-end slip: two clusters; the split on mu_y that best separates them
    slip = d["slip_low"]
    bestsplit = None
    for t in np.unique(muy):
        lo_, hi_ = slip[muy <= t], slip[muy > t]
        if len(lo_) < 5 or len(hi_) < 5:
            continue
        c = np.median(np.abs(np.concatenate([lo_ - np.median(lo_), hi_ - np.median(hi_)])))
        if bestsplit is None or c < bestsplit[0]:
            bestsplit = (c, float(t), q(lo_), q(hi_), len(lo_), len(hi_))
    c, t, sl, sh, nl, nh = bestsplit
    say(f"  slip at the low-speed plateau end (5.2): best mu_y split {t!r}: median {sl!r} at or below ({nl} cars), "
        f"{sh!r} above ({nh} cars); median|res| {c:.4f} (one median for all: {q(np.abs(slip - q(slip))):.4f})")

    # -------------------------------------------------------------- braking
    say("\n## acceleration_G_braking (physics_hints 11) vs mu_x and T / (r m g)")
    say("mu_x like mu_y; T the brake torque (BrakesData 1), r the mean tyre radius")
    say("scatter (sorted by T/(r m g)):  T/(r m g)  mu_x  braking")
    for i in np.argsort(bt):
        say(f"  {bt[i]:.4f}  {mux[i]:.4f}  {brk[i]:.4f}  {d['id'][i]}")
    k1 = q(brk / mux)
    k2 = q(brk / bt)
    rb1, rb2 = brk - k1 * mux, brk - k2 * bt
    say(f"A  k mu_x            k {k1!r}  median|res| {stats(rb1)[0]:.4f} rms {stats(rb1)[1]:.4f}")
    say(f"B  k T/(r m g)       k {k2!r}  median|res| {stats(rb2)[0]:.4f} rms {stats(rb2)[1]:.4f}")
    (pm, cm) = lad_grid(lambda p: float(np.sum(np.abs(brk - np.minimum(p[0] * mux, p[1] * bt)))), [k1, k2], [0.5, 1.0])
    rb3 = brk - np.minimum(pm[0] * mux, pm[1] * bt)
    say(f"C  min(a mu_x, b T/(r m g))  a {pm[0]!r} b {pm[1]!r} (least absolute deviation)"
        f"  median|res| {stats(rb3)[0]:.4f} rms {stats(rb3)[1]:.4f}")
    X = np.vstack([np.ones(n), np.log(mux), np.log(bt)]).T
    (lc4, p4, q4), *_ = np.linalg.lstsq(X, np.log(brk), rcond=None)
    rb4 = brk - math.exp(lc4) * mux ** p4 * bt ** q4
    say(f"D  c mu_x^p (T/(r m g))^q  c {math.exp(lc4)!r} p {p4!r} q {q4!r}  median|res| {stats(rb4)[0]:.4f} rms {stats(rb4)[1]:.4f}")
    cand = {"A": (stats(rb1)[0], k1 * mux), "B": (stats(rb2)[0], k2 * bt), "C": (stats(rb3)[0], np.minimum(pm[0] * mux, pm[1] * bt)),
            "D": (stats(rb4)[0], math.exp(lc4) * mux ** p4 * bt ** q4)}
    pick_b = choose([("A", rb1), ("B", rb2), ("C", rb3), ("D", rb4)])
    say(f"chosen: {pick_b}")
    lo, hi, clo, chi = bounds(brk)
    say(f"stock braking min {lo!r} max {hi!r} -> clamp {clo!r} .. {chi!r}")
    pred["braking"] = np.clip(cand[pick_b][1], clo, chi)
    r = brk - pred["braking"]
    say(f"chosen model: median residual {np.median(r):+.4f}, median|res| {stats(r)[0]:.4f}, p10 / p90 {np.percentile(r, 10):+.4f} / {np.percentile(r, 90):+.4f}")

    # ----------------------------------------------------------- auto shift
    say("\n## auto-shift rpm over the limiter")
    ok = np.isfinite(d["ai_up"]) & (d["ai_redline"] < 50000)
    say(f"aicardata (AiGears 1 / 5), {int(ok.sum())} cars with a redline (electric cars: none):")
    for name, v in [("upshift / redline", d["ai_up"][ok] / d["ai_redline"][ok]), ("downshift / redline", d["ai_down"][ok] / d["ai_redline"][ok])]:
        say(f"  {name:22} median {q(v)!r}  min {v.min():.4f}  max {v.max():.4f}")
    g = ok & np.isfinite(d["gb_up"]) & (d["gb_up"] > 0)
    v = d["gb_up"][g] / d["limiter"][g]
    say(f"gearbox (GearBoxData.autoshifter.up / engine limiter), the same {int(g.sum())} cars:")
    say(f"  {'up / limiter':22} median {q(v)!r}  p10 {np.percentile(v, 10):.4f}  p90 {np.percentile(v, 90):.4f}  min {v.min():.4f}  max {v.max():.4f}")
    say(f"  down_rpm_threshold (5): on {int(np.isfinite(d['gb_down']).sum())} stock gearboxes")

    # ------------------------------------------------- longitudinal model
    say("\n## check.rs run: model top speed / shipped top speed (physics_hints 8)")
    v = d["model_top_kmh"][w] / d["top_kmh"][w] - 1
    say(f"the {int(w.sum())} cars without aero maps (their drag is all in the wings): median {np.median(v) * 100:+.1f} %, "
        f"p10 {np.percentile(v, 10) * 100:+.1f} %, p90 {np.percentile(v, 90) * 100:+.1f} %")

    # ---------------------------------------------------------- write out
    with open(os.path.join(HERE, "aifit_fit.tsv"), "w", newline="\n") as fh:
        cols = ["id", "s", "mu_y", "mu_x", "T_rmg", "aero_hint", "aero_hint_fit", "lat_low", "lat_low_fit", "braking", "braking_fit"]
        fh.write("\t".join(cols) + "\n")
        for i in range(n):
            vals = [s[i], muy[i], mux[i], bt[i], hint[i], pred["aero_hint"][i], lat[i], pred["lat_low"][i], brk[i], pred["braking"][i]]
            fh.write(d["id"][i] + "\t" + "\t".join(f"{x:.6g}" for x in vals) + "\n")
    with open(os.path.join(HERE, "aifit_summary.txt"), "w", newline="\n") as fh:
        fh.write("\n".join(out_lines) + "\n")


if __name__ == "__main__":
    sys.exit(main())

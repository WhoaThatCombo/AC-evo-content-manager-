"""Track EDGES from a .aisplinedata file, not just the racing line.

`parse_spline.points()` returns field 1 - the ideal line - and drops field 2,
a parallel array with one entry per point carrying the local frame and the
track width:

    f9   up vector          (x, y, z), y ~= 1
    f10  tangent            (x, y, z), verified parallel to p[i+1]-p[i]
    f3   distance to the left edge from the racing line
    f4   distance to the right edge
    f11  total width, and f3 + f4 == f11 (that is how f3/f4 were identified)

The racing line is NOT the centreline - it hugs the inside of corners - which
is why f3 and f4 differ point by point and why cars legitimately sit several
metres off the drawn line. Drawing the edges instead gives a corridor a car
actually stays inside.
"""
import math

from parse_spline import fields, points


def _norm(v):
    m = math.sqrt(sum(c * c for c in v))
    return [c / m for c in v] if m else list(v)


def _cross(a, b):
    return [a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0]]


def meta(path):
    """The per-point field-2 records, parallel to points()."""
    d = open(path, "rb").read()
    out = []
    for fn, wt, v in fields(d, 0, len(d)):
        if fn != 2 or wt != 2:
            continue
        rec = {}
        for a, w, b in fields(d, v[0], v[1]):
            if w == 5:
                rec[a] = b
            elif w == 2:
                rec[a] = [c for _f, ww, c in fields(d, b[0], b[1]) if ww == 5]
        out.append(rec)
    return out


def edges(path):
    """(left, right) as [(x, z), ...] in world coordinates."""
    pts = points(path)
    md = meta(path)
    left, right = [], []
    for i, p in enumerate(pts):
        if i >= len(md):
            break
        m = md[i]
        up, tan = m.get(9), m.get(10)
        wl, wr = m.get(3), m.get(4)
        if not (up and tan) or wl is None or wr is None:
            continue
        lat = _norm(_cross(_norm(up), _norm(tan)))
        left.append((p[0] + lat[0] * wl, p[2] + lat[2] * wl))
        right.append((p[0] - lat[0] * wr, p[2] - lat[2] * wr))
    return left, right

"""Extract the ideal-line polyline from a .aisplinedata file.

The format is plain protobuf. Top level is field 1 (length-delimited) holding a
long run of point submessages; each point carries a nested message with the
world position as three fixed32 floats:

    1 { 1 { 1:float x, 2:float y, 3:float z }, 2:float ?, 3:varint ? }
"""
import struct, sys


def varint(d, i):
    r = s = 0
    while True:
        b = d[i]; i += 1
        r |= (b & 0x7F) << s
        if not b & 0x80:
            return r, i
        s += 7


def fields(d, i, end):
    """Yield (field_number, wire_type, value_or_slice, next_index)."""
    while i < end:
        key, i = varint(d, i)
        fn, wt = key >> 3, key & 7
        if wt == 0:
            v, i = varint(d, i)
        elif wt == 2:
            ln, i = varint(d, i)
            v = (i, i + ln); i += ln
        elif wt == 5:
            v = struct.unpack_from("<f", d, i)[0]; i += 4
        elif wt == 1:
            v = struct.unpack_from("<d", d, i)[0]; i += 8
        else:
            raise ValueError(f"wire type {wt} at {i}")
        yield fn, wt, v
        

def points(path):
    d = open(path, "rb").read()
    out = []
    for fn, wt, v in fields(d, 0, len(d)):
        if fn == 1 and wt == 2:
            s, e = v
            for pfn, pwt, pv in fields(d, s, e):     # each point
                if pfn != 1 or pwt != 2:
                    continue
                ps, pe = pv
                for vfn, vwt, vv in fields(d, ps, pe):   # point -> vec3 wrapper
                    if vfn != 1 or vwt != 2:
                        continue
                    vs, ve = vv
                    xyz = {f: c for f, w, c in fields(d, vs, ve) if w == 5}
                    if len(xyz) >= 3:
                        out.append((xyz[1], xyz[2], xyz[3]))
            break
    return out


if __name__ == "__main__":
    p = points(sys.argv[1])
    xs = [a for a, _, _ in p]; ys = [b for _, b, _ in p]; zs = [c for _, _, c in p]
    print(f"{len(p)} points")
    print(f"  x {min(xs):9.1f} .. {max(xs):9.1f}   span {max(xs)-min(xs):8.1f}")
    print(f"  y {min(ys):9.1f} .. {max(ys):9.1f}   span {max(ys)-min(ys):8.1f}")
    print(f"  z {min(zs):9.1f} .. {max(zs):9.1f}   span {max(zs)-min(zs):8.1f}")
    d = sum(((p[i][0]-p[i-1][0])**2 + (p[i][2]-p[i-1][2])**2)**.5 for i in range(1, len(p)))
    print(f"  path length (x/z plane): {d:.0f} m")

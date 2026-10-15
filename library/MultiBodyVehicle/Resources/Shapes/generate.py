"""Generate the STL meshes used by the MultiBodyVehicle animation.

    python3 generate.py      # writes *.stl next to this file

Units are metres. Each mesh is in the coordinates of the Shape that draws it
(MB.Visualizers.FixedShape or Advanced.Shape with shapeType="modelica://..."),
with x along lengthDirection, y along widthDirection and z = x cross y:
- wheel parts (tyre, rim, disc, caliper) have their spin axis along x, centred
  on the wheel centre, outboard face towards +x;
- body parts are in the Vehicle body frame (x forward, y left, z up, origin on
  the front axle at wheel-centre height, road at about z = -0.30).
Only numpy is needed.
"""
import math
import os
import struct

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))


class Mesh:
    def __init__(self):
        self.tris = []

    def add(self, a, b, c):
        self.tris.append((np.asarray(a, float), np.asarray(b, float), np.asarray(c, float)))

    def quad(self, a, b, c, d):
        self.add(a, b, c)
        self.add(a, c, d)

    def extend(self, other):
        self.tris.extend(other.tris)
        return self

    def write(self, name):
        path = os.path.join(HERE, name)
        with open(path, "wb") as f:
            f.write(b"MultiBodyVehicle generate.py".ljust(80, b" "))
            f.write(struct.pack("<I", len(self.tris)))
            for a, b, c in self.tris:
                n = np.cross(b - a, c - a)
                ln = np.linalg.norm(n)
                n = n / ln if ln > 0 else n
                f.write(struct.pack("<12fH", *n, *a, *b, *c, 0))
        print(f"{name}: {len(self.tris)} triangles")


def grid(points, closed_u, closed_v, flip=False):
    """Quad mesh over a (nu, nv, 3) grid of points."""
    m = Mesh()
    nu, nv = points.shape[:2]
    for i in range(nu if closed_u else nu - 1):
        i1 = (i + 1) % nu
        for j in range(nv if closed_v else nv - 1):
            j1 = (j + 1) % nv
            a, b, c, d = points[i, j], points[i1, j], points[i1, j1], points[i, j1]
            if flip:
                m.quad(a, d, c, b)
            else:
                m.quad(a, b, c, d)
    return m


def revolve(profile, n=72, radius_fn=None, flip=False):
    """Surface of revolution about x of a closed (x, r) profile; radius_fn(theta, k, r) may modulate r."""
    th = np.linspace(0, 2 * math.pi, n, endpoint=False)
    pts = np.zeros((n, len(profile), 3))
    for i, t in enumerate(th):
        for k, (x, r) in enumerate(profile):
            rr = radius_fn(t, k, x, r) if radius_fn else r
            pts[i, k] = (x, rr * math.cos(t), rr * math.sin(t))
    return grid(pts, True, True, flip)


def loft(sections, cap=True):
    """Closed tube through rings (each an (m, 3) array, same m, counter-clockwise seen from +x)."""
    pts = np.array(sections)
    m = grid(pts, False, True, flip=True)
    if cap:
        for ring, front in ((pts[0], False), (pts[-1], True)):
            c = ring.mean(axis=0)
            for j in range(len(ring)):
                a, b = ring[j], ring[(j + 1) % len(ring)]
                if front:
                    m.add(c, a, b)
                else:
                    m.add(c, b, a)
    return m


def superellipse(yc, zc, hw, hh, n=4.0, m=40, zflat=None):
    out = []
    for k in range(m):
        t = 2 * math.pi * k / m
        c, s = math.cos(t), math.sin(t)
        y = yc + hw * math.copysign(abs(c) ** (2 / n), c)
        z = zc + hh * math.copysign(abs(s) ** (2 / n), s)
        if zflat is not None:
            z = max(z, zflat)
        out.append((y, z))
    return out


def body_loft(stations, n=4.0, m=40):
    secs = []
    for x, yc, hw, zb, zt in stations:
        ring = superellipse(yc, (zb + zt) / 2, hw, (zt - zb) / 2, n, m)
        secs.append([(x, y, z) for y, z in ring])
    return loft(secs)


def box(lo, hi):
    (x0, y0, z0), (x1, y1, z1) = lo, hi
    v = np.array([[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
                  [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]])
    m = Mesh()
    for f in ((0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)):
        m.quad(*(v[i] for i in f))
    return m


def airfoil(x_le, chord, z, span, thick=0.12, camber=0.06, aoa=0.0, n=24):
    """Wing along y from -span/2 to span/2; inverted (downforce) camber."""
    up, lo = [], []
    for k in range(n + 1):
        u = (1 - math.cos(math.pi * k / n)) / 2
        t = 5 * thick * (0.2969 * math.sqrt(u) - 0.126 * u - 0.3516 * u**2 + 0.2843 * u**3 - 0.1036 * u**4)
        c = -camber * 4 * u * (1 - u)
        up.append((u, c + t))
        lo.append((u, c - t))
    ring = up[::-1] + lo[1:-1]
    ca, sa = math.cos(aoa), math.sin(aoa)
    pts2 = [(x_le - chord * (u * ca), z + chord * (v * ca + u * sa)) for u, v in ring]
    secs = []
    for y in (-span / 2, span / 2):
        secs.append([(x, y, zz) for x, zz in pts2][::-1])
    # loft along y: rotate so rings are counter-clockwise seen from +y; reuse loft and accept winding
    return loft(secs)


# ------------------------------------------------------------------ wheel parts
def tyre(R=0.31, W=0.225, Rb=0.19, pitch_deg=7.5):
    s = 0.035  # shoulder radius
    prof = []
    # bead (inboard) -> sidewall -> shoulder -> tread -> shoulder -> sidewall -> bead, then the inside back
    for k in range(7):
        prof.append((-W / 2 + 0.006 * math.sin(math.pi * k / 6), Rb + (R - s - Rb) * k / 6))
    for k in range(1, 7):
        a = math.pi / 2 * k / 6
        prof.append((-W / 2 + s - s * math.cos(a), R - s + s * math.sin(a)))
    tread_x = np.linspace(-W / 2 + s, W / 2 - s, 26)
    tread_start = len(prof)
    for x in tread_x[1:-1]:
        prof.append((x, R))
    tread_end = len(prof)
    for k in range(0, 6):
        a = math.pi / 2 * (6 - k) / 6
        prof.append((W / 2 - s + s * math.cos(a), R - s + s * math.sin(a)))
    for k in range(7):
        prof.append((W / 2 - 0.006 * math.sin(math.pi * k / 6), R - s - (R - s - Rb) * k / 6))
    prof.append((W / 2 - 0.02, Rb - 0.005))
    prof.append((-W / 2 + 0.02, Rb - 0.005))
    n = int(round(360 / pitch_deg * 6))
    depth = 0.008

    def fn(t, k, x, r):
        if not (tread_start <= k < tread_end):
            return r
        if abs(abs(x) - 0.04) < 0.007:  # two circumferential grooves
            return r - depth
        phase = (t / math.radians(pitch_deg)) % 1.0
        off = 0.5 if x > 0 else 0.0  # staggered blocks
        if abs(x) > 0.05 and ((phase + off) % 1.0) < 0.17:  # lateral grooves on the shoulders
            return r - depth
        if abs(x) <= 0.033 and ((phase + 0.25) % 1.0) < 0.08:  # sipes on the centre rib
            return r - depth * 0.5
        return r

    return revolve(prof, n, fn)


def rim(Rr=0.195, W=0.21, spokes=5):
    m = Mesh()
    # barrel with flanges (open inner side faces the hub)
    prof = [(-W / 2, Rr - 0.004), (-W / 2, Rr + 0.012), (-W / 2 + 0.012, Rr + 0.012), (-W / 2 + 0.014, Rr),
            (W / 2 - 0.014, Rr), (W / 2 - 0.012, Rr + 0.012), (W / 2, Rr + 0.012), (W / 2, Rr - 0.004),
            (W / 2 - 0.02, Rr - 0.012), (-W / 2 + 0.02, Rr - 0.012)]
    m.extend(revolve(prof, 96))
    # hub boss and centre nut
    m.extend(revolve([(0.02, 0.0), (0.02, 0.07), (0.075, 0.062), (0.085, 0.045), (0.085, 0.0)], 48))
    m.extend(revolve([(0.085, 0.0), (0.085, 0.025), (0.105, 0.022), (0.105, 0.0)], 6))
    # tapered, slightly swept spokes from the boss to the barrel
    for k in range(spokes):
        a0 = 2 * math.pi * k / spokes
        secs = []
        for r, w, x in ((0.055, 0.050, 0.075), (0.12, 0.040, 0.075), (Rr - 0.004, 0.034, W / 2 - 0.02)):
            a = a0 + 0.12 * (r - 0.055) / Rr
            c, s = math.cos(a), math.sin(a)
            ring = []
            for dy, dz in ((-w / 2, -0.014), (w / 2, -0.014), (w / 2, 0.014), (-w / 2, 0.014)):
                # dy across the spoke (tangential), dz along the axle (x)
                ring.append((x + dz, r * c - dy * s, r * s + dy * c))
            secs.append(ring)
        m.extend(loft(secs))
    return m


def disc(Ro=0.165, Ri=0.095, t=0.028, x=-0.035):
    return revolve([(x - t / 2, Ri), (x - t / 2, Ro), (x + t / 2, Ro), (x + t / 2, Ri)], 96)


def caliper(x=-0.035):
    secs = []
    for k in range(9):
        a = math.radians(20 + 60 * k / 8)
        c, s = math.cos(a), math.sin(a)
        ring = [(x - 0.035, r * c, r * s) for r in (0.13,)] + [(x - 0.035, 0.185 * c, 0.185 * s),
                                                                (x + 0.035, 0.185 * c, 0.185 * s), (x + 0.035, 0.13 * c, 0.13 * s)]
        secs.append(ring)
    return loft(secs)


# ------------------------------------------------------------------ body shell (open-wheel car)
# The body is built once by build_body() as a few closed, outward-oriented parts whose faces are
# sorted into three colour groups: "main" (paint), "dark" (carbon, black) and "accent" (livery).

def pchip(xk, yk, x):
    """Monotone piecewise-cubic (Fritsch-Carlson) interpolation; xk increasing, yk (n,) or (n, k)."""
    xk = np.asarray(xk, float)
    yk = np.asarray(yk, float)
    x = np.atleast_1d(np.asarray(x, float))
    if yk.ndim == 1:
        return pchip(xk, yk[:, None], x)[:, 0]
    h = np.diff(xk)
    d = np.diff(yk, axis=0) / h[:, None]
    m = np.zeros_like(yk)
    for i in range(1, len(xk) - 1):
        a, b = d[i - 1], d[i]
        w1, w2 = 2 * h[i] + h[i - 1], h[i] + 2 * h[i - 1]
        with np.errstate(divide="ignore", invalid="ignore"):
            hm = (w1 + w2) / (w1 / a + w2 / b)
        m[i] = np.where(a * b > 0, hm, 0.0)
    m[0], m[-1] = d[0], d[-1]
    idx = np.clip(np.searchsorted(xk, x) - 1, 0, len(xk) - 2)
    t = ((x - xk[idx]) / h[idx])[:, None]
    hh = h[idx][:, None]
    h00, h10, h01, h11 = 2 * t**3 - 3 * t**2 + 1, t**3 - 2 * t**2 + t, -2 * t**3 + 3 * t**2, t**3 - t**2
    return h00 * yk[idx] + h10 * hh * m[idx] + h01 * yk[idx + 1] + h11 * hh * m[idx + 1]


def smoothstep(t):
    t = np.clip(t, 0.0, 1.0)
    return t * t * (3 - 2 * t)


def signed_volume(tris):
    a, b, c = tris[:, 0], tris[:, 1], tris[:, 2]
    return float(np.einsum("ij,ij->i", a, np.cross(b, c)).sum() / 6)


class Part:
    """Closed triangle surface with a colour tag per triangle; orient() makes it point outwards."""

    def __init__(self):
        self.tris, self.tags = [], []

    def add(self, a, b, c, tag):
        self.tris.append((a, b, c))
        self.tags.append(tag)

    def quad(self, a, b, c, d, tag):
        self.add(a, b, c, tag)
        self.add(a, c, d, tag)

    def orient(self):
        t = np.array(self.tris, float)
        if signed_volume(t) < 0:
            self.tris = [(a, c, b) for a, b, c in self.tris]
        return self


def loft_part(rings, tag="main", tagfn=None, cap_tags=None):
    """Closed tube through rings (k, m, 3); tagfn(i, j) colours the quad between rings i, i+1 and points j, j+1."""
    P = np.asarray(rings, float)
    k, m = P.shape[:2]
    part = Part()
    for i in range(k - 1):
        for j in range(m):
            j1 = (j + 1) % m
            part.quad(P[i, j], P[i + 1, j], P[i + 1, j1], P[i, j1], tagfn(i, j) if tagfn else tag)
    ct = cap_tags or (tag, tag)
    for ring, tg in ((P[0], ct[0]), (P[-1], ct[1])):
        c = ring.mean(axis=0)
        for j in range(m):
            part.add(c, ring[(j + 1) % m], ring[j], tg)
    return part.orient()


U_KNOTS = np.array([0.0, 0.1, 0.25, 0.45, 0.65, 0.82, 0.93, 1.0])


def section(yc, zb, zt, hw, nb=4, ns=18, nt=12):
    """Ring (y, z) of a body section: flat-ish bottom, side whose half-width follows hw at U_KNOTS of the
    height, flat-ish top. Counter-clockwise seen from +x; point count 2 * (nb + ns + nt)."""
    u = (1 - np.cos(np.linspace(0, math.pi, ns + 1))) / 2
    hwu = pchip(U_KNOTS, hw, u)
    right = [(hw[0] * k / nb, zb) for k in range(nb)]
    right += [(w, zb + uu * (zt - zb)) for uu, w in zip(u, hwu)]
    right += [(hw[-1] * (1 - k / nt), zt) for k in range(1, nt)]
    ring = right + [(0.0, zt)] + [(-y, z) for y, z in reversed(right[1:])]
    return [(yc + y, z) for y, z in ring]


def station_loft(stations, xs, yc=0.0, dip=None, tagfn=None, cap_tags=None, tag="main", **kw):
    """Loft through stations (x, zb, zt, hw[8]) resampled smoothly at xs. dip(x, y) lowers the top
    (cockpit); returns the Part and the per-point dip used for colouring."""
    st = sorted(stations, key=lambda s: s[0])
    xk = [s[0] for s in st]
    vals = np.array([[s[1], s[2], *s[3]] for s in st])
    rings, dips = [], []
    for x, v in zip(xs, pchip(xk, vals, xs)):
        ring = section(yc, v[0], v[1], v[2:], **kw)
        dd = [dip(x, y - yc) if dip and z > 0.5 * (v[0] + v[1]) else 0.0 for y, z in ring]
        dips.append(dd)
        rings.append([(x, y, z - d) for (y, z), d in zip(ring, dd)])
    return rings, np.array(dips)


def airfoil_ring(x_le, chord, z, thick=0.12, camber=0.06, aoa=0.0, n=16):
    """Inverted (downforce) cambered section in the x-z plane, leading edge at x_le, trailing edge aft."""
    up, lo = [], []
    for k in range(n + 1):
        u = (1 - math.cos(math.pi * k / n)) / 2
        t = 5 * thick * (0.2969 * math.sqrt(u) - 0.126 * u - 0.3516 * u**2 + 0.2843 * u**3 - 0.1036 * u**4)
        c = -camber * 4 * u * (1 - u)
        up.append((u, c + t))
        lo.append((u, c - t))
    ring = up[::-1] + lo[1:-1]
    ca, sa = math.cos(aoa), math.sin(aoa)
    return [(x_le - chord * (u * ca - v * sa), z + chord * (v * ca + u * sa)) for u, v in ring]


def wing_part(ys, fn, tag="dark", **kw):
    """Wing element along y; fn(y) -> (x_le, chord, z, aoa) at each spanwise station."""
    rings = []
    for y in ys:
        x_le, chord, z, aoa = fn(y)
        rings.append([(x, y, zz) for x, zz in airfoil_ring(x_le, chord, z, aoa=aoa, **kw)])
    return loft_part(rings, tag)


def ear_clip(poly):
    """Triangulate a simple 2-D polygon (counter-clockwise or not); returns index triples."""
    P = np.asarray(poly, float)
    area = 0.5 * np.sum(P[:, 0] * np.roll(P[:, 1], -1) - np.roll(P[:, 0], -1) * P[:, 1])
    idx = list(range(len(P))) if area > 0 else list(range(len(P)))[::-1]
    out = []

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    guard = 0
    while len(idx) > 3 and guard < 10000:
        guard += 1
        for k in range(len(idx)):
            i0, i1, i2 = idx[k - 1], idx[k], idx[(k + 1) % len(idx)]
            a, b, c = P[i0], P[i1], P[i2]
            if cross(a, b, c) <= 1e-12:
                continue
            if any(cross(a, b, P[q]) > 0 and cross(b, c, P[q]) > 0 and cross(c, a, P[q]) > 0
                   for q in idx if q not in (i0, i1, i2)):
                continue
            out.append((i0, i1, i2))
            idx.pop(k)
            break
        else:
            break
    if len(idx) == 3:
        out.append(tuple(idx))
    return out


def plate(poly, axis, pos, t, tag="dark", side_tag=None):
    """Flat plate: 2-D polygon extruded by t about pos along axis 'y' (poly in x-z) or 'z' (poly in x-y)."""
    def p3(u, v, w):
        return (u, w, v) if axis == "y" else (u, v, w)
    part = Part()
    tri = ear_clip(poly)
    n = len(poly)
    lo, hi = pos - t / 2, pos + t / 2
    for a, b, c in tri:
        part.add(p3(*poly[a], lo), p3(*poly[b], lo), p3(*poly[c], lo), tag)
        part.add(p3(*poly[a], hi), p3(*poly[c], hi), p3(*poly[b], hi), tag)
    for k in range(n):
        a, b = poly[k], poly[(k + 1) % n]
        part.quad(p3(*a, lo), p3(*b, lo), p3(*b, hi), p3(*a, hi), side_tag or tag)
    return part.orient()


def catmull(points, per=8):
    P = np.asarray(points, float)
    P = np.vstack([2 * P[0] - P[1], P, 2 * P[-1] - P[-2]])
    out = []
    for i in range(1, len(P) - 2):
        p0, p1, p2, p3 = P[i - 1], P[i], P[i + 1], P[i + 2]
        for k in range(per):
            t = k / per
            out.append(0.5 * ((2 * p1) + (-p0 + p2) * t + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t**2
                              + (-p0 + 3 * p1 - 3 * p2 + p3) * t**3))
    out.append(P[-2])
    return np.array(out)


def tube(path, radius, n=12, tag="dark", scale=None):
    """Tube along a polyline; radius may be a function of the path fraction; scale=(sy, sz) squashes it."""
    path = np.asarray(path, float)
    T = np.gradient(path, axis=0)
    T /= np.linalg.norm(T, axis=1)[:, None]
    ref = np.array([0, 0, 1.0]) if abs(T[0][2]) < 0.9 else np.array([1.0, 0, 0])
    N = np.cross(ref, T[0])
    N /= np.linalg.norm(N)
    rings = []
    for k, (p, t) in enumerate(zip(path, T)):
        N = N - t * (N @ t)
        N /= np.linalg.norm(N)
        B = np.cross(t, N)
        r = radius(k / (len(path) - 1)) if callable(radius) else radius
        a = np.linspace(0, 2 * math.pi, n, endpoint=False)
        rings.append([p + r * (math.cos(q) * N + math.sin(q) * B) for q in a])
    return loft_part(rings, tag)


def xs_between(*spans):
    """Concatenate (x0, x1, step) spans running from front (large x) to back."""
    out = []
    for x0, x1, dx in spans:
        n = max(2, int(round(abs(x1 - x0) / dx)) + 1)
        seg = np.linspace(x0, x1, n)
        out.extend(seg[1:] if out else seg)
    return np.array(out)


_BODY = None


def build_body():
    global _BODY
    if _BODY is not None:
        return _BODY
    parts = []

    # --- survival cell: nose, monocoque, engine cover and gearbox fairing (x, zb, zt, half widths)
    chassis = [
        (1.93, -0.215, -0.165, [.012, .02, .026, .028, .028, .025, .02, .01]),
        (1.88, -0.222, -0.140, [.025, .036, .042, .045, .045, .041, .034, .02]),
        (1.74, -0.222, -0.105, [.036, .050, .057, .060, .060, .055, .046, .028]),
        (1.55, -0.205, -0.050, [.045, .063, .072, .076, .076, .071, .060, .036]),
        (1.15, -0.180, 0.025, [.055, .078, .090, .095, .094, .088, .075, .045]),
        (0.70, -0.165, 0.110, [.075, .100, .116, .122, .120, .112, .096, .058]),
        (0.30, -0.180, 0.180, [.115, .145, .165, .172, .168, .155, .132, .080]),
        (-0.05, -0.215, 0.212, [.165, .195, .214, .220, .215, .200, .175, .125]),
        (-0.35, -0.228, 0.222, [.200, .232, .250, .255, .250, .238, .218, .195]),
        (-0.65, -0.230, 0.228, [.210, .245, .262, .268, .264, .250, .225, .195]),
        (-0.95, -0.230, 0.232, [.210, .245, .262, .266, .258, .242, .220, .195]),
        (-1.20, -0.230, 0.265, [.205, .238, .250, .245, .225, .190, .150, .085]),
        (-1.55, -0.228, 0.268, [.185, .212, .218, .208, .182, .145, .108, .055]),
        (-2.00, -0.222, 0.215, [.150, .165, .165, .155, .135, .108, .080, .040]),
        (-2.45, -0.205, 0.150, [.115, .125, .124, .117, .103, .084, .062, .030]),
        (-2.85, -0.170, 0.085, [.085, .092, .092, .087, .077, .063, .047, .024]),
        (-3.15, -0.120, 0.040, [.055, .060, .060, .057, .051, .042, .032, .016]),
        (-3.28, -0.080, 0.015, [.022, .028, .030, .030, .027, .023, .017, .008]),
    ]
    # cockpit opening (plan view superellipse) where the top is pressed down into a dark tub
    x_front, x_rear, hwid, depth = -0.32, -0.96, 0.150, 0.20

    def dip(x, y):
        fx = smoothstep((x_front - x) / 0.035) * smoothstep((x - x_rear) / 0.035)
        return depth * float(fx * smoothstep((hwid - abs(y)) / 0.022))

    xs = xs_between((1.93, 1.60, 0.03), (1.60, -0.25, 0.06), (-0.25, -1.03, 0.02),
                    (-1.05, -3.28, 0.06))
    rings, dips = station_loft(chassis, xs, dip=dip, nb=5, ns=18, nt=14)

    def ctag(i, j):
        m = len(rings[0])
        dd = max(dips[i, j], dips[i + 1, j], dips[i, (j + 1) % m], dips[i + 1, (j + 1) % m])
        if dd > 0.03:
            return "dark"
        xm = 0.5 * (xs[i] + xs[i + 1])
        if xm > 1.50:
            return "accent"
        return "main"
    parts.append(loft_part(rings, tagfn=ctag, cap_tags=("accent", "dark")))

    # --- sidepods: inlet (recessed, dark), undercut, then sweeping down into the coke-bottle waist
    pod = [
        (-0.30, 0.300, -0.030, 0.140, [.115, .140, .152, .158, .160, .160, .156, .140]),
        (-0.42, 0.300, -0.150, 0.150, [.040, .060, .100, .148, .165, .166, .160, .140]),
        (-0.80, 0.295, -0.205, 0.140, [.050, .070, .100, .140, .158, .160, .150, .125]),
        (-1.20, 0.275, -0.210, 0.098, [.060, .080, .100, .125, .135, .132, .120, .090]),
        (-1.60, 0.240, -0.212, 0.035, [.065, .078, .088, .098, .100, .095, .080, .050]),
        (-1.95, 0.205, -0.212, -0.045, [.060, .066, .070, .072, .068, .060, .048, .028]),
        (-2.20, 0.175, -0.212, -0.120, [.040, .044, .046, .046, .043, .038, .030, .018]),
    ]
    st = sorted(pod, key=lambda s: s[0])
    xk = [s[0] for s in st]
    vals = np.array([[s[1], s[2], s[3], *s[4]] for s in st])
    pxs = xs_between((-0.30, -2.20, 0.045))
    for side in (1, -1):
        rings = []
        v0 = vals[-1]  # station at the inlet lip (sorted ascending, last = -0.30)
        lip = section(side * v0[0], v0[1], v0[2], v0[3:], nb=4, ns=14, nt=8)
        ym, zm = side * v0[0] + side * 0.02, 0.5 * (v0[1] + v0[2]) + 0.01
        for x, s in ((-0.40, 0.70), (-0.31, 0.84)):
            rings.append([(x, ym + s * (y - ym), zm + s * (z - zm)) for y, z in lip])
        for x, v in zip(pxs, pchip(xk, vals, pxs)):
            ring = section(side * v[0], v[1], v[2], v[3:], nb=4, ns=14, nt=8)
            if side < 0:
                ring = ring  # same ccw order: the section is symmetric about its own centre
            rings.append([(x, y, z) for y, z in ring])
        rings = rings[::-1]  # front to back order does not matter; keep tail first for the caps
        nr = len(rings)
        parts.append(loft_part(rings, tagfn=lambda i, j, nr=nr: "dark" if i >= nr - 2 else "main",
                               cap_tags=("main", "dark")))

    # --- airbox / roll hoop above the driver's head, with a recessed dark intake
    abox = [
        (-0.97, 0.140, 0.455, [.100, .104, .100, .090, .074, .058, .042, .018]),
        (-1.10, 0.140, 0.470, [.098, .100, .095, .085, .070, .054, .038, .016]),
        (-1.35, 0.130, 0.452, [.090, .088, .082, .072, .059, .044, .030, .014]),
        (-1.75, 0.100, 0.370, [.070, .066, .060, .052, .042, .031, .021, .010]),
        (-2.20, 0.050, 0.230, [.050, .046, .042, .036, .029, .021, .014, .007]),
    ]
    st = sorted(abox, key=lambda s: s[0])
    xk = [s[0] for s in st]
    vals = np.array([[s[1], s[2], *s[3]] for s in st])
    v0 = vals[-1]
    lip = section(0, v0[0], v0[1], v0[2:], nb=3, ns=12, nt=6)
    zm = 0.365
    rings = []
    for x, s in ((-1.05, 0.62), (-0.98, 0.80)):
        rings.append([(x, s * y, zm + s * (z - zm)) for y, z in lip])
    axs = xs_between((-0.97, -2.20, 0.06))
    for x, v in zip(axs, pchip(xk, vals, axs)):
        rings.append([(x, y, z) for y, z in section(0, v[0], v[1], v[2:], nb=3, ns=12, nt=6)])
    parts.append(loft_part(rings, tagfn=lambda i, j: "dark" if i < 1 else "main", cap_tags=("dark", "main")))
    # T-camera on the airbox and the shark fin
    parts.append(loft_part([[(x, y, z) for y, z in section(0, 0.465, 0.49, [.03] * 8, 2, 4, 3)]
                            for x in (-1.03, -1.10)], "accent"))
    fin = [(-1.40, 0.43), (-1.80, 0.40), (-2.20, 0.335), (-2.55, 0.270), (-2.66, 0.255),
           (-2.68, 0.13), (-2.30, 0.19), (-1.80, 0.32), (-1.45, 0.40)]
    parts.append(plate(fin, "y", 0.0, 0.012, "main"))

    # --- floor: planform half width w(x), with edge wings, tunnel fences and a plank
    fl = [(0.32, 0.17), (0.05, 0.26), (-0.22, 0.42), (-0.42, 0.64), (-0.62, 0.70), (-2.08, 0.70),
          (-2.20, 0.64), (-2.24, 0.55), (-2.96, 0.55), (-3.05, 0.53)]
    rings = []
    for x, w in fl:
        rings.append([(x, -w, -0.240), (x, w, -0.240), (x, w, -0.222), (x, -w, -0.222)])
    parts.append(loft_part(rings, "dark"))
    parts.append(loft_part([[(x, -0.13, -0.250), (x, 0.13, -0.250), (x, 0.13, -0.238), (x, -0.13, -0.238)]
                            for x in (-0.10, -2.20)], "dark"))
    for side in (1, -1):
        # floor edge wing: a curled strip along the outer edge
        edge = [(-0.55, -0.222), (-0.70, -0.175), (-1.70, -0.170), (-2.05, -0.190), (-2.12, -0.222)]
        parts.append(plate(edge, "y", side * 0.690, 0.010, "dark"))
        # tunnel inlet fences under the chassis
        for k, y0 in enumerate((0.20, 0.27, 0.34)):
            fence = [(0.10 - 0.06 * k, -0.235), (-0.05 - 0.06 * k, -0.120 - 0.02 * k),
                     (-0.55 - 0.05 * k, -0.150), (-0.60 - 0.05 * k, -0.235)]
            parts.append(plate(fence, "y", side * (y0 + 0.02 * k), 0.008, "dark"))

    # --- diffuser: ramped roof, outer walls and strakes
    def droof(x):
        return -0.222 + 0.18 * float(smoothstep((-2.24 - x) / 0.90)) ** 0.8
    dxs = np.linspace(-2.24, -3.14, 16)
    rings = [[(x, -0.55, droof(x)), (x, 0.55, droof(x)), (x, 0.55, droof(x) + 0.014), (x, -0.55, droof(x) + 0.014)]
             for x in dxs]
    parts.append(loft_part(rings, "dark"))
    for y in (0.55, 0.38, 0.20, -0.20, -0.38, -0.55):
        wall = [(x, droof(x)) for x in dxs] + [(-3.14, -0.236), (-2.24, -0.236)]
        if abs(y) > 0.5:
            wall = [(x, droof(x) + 0.02) for x in dxs] + [(-3.14, -0.236), (-2.24, -0.236)]
        parts.append(plate(wall[::-1], "y", y, 0.010, "dark"))

    # --- front wing: four elements (inboard low, outboard flaps curling up into the endplates)
    def fw(x_le, chord, z, aoa, lift, sweep):
        def f(y):
            s = abs(y) / 0.93
            up = lift * float(smoothstep((s - 0.55) / 0.45))
            return (x_le - sweep * s * s, chord * (1 - 0.15 * s), z + up, aoa + 3.0 * up)
        return f
    fys = np.concatenate([-np.linspace(0.93, 0.0, 15)[:-1], np.linspace(0.0, 0.93, 15)])
    for args in ((1.93, 0.240, -0.262, 0.10, 0.010, 0.03), (1.72, 0.135, -0.228, 0.32, 0.02, 0.05),
                 (1.62, 0.115, -0.192, 0.55, 0.03, 0.07), (1.545, 0.100, -0.155, 0.80, 0.04, 0.09)):
        tag = "main" if args[0] < 1.6 else "dark"
        parts.append(wing_part(fys, fw(*args), tag, thick=0.11, camber=0.05))
    for side in (1, -1):
        ep = [(1.96, -0.272), (1.98, -0.240), (1.93, -0.185), (1.80, -0.140), (1.62, -0.108),
              (1.46, -0.100), (1.43, -0.150), (1.46, -0.272)]
        parts.append(plate(ep, "y", side * 0.945, 0.012, "accent"))
        foot = [(1.96, 0.90), (1.46, 0.90), (1.46, 0.99), (1.92, 0.99)]
        parts.append(plate([(x, side * y) for x, y in foot], "z", -0.272, 0.008, "dark"))

    # --- rear wing: spoon main plane + DRS flap, rounded endplates, pylon, beam wing
    def rw(x_le, chord, z, aoa, drop):
        def f(y):
            s = abs(y) / 0.49
            d = drop * float(smoothstep((s - 0.6) / 0.4)) ** 1.5
            return (x_le + 0.02 * s * s, chord, z - d, aoa)
        return f
    rys = np.concatenate([-np.linspace(0.49, 0.0, 13)[:-1], np.linspace(0.0, 0.49, 13)])
    parts.append(wing_part(rys, rw(-2.93, 0.250, 0.505, 0.14, 0.06), "dark", thick=0.12, camber=0.07))
    parts.append(wing_part(rys, rw(-3.135, 0.160, 0.585, 0.62, 0.05), "main", thick=0.10, camber=0.06))
    for side in (1, -1):
        ep = [(-2.86, 0.39), (-2.86, 0.58), (-2.90, 0.70), (-2.98, 0.755), (-3.10, 0.775), (-3.26, 0.77),
              (-3.36, 0.73), (-3.38, 0.60), (-3.30, 0.42), (-3.18, 0.34), (-3.00, 0.33)]
        parts.append(plate(ep, "y", side * 0.495, 0.014, "dark"))
    pylon = [(-2.96, 0.08), (-3.12, 0.08), (-3.13, 0.30), (-3.08, 0.52), (-3.00, 0.52), (-2.98, 0.30)]
    for y in (0.035, -0.035):
        parts.append(plate(pylon, "y", y, 0.012, "dark"))
    bys = np.linspace(-0.40, 0.40, 9)
    parts.append(wing_part(bys, lambda y: (-2.98, 0.11, 0.115 + 0.08 * (abs(y) / 0.4) ** 2, 0.15), "dark",
                           thick=0.11, camber=0.06))
    parts.append(wing_part(bys, lambda y: (-3.09, 0.09, 0.155 + 0.08 * (abs(y) / 0.4) ** 2, 0.50), "dark",
                           thick=0.11, camber=0.06))
    # rain light on the crash structure
    parts.append(loft_part([[(x, y, z) for y, z in section(0, -0.04, 0.0, [.025] * 8, 2, 4, 3)]
                            for x in (-3.25, -3.30)], "accent"))

    # --- halo: titanium hoop around the helmet with its central pillar
    hoop = catmull([(-1.00, 0.195, 0.205), (-0.93, 0.192, 0.300), (-0.80, 0.186, 0.338), (-0.62, 0.172, 0.352),
                    (-0.49, 0.125, 0.358), (-0.425, 0.055, 0.360), (-0.415, 0.0, 0.360),
                    (-0.425, -0.055, 0.360), (-0.49, -0.125, 0.358), (-0.62, -0.172, 0.352),
                    (-0.80, -0.186, 0.338), (-0.93, -0.192, 0.300), (-1.00, -0.195, 0.205)], per=6)
    parts.append(tube(hoop, 0.020, 14, "dark"))
    pillar = catmull([(-0.42, 0.0, 0.355), (-0.36, 0.0, 0.300), (-0.29, 0.0, 0.240), (-0.24, 0.0, 0.205)], per=5)
    parts.append(tube(pillar, lambda f: 0.019 + 0.010 * f, 12, "dark"))

    # --- mirrors on stalks beside the cockpit: streamlined housing, dark glass facing aft
    for side in (1, -1):
        rings = []
        for x, s in ((-0.150, 0.25), (-0.165, 0.62), (-0.190, 0.88), (-0.220, 0.98), (-0.250, 1.0), (-0.262, 0.93)):
            ring = section(side * 0.365, 0.262 - 0.024 * s, 0.262 + 0.022 * s, [0.075 * (0.4 + 0.6 * s)] * 8, 2, 8, 4)
            rings.append([(x, y, z) for y, z in ring])
        parts.append(loft_part(rings[::-1], "main", cap_tags=("dark", "main")))
        for a, b in (((-0.225, 0.315, 0.250), (-0.265, 0.205, 0.214)), ((-0.225, 0.40, 0.245), (-0.245, 0.41, 0.135))):
            stalk = catmull([a, ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2), b], per=3)
            parts.append(tube([(x, side * y, z) for x, y, z in stalk], 0.009, 8, "dark"))

    out = {"main": Mesh(), "dark": Mesh(), "accent": Mesh()}
    for p in parts:
        for (a, b, c), tg in zip(p.tris, p.tags):
            out[tg].add(a, b, c)
    _BODY = out
    return out


def body_main():
    return build_body()["main"]


def body_dark():
    return build_body()["dark"]


def body_accent():
    return build_body()["accent"]


def bump(n=48):
    """Unit half-cosine road bump for extra=1 shapes: x 0..1 along the road, y 0..1 across, z = (1 - cos 2 pi x)/2."""
    x = np.linspace(0, 1, n + 1)
    z = 0.5 * (1 - np.cos(2 * np.pi * x))
    pts = np.array([[[xi, yj, zi] for yj in (0.0, 1.0)] for xi, zi in zip(x, z)])
    m = grid(pts, False, False)
    for k in range(n):
        # side caps (y = 0 seen from -y, y = 1 seen from +y)
        a, b = np.array([x[k], 0, 0]), np.array([x[k + 1], 0, 0])
        m.quad(a, b, np.array([x[k + 1], 0, z[k + 1]]), np.array([x[k], 0, z[k]]))
        a, b = np.array([x[k], 1, 0]), np.array([x[k + 1], 1, 0])
        m.quad(a, np.array([x[k], 1, z[k]]), np.array([x[k + 1], 1, z[k + 1]]), b)
    return m


if __name__ == "__main__":
    tyre().write("tyre.stl")
    rim().write("rim.stl")
    disc().write("disc.stl")
    caliper().write("caliper.stl")
    body_main().write("body_main.stl")
    body_dark().write("body_dark.stl")
    body_accent().write("body_accent.stl")
    bump().write("bump.stl")

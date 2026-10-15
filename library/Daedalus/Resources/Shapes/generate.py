"""Generate the STL meshes used by Daedalus.Visualizers (the 3-D Animation view of the examples).

    python3 generate.py      # writes *.stl next to this file

Units are metres. Each mesh is in the coordinates of the Shape that draws it, with x along
lengthDirection, y along widthDirection and z = x cross y (extra=0: drawn unscaled):
- car_* (Visualizers.Car): x forward, y left, z up, origin on the ground under the rear axle;
- car_tyre and car_rim: spin axis along x, centred on the wheel centre, outboard face towards +x;
- wing and aileron (Visualizers.Aileron): span along x, hinge line on the x axis, leading edge +y;
- gear_wing (Visualizers.LandingGear): chord along x, span along y.
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
            f.write(b"Daedalus generate.py".ljust(80, b" "))
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




# ------------------------------------------------------------------ saloon car (Visualizers.Car)
# Car frame: origin on the ground under the rear axle, x forward, y left, z up.
WB, RW = 2.7, 0.32  # wheelbase and wheel radius (Visualizers.Car defaults)


def tapered(yc, zb, zt, hw_bot, hw_top, n=5.0, m=48):
    """Superellipse ring whose half-width goes from hw_bot at zb to hw_top at zt (tumblehome)."""
    ring = superellipse(yc, (zb + zt) / 2, 1.0, (zt - zb) / 2, n, m)
    out = []
    for y, z in ring:
        f = (z - zb) / max(zt - zb, 1e-9)
        out.append((yc + (y - yc) * (hw_bot + (hw_top - hw_bot) * f), z))
    return out


def smooth(xs, pts):
    """Piecewise-cubic (smoothstep) interpolation through (x, value) points."""
    xs = np.asarray(xs, float)
    out = np.empty_like(xs)
    for i, x in enumerate(xs):
        for (x0, v0), (x1, v1) in zip(pts[:-1], pts[1:]):
            if x0 <= x <= x1:
                u = (x - x0) / (x1 - x0)
                u = u * u * (3 - 2 * u)
                out[i] = v0 + (v1 - v0) * u
                break
        else:
            out[i] = pts[0][1] if x < pts[0][0] else pts[-1][1]
    return out


def arch_bottom(x, base=0.20, r_arch=0.41):
    """Underside of the body: ground clearance, raised into a wheel arch over each axle."""
    z = base
    for xw in (0.0, WB):
        d = abs(x - xw)
        if d < r_arch:
            z = max(z, RW + math.sqrt(r_arch**2 - d**2) * 0.98)
    return z


def car_body():
    xs = np.concatenate([np.linspace(-0.88, -0.6, 8), np.linspace(-0.6, 3.35, 72), np.linspace(3.35, 3.62, 8)[1:]])
    xs = np.unique(xs)
    top = smooth(xs, [(-0.88, 0.74), (-0.8, 0.93), (-0.55, 1.00), (0.25, 1.00), (2.0, 0.97), (2.4, 0.92),
                      (3.3, 0.80), (3.55, 0.70), (3.62, 0.58)])
    bot = smooth(xs, [(-0.88, 0.34), (-0.75, 0.24), (-0.5, 0.20), (3.2, 0.20), (3.5, 0.24), (3.62, 0.34)])
    hw = smooth(xs, [(-0.88, 0.70), (-0.7, 0.86), (-0.4, 0.90), (3.1, 0.90), (3.45, 0.85), (3.62, 0.70)])
    secs = []
    for x, zt, zb, w in zip(xs, top, bot, hw):
        zb = max(zb, arch_bottom(x))
        secs.append([(x, y, z) for y, z in tapered(0.0, zb, zt, w, w - 0.05, 6.0, 56)])
    m = loft(secs)
    # roof panel and pillars: bands of the glasshouse section, just proud of the glass
    gl = glass_top
    xr = np.linspace(0.5, 1.82, 48)
    secs = []
    for x in xr:
        zt = float(gl(x))
        z0 = max(zt - 0.075, 0.95)
        w0 = glass_hw(z0, zt)
        secs.append([(x, y, z) for y, z in tapered(0.0, z0, zt + 0.012, w0 + 0.008, 0.648, 5.0, 48)])
    m.extend(loft(secs))
    for xa, xb in ((1.12, 1.22),):  # B pillar
        xs2 = np.linspace(xa, xb, 4)
        m.extend(loft([[(x, y, z) for y, z in tapered(0.0, 0.94, float(gl(x)) + 0.006, 0.848, 0.648, 5.0, 48)] for x in xs2]))
    return m


def glass_top(x):
    return smooth(np.atleast_1d(x), [(0.02, 1.0), (0.66, 1.39), (1.6, 1.40), (2.12, 1.0)])[0]


def glass_hw(z, zt, zb=0.93, hw_bot=0.84, hw_top=0.64):
    return hw_bot + (hw_top - hw_bot) * (z - zb) / max(zt - zb, 1e-9)


def car_glass():
    """Tinted glasshouse: rear screen, side windows and windscreen in one loft."""
    xs = np.linspace(0.02, 2.12, 70)
    secs = []
    for x in xs:
        secs.append([(x, y, zz) for y, zz in tapered(0.0, 0.93, glass_top(x), 0.84, 0.64, 5.0, 48)])
    return loft(secs)


def car_lamps():
    m = Mesh()
    for s in (1, -1):
        y0, y1 = sorted((s * 0.52, s * 0.80))
        m.extend(box((3.50, y0, 0.58), (3.60, y1, 0.66)))
    return m


def car_tail():
    m = Mesh()
    for s in (1, -1):
        y0, y1 = sorted((s * 0.50, s * 0.82))
        m.extend(box((-0.87, y0, 0.70), (-0.78, y1, 0.82)))
    return m


def car_trim():
    """Bumper skirts, grille, side sills and mirrors."""
    m = Mesh()
    m.extend(box((3.50, -0.78, 0.24), (3.635, 0.78, 0.38)))   # front bumper lip
    m.extend(box((3.52, -0.42, 0.45), (3.60, 0.42, 0.62)))    # grille
    m.extend(box((-0.895, -0.80, 0.26), (-0.78, 0.80, 0.42)))  # rear bumper
    for s in (1, -1):
        y0, y1 = sorted((s * 0.86, s * 0.905))
        m.extend(box((0.42, y0, 0.20), (2.28, y1, 0.30)))      # sill
        y0, y1 = sorted((s * 0.84, s * 0.97))
        m.extend(box((1.96, y0, 0.97), (2.06, y1, 1.04)))      # mirror
    return m


# ------------------------------------------------------------------ wing sections (Visualizers.Aileron, LandingGear)
def naca(u, thick):
    return 5 * thick * (0.2969 * math.sqrt(u) - 0.126 * u - 0.3516 * u**2 + 0.2843 * u**3 - 0.1036 * u**4)


def section(u0, u1, chord, thick, n=28):
    """Closed (s, z) ring of a symmetric aerofoil between chord fractions u0 and u1 (s = u*chord)."""
    us = [u0 + (u1 - u0) * (1 - math.cos(math.pi * k / n)) / 2 for k in range(n + 1)]
    up = [(u * chord, chord * naca(u, thick)) for u in us]
    lo = [(u * chord, -chord * naca(u, thick)) for u in us]
    if u1 >= 1:
        lo = lo[:-1]
    if u0 <= 0:
        up = up[1:]
    return up + lo[::-1]


def extrude_x(ring, x0, x1, to_yz):
    """Loft a 2-D ring (mapped to (y, z) by to_yz) from x0 to x1."""
    pts = [to_yz(s, z) for s, z in ring]
    secs = [[(x, y, z) for y, z in pts] for x in (x0, x1)]
    # make the ring counter-clockwise seen from +x
    y, z = np.array([p[0] for p in pts]), np.array([p[1] for p in pts])
    area = 0.5 * np.sum(y * np.roll(z, -1) - np.roll(y, -1) * z)
    if area < 0:
        secs = [s[::-1] for s in secs]
    return loft(secs)


CHORD, U_HINGE, SPAN = 2.1, 0.76, 1.6


def wing():
    """Wing box ahead of the hinge line: span along x (-SPAN/2..SPAN/2), leading edge towards +y, hinge at y = 0."""
    ring = section(0.0, U_HINGE - 0.01, CHORD, 0.12)
    return extrude_x(ring, -SPAN / 2, SPAN / 2, lambda s, z: (U_HINGE * CHORD - s, z))


def aileron():
    """Control surface behind the hinge line (hinge on the x axis), trailing edge towards -y."""
    ring = section(U_HINGE, 1.0, CHORD, 0.12)
    return extrude_x(ring, -SPAN / 2 + 0.02, SPAN / 2 - 0.02, lambda s, z: (U_HINGE * CHORD - s, z))


def gear_wing():
    """Wing segment above a main landing gear: chord along x (leading edge +x), span along y, centred."""
    ring = section(0.0, 1.0, 3.0, 0.11)
    m = extrude_x(ring, -0.9, 0.9, lambda s, z: (1.2 - s, z))
    # swap axes: extrude_x makes span along x; rotate so span is along y and chord along x
    out = Mesh()
    for a, b, c in m.tris:
        rot = lambda p: np.array([p[1], -p[0], p[2]])
        out.add(rot(a), rot(b), rot(c))
    return out


if __name__ == "__main__":
    tyre(R=RW, W=0.205, Rb=0.205, pitch_deg=15).write("car_tyre.stl")
    rim(Rr=0.205, W=0.19).write("car_rim.stl")
    car_body().write("car_body.stl")
    car_glass().write("car_glass.stl")
    car_lamps().write("car_lamps.stl")
    car_tail().write("car_tail.stl")
    car_trim().write("car_trim.stl")
    wing().write("wing.stl")
    aileron().write("aileron.stl")
    gear_wing().write("gear_wing.stl")

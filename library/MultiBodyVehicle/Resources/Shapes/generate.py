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
def body_main():
    m = Mesh()
    # monocoque and nose: x, yc, half width, z bottom, z top
    m.extend(body_loft([
        (1.78, 0, 0.05, -0.15, -0.08),
        (1.55, 0, 0.10, -0.17, -0.03),
        (1.20, 0, 0.16, -0.19, 0.03),
        (0.70, 0, 0.21, -0.21, 0.10),
        (0.20, 0, 0.25, -0.22, 0.15),
        (-0.35, 0, 0.27, -0.22, 0.17),
        (-0.90, 0, 0.27, -0.22, 0.17),
        (-1.25, 0, 0.26, -0.22, 0.30),
        (-1.60, 0, 0.22, -0.22, 0.40),
        (-2.10, 0, 0.18, -0.21, 0.30),
        (-2.60, 0, 0.13, -0.19, 0.17),
        (-3.05, 0, 0.08, -0.15, 0.06),
        (-3.20, 0, 0.04, -0.12, 0.00),
    ], n=3.2, m=48))
    # sidepods
    for side in (1, -1):
        m.extend(body_loft([
            (-0.30, side * 0.40, 0.03, -0.17, -0.05),
            (-0.40, side * 0.43, 0.15, -0.21, 0.07),
            (-0.80, side * 0.45, 0.17, -0.21, 0.08),
            (-1.30, side * 0.42, 0.14, -0.21, 0.04),
            (-1.80, side * 0.34, 0.09, -0.20, -0.04),
            (-2.10, side * 0.28, 0.04, -0.19, -0.10),
        ], n=3.0, m=32))
    return m


def body_dark():
    m = Mesh()
    # floor and plank
    m.extend(box((-2.75, -0.62, -0.235), (0.45, 0.62, -0.215)))
    # front wing: two elements and end plates
    m.extend(airfoil(1.95, 0.32, -0.235, 1.56, camber=0.05))
    m.extend(airfoil(1.68, 0.18, -0.18, 1.40, camber=0.08, aoa=-0.35))
    for y in (0.78, -0.78):
        m.extend(box((1.55, y - 0.008, -0.27), (1.98, y + 0.008, -0.12)))
    # rear wing: main plane, flap, end plates and pylon
    m.extend(airfoil(-2.93, 0.30, 0.52, 0.96, camber=0.08, aoa=-0.12))
    m.extend(airfoil(-3.20, 0.16, 0.63, 0.96, camber=0.10, aoa=-0.55))
    for y in (0.485, -0.485):
        m.extend(box((-3.42, y - 0.008, 0.36), (-2.88, y + 0.008, 0.74)))
    m.extend(box((-3.15, -0.02, 0.05), (-3.02, 0.02, 0.52)))
    # cockpit rim and roll hoop
    m.extend(body_loft([(-0.40, 0, 0.20, 0.15, 0.20), (-1.00, 0, 0.20, 0.15, 0.20)], n=6, m=32))
    m.extend(body_loft([(-1.20, 0, 0.07, 0.30, 0.47), (-1.32, 0, 0.09, 0.28, 0.43)], n=4, m=24))
    return m


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
    bump().write("bump.stl")

"""TextureCraft app icon: an octopus drawn in code, written to icon.svg beside this file.

Original artwork (no traced or third-party source), MIT OR Apache-2.0 like the rest of the repo.
Follows craftrules standards/icon-design.md: 512 tile with rx=112, three colours only (ink, paper,
TextureCraft copper), a tight head-and-shoulders portrait with big eyes, carved lines that swell
and taper. Edit the shapes here, run `python3 assets/app-icon/icon.py`, then `packaging/icons.sh`
to render every PNG, the .ico and the .icns. Plain Python 3, no dependencies.
"""
import math
from pathlib import Path

INK, PAPER, APP = "#0b0b0c", "#efe9dc", "#c2672b"


def catmull(pts, n=24, closed=False):
    out = []
    m = len(pts)
    segs = m if closed else m - 1
    for i in range(segs):
        g = (lambda k: pts[k % m]) if closed else (lambda k: pts[min(max(k, 0), m - 1)])
        p0, p1, p2, p3 = g(i - 1), g(i), g(i + 1), g(i + 2)
        for s in range(n):
            t = s / n
            t2, t3 = t * t, t * t * t
            out.append(tuple(0.5 * ((2 * p1[j]) + (-p0[j] + p2[j]) * t + (2 * p0[j] - 5 * p1[j] + 4 * p2[j] - p3[j]) * t2
                                     + (-p0[j] + 3 * p1[j] - 3 * p2[j] + p3[j]) * t3) for j in range(2)))
    if not closed:
        out.append(pts[-1])
    return out


def arclen(pts):
    acc = [0.0]
    for a, b in zip(pts, pts[1:]):
        acc.append(acc[-1] + math.dist(a, b))
    total = acc[-1] or 1.0
    return [a / total for a in acc]


def normals(pts):
    out = []
    for i in range(len(pts)):
        a = pts[max(i - 1, 0)]
        b = pts[min(i + 1, len(pts) - 1)]
        dx, dy = b[0] - a[0], b[1] - a[1]
        d = math.hypot(dx, dy) or 1.0
        out.append((-dy / d, dx / d))
    return out


def path(pts, close=True):
    s = "M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in pts)
    return s + (" Z" if close else "")


def ribbon(center, width, offset=lambda u: 0.0, u0=0.0, u1=1.0):
    """Closed outline of a stroke along `center` whose width (and sideways offset) vary with u."""
    us = arclen(center)
    ns = normals(center)
    left, right = [], []
    for p, n, u in zip(center, ns, us):
        if u < u0 or u > u1:
            continue
        v = (u - u0) / ((u1 - u0) or 1)
        w, o = width(v) / 2, offset(u)
        left.append((p[0] + n[0] * (o + w), p[1] + n[1] * (o + w)))
        right.append((p[0] + n[0] * (o - w), p[1] + n[1] * (o - w)))
    return left + right[::-1]


def swell(peak, lo=0.0):
    """Carved line: thin at both ends, `peak` wide in the middle."""
    return lambda v: lo + (peak - lo) * math.sin(math.pi * min(max(v, 0), 1)) ** 0.8


def at(center, u):
    us = arclen(center)
    ns = normals(center)
    for i in range(1, len(us)):
        if us[i] >= u:
            return center[i], ns[i]
    return center[-1], ns[-1]


out = []
el = out.append


def arm(points, base, tip, side, suckers=(0.45, 0.97), shade=(0.05, 0.75)):
    """An arm: paper body with ink contour, a carved shading line, suckers along `side`."""
    c = catmull(points)
    w = lambda v: tip + (base - tip) * (1 - v) ** 1.15
    el(f'<path d="{path(ribbon(c, w))}" fill="{PAPER}" stroke="{INK}" stroke-width="7" stroke-linejoin="round"/>')
    # Shading line on the side away from the suckers.
    el(f'<path d="{path(ribbon(c, swell(9, 0.6), offset=lambda u: -side * w(u) * 0.22, u0=shade[0], u1=shade[1]))}" fill="{INK}"/>')
    # Suckers: paper discs with an ink ring and a dark centre, shrinking toward the tip.
    u = suckers[0]
    while u < suckers[1]:
        p, n = at(c, u)
        r = max(w(u) * 0.2, 3.2)
        cx, cy = p[0] + n[0] * side * w(u) * 0.36, p[1] + n[1] * side * w(u) * 0.36
        el(f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="{r:.1f}" fill="{PAPER}" stroke="{INK}" stroke-width="{max(2.6, r * 0.42):.1f}"/>')
        if r > 5:
            el(f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="{r * 0.32:.1f}" fill="{INK}"/>')
        u += max(r * 2.9, 8) / 420


# Arms, back to front.
arm([(206, 262), (140, 292), (78, 300), (38, 278), (32, 242), (58, 226), (80, 242), (70, 262)], 96, 7, 1, suckers=(0.42, 0.97))  # left curl
arm([(306, 262), (372, 292), (434, 300), (474, 278), (480, 242), (454, 226), (432, 242), (442, 262)], 96, 7, -1, suckers=(0.42, 0.97))  # right curl
arm([(222, 262), (196, 300), (150, 352), (104, 424), (76, 540)], 98, 34, -1, suckers=(0.3, 1.0))  # left, off the bottom
arm([(290, 262), (316, 300), (360, 352), (420, 402), (462, 452), (452, 494), (416, 492), (410, 464), (428, 452)], 98, 7, 1, suckers=(0.4, 0.97))  # right, curl
arm([(240, 250), (236, 302), (222, 382), (196, 446), (184, 540)], 104, 40, 1, suckers=(0.34, 1.0))  # front left
arm([(274, 250), (280, 302), (302, 372), (302, 432), (278, 476), (244, 472), (236, 446), (254, 436)], 104, 7, 1, suckers=(0.4, 0.97))  # front right curl

# Head: mantle + eye region, one silhouette.
head = catmull([(262, 62), (338, 74), (390, 116), (406, 176), (378, 226), (364, 254), (332, 286), (256, 302), (180, 286),
                (148, 254), (134, 226), (108, 176), (128, 112), (186, 72)], closed=True)
el(f'<clipPath id="head"><path d="{path(head)}"/></clipPath>')
el(f'<path d="{path(head)}" fill="{PAPER}" stroke="{INK}" stroke-width="8" stroke-linejoin="round"/>')
# Engraved shading: contour-following lines that thicken toward the shadow side (right and below).
cx0, cy0 = 236, 156
for k, f in enumerate([0.95, 0.88, 0.81, 0.74, 0.67]):
    ring = [(cx0 + (x - cx0) * f, cy0 + (y - cy0) * f) for x, y in head]
    n = len(ring)
    # Right flank of the mantle, in shadow.
    arc = ring[int(n * (0.03 + k * 0.012)):int(n * (0.30 - k * 0.012))]
    el(f'<path d="{path(ribbon(arc, swell(7.5 - k * 1.0, 0.5)))}" fill="{INK}" clip-path="url(#head)"/>')
for k, f in enumerate([0.95, 0.9]):
    ring = [(cx0 + (x - cx0) * f, cy0 + (y - cy0) * f) for x, y in head]
    n = len(ring)
    # Under the head, where the arms join.
    arc = ring[int(n * (0.40 + k * 0.02)):int(n * (0.60 - k * 0.02))]
    el(f'<path d="{path(ribbon(arc, swell(6 - k * 2.5, 0.5)))}" fill="{INK}" clip-path="url(#head)"/>')
# A few carved strokes over the dome (texture), left side lighter.
for pts, wd in [([(196, 104), (176, 124), (166, 150)], 6), ([(222, 96), (212, 108)], 4.5), ([(288, 98), (312, 112), (330, 134)], 6)]:
    el(f'<path d="{path(ribbon(catmull(pts), swell(wd, 0.3)))}" fill="{INK}"/>')

# Eyes.
for ex, ey, look in [(172, 214, 4), (340, 214, -4)]:
    el(f'<circle cx="{ex}" cy="{ey}" r="52" fill="{PAPER}" stroke="{INK}" stroke-width="7"/>')
    el(f'<circle cx="{ex + look}" cy="{ey + 3}" r="37" fill="{APP}" stroke="{INK}" stroke-width="5"/>')
    el(f'<ellipse cx="{ex + look * 1.5}" cy="{ey + 4}" rx="26" ry="18" fill="{INK}"/>')
    el(f'<circle cx="{ex + look * 1.5 - 9}" cy="{ey - 4}" r="7.5" fill="{PAPER}"/>')
    el(f'<circle cx="{ex + look * 1.5 + 10}" cy="{ey + 10}" r="3.5" fill="{PAPER}"/>')
    s = 1 if ex < 256 else -1
    # Upper lid: a heavy carved arc.
    lid = catmull([(ex - 50 * s, ey - 18), (ex - 20 * s, ey - 50), (ex + 22 * s, ey - 52), (ex + 48 * s, ey - 28)])
    el(f'<path d="{path(ribbon(lid, swell(11, 2)))}" fill="{INK}"/>')
    # Lower lid crease.
    low = catmull([(ex - 34 * s, ey + 44), (ex, ey + 58), (ex + 36 * s, ey + 46)])
    el(f'<path d="{path(ribbon(low, swell(6, 0.5)))}" fill="{INK}"/>')

body = "\n  ".join(out)
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
  <title>TextureCraft</title>
  <clipPath id="tile"><rect width="512" height="512" rx="112"/></clipPath>
  <g clip-path="url(#tile)">
  <rect width="512" height="512" fill="{APP}"/>
  {body}
  </g>
</svg>
'''
Path(__file__).with_name("icon.svg").write_text(svg, encoding="utf-8", newline="\n")

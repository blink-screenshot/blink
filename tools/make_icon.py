"""Generates the app icons from code (run once; the output is committed).

    pip install cairosvg pillow
    python tools/make_icon.py

Writes to assets/:
  blink.svg           the logo: two squinting eyes in a viewfinder, on the hot pink tile
  blink.png           256 px, the window icon
  blink.ico           the logo at 16..256 px, one drawing per size (see "Optical sizes")
  blink-tray.svg      the tray glyph: one eye inside four rounded corners, hot pink, no tile
  blink-tray.ico      the tray glyph at 16..48 px (embedded as icon resource 2, see build.rs)
  alt/mini-eye-tile/  option kept for later: the small logo WITH a tile (one big eye on the tile)

Colors (src/theme.rs): hot pink #FF2E93 tile, ink #101114 eyes, white corner marks.

Optical sizes: below 56 px the viewfinder corners are drawn with a heavier stroke that sits on
whole pixels (so the edges stay crisp) and the eyes get a bit more room between them. The tile
radius is 58/256 of the icon, and the corner marks run parallel to it (radius = 58 - their inset).

The eye is built from two smooth tapered arms (a thick vertex that thins towards the tips), so the
outline is exact curves rather than traced pixels.
"""
import io
import math
import os
import struct

import cairosvg

PINK, INK, WHITE = "#FF2E93", "#101114", "#FFFFFF"
TILE_R = 58  # corner radius of the tile on the 256 grid
ASSETS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
TRAY_SIZES = [16, 20, 24, 32, 40, 48]

# ---- the eye ------------------------------------------------------------------------------
# Two tapered arms from a common vertex (v) to the tips (e upper, t lower), each a quadratic curve
# with control point (cu / cl). Coordinates are in an arbitrary unit square. The arm width goes
# from `wv` at the vertex to `wu` / `wl` at the tip as  we + (wv - we) * (1 - t^2)^q.
EYE = dict(vx=0.0972, vy=0.5244, ex=0.8138, ey=0.0927, cux=0.5086, cuy=0.1753,
           tx=0.9251, ty=0.7827, clx=0.5623, cly=0.4595, wv=0.2885, wu=0.1831, wl=0.1289, q=0.9764)


def _quad(p0, c, p1, t):
    u = 1 - t
    return (u * u * p0[0] + 2 * u * t * c[0] + t * t * p1[0], u * u * p0[1] + 2 * u * t * c[1] + t * t * p1[1])


def _quad_d(p0, c, p1, t):
    u = 1 - t
    return (2 * u * (c[0] - p0[0]) + 2 * t * (p1[0] - c[0]), 2 * u * (c[1] - p0[1]) + 2 * t * (p1[1] - c[1]))


def _arm(p0, c, p1, wv, we, q, n=96):
    left, right = [], []
    for i in range(n + 1):
        t = i / n
        x, y = _quad(p0, c, p1, t)
        dx, dy = _quad_d(p0, c, p1, t)
        norm = math.hypot(dx, dy) or 1e-9
        half = (we + (wv - we) * (1 - t * t) ** q) / 2
        nx, ny = -dy / norm, dx / norm
        left.append((x + nx * half, y + ny * half))
        right.append((x - nx * half, y - ny * half))
    return left + right[::-1]


def _eye_shapes():
    p = EYE
    v, eu, tl = (p["vx"], p["vy"]), (p["ex"], p["ey"]), (p["tx"], p["ty"])
    polys = [_arm(v, (p["cux"], p["cuy"]), eu, p["wv"], p["wu"], p["q"]),
             _arm(v, (p["clx"], p["cly"]), tl, p["wv"], p["wl"], p["q"])]
    circles = [(v, p["wv"] / 2), (eu, p["wu"] / 2), (tl, p["wl"] / 2)]
    return polys, circles


def _bbox(polys, circles):
    pts = [q for poly in polys for q in poly]
    for (cx, cy), r in circles:
        pts += [(cx + r * math.cos(a / 16 * math.tau), cy + r * math.sin(a / 16 * math.tau)) for a in range(16)]
    xs, ys = [q[0] for q in pts], [q[1] for q in pts]
    return min(xs), min(ys), max(xs), max(ys)


_POLYS, _CIRCLES = _eye_shapes()
_BX0, _BY0, _BX1, _BY1 = _bbox(_POLYS, _CIRCLES)
EYE_RATIO = (_BX1 - _BX0) / (_BY1 - _BY0)  # width / height of the eye


def eye(cx, cy, width, color, mirror=False):
    """The eye, `width` wide (height follows), its bounding box centred on (cx, cy)."""
    mx, my = (_BX0 + _BX1) / 2, (_BY0 + _BY1) / 2
    k = width / (_BX1 - _BX0)

    def t(q):
        x = (q[0] - mx) * k
        return (cx + (-x if mirror else x), cy + (q[1] - my) * k)

    out = []
    for poly in _POLYS:
        pts = " L".join(f"{t(q)[0]:.2f},{t(q)[1]:.2f}" for q in poly)
        out.append(f'<path d="M{pts} Z" fill="{color}"/>')
    for c, r in _CIRCLES:
        x, y = t(c)
        out.append(f'<circle cx="{x:.2f}" cy="{y:.2f}" r="{r * k:.2f}" fill="{color}"/>')
    return "".join(out)


def eye_pair(width, gap, cy=128, color=INK):
    off = width / 2 + gap / 2
    return eye(128 - off, cy, width, color, mirror=True) + eye(128 + off, cy, width, color)


# ---- corner marks -------------------------------------------------------------------------
def corner_marks(lo, hi, arm, stroke, radius, color, cap="round", opacity=1.0):
    """Four L-shaped marks with round corners on the square lo..hi (stroke centre lines)."""
    d = (f"M{lo},{lo + arm} V{lo + radius} A{radius},{radius} 0 0 1 {lo + radius},{lo} H{lo + arm} "
         f"M{hi - arm},{lo} H{hi - radius} A{radius},{radius} 0 0 1 {hi},{lo + radius} V{lo + arm} "
         f"M{hi},{hi - arm} V{hi - radius} A{radius},{radius} 0 0 1 {hi - radius},{hi} H{hi - arm} "
         f"M{lo + arm},{hi} H{lo + radius} A{radius},{radius} 0 0 1 {lo},{hi - radius} V{hi - arm}")
    return (f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{stroke:g}" stroke-linecap="{cap}" '
            f'stroke-linejoin="round" opacity="{opacity:g}"/>')


# Optical sizes of the logo: size -> (stroke centre inset, arm length, stroke width), all on the
# 256 grid and chosen so that they fall on whole pixels at that size.
CUTS = {16: (40, 40, 16), 20: (44.8, 32, 12.8), 24: (42.667, 32, 21.333), 32: (40, 40, 16),
        40: (44.8, 38.4, 12.8), 48: (40, 40, 16)}


def _tile(inner):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">'
            f'<rect width="256" height="256" rx="{TILE_R}" fill="{PINK}"/>{inner}</svg>')


def logo_svg(size=256):
    if size >= 56:
        # master: marks 28 from the edge, parallel to the tile curve, eyes 84 wide
        inset = 28
        marks = corner_marks(inset, 256 - inset, TILE_R - inset + 14, 10, TILE_R - inset, WHITE, "round", 0.95)
        return _tile(eye_pair(84, 12) + marks)
    c, arm, stroke = CUTS[size]
    marks = corner_marks(c, 256 - c, arm, stroke, TILE_R - c, WHITE, "butt")
    return _tile(eye_pair(80, 16) + marks)


# ---- tray glyph: one eye inside four rounded corners, no tile ------------------------------
# size -> (inset, stroke, arm, corner radius, eye height) in pixels at that size
GLYPH = {16: (0, 1, 4, 2, 8), 20: (0, 1, 5, 2.5, 12), 24: (0, 2, 5, 3, 14), 32: (1, 2, 8, 4, 18)}


def glyph_svg(size=32):
    base = size if size in GLYPH else 32
    inset, stroke, arm, radius, height = GLYPH[base]
    lo, hi = inset + stroke / 2, base - inset - stroke / 2
    body = (corner_marks(lo, hi, arm, stroke, radius, PINK, "butt")
            + eye(base / 2, base / 2, height * EYE_RATIO, PINK))
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {base} {base}" width="{size}" height="{size}">'
            f'{body}</svg>')


# ---- option kept for later: the small logo WITH a tile --------------------------------------
def mini_tile_svg(size):
    if size >= 56:
        raise ValueError("the small tile logo is for sizes below 56 px")
    c, arm, stroke = CUTS[size]
    marks = corner_marks(c, 256 - c, arm, stroke, TILE_R - c, WHITE, "butt")
    return _tile(eye(128, 128, 124, INK) + marks)


# ---- output ---------------------------------------------------------------------------------
def png(svg, size):
    return cairosvg.svg2png(bytestring=svg.encode(), output_width=size, output_height=size)


def write_ico(path, frames):
    """frames: [(size, png_bytes)] -> .ico with PNG-compressed entries."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    head = struct.pack("<HHH", 0, 1, len(frames))
    offset = 6 + 16 * len(frames)
    entries, data = b"", b""
    for size, blob in frames:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(blob), offset + len(data))
        data += blob
    with open(path, "wb") as f:
        f.write(head + entries + data)


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


def main():
    write(os.path.join(ASSETS, "blink.svg"), logo_svg(256).replace("<svg ", "<svg role=\"img\" ", 1)
          .replace("<rect", "<title>Blink</title><rect", 1))
    with open(os.path.join(ASSETS, "blink.png"), "wb") as f:
        f.write(png(logo_svg(256), 256))
    write_ico(os.path.join(ASSETS, "blink.ico"), [(s, png(logo_svg(s), s)) for s in ICO_SIZES])

    write(os.path.join(ASSETS, "blink-tray.svg"), glyph_svg(32))
    write_ico(os.path.join(ASSETS, "blink-tray.ico"), [(s, png(glyph_svg(s), s)) for s in TRAY_SIZES])

    alt = os.path.join(ASSETS, "alt", "mini-eye-tile")
    sizes = [16, 20, 24, 32]
    write_ico(os.path.join(alt, "mini-eye-tile.ico"), [(s, png(mini_tile_svg(s), s)) for s in sizes])
    for s in sizes:
        write(os.path.join(alt, f"mini-eye-tile-{s}.svg"), mini_tile_svg(s))
    print("eye width / height =", round(EYE_RATIO, 3))
    print("wrote", ", ".join(sorted(os.listdir(ASSETS))))


if __name__ == "__main__":
    main()

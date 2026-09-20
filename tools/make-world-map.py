#!/usr/bin/env python3
"""Build web/assets/geo/world.json, the country outlines behind the Security map.

Input:  Natural Earth "Admin 0 – Countries", 1:50m, as GeoJSON (public domain, naturalearthdata.com):
        https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_admin_0_countries.geojson
Output: one SVG path per country, already projected (Mercator), so the browser only has to draw it.

    python3 tools/make-world-map.py ne_50m_admin_0_countries.geojson web/assets/geo/world.json

The projection here and in web/assets/js/worldmap.js must stay the same, or the dots leave the land.
"""
import json
import math
import sys

WIDTH = 2000.0                      # user units across the whole world
LAT_TOP, LAT_BOTTOM = 84.0, -58.0   # no Antarctica: nobody streams from there, and it costs a fifth of the height
TOLERANCE = 0.12                    # simplification, in user units (about 2 km at the equator)
MIN_RING_AREA = 0.6                 # islands smaller than this go, unless they are all a country has

# Mercator: north is straight up everywhere and shapes keep their angles, which is what a map you zoom into should
# do. (An equal-area projection was tried first; it leans everything far from the equator and the centre line.)
SCALE = WIDTH / (2 * math.pi)


def project(lon, lat):
    return math.radians(lon) * SCALE + WIDTH / 2, -math.log(math.tan(math.pi / 4 + math.radians(lat) / 2)) * SCALE


TOP = project(0, LAT_TOP)[1]
HEIGHT = project(0, LAT_BOTTOM)[1] - TOP


def simplify(points, tol):
    """Douglas–Peucker, iterative."""
    if len(points) < 3:
        return points
    keep = [False] * len(points)
    keep[0] = keep[-1] = True
    stack = [(0, len(points) - 1)]
    while stack:
        a, b = stack.pop()
        (ax, ay), (bx, by) = points[a], points[b]
        dx, dy = bx - ax, by - ay
        norm = math.hypot(dx, dy) or 1e-12
        worst, index = 0.0, -1
        for i in range(a + 1, b):
            px, py = points[i]
            d = abs(dy * (px - ax) - dx * (py - ay)) / norm if norm > 1e-9 else math.hypot(px - ax, py - ay)
            if d > worst:
                worst, index = d, i
        if worst > tol and index > 0:
            keep[index] = True
            stack += [(a, index), (index, b)]
    return [p for p, k in zip(points, keep) if k]


def area(ring):
    return abs(sum(x1 * y2 - x2 * y1 for (x1, y1), (x2, y2) in zip(ring, ring[1:] + ring[:1]))) / 2


def fmt(v):
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def ring_path(ring):
    out, (px, py) = [f"M{fmt(ring[0][0])},{fmt(ring[0][1])}"], (round(ring[0][0], 1), round(ring[0][1], 1))
    for x, y in ring[1:]:
        x, y = round(x, 1), round(y, 1)
        if (x, y) == (px, py):
            continue
        out.append(f"l{fmt(x - px)},{fmt(y - py)}")
        px, py = x, y
    return "".join(out) + "z"


def main(src, dst):
    countries = []
    for f in json.load(open(src))["features"]:
        p, g = f["properties"], f["geometry"]
        code = p.get("ISO_A2_EH") or ""
        if p.get("ADM0_A3") == "ATA":
            continue
        polygons = g["coordinates"] if g["type"] == "MultiPolygon" else [g["coordinates"]]
        rings = []
        for poly in polygons:
            outer = [project(lon, max(min(lat, LAT_TOP), LAT_BOTTOM)) for lon, lat in poly[0]]
            outer = [(x, y - TOP) for x, y in outer]
            rings.append(outer)               # holes (lakes, enclaves) are left out: enclaves are drawn on top anyway
        rings.sort(key=area, reverse=True)
        kept = [simplify(r, TOLERANCE) for i, r in enumerate(rings) if i == 0 or area(r) >= MIN_RING_AREA]
        kept = [r for r in kept if len(r) >= 4]
        if not kept:
            continue
        countries.append({"c": code if len(code) == 2 and code.isalpha() else "", "n": p["NAME_LONG"], "d": "".join(ring_path(r) for r in kept)})
    countries.sort(key=lambda c: c["n"])
    world = {"source": "Natural Earth 1:50m Admin 0 Countries (public domain)", "projection": "mercator", "width": WIDTH, "height": round(HEIGHT, 1),
             "top": round(TOP, 3), "scale": round(SCALE, 6), "countries": countries}
    json.dump(world, open(dst, "w"), separators=(",", ":"), ensure_ascii=False)
    print(f"{len(countries)} countries, {sum(len(c['d']) for c in countries) / 1024:.0f} KB of paths, {WIDTH:.0f} x {HEIGHT:.0f}")


if __name__ == "__main__":
    main(*sys.argv[1:3])

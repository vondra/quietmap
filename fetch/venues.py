"""Venues of an OpenStreetMap extract (`venues.sh`), one line each: `lat lon kind outdoor_seating
area_m2 opening_hours name` (tab-separated, sorted by latitude then longitude), the point of a node
or the mean of an outline's vertices. Kinds: bar, pub, nightclub, biergarten, restaurant, cafe,
fast_food, food_court; outdoor_seating the tag's value or unknown; area_m2 an outline's area, 0
for a node; opening_hours and name as mapped, empty when none. Mapped terraces
(leisure=outdoor_seating) are the leisure layer's. Reads osmium's GeoJSON sequence on stdin.

    osmium export venues.osm.pbf -f geojsonseq | python3 venues.py venues.txt
"""
import json
import math
import re
import sys

KINDS = ('bar', 'pub', 'nightclub', 'biergarten', 'restaurant', 'cafe', 'fast_food', 'food_court')
CONTROL = re.compile(r'[\x00-\x1f\x7f]+')


def centroid(geometry):
    """The mean of an outline's vertices (a point's own coordinates); None for anything else."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    if kind == 'Point':
        return coordinates[1], coordinates[0]
    rings = {'Polygon': lambda c: c[:1], 'MultiPolygon': lambda c: [p[0] for p in c]}.get(kind)
    if rings is None:
        return None
    points = [point for ring in rings(coordinates) for point in ring]
    if not points:
        return None
    return sum(p[1] for p in points) / len(points), sum(p[0] for p in points) / len(points)


def area_m2(geometry):
    """An outline's area (outer rings less holes, equirectangular at its first vertex); 0 else."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    polygons = {'Polygon': lambda c: [c], 'MultiPolygon': lambda c: c}.get(kind)
    if polygons is None:
        return 0.0
    total = 0.0
    for polygon in polygons(coordinates):
        for index, ring in enumerate(polygon):
            if len(ring) < 3:
                continue
            scale = math.cos(math.radians(ring[0][1]))
            points = [(x * 111320.0 * scale, y * 110540.0) for x, y in ring]
            twice = sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(points, points[1:] + points[:1]))
            total += abs(twice) / 2.0 * (1 if index == 0 else -1)
    return max(total, 0.0)


def clean(text):
    """A tag's value on one line without tabs."""
    return CONTROL.sub(' ', text or '').strip()


def main(out_path):
    rows = []
    for line in sys.stdin:
        line = line.strip().lstrip('\x1e')
        if not line:
            continue
        feature = json.loads(line)
        tags = feature.get('properties') or {}
        kind = tags.get('amenity', '')
        if kind not in KINDS:
            continue
        geometry = feature.get('geometry') or {}
        point = centroid(geometry)
        if point is None:
            continue
        seating = clean(tags.get('outdoor_seating')).lower() or 'unknown'
        rows.append((round(point[0], 6), round(point[1], 6), kind, seating,
                     round(area_m2(geometry)), clean(tags.get('opening_hours')),
                     clean(tags.get('name'))))
    rows.sort()
    with open(out_path, 'w') as out:
        for lat, lon, kind, seating, area, hours, name in rows:
            out.write(f'{lat}\t{lon}\t{kind}\t{seating}\t{area}\t{hours}\t{name}\n')
    print(len(rows), 'venues', file=sys.stderr)


if __name__ == '__main__':
    main(sys.argv[1])

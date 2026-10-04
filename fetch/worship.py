"""Places of worship and bell towers of an OpenStreetMap extract (`worship.sh`), one line each:
`lat lon religion kind height_m` (tab-separated, sorted by latitude then longitude), the point
of a node or the centroid of an outline. Kinds: cathedral, church, chapel, bell_tower, other;
height the mapped height in metres, 0 when none. Reads osmium's GeoJSON sequence on stdin.

    osmium export worship.osm.pbf -f geojsonseq | python3 worship.py worship.txt
"""
import json
import sys


def centroid(geometry):
    """The mean of an outline's vertices (a point's own coordinates); None for anything else."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    if kind == 'Point':
        return coordinates[1], coordinates[0]
    rings = {'Polygon': lambda c: c[:1], 'MultiPolygon': lambda c: [p[0] for p in c],
             'LineString': lambda c: [c]}.get(kind)
    if rings is None:
        return None
    points = [point for ring in rings(coordinates) for point in ring]
    if not points:
        return None
    return sum(p[1] for p in points) / len(points), sum(p[0] for p in points) / len(points)


def kind(tags):
    if tags.get('tower:type') == 'bell_tower' or tags.get('building') == 'bell_tower':
        return 'bell_tower'
    building = tags.get('building', '')
    if building in ('cathedral', 'church', 'chapel'):
        return building
    if tags.get('amenity') == 'place_of_worship':
        return {'cathedral': 'cathedral', 'church': 'church', 'chapel': 'chapel'}.get(tags.get('place_of_worship', ''), 'other')
    return 'other'


def height(tags):
    text = (tags.get('height') or '').replace(',', '.').replace('m', '').strip()
    try:
        value = float(text)
    except ValueError:
        return 0.0
    return value if 0.0 < value < 200.0 else 0.0


def main(out_path):
    rows = []
    for line in sys.stdin:
        line = line.strip().lstrip('\x1e')
        if not line:
            continue
        feature = json.loads(line)
        tags = feature.get('properties') or {}
        point = centroid(feature.get('geometry') or {})
        if point is None:
            continue
        religion = (tags.get('religion') or '').strip().lower().replace('\t', ' ') or 'unknown'
        rows.append((round(point[0], 6), round(point[1], 6), religion, kind(tags), height(tags)))
    rows.sort()
    with open(out_path, 'w') as out:
        for lat, lon, religion, k, h in rows:
            out.write(f'{lat}\t{lon}\t{religion}\t{k}\t{h:g}\n')
    print(len(rows), 'places', file=sys.stderr)


if __name__ == '__main__':
    main(sys.argv[1])

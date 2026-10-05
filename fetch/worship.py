"""Places of worship, bell towers and minarets of an OpenStreetMap extract (`worship.sh`), one line
each: `lat lon religion kind height_m denomination` (tab-separated, sorted by latitude then
longitude), the point of a node or the mean of an outline's vertices. Kinds: cathedral, church,
chapel, bell_tower, minaret, other; religion as tagged (a mosque building or a minaret: muslim),
unknown when none; height the mapped height in metres, 0 when none; denomination as tagged, empty
when none. Reads osmium's GeoJSON sequence on stdin.

    osmium export worship.osm.pbf -f geojsonseq | python3 worship.py worship.txt
"""
import json
import sys


def centroid(geometry):
    """The mean of an outline's vertices (a point's own coordinates); None for anything else."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    if kind == 'Point':
        return coordinates[1], coordinates[0]
    rings = {'Polygon': lambda c: c[:1], 'MultiPolygon': lambda c: [p[0] for p in c]}.get(kind)
    if rings is None:
        return None
    # A ring repeats its first vertex at its end: once is enough.
    points = [point for ring in rings(coordinates) for point in ring[:-1] or ring]
    if not points:
        return None
    return sum(p[1] for p in points) / len(points), sum(p[0] for p in points) / len(points)


def kind(tags):
    if 'minaret' in (tags.get('tower:type'), tags.get('building'), tags.get('man_made')):
        return 'minaret'
    if tags.get('tower:type') == 'bell_tower' or tags.get('building') == 'bell_tower':
        return 'bell_tower'
    building = tags.get('building', '')
    if building in ('cathedral', 'church', 'chapel'):
        return building
    if tags.get('amenity') == 'place_of_worship':
        return {'cathedral': 'cathedral', 'church': 'church', 'chapel': 'chapel'}.get(tags.get('place_of_worship', ''), 'other')
    return 'other'


def clean(text):
    """A tag's value lowercased on one line without tabs."""
    return ' '.join((text or '').split()).lower()


def religion(tags, site_kind):
    tagged = clean(tags.get('religion'))
    if tagged:
        return tagged
    return 'muslim' if site_kind == 'minaret' or tags.get('building') == 'mosque' else 'unknown'


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
        site_kind = kind(tags)
        rows.append((round(point[0], 6), round(point[1], 6), religion(tags, site_kind), site_kind,
                     height(tags), clean(tags.get('denomination'))))
    rows.sort()
    with open(out_path, 'w') as out:
        for lat, lon, faith, k, h, denomination in rows:
            out.write(f'{lat}\t{lon}\t{faith}\t{k}\t{h:g}\t{denomination}\n')
    print(len(rows), 'places', file=sys.stderr)


if __name__ == '__main__':
    main(sys.argv[1])

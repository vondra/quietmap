"""Lodgings of OpenStreetMap that state their rooms (`qm-build lodgings`), one line each:
`type id lat lon rooms` (tab-separated, sorted by type then id): the OSM type (node, way or
relation) and id, the point of a node or the mean of the vertices of an outline's first exterior
ring (the ring whose centroid places a building in its square), and the stated rooms. A lodging is
tagged as the builder types a hotel building: tourism hotel, hostel, motel or guest_house, or
building hotel, hostel or motel. Rooms is a whole number from 1 to 5,000; anything else states no
rooms. Reads osmium's GeoJSON sequence with type and id attributes on stdin.

    osmium export rooms.osm.pbf -f geojsonseq -a type,id | python3 lodgings.py lodgings.tsv
"""
import json
import re
import sys

TOURISM = {'hotel', 'hostel', 'motel', 'guest_house'}
BUILDING = {'hotel', 'hostel', 'motel'}
# The 2026 derivation's bound: it drops typos and beds stated as rooms, and with them the few
# hotels that are larger still.
MOST_ROOMS = 5000


def rooms(text):
    """The stated rooms, or None when the tag is no whole number in range."""
    match = re.fullmatch(r'\s*(\d{1,5})\s*', text or '')
    if not match:
        return None
    value = int(match.group(1))
    return value if 1 <= value <= MOST_ROOMS else None


def position(geometry):
    """(lat, lon) of a point, or the mean of the first exterior ring's vertices; None otherwise."""
    kind, coordinates = geometry.get('type'), geometry.get('coordinates')
    if kind == 'Point':
        return coordinates[1], coordinates[0]
    ring = {'Polygon': lambda c: c[0], 'MultiPolygon': lambda c: c[0][0]}.get(kind)
    if ring is None:
        return None
    # A ring repeats its first vertex at its end: once is enough.
    points = ring(coordinates)[:-1] or ring(coordinates)
    return sum(p[1] for p in points) / len(points), sum(p[0] for p in points) / len(points)


def main(out_path):
    rows = []
    for line in sys.stdin:
        line = line.strip().lstrip('\x1e')
        if not line:
            continue
        feature = json.loads(line)
        tags = feature.get('properties') or {}
        if tags.get('tourism') not in TOURISM and tags.get('building') not in BUILDING:
            continue
        stated = rooms(tags.get('rooms'))
        point = position(feature.get('geometry') or {})
        if stated is None or point is None:
            continue
        rows.append((tags['@type'], int(tags['@id']), point[0], point[1], stated))
    rows.sort()
    with open(out_path, 'w') as out:
        for osm_type, osm_id, lat, lon, stated in rows:
            out.write(f'{osm_type}\t{osm_id}\t{lat:.7f}\t{lon:.7f}\t{stated}\n')


if __name__ == '__main__':
    main(sys.argv[1])

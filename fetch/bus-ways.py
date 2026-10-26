#!/usr/bin/env python3
"""Bus route relations as OPL on stdin -> one line per way they run on: `way bus trolleybus coach
bus_departures coach_departures`, the line directions of each kind without an interval on the way
and the departures a day of the bus and trolleybus routes and of the coach routes that tag their
interval (18 hours of it). A line is its kind, ref and network (else operator), a route without a
ref a line of its own: on a way its variants count at most twice, for both directions, and where
one of them tags its interval each of those runs the line's shortest (Toluca's terminal: 132 routes
of 29 refs every 5-15 minutes had summed to 21,601 departures a day). A public_transport v2 route is
one direction, an older route both unless its member is forward or backward. Platforms and stops
are not driven. Usage: osmium cat ROUTES.osm.pbf -f opl | bus-ways.py OUT.txt"""
import sys, re, collections
def unescape(text):
    # OPL escapes: %XXXX% hex code points
    return re.sub(r'%([0-9a-fA-F]+)%', lambda m: chr(int(m.group(1), 16)), text)
# way -> (kind, line) -> its variants' directions, and the most departures a day a direction its
# variants that tag an interval run.
lines = collections.defaultdict(lambda: collections.defaultdict(int))
timed = collections.defaultdict(dict)
stats = collections.Counter()
KIND = {'bus': 0, 'trolleybus': 1, 'coach': 2}
for line in sys.stdin:
    if not line.startswith('r'):
        continue
    fields = line.rstrip('\n').split(' ')
    tags = {}; members = ''
    for f in fields[1:]:
        if f.startswith('T'):
            for kv in f[1:].split(','):
                if '=' in kv:
                    k, v = kv.split('=', 1); tags[unescape(k)] = unescape(v)
        elif f.startswith('M'):
            members = f[1:]
    route = tags.get('route')
    if route not in KIND:
        stats['skipped ' + str(route)] += 1
        continue
    # A lifecycle tag retires the route when it says so (`disused=no` does not).
    retired = any(tags.get(key, 'no').strip().lower() not in ('no', 'false', '0', '') for key in ('disused', 'abandoned'))
    if retired or tags.get('type') not in ('route', None):
        stats['disused'] += 1
        continue
    v2 = tags.get('public_transport:version') == '2'
    # `interval` in minutes ("10"), hh:mm ("00:10") or hh:mm:ss ("00:02:30"), in minutes.
    interval = None
    match = re.match(r'^\s*(\d+)(?::(\d+))?(?::(\d+))?\s*$', tags.get('interval', ''))
    if match:
        first, minutes, seconds = match.groups()
        if minutes is None:
            value = float(first)
        elif int(minutes) < 60 and int(seconds or 0) < 60:
            value = 60 * int(first) + int(minutes) + int(seconds or 0) / 60
        else:
            value = 0.0
        if 2 <= value <= 720:
            interval = value
    ref = tags.get('ref', '').strip()
    line = (KIND[route], ref, (tags.get('network') or tags.get('operator') or '').strip().lower()) if ref else fields[0]
    stats[route] += 1
    if interval: stats['with interval'] += 1
    for member in members.split(','):
        if not member.startswith('w') or '@' not in member:
            continue
        way, role = member[1:].split('@', 1)
        if role in ('platform', 'platform_entry_only', 'platform_exit_only', 'stop', 'stop_entry_only', 'stop_exit_only'):
            continue
        directions = 1 if (v2 or role in ('forward', 'backward')) else 2
        lines[int(way)][(KIND[route], line)] += directions
        if interval:
            timed[int(way)][line] = max(timed[int(way)].get(line, 0.0), 18 * 60 / interval)
out = open(sys.argv[1], 'w')
ways = sorted(lines)
for way in ways:
    kinds, departures = [0, 0, 0], [0.0, 0.0]
    for (kind, line), directions in lines[way].items():
        most = timed.get(way, {}).get(line)
        if most:
            departures[1 if kind == KIND['coach'] else 0] += min(directions, 2) * most
        else:
            kinds[kind] += min(directions, 2)
    out.write(f"{way} {kinds[0]} {kinds[1]} {kinds[2]} {departures[0]:.1f} {departures[1]:.1f}\n")
print('ways', len(ways), dict(stats.most_common(12)), file=sys.stderr)

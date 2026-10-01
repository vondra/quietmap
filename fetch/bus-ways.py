#!/usr/bin/env python3
"""Bus route relations as OPL on stdin -> one line per way they run on: `way bus trolleybus coach
departures`, the directions the routes of each kind without an interval serve on the way (a
public_transport v2 route is one direction, an older route both unless its member is forward or
backward) and the departures a day of the routes that tag their interval (18 hours of it). Platforms and stops
are not driven. Usage: osmium cat ROUTES.osm.pbf -f opl | bus-ways.py OUT.txt"""
import sys, re, collections
def unescape(text):
    # OPL escapes: %XXXX% hex code points
    return re.sub(r'%([0-9a-fA-F]+)%', lambda m: chr(int(m.group(1), 16)), text)
ways = collections.defaultdict(lambda: [0, 0, 0, 0.0])
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
    if tags.get('disused') or tags.get('abandoned') or tags.get('type') not in ('route', None):
        stats['disused'] += 1
        continue
    v2 = tags.get('public_transport:version') == '2'
    # `interval` in minutes ("10"), hh:mm ("00:10") or hh:mm:ss.
    interval = None
    match = re.match(r'^\s*(\d+)(?::(\d+))?(?::\d+)?\s*$', tags.get('interval', ''))
    if match:
        hours_or_minutes, minutes = match.groups()
        value = int(hours_or_minutes) if minutes is None else 60 * int(hours_or_minutes) + int(minutes)
        if 2 <= value <= 720:
            interval = value
    stats[route] += 1
    if interval: stats['with interval'] += 1
    for member in members.split(','):
        if not member.startswith('w') or '@' not in member:
            continue
        ref, role = member[1:].split('@', 1)
        if role in ('platform', 'platform_entry_only', 'platform_exit_only', 'stop', 'stop_entry_only', 'stop_exit_only'):
            continue
        directions = 1 if (v2 or role in ('forward', 'backward')) else 2
        entry = ways[int(ref)]
        if interval:
            entry[3] += directions * 18 * 60 / interval
        else:
            entry[KIND[route]] += directions
out = open(sys.argv[1], 'w')
for way, (b, t, c, d) in sorted(ways.items()):
    out.write(f"{way} {b} {t} {c} {d:.1f}\n")
print('ways', len(ways), dict(stats.most_common(12)), file=sys.stderr)

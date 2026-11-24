"""The wind turbines standing at the planet's date (`turbines.sh`), one line each: `lat lon hub_m
power_kw source` (tab-separated, sorted by latitude then longitude, 0 when unknown; source `osm`
or the register that placed it). OpenStreetMap's standing turbines (no lifecycle prefix, key or
value; a node its point, an outline the mean of its vertices) come first; each admitted register
(the names after OUT, all when none is given) in turn pairs its standing turbines one to one with
those not yet paired, nearest pairs first, within PAIR_M: a pair keeps its position and takes the
register's hub and power where known, an unpaired standing turbine of the register is added, and
an unpaired OpenStreetMap turbine within GONE_M of one the register calls dismantled is dropped.
With `--as-of YYYY-MM-DD`, turbines commissioned on or after the cutoff are excluded, turbines
dismantled on or after the cutoff are restored to standing before pairing, and undated later
dismantlements do not drop mapped turbines. Reads osmium's GeoJSON sequence on stdin.

    osmium export turbines.osm.pbf -f geojsonseq -a type,id | python3 turbines.py [--as-of YYYY-MM-DD] REGISTERS OUT [NAME ..]
"""
import argparse
import calendar
import csv
import datetime
import glob
import json
import math
import re
import sys
import xml.etree.ElementTree as ElementTree

from osgeo import ogr, osr

ogr.UseExceptions()
# Pairs farther apart are two turbines (m): the surveyed registers pair out by 200-300 m, and 500 m
# keeps the Marktstammdatenregister's operator-entered positions from doubling farms (evidence
# 2026-10-08, wind).
PAIR_M = 500.0
GONE_M = 50.0
EARTH_M = 6_371_008.8
LIFECYCLES = ('disused', 'abandoned', 'removed', 'demolished', 'razed', 'destroyed',
              'construction', 'proposed', 'planned', 'was')
DECOMMISSION_YEAR = re.compile(r'(?:decommission\w*|removed)\s+(?:in\s+)?((?:19|20)\d{2})\b')


def number(value):
    """A register's number (a decimal comma allowed); 0 when empty or none."""
    try:
        return float(str(value).strip().replace(',', '.')) if value not in (None, '') else 0.0
    except ValueError:
        return 0.0


def parse_as_of(value):
    """Strict YYYY-MM-DD exclusive cutoff date."""
    if not re.fullmatch(r'\d{4}-\d{2}-\d{2}', value):
        raise ValueError(f'invalid --as-of date {value!r}; expected YYYY-MM-DD')
    return datetime.date.fromisoformat(value)


def parse_date_bounds(value):
    """Parse ISO/slash dates, year-months, years or year ranges into inclusive (start, end) dates."""
    if value is None:
        return None
    if isinstance(value, datetime.datetime):
        day = value.date()
        return day, day
    if isinstance(value, datetime.date):
        return value, value
    text = str(value).strip()
    if not text:
        return None
    if re.fullmatch(r'\d{4}\.0+', text):
        text = text.split('.', 1)[0]
    head = re.split(r'[T ]', text, maxsplit=1)[0]
    match = re.fullmatch(r'(\d{4})[-/](\d{1,2})[-/](\d{1,2})', head)
    if match:
        year, month, day = (int(part) for part in match.groups())
        if 1900 <= year <= 2100:
            try:
                parsed = datetime.date(year, month, day)
                return parsed, parsed
            except ValueError:
                return None
    match = re.fullmatch(r'(\d{4})[-/](\d{1,2})', head)
    if match:
        year, month = int(match.group(1)), int(match.group(2))
        if 1900 <= year <= 2100 and 1 <= month <= 12:
            last = calendar.monthrange(year, month)[1]
            return datetime.date(year, month, 1), datetime.date(year, month, last)
    match = re.fullmatch(r'(\d{4})[-/](\d{4})', head)
    if match:
        first_year, last_year = int(match.group(1)), int(match.group(2))
        if 1900 <= first_year <= last_year <= 2100:
            return datetime.date(first_year, 1, 1), datetime.date(last_year, 12, 31)
    match = re.fullmatch(r'(\d{4})', head)
    if match:
        year = int(match.group(1))
        if 1900 <= year <= 2100:
            return datetime.date(year, 1, 1), datetime.date(year, 12, 31)
    return None


def parse_epoch_ms(value):
    """Parse an epoch-millisecond timestamp into an inclusive (day, day) UTC date bound."""
    if value in (None, ''):
        return None
    try:
        ms = float(value)
        day = datetime.datetime.fromtimestamp(ms / 1000.0, tz=datetime.timezone.utc).date()
    except (TypeError, ValueError, OverflowError, OSError):
        return None
    if not (1900 <= day.year <= 2100):
        return None
    return day, day


def commissioned_at_or_after(bounds, as_of):
    # Fail closed on ranges straddling as_of (e.g. "2024/2025"): only turbines known to be
    # commissioned before as_of stand at the cutoff.
    return as_of is not None and bounds is not None and bounds[1] >= as_of


def historical_dismantled_state(bounds, as_of):
    """Resolve a dismantled row at `as_of`: restore post-cutoff removals, drop undated removals."""
    if as_of is None:
        return 'dismantled'
    if bounds is None:
        return None
    if bounds[0] >= as_of:
        return 'standing'
    if bounds[1] < as_of:
        return 'dismantled'
    return None


def to_degrees(source):
    """Easting, northing of `source` (an EPSG code or a spatial reference) to (lat, lon)."""
    if isinstance(source, int):
        code, source = source, osr.SpatialReference()
        source.ImportFromEPSG(code)
    target = osr.SpatialReference()
    target.ImportFromEPSG(4326)
    for reference in (source, target):
        reference.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
    transform = osr.CoordinateTransformation(source, target)
    return lambda x, y: transform.TransformPoint(x, y)[1::-1]


def layer_rows(path, headers=True):
    """The rows of a GDAL-readable file's first layer as dicts (and the layer's projection)."""
    options = [] if headers else ['HEADERS=DISABLE']
    source = gdal_open(path, options)
    layer = source.GetLayer(0)
    names = [layer.GetLayerDefn().GetFieldDefn(i).GetName()
             for i in range(layer.GetLayerDefn().GetFieldCount())]
    for feature in layer:
        geometry = feature.GetGeometryRef()
        point = geometry.Centroid().GetPoint_2D() if geometry else None
        yield {name: feature.GetField(name) for name in names}, point, layer.GetSpatialRef()


def gdal_open(path, options):
    from osgeo import gdal
    gdal.UseExceptions()
    return gdal.OpenEx(path, gdal.OF_VECTOR, open_options=options)


# Each register: its turbines as (state, lat, lon, hub_m, power_kw), state standing or dismantled
# (planned and applied-for turbines are not written).
def uswtdb(registers, as_of=None):
    (path,) = glob.glob(f'{registers}/uswtdb/uswtdb_[Vv]*.csv')
    with open(path, encoding='utf-8') as handle:
        for row in csv.DictReader(handle):
            if commissioned_at_or_after(parse_date_bounds(row.get('p_year')), as_of):
                continue
            # A post-cutoff retrofit updates t_hh and t_cap to the repowered machine; keep the
            # standing turbine without copying its later specifications onto an earlier map.
            if commissioned_at_or_after(parse_date_bounds(row.get('t_retro_yr')), as_of):
                hub, power = 0.0, 0.0
            else:
                hub, power = number(row['t_hh']), number(row['t_cap'])
            yield 'standing', number(row['ylat']), number(row['xlong']), hub, power


def nrcan(registers, as_of=None):
    for row, _, _ in layer_rows(f'{registers}/nrcan/Wind_Turbine_Database_en.xlsx'):
        if commissioned_at_or_after(parse_date_bounds(row.get('Commissioning')), as_of):
            continue
        note = str(row['Notes'] or '').lower()
        if 'decommission' in note or 'removed' in note:
            match = DECOMMISSION_YEAR.search(note)
            state = historical_dismantled_state(
                parse_date_bounds(match.group(1)) if match else None, as_of)
            if state is None:
                continue
        else:
            state = 'standing'
        yield state, number(row['Latitude']), number(row['Longitude']), \
            number(row['Hub Height (m)']), number(row['Turbine Rated Capacity (kW)'])


MASTR_STATES = {'35': 'standing', '37': 'dismantled', '38': 'dismantled'}
MASTR_SHUTDOWN_FIELD = {
    '37': 'DatumBeginnVoruebergehendeStilllegung',
    '38': 'DatumEndgueltigeStilllegung',
}


def mastr(registers, as_of=None):
    for path in sorted(glob.glob(f'{registers}/mastr/EinheitenWind*.xml')):
        for _, element in ElementTree.iterparse(path):
            if element.tag != 'EinheitWind':
                continue
            field = {child.tag: child.text for child in element}
            element.clear()
            status = field.get('EinheitBetriebsstatus')
            state = MASTR_STATES.get(status)
            lat, lon = number(field.get('Breitengrad')), number(field.get('Laengengrad'))
            if not (state and lat and lon):
                continue
            if commissioned_at_or_after(parse_date_bounds(field.get('Inbetriebnahmedatum')), as_of):
                continue
            if state == 'dismantled':
                shutdown = parse_date_bounds(field.get(MASTR_SHUTDOWN_FIELD[status]))
                state = historical_dismantled_state(shutdown, as_of)
                if state is None:
                    continue
            power = number(field.get('Bruttoleistung')) or number(field.get('Nettonennleistung'))
            yield state, lat, lon, number(field.get('Nabenhoehe')), power


def ens(registers, as_of=None):
    """Columns of the current and the historic file (three title rows each); UTM 32 (ETRS89); a
    deregistration date or the historic file means dismantled; small turbines lack positions."""
    degrees = to_degrees(25832)
    files = (('current.xlsx', dict(conn=2, gone=3, kw=4, hub=6, x=13, y=14), False),
             ('historic.xlsx', dict(conn=1, gone=2, kw=3, hub=5, x=12, y=13), True))
    for name, column, historic in files:
        for index, (row, _, _) in enumerate(layer_rows(f'{registers}/ens/{name}', headers=False)):
            values = list(row.values())
            x, y = number(values[column['x']]), number(values[column['y']])
            if index < 3 or x < 100_000 or y < 1_000_000:
                continue
            if commissioned_at_or_after(parse_date_bounds(values[column['conn']]), as_of):
                continue
            lat, lon = degrees(x, y)
            gone_raw = values[column['gone']]
            if historic or gone_raw:
                state = historical_dismantled_state(parse_date_bounds(gone_raw), as_of)
                if state is None:
                    continue
            else:
                state = 'standing'
            yield state, lat, lon, number(values[column['hub']]), number(values[column['kw']])


SWEDISH_STATES = {'Uppfört': 'standing', 'Nedmonterat': 'dismantled'}


def vindbrukskollen(registers, as_of=None):
    (path,) = glob.glob(f'{registers}/vindbrukskollen/**/*.gpkg', recursive=True) or \
        glob.glob(f'{registers}/vindbrukskollen/**/*.shp', recursive=True)
    degrees = None
    for row, point, reference in layer_rows(path):
        degrees = degrees or to_degrees(reference)
        state = SWEDISH_STATES.get(row['STATUS'])
        if not (state and point):
            continue
        if commissioned_at_or_after(parse_date_bounds(row.get('UPPFORT')), as_of):
            continue
        # Vindbrukskollen carries no dismantlement date, so a later Nedmonterat cannot drop an
        # earlier mapped turbine.
        if state == 'dismantled' and as_of is not None:
            continue
        lat, lon = degrees(*point)
        yield state, lat, lon, number(row['NAVHOJD']), number(row['MAXEFFEKT']) * 1000


def nve(registers, as_of=None):
    """Turbines of farms in operation (status D); a turbine's rating is its farm's power in
    operation over its turbine count; no hub heights."""
    with open(f'{registers}/nve/parks.geojson', encoding='utf-8') as handle:
        farms = json.load(handle)['features']
    unit_kw = {}
    farm_dates = {}
    for farm in farms:
        props = farm['properties']
        key = props['anleggsnr']
        if props.get('antallturbiner') and props.get('effekt_mw_idrift'):
            unit_kw[key] = props['effekt_mw_idrift'] * 1000 / props['antallturbiner']
        farm_dates[key] = (
            parse_epoch_ms(props.get('forsteidriftdato')),
            parse_epoch_ms(props.get('utavdriftdato')),
        )
    with open(f'{registers}/nve/turbines.geojson', encoding='utf-8') as handle:
        turbines = json.load(handle)['features']
    for turbine in turbines:
        p, geometry = turbine['properties'], turbine['geometry']
        if not geometry:
            continue
        opened, closed = farm_dates.get(p['anleggsnr'], (None, None))
        if commissioned_at_or_after(opened, as_of):
            continue
        lon, lat = geometry['coordinates'][:2]
        if closed is not None and as_of is not None:
            state = historical_dismantled_state(closed, as_of)
            if state is None:
                continue
            yield state, lat, lon, 0.0, unit_kw.get(p['anleggsnr'], 0.0)
            continue
        if p['status'] != 'D':
            continue
        yield 'standing', lat, lon, 0.0, unit_kw.get(p['anleggsnr'], 0.0)


def clm(registers, as_of=None):
    """Castilla-La Mancha: no state field; UTM 30 on ED50 (EPSG:23030), not ETRS89 (as ETRS89 a
    median 136 m from the mapped turbines, as ED50 3 m)."""
    del as_of
    degrees = to_degrees(23030)
    with open(f'{registers}/clm/turbines.csv', encoding='latin-1') as handle:
        for row in csv.DictReader(handle):
            x, y = number(row['UTM_X']), number(row['UTM_Y'])
            if x and y:
                lat, lon = degrees(x, y)
                yield 'standing', lat, lon, number(row['ALTURA_BUJE']), number(row['POTENCIA_UNI'])


def rivm(registers, as_of=None):
    """The Netherlands' and Belgium's turbines (its German border ones are the
    Marktstammdatenregister's), from their RD New coordinates (EPSG:28992)."""
    del as_of
    degrees = to_degrees(28992)
    with open(f'{registers}/rivm/turbines.geojson', encoding='utf-8') as handle:
        features = json.load(handle)['features']
    for feature in features:
        p = feature['properties']
        if p['land'] != 'Duitsland':
            lat, lon = degrees(p['x'], p['y'])
            yield 'standing', lat, lon, number(p['ash']), number(p['kw'])


REGISTERS = (uswtdb, nrcan, mastr, ens, vindbrukskollen, nve, clm, rivm)


def standing_in_osm(tags):
    """Whether OpenStreetMap's tags describe a turbine standing today: no farm outline, no
    lifecycle prefix (`removed:power`), key (`disused=yes`) or value (`power=construction`)."""
    if tags.get('power') == 'plant' or tags.get('power') in LIFECYCLES:
        return False
    if any(any(key.startswith(lifecycle + ':') for key in tags)
           or tags.get(lifecycle) not in (None, 'no') for lifecycle in LIFECYCLES):
        return False
    return tags.get('operational_status') not in ('closed', 'abandoned', 'disused', 'decommissioned')


def osm_turbines(lines):
    seen = set()
    for line in lines:
        line = line.strip().lstrip('\x1e')
        if not line:
            continue
        feature = json.loads(line)
        properties = feature['properties']
        tags = {k: v for k, v in properties.items() if not k.startswith('@')}
        identity = (properties.get('@type'), properties.get('@id'))
        turbine = tags.get('generator:source') == 'wind' or tags.get('man_made') == 'wind_turbine'
        if not turbine or identity in seen or not standing_in_osm(tags):
            continue
        seen.add(identity)
        geometry = feature['geometry']
        points = {'Point': lambda c: [c], 'LineString': lambda c: c,
                  'Polygon': lambda c: c[0][:-1],
                  'MultiPolygon': lambda c: [p for polygon in c for p in polygon[0][:-1]]
                  }[geometry['type']](geometry['coordinates'])
        hub = re.match(r'\s*([0-9]+(?:[.,][0-9]+)?)\s*(m|metres|meters)?\s*$',
                       tags.get('height:hub') or tags.get('hub:height') or tags.get('hub_height') or '')
        power = re.match(r'\s*([0-9]+(?:[.,][0-9]+)?)\s*(gw|mw|kw|w)\b',
                         (tags.get('generator:output:electricity') or '').lower())
        yield [sum(p[1] for p in points) / len(points), sum(p[0] for p in points) / len(points),
               number(hub.group(1)) if hub else 0.0,
               number(power.group(1)) * {'gw': 1e6, 'mw': 1e3, 'kw': 1.0, 'w': 1e-3}[power.group(2)]
               if power else 0.0, 'osm']


def unit(lat, lon):
    phi, lam = math.radians(lat), math.radians(lon)
    return math.cos(phi) * math.cos(lam), math.cos(phi) * math.sin(lam), math.sin(phi)


class Grid:
    """Points on the unit sphere in cells of a chord's side, to find those within it."""

    def __init__(self, metres):
        self.chord = 2.0 * math.sin(metres / EARTH_M / 2.0)
        self.cells = {}

    def key(self, v):
        return tuple(math.floor(c / self.chord) for c in v)

    def add(self, index, v):
        self.cells.setdefault(self.key(v), []).append((index, v))

    def near(self, v):
        kx, ky, kz = self.key(v)
        for cell in ((kx + i, ky + j, kz + k) for i in (-1, 0, 1) for j in (-1, 0, 1)
                     for k in (-1, 0, 1)):
            for index, w in self.cells.get(cell, ()):
                d = math.dist(v, w)
                if d <= self.chord:
                    yield d, index


def main(registers, out_path, admitted, as_of=None, stdin=None):
    turbines = list(osm_turbines(sys.stdin if stdin is None else stdin))
    osm_count = len(turbines)
    paired = [False] * osm_count
    dropped = set()
    for register in REGISTERS:
        if register.__name__ not in admitted:
            print(f'{register.__name__}: not admitted for this year', file=sys.stderr)
            continue
        rows = list(register(registers, as_of=as_of))
        grid = Grid(PAIR_M)
        for index, turbine in enumerate(turbines):
            if index >= osm_count or not paired[index]:
                grid.add(index, unit(turbine[0], turbine[1]))
        standing = [row for row in rows if row[0] == 'standing']
        pairs = sorted((d, index, j) for j, row in enumerate(standing)
                       for d, index in grid.near(unit(row[1], row[2])))
        taken, matched = set(), set()
        for _, index, j in pairs:
            if index in taken or j in matched:
                continue
            taken.add(index)
            matched.add(j)
            hub, power = standing[j][3], standing[j][4]
            turbines[index][2] = hub or turbines[index][2]
            turbines[index][3] = power or turbines[index][3]
            if index < osm_count:
                paired[index] = True
        turbines += [[row[1], row[2], row[3], row[4], register.__name__]
                     for j, row in enumerate(standing) if j not in matched]
        gone = Grid(GONE_M)
        for index in range(osm_count):
            gone.add(index, unit(turbines[index][0], turbines[index][1]))
        for row in rows:
            if row[0] == 'dismantled':
                dropped.update(index for _, index in gone.near(unit(row[1], row[2]))
                               if not paired[index])
        print(f'{register.__name__}: {len(standing)} standing, {len(matched)} paired, '
              f'{len(standing) - len(matched)} added', file=sys.stderr)
    kept = sorted(t for index, t in enumerate(turbines) if index not in dropped)
    with open(out_path, 'w') as out:
        for lat, lon, hub, power, source in kept:
            out.write(f'{lat:.6f}\t{lon:.6f}\t{hub:g}\t{power:g}\t{source}\n')
    print(f'{len(kept)} turbines: {osm_count} standing in OpenStreetMap, {len(dropped)} of them '
          f'dismantled, {len(turbines) - osm_count} from the registers', file=sys.stderr)


def parse_args(argv):
    parser = argparse.ArgumentParser(description='Merge OpenStreetMap and national wind turbine registers.')
    parser.add_argument('--as-of', help='Exclusive map cutoff date YYYY-MM-DD for historical register filtering.')
    parser.add_argument('registers')
    parser.add_argument('out')
    parser.add_argument('names', nargs='*')
    args = parser.parse_args(argv)
    as_of = parse_as_of(args.as_of) if args.as_of else None
    named = args.names or [register.__name__ for register in REGISTERS]
    unknown = set(named) - {register.__name__ for register in REGISTERS}
    if unknown:
        sys.exit(f'unknown registers {sorted(unknown)}')
    return args.registers, args.out, set(named), as_of


if __name__ == '__main__':
    registers, out_path, admitted, as_of = parse_args(sys.argv[1:])
    main(registers, out_path, admitted, as_of=as_of)

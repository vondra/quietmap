"""The wind turbines standing today (`turbines.sh`), one line each: `lat lon hub_m power_kw source`
(tab-separated, sorted by latitude then longitude, 0 when unknown; source `osm` or the register
that placed it). OpenStreetMap's standing turbines (no lifecycle prefix, key or value; a node its
point, an outline the mean of its vertices) come first; each register in turn pairs its standing
turbines one to one with those not yet paired, nearest pairs first, within PAIR_M: a pair keeps
its position and takes the register's hub and power where known, an unpaired standing turbine of
the register is added, and an unpaired OpenStreetMap turbine within GONE_M of one the register
calls dismantled is dropped. Reads osmium's GeoJSON sequence of the turbines on stdin.

    osmium export turbines.osm.pbf -f geojsonseq -a type,id | python3 turbines.py REGISTERS OUT
"""
import csv
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


def number(value):
    """A register's number (a decimal comma allowed); 0 when empty or none."""
    try:
        return float(str(value).strip().replace(',', '.')) if value not in (None, '') else 0.0
    except ValueError:
        return 0.0


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
def uswtdb(registers):
    (path,) = glob.glob(f'{registers}/uswtdb/uswtdb_V*.csv')
    for row in csv.DictReader(open(path, encoding='utf-8')):
        yield 'standing', number(row['ylat']), number(row['xlong']), number(row['t_hh']), \
            number(row['t_cap'])


def nrcan(registers):
    for row, _, _ in layer_rows(f'{registers}/nrcan/Wind_Turbine_Database_en.xlsx'):
        note = str(row['Notes'] or '').lower()
        state = 'dismantled' if 'decommission' in note or 'removed' in note else 'standing'
        yield state, number(row['Latitude']), number(row['Longitude']), \
            number(row['Hub Height (m)']), number(row['Turbine Rated Capacity (kW)'])


MASTR_STATES = {'35': 'standing', '37': 'dismantled', '38': 'dismantled'}


def mastr(registers):
    for path in sorted(glob.glob(f'{registers}/mastr/EinheitenWind*.xml')):
        for _, element in ElementTree.iterparse(path):
            if element.tag != 'EinheitWind':
                continue
            field = {child.tag: child.text for child in element}
            element.clear()
            state = MASTR_STATES.get(field.get('EinheitBetriebsstatus'))
            lat, lon = number(field.get('Breitengrad')), number(field.get('Laengengrad'))
            if state and lat and lon:
                power = number(field.get('Bruttoleistung')) or number(field.get('Nettonennleistung'))
                yield state, lat, lon, number(field.get('Nabenhoehe')), power


def ens(registers):
    """Columns of the current and the historic file (three title rows each); UTM 32 (ETRS89); a
    deregistration date or the historic file means dismantled; small turbines lack positions."""
    degrees = to_degrees(25832)
    files = (('current.xlsx', dict(gone=3, kw=4, hub=6, x=13, y=14), False),
             ('historic.xlsx', dict(gone=2, kw=3, hub=5, x=12, y=13), True))
    for name, column, historic in files:
        for index, (row, _, _) in enumerate(layer_rows(f'{registers}/ens/{name}', headers=False)):
            values = list(row.values())
            x, y = number(values[column['x']]), number(values[column['y']])
            if index < 3 or x < 100_000 or y < 1_000_000:
                continue
            lat, lon = degrees(x, y)
            state = 'dismantled' if historic or values[column['gone']] else 'standing'
            yield state, lat, lon, number(values[column['hub']]), number(values[column['kw']])


SWEDISH_STATES = {'Uppfört': 'standing', 'Nedmonterat': 'dismantled'}


def vindbrukskollen(registers):
    (path,) = glob.glob(f'{registers}/vindbrukskollen/**/*.gpkg', recursive=True) or \
        glob.glob(f'{registers}/vindbrukskollen/**/*.shp', recursive=True)
    degrees = None
    for row, point, reference in layer_rows(path):
        degrees = degrees or to_degrees(reference)
        state = SWEDISH_STATES.get(row['STATUS'])
        if state and point:
            lat, lon = degrees(*point)
            yield state, lat, lon, number(row['NAVHOJD']), number(row['MAXEFFEKT']) * 1000


def nve(registers):
    """Turbines of farms in operation (status D); a turbine's rating is its farm's power in
    operation over its turbine count; no hub heights."""
    farms = json.load(open(f'{registers}/nve/parks.geojson'))['features']
    unit_kw = {farm['properties']['anleggsnr']: farm['properties']['effekt_mw_idrift'] * 1000
               / farm['properties']['antallturbiner']
               for farm in farms if farm['properties'].get('antallturbiner')
               and farm['properties'].get('effekt_mw_idrift')}
    for turbine in json.load(open(f'{registers}/nve/turbines.geojson'))['features']:
        p, geometry = turbine['properties'], turbine['geometry']
        if geometry and p['status'] == 'D':
            lon, lat = geometry['coordinates'][:2]
            yield 'standing', lat, lon, 0.0, unit_kw.get(p['anleggsnr'], 0.0)


def clm(registers):
    """Castilla-La Mancha: no state field; UTM 30 on ED50 (EPSG:23030), not ETRS89 (as ETRS89 a
    median 136 m from the mapped turbines, as ED50 3 m)."""
    degrees = to_degrees(23030)
    for row in csv.DictReader(open(f'{registers}/clm/turbines.csv', encoding='latin-1')):
        x, y = number(row['UTM_X']), number(row['UTM_Y'])
        if x and y:
            lat, lon = degrees(x, y)
            yield 'standing', lat, lon, number(row['ALTURA_BUJE']), number(row['POTENCIA_UNI'])


def rivm(registers):
    """The Netherlands' and Belgium's turbines (its German border ones are the
    Marktstammdatenregister's), from their RD New coordinates (EPSG:28992)."""
    degrees = to_degrees(28992)
    for feature in json.load(open(f'{registers}/rivm/turbines.geojson', encoding='utf-8'))['features']:
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


def main(registers, out_path):
    turbines = list(osm_turbines(sys.stdin))
    osm_count = len(turbines)
    paired = [False] * osm_count
    dropped = set()
    for register in REGISTERS:
        rows = list(register(registers))
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


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])

"""Fetch Hessen DGM1: per-Gemeinde zips reduced to 5 m in-flight.

The download-center REST API lists one zip per municipality; each zip holds
1 km DGM1 tiles (plain TIFF plus world file). Tiles are verified, the 1 m
lattice is area-averaged to retained 5 m grids, and the raw zips are deleted;
a checksum manifest keeps the audit trail. Per-tile flight dates come from
the provider metadata table, fetched once with its own provenance.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime
import hashlib
import json
from pathlib import Path
import re
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

from osgeo import gdal
from dgm_reduce import (append_journal, assert_crs, derive_provenance, digest, load_journal,
                        manifest_entry, publish_country_sources, reduce_geotiff,
                        write_manifest)
from terrain_io import fetch, source_budget

gdal.UseExceptions()
gdal.SetConfigOption('GDAL_PAM_ENABLED', 'NO')

PROVIDER = 'de-he-dgm1'
REST = 'https://gds.hessen.de/INTERSHOP/rest/WFS/HLBG-Geodaten-Site/-/downloadcenter'
DGM1_PATH = '3D-Daten/Digitales Geländemodell (DGM1)'
DOWNLOAD_HOST = 'https://gds.hessen.de'
METADATA_URL = ('https://gds.hessen.de/INTERSHOP/static/WFS/HLBG-Geodaten-Site/-/HLBG-Geodaten/'
                'de_DE/Downloadcenter/Daten/3D-Daten/Metadaten_DGM-DOM_August-2026.xlsx')
METADATA_NAME = 'Metadaten_DGM-DOM_August-2026.xlsx'
LICENCE = 'Datenlizenz Deutschland - Zero - Version 2.0'
LICENCE_URL = 'https://www.govdata.de/dl-de/zero-2-0'
GROUP = 'DE-HE-DGM1'
EPSG = 25832
VERTICAL_EPSG = 7837
USER_AGENT = {'User-Agent': 'quietmap-dem-de/1.0 (+https://quietmap.example.invalid)'}


def rest_get(path, page=None):
    query = {'path': path, 'navigation': 'all'}
    if page:
        query['page'] = page
    url = REST + '?' + urllib.parse.urlencode(query)
    request = urllib.request.Request(url, headers=USER_AGENT)
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.loads(response.read())


def list_kreise():
    """Level-3 categories below the DGM1 product are the Hessian districts."""
    document = rest_get(DGM1_PATH)
    # The DGM1 node id selects its own children; names repeat across branches.
    dgm1 = next(e for e in document['navigation'] if e['name'] == 'Digitales Geländemodell (DGM1)')
    kreise = [e['name'] for e in document['navigation']
              if e.get('level') == 3 and e.get('parentId') == dgm1['id']]
    if not kreise:
        raise ValueError('no Hessian districts below the DGM1 category')
    return kreise


def list_gemeinden(kreis):
    """Per-municipality zips; Kreis packages are redundant and skipped."""
    items, page = [], 1
    while True:
        document = rest_get(f'{DGM1_PATH}/{kreis}', page=page if page > 1 else None)
        result = document['searchresult']
        for download in result['downloads']:
            if download.get('downloadType') != 'File':
                continue
            uri = download['downloadLink']['uri']
            items.append(dict(name=uri.rsplit('/', 1)[1],
                              url=DOWNLOAD_HOST + urllib.parse.quote(uri),
                              kreis=kreis, creation=download.get('creationDate', '')))
        paging = result.get('paging', {})
        if not paging.get('next', False):
            break
        page += 1
    if not items:
        raise ValueError(f'{kreis}: no municipality zips in the download center')
    return items


def excel_date(serial):
    return (datetime.date(1970, 1, 1) + datetime.timedelta(days=int(serial) - 25569)).isoformat()


def load_flight_dates(path):
    """Map Kachelname to flight date from the provider metadata table (stdlib xlsx)."""
    namespace = '{http://schemas.openxmlformats.org/spreadsheetml/2006/main}'
    with zipfile.ZipFile(path) as archive:
        strings = re.findall(r'<t>([^<]*)</t>',
                             archive.read('xl/sharedStrings.xml').decode('utf-8'))
        sheet = ET.fromstring(archive.read('xl/worksheets/sheet1.xml'))
    dates = {}
    for row in sheet.findall(f'.//{namespace}row'):
        cells = {}
        for cell in row.findall(f'{namespace}c'):
            value = cell.find(f'{namespace}v')
            if value is None or value.text is None:
                continue
            text = strings[int(value.text)] if cell.get('t') == 's' else value.text
            cells[cell.get('r').rstrip('0123456789')] = text
        kachel = cells.get('A', '')
        if kachel.isdigit() and len(kachel) == 7 and cells.get('B', '').isdigit():
            date = excel_date(cells['B'])
            if kachel in dates and dates[kachel] != date:
                raise ValueError(f'{path}: conflicting flight dates for tile {kachel}')
            dates[kachel] = date
    if len(dates) < 20000:
        raise ValueError(f'{path}: only {len(dates)} dated tiles; the table format moved')
    return dates


def kachel_of(member):
    # dgm1_32_492_5509_1_he.tif -> 4925509, the metadata table key.
    parts = Path(member).stem.split('_')
    return parts[2] + parts[3]


def decode_zip(root, item, flight_dates, done):
    """Reduce every 1 km member of one municipality zip to a retained 5 m grid."""
    raw_path = Path(root) / PROVIDER / item['name']
    derived = []
    with zipfile.ZipFile(raw_path) as archive:
        members = sorted(n for n in archive.namelist() if n.endswith('.tif'))
        if not members:
            raise ValueError(f"{item['name']}: no tif members in the archive")
        sidecars = set(archive.namelist())
        for member in members:
            out_name = Path(member).stem + '-5m.tif'
            out_path = Path(root) / PROVIDER / out_name
            if out_name in done and out_path.exists():
                continue
            member_bytes = archive.read(member)
            with tempfile.TemporaryDirectory() as temp:
                tif_path = Path(temp) / Path(member).name
                tif_path.write_bytes(member_bytes)
                # Plain TIFF plus world file: the world file carries the georeference.
                world = member.rsplit('.', 1)[0] + '.tfw'
                if world in sidecars:
                    (Path(temp) / Path(world).name).write_bytes(archive.read(world))
                probe = gdal.Open(str(tif_path))
                nodata = probe.GetRasterBand(1).GetNoDataValue()
                probe = None
                if nodata is None:
                    # Hessian tiles carry no nodata tag; the AdV DGM1 void
                    # value applies, and no Hessian terrain sits at -9999 m.
                    nodata = -9999.0
                stats = reduce_geotiff(tif_path, out_path, nodata, epsg=EPSG)
            assert_crs(out_path, EPSG)
            kachel = kachel_of(member)
            epoch = (f"ALS {flight_dates[kachel]}" if kachel in flight_dates else
                     'unknown ALS epoch (absent from the provider metadata table)')
            derive_provenance(raw_path, out_path,
                              f'Decoded {member} ({item["kreis"]}), area-averaged to 5 m')
            entry = manifest_entry(item['url'], raw_path, out_path,
                                   'Area-averaged 5 m grid from the verified DGM1 tile')
            # One zip holds many tiles; the manifest counts the member, not the zip.
            entry['raw_bytes'] = len(member_bytes)
            entry['raw_sha256'] = hashlib.sha256(member_bytes).hexdigest()
            entry['epoch'] = epoch
            entry['member'] = member
            if stats['valid_fraction'] == 0:
                entry['method'] = 'fully void tile, nothing retained'
                entry['derived'], entry['derived_sha256'], entry['derived_bytes'] = None, None, 0
                out_path.unlink()
                Path(str(out_path) + '.provenance.json').unlink()
            derived.append(entry)
    return derived


def process_item(root, item, flight_dates, delay, state, done):
    with state['lock']:
        wait = delay - (time.monotonic() - state['last'])
        if wait > 0:
            time.sleep(wait)
        state['last'] = time.monotonic()
    fetch(root, PROVIDER, item['name'], item['url'], licence=LICENCE,
          licence_url=LICENCE_URL, terms_checked_utc='2026-09-25')
    entries = decode_zip(root, item, flight_dates, done)
    raw_path = Path(root) / PROVIDER / item['name']
    raw_path.unlink()
    Path(str(raw_path) + '.provenance.json').unlink()
    with state['lock']:
        for entry in entries:
            append_journal(state['journal'], entry)
        append_journal(state['zips'], dict(zip=item['name'], members=len(entries)))
        state['entries'].extend(entries)
    return entries


def fetch_all(root, workers=3, delay=0.5, limit=None, only=None):
    journal = Path(root) / PROVIDER / 'stream-manifest.journal.jsonl'
    zips_journal = Path(root) / PROVIDER / 'zips.journal.jsonl'
    journal.parent.mkdir(parents=True, exist_ok=True)
    entries, done = load_journal(journal)
    done_zips = set()
    if zips_journal.exists():
        done_zips = {json.loads(line)['zip'] for line in zips_journal.read_text().splitlines()
                     if line.strip()}
    fetch(root, PROVIDER, METADATA_NAME, METADATA_URL, licence=LICENCE,
          licence_url=LICENCE_URL, terms_checked_utc='2026-09-25')
    flight_dates = load_flight_dates(Path(root) / PROVIDER / METADATA_NAME)
    items = []
    for kreis in list_kreise():
        items.extend(list_gemeinden(kreis))
    if only:
        items = [item for item in items if item['name'] in only]
    items = [item for item in items if item['name'] not in done_zips]
    items = items[:limit]
    print(f'{PROVIDER}: {len(items)} municipality zips indexed, {len(flight_dates)} dated tiles')
    state = {'lock': threading.Lock(), 'last': 0.0, 'journal': journal,
             'zips': zips_journal, 'entries': entries}
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(process_item, root, item, flight_dates, delay, state, done)
                   for item in items]
        for future in futures:
            future.result()
    manifest = Path(root) / PROVIDER / 'stream-manifest.json'
    try:
        write_manifest(manifest, entries)
    except ValueError:
        manifest.unlink()
        write_manifest(manifest, entries)
    value = publish_country_sources(root, PROVIDER, entries, EPSG, VERTICAL_EPSG,
                                    GROUP, 'unknown ALS epoch')
    with source_budget(Path(root)) as available:
        print(f'{PROVIDER}: {len(value)} tiles retained, {available / 1e9:.1f} GB budget left')
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root')
    parser.add_argument('--workers', type=int, default=3)
    parser.add_argument('--delay', type=float, default=0.5)
    parser.add_argument('--limit', type=int, default=None)
    parser.add_argument('--only', nargs='*', default=None)
    args = parser.parse_args()
    fetch_all(args.root, workers=args.workers, delay=args.delay,
              limit=args.limit, only=args.only)


if __name__ == '__main__':
    main()

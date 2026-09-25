"""Fetch per-tile DGM for Rheinland-Pfalz, Niedersachsen, Sachsen, Thüringen, MV.

Each provider has its own open index (metalink, STAC, ArcGIS REST, INSPIRE ATOM
or the LAiV download AJAX). Tiles are verified, area-averaged to retained 5 m
grids, and the raw 1 m bytes are deleted; a checksum manifest keeps the audit
trail. MV serves DGM5 directly, which is only normalised, never re-averaged.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path
import struct
import threading
import time
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
import json
import hashlib
import tempfile
import zlib

from osgeo import gdal
from dgm_reduce import (append_journal, assert_crs, derive_provenance, digest, load_journal,
                        manifest_entry, normalize_grid, publish_country_sources,
                        read_zip_member, reduce_geotiff, write_manifest)
from terrain_io import fetch, publish_json, source_budget

gdal.UseExceptions()
gdal.SetConfigOption('GDAL_PAM_ENABLED', 'NO')

VERTICAL_EPSG = 7837
USER_AGENT = {'User-Agent': 'quietmap-dem-de/1.0 (+https://quietmap.example.invalid)'}

PROVIDERS = {
    'de-rp-dgm1': dict(
        kind='metalink', epsg=25832, workers=3, delay=0.4,
        catalogue='https://geobasis-rlp.de/data/dgm1/current/meta4/dgm1_tif_07.meta4',
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0', group='DE-RP-DGM1'),
    'de-ni-dgm1': dict(
        kind='stac', epsg=25832, workers=6, delay=0.2,
        catalogue='https://dgm.stac.lgln.niedersachsen.de/collections/dgm1/items',
        licence='Creative Commons Attribution 4.0 International',
        licence_url='https://creativecommons.org/licenses/by/4.0/', group='DE-NI-DGM1'),
    'de-sn-dgm1': dict(
        kind='arcgis', epsg=25833, workers=4, delay=0.5,
        catalogue=('https://geodienste.sachsen.de/ags-relay/ArcGISServer/guest/arcgis/rest/'
                   'services/geosn/rest_geosn_downloadlinks/MapServer/6/query'),
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0', group='DE-SN-DGM1'),
    'de-th-dgm1': dict(
        kind='atom', epsg=25832, workers=3, delay=0.5,
        catalogue=('https://geoportal.geoportal-th.de/dienste/atom_th_hoehendaten_dgm'
                   '?type=dataset&id=14418d25-fcd7-4a3f-99a9-e3059a2772af'),
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0', group='DE-TH-DGM1'),
    'de-mv-dgm5': dict(
        kind='dla', epsg=25833, workers=2, delay=0.5,
        catalogue='https://laiv.geodaten-mv.de/afgvk/_apps/dladownload/_ajax/overview.php',
        licence='Creative Commons Attribution 4.0 International',
        licence_url='https://creativecommons.org/licenses/by/4.0/', group='DE-MV-DGM5'),
}


def http_get(url, params=None, timeout=120):
    if params:
        url = url + ('&' if '?' in url else '?') + urllib.parse.urlencode(params)
    request = urllib.request.Request(url, headers=USER_AGENT)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read()


def parse_metalink(data):
    root = ET.fromstring(data)
    namespace = '{urn:ietf:params:xml:ns:metalink}'
    items = []
    for entry in root.findall(f'.//{namespace}file'):
        name = entry.attrib['name']
        if not name.endswith('.tif'):
            continue
        size = int(entry.find(namespace + 'size').text)
        digest_node = entry.find(namespace + 'hash')
        url = entry.find(namespace + 'url').text
        year = name.rsplit('_', 1)[1].replace('.tif', '')
        items.append(dict(name=name, url=url, epoch=f'ALS {year}',
                          expected_size=size,
                          expected_sha256=digest_node.text if digest_node is not None else None))
    return items


def parse_stac_page(page):
    items = []
    for feature in page.get('features', []):
        asset = feature['assets']['dgm1-tif']['href']
        stamp = feature['properties'].get('datetime', '')[:10]
        items.append(dict(name=feature['id'] + '.tif', url=asset,
                          epoch=f'ALS {stamp}' if stamp else 'ALS epoch in STAC item'))
    links = {link.get('rel'): link.get('href') for link in page.get('links', [])}
    return items, links.get('next')


def list_stac(catalogue):
    # Token paging: the server ignores offset and issues opaque next links.
    # Several vintages share a footprint; keep the newest acquisition per tile.
    seen, best = set(), {}
    url = catalogue + '?limit=500'
    while url:
        request = urllib.request.Request(url, headers=USER_AGENT)
        with urllib.request.urlopen(request, timeout=120) as response:
            page = json.loads(response.read())
        batch, url = parse_stac_page(page)
        fresh = [item for item in batch if item['name'] not in seen]
        if not fresh:
            break
        seen.update(item['name'] for item in fresh)
        for item in fresh:
            footprint = tuple(item['name'].split('_')[2:4])
            if footprint not in best or _vintage(item) > _vintage(best[footprint]):
                best[footprint] = item
    return sorted(best.values(), key=lambda item: item['name'])


def _vintage(item):
    """Newest acquisition wins; an undated item never beats a dated one."""
    stamp = item['epoch'][4:]
    return (stamp[:4].isdigit(), stamp)


def parse_arcgis_page(page):
    items = []
    for feature in page.get('features', []):
        attrs = feature['attributes']
        if attrs.get('Produkt') != 'DGM1':
            continue
        items.append(dict(name=f"dgm1_sn_{attrs['Kachel']}.zip", url=attrs['Download'],
                          epoch=f"ALS {attrs.get('Stand', '')}".rstrip()))
    more = page.get('exceededTransferLimit', False)
    return items, more


def list_arcgis(catalogue):
    items, offset, limit = [], 0, 1000
    while True:
        page = json.loads(http_get(catalogue, {
            'where': '1=1', 'resultOffset': offset, 'resultRecordCount': limit,
            'outFields': 'Produkt,Kachel,Download,Stand', 'returnGeometry': 'false', 'f': 'json'}))
        batch, more = parse_arcgis_page(page)
        fresh = [item for item in batch if item['name'] not in {i['name'] for i in items}]
        items.extend(fresh)
        got = len(page.get('features', []))
        if not more or not got or not fresh:
            break
        offset += got
    return items


def parse_atom(data):
    root = ET.fromstring(data)
    namespace = '{http://www.w3.org/2005/Atom}'
    rank = {'dgm_2020-2025': 0, 'dgm_2014-2019': 1, 'dgm_2010-2013': 2}
    best = {}
    for link in root.findall(f'.//{namespace}link[@rel="section"]'):
        url = link.attrib['href']
        if '/hoehendaten/DGM/' not in url or not url.endswith('.zip'):
            continue
        epoch = url.split('/hoehendaten/DGM/')[1].split('/')[0]
        name = url.rsplit('/', 1)[1]
        parts = name.replace('.zip', '').split('_')
        kachel = (parts[-5], parts[-4])
        if kachel not in best or rank.get(epoch, 9) < rank.get(best[kachel][0], 9):
            best[kachel] = (epoch, name, url)
    return [dict(name=name, url=url, epoch=f"ALS {epoch.removeprefix('dgm_')}")
            for epoch, name, url in best.values()]


MV_BBOX = (200000.0, 5900000.0, 560000.0, 6100000.0)
MV_DOWNLOAD = 'https://www.geodaten-mv.de/dienste/dgm_download'
MV_DATASET = 'df758744-b2a1-4d39-b34c-9ec5bf9b4372'


def list_dla(catalogue):
    west, south, east, north = MV_BBOX
    step = 20000.0
    tiles = {}
    y = south
    while y < north:
        x = west
        while x < east:
            page = json.loads(http_get(catalogue, {
                'bbox': f'{x:.0f},{y:.0f},{min(x + step, east):.0f},{min(y + step, north):.0f}',
                'crs': 'EPSG:25833', 'type': 'dgm5'}))
            if not page.get('success'):
                raise ValueError(f"MV index refused the {x:.0f}/{y:.0f} cell: {page}")
            for feature in page['result']['features']:
                props = feature['properties']
                kachel = props['kachel_nr']
                url = (f'{MV_DOWNLOAD}?index=2&dataset={MV_DATASET}'
                       f'&file=dgm5_{kachel}_2_gtiff.tif')
                candidate = dict(name=f'dgm5_{kachel}_2_gtiff.tif', url=url,
                                 epoch=f"ALS {props.get('aktualitaet', '')}".rstrip())
                # Grid cells overlap at edges; keep the newest vintage per tile.
                if kachel not in tiles or candidate['epoch'] > tiles[kachel]['epoch']:
                    tiles[kachel] = candidate
            x += step
        y += step
    return sorted(tiles.values(), key=lambda item: item['name'])


def list_metalink(catalogue):
    return parse_metalink(http_get(catalogue))


def list_atom(catalogue):
    return parse_atom(http_get(catalogue))


LISTERS = {'metalink': list_metalink, 'stac': list_stac, 'arcgis': list_arcgis,
           'atom': list_atom, 'dla': list_dla}


def http_range(url, first, last, timeout=120):
    request = urllib.request.Request(url, headers=dict(USER_AGENT, Range=f'bytes={first}-{last}'))
    with urllib.request.urlopen(request, timeout=timeout) as response:
        if response.status != 206:
            raise ValueError(f'{url}: range requests unsupported (HTTP {response.status})')
        return response.read(), dict(response.headers)


def fetch_zip_member(url, suffix):
    """Fetch one member of a remote zip without downloading the whole archive."""
    head = urllib.request.Request(url, method='HEAD', headers=USER_AGENT)
    with urllib.request.urlopen(head, timeout=60) as response:
        length = int(response.headers['Content-Length'])
        etag = response.headers.get('ETag')
    tail, _ = http_range(url, max(0, length - 262144), length - 1)
    anchor = tail.rfind(b'PK\x05\x06')
    if anchor < 0:
        raise ValueError(f'{url}: no zip central directory in the trailing bytes')
    count, size, offset = struct.unpack('<HII', tail[anchor + 10:anchor + 20])
    directory, _ = http_range(url, offset, offset + size - 1)
    member = None
    cursor = 0
    for _ in range(count):
        if directory[cursor:cursor + 4] != b'PK\x01\x02':
            raise ValueError(f'{url}: corrupt zip central directory')
        fields = struct.unpack('<IHHHHHHIIIHHHHHII', directory[cursor:cursor + 46])
        method, packed = fields[4], fields[8]
        name_len, extra_len, comment_len, local = fields[10], fields[11], fields[12], fields[16]
        name = directory[cursor + 46:cursor + 46 + name_len].decode()
        if name.endswith(suffix):
            member = (name, method, packed, local)
        cursor += 46 + name_len + extra_len + comment_len
    if member is None:
        raise ValueError(f'{url}: no {suffix} member in the archive')
    name, method, packed, local = member
    header, _ = http_range(url, local, local + 29)
    name_len, extra_len = struct.unpack('<HH', header[26:30])
    start = local + 30 + name_len + extra_len
    packed_bytes, _ = http_range(url, start, start + packed - 1)
    if method == 8:
        raw = zlib.decompress(packed_bytes, -15)
    elif method == 0:
        raw = packed_bytes
    else:
        raise ValueError(f'{url}: unsupported zip method {method} for {name}')
    identity = dict(url=url, file_bytes=length, etag=etag, member=name,
                    member_bytes=len(raw), member_sha256=hashlib.sha256(raw).hexdigest())
    return identity, raw


def process_item(root, provider, item, delay, state, done):
    config = PROVIDERS[provider]
    out_name = Path(item['name']).stem + '-5m.tif'
    out_path = Path(root) / provider / out_name
    if out_name in done and out_path.exists():
        return None
    with state['lock']:
        wait = delay - (time.monotonic() - state['last'])
        if wait > 0:
            time.sleep(wait)
        state['last'] = time.monotonic()
    raw_path = Path(root) / provider / item['name']
    method = 'Area-averaged 5 m grid from the verified DGM1 tile'
    if config['kind'] == 'atom':
        identity, raw = fetch_zip_member(item['url'], '.tif')
        with tempfile.NamedTemporaryFile(suffix='.tif', delete=False) as handle:
            handle.write(raw)
            tmp = handle.name
        stats = reduce_geotiff(tmp, out_path, -9999.0)
        Path(tmp).unlink()
        assert_crs(out_path, config['epsg'], VERTICAL_EPSG)
        entry = dict(url=item['url'], raw_bytes=identity['file_bytes'],
                     raw_sha256=identity['member_sha256'], etag=identity['etag'],
                     member=identity['member'], derived=out_name,
                     derived_sha256=digest(out_path), derived_bytes=out_path.stat().st_size,
                     epoch=item['epoch'],
                     method='Range-read DGM1 member, ' + method)
        publish_source_provenance(root, provider, out_name, item, config, entry['raw_sha256'],
                                  entry['raw_bytes'], method)
    else:
        fetch(root, provider, item['name'], item['url'], licence=config['licence'],
              licence_url=config['licence_url'], terms_checked_utc='2026-09-25',
              expected_sha256=item.get('expected_sha256'))
        if config['kind'] == 'arcgis':
            member_name, member_bytes = read_zip_member(raw_path, '.tif')
            with tempfile.NamedTemporaryFile(suffix='.tif', delete=False) as handle:
                handle.write(member_bytes)
                tmp = handle.name
            stats = reduce_geotiff(tmp, out_path, -9999.0)
            Path(tmp).unlink()
            method = f'Zip member {member_name}, ' + method
        elif config['kind'] == 'dla':
            normalize_grid(raw_path, out_path, assign_epsg=config['epsg'])
            method = 'Normalised official DGM5 tile (natively 5 m, land values untouched)'
            stats = {}
        else:
            stats = reduce_geotiff(raw_path, out_path, -9999.0)
        assert_crs(out_path, config['epsg'], VERTICAL_EPSG)
        derive_provenance(raw_path, out_path, method)
        entry = manifest_entry(item['url'], raw_path, out_path, method)
        entry['epoch'] = item['epoch']
        if stats.get('valid_fraction') == 0:
            entry['method'] = 'fully empty tile, nothing retained'
            entry['derived'], entry['derived_sha256'], entry['derived_bytes'] = None, None, 0
            out_path.unlink()
            Path(str(out_path) + '.provenance.json').unlink()
        raw_path.unlink()
        Path(str(raw_path) + '.provenance.json').unlink()
    append_journal(state['journal'], entry, state['lock'])
    return entry


def publish_source_provenance(root, provider, out_name, item, config, parent_sha256,
                              parent_bytes, method):
    """Write the derived provenance for range-read tiles whose raw zip never lands."""
    record = dict(url=item['url'], fetched_utc=datetime.now(timezone.utc).isoformat(),
                  sha256=digest(Path(root) / provider / out_name),
                  bytes=(Path(root) / provider / out_name).stat().st_size,
                  licence=config['licence'], licence_url=config['licence_url'],
                  terms_checked_utc='2026-09-25', parent_sha256=parent_sha256,
                  parent_bytes=parent_bytes, notes=method)
    publish_json(str(Path(root) / provider / out_name) + '.provenance.json', record)


def fetch_all(root, provider, workers=None, delay=None, limit=None, only=None):
    config = PROVIDERS[provider]
    journal = Path(root) / provider / 'stream-manifest.journal.jsonl'
    journal.parent.mkdir(parents=True, exist_ok=True)
    entries, done = load_journal(journal)
    items = LISTERS[config['kind']](config['catalogue'])
    if only:
        items = [item for item in items if item['name'] in only]
    items = items[:limit]
    print(f'{provider}: {len(items)} tiles indexed')
    state = {'lock': threading.Lock(), 'last': 0.0, 'journal': journal}
    with ThreadPoolExecutor(max_workers=workers or config['workers']) as pool:
        futures = [pool.submit(process_item, root, provider, item, delay or config['delay'],
                               state, done) for item in items]
        for future in futures:
            result = future.result()
            if result is not None:
                entries.append(result)
    manifest = Path(root) / provider / 'stream-manifest.json'
    try:
        write_manifest(manifest, entries)
    except ValueError:
        manifest.unlink()
        write_manifest(manifest, entries)
    value = publish_country_sources(root, provider, entries, config['epsg'],
                                    VERTICAL_EPSG, config['group'], 'unknown ALS epoch')
    with source_budget(Path(root)) as available:
        print(f'{provider}: {len(value)} tiles retained, {available / 1e9:.1f} GB budget left')
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root')
    parser.add_argument('provider', choices=sorted(PROVIDERS))
    parser.add_argument('--workers', type=int, default=None)
    parser.add_argument('--delay', type=float, default=None)
    parser.add_argument('--limit', type=int, default=None)
    parser.add_argument('--only', nargs='*', default=None)
    args = parser.parse_args()
    fetch_all(args.root, args.provider, workers=args.workers, delay=args.delay,
              limit=args.limit, only=args.only)


if __name__ == '__main__':
    main()

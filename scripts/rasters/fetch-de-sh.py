"""Fetch Schleswig-Holstein DGM1: per-tile XYZ reduced to 5 m in-flight.

The mass-download index is one official GeoJSON; each tile is a complete 1 m
XYZ lattice followed by a portal HTML footer. Tiles are verified, the footer
is stripped, the lattice is area-averaged to a retained 5 m grid, and the raw
XYZ bytes are deleted; a checksum manifest keeps the audit trail.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import threading
import time
import urllib.request

from dgm_reduce import (append_journal, assert_crs, derive_provenance, digest, load_journal,
                        manifest_entry, reduce_xyz, write_manifest)
from terrain_io import fetch, publish_json, publish_source_json, source_budget

PROVIDER = 'de-sh-dgm1'
INDEX_URL = ('https://geodaten.schleswig-holstein.de/gaialight-sh/_apps/dladownload/'
             'single.php?file=DGM1_SH__Massendownload.geojson&id=4')
INDEX_NAME = 'DGM1_SH__Massendownload.geojson'
LICENCE = 'Creative Commons Attribution 4.0 International'
LICENCE_URL = 'https://creativecommons.org/licenses/by/4.0/'
GROUP = 'DE-SH-DGM1'
EPSG = 25832
VERTICAL_EPSG = 7837
FOOTER_MARKER = b'<!DOCTYPE html>'
USER_AGENT = {'User-Agent': 'quietmap-dem-de/1.0 (+https://quietmap.example.invalid)'}


def parse_index(data):
    """Keep every indexed tile with its per-tile acquisition date."""
    document = json.loads(data)
    items = []
    for feature in document['features']:
        props = feature['properties']
        items.append(dict(name=props['link_data'].split('file=')[1].split('&')[0],
                          url=props['link_data'], kachel=props['kachel'],
                          epoch=f"ALS {props['datum']}"))
    if len({i['name'] for i in items}) != len(items):
        raise ValueError('tile names in the SH index are not unique')
    return items


def strip_footer(raw_path):
    """Cut the portal HTML footer; a response without it is a format failure."""
    blob = Path(raw_path).read_bytes()
    parts = blob.split(FOOTER_MARKER)
    if len(parts) != 2 or not parts[1].rstrip().endswith(b'</html>'):
        raise ValueError(f'{raw_path}: missing the portal footer; refusing a partial download')
    data = b'\n'.join(line for line in parts[0].split(b'\n') if line.strip()) + b'\n'
    if len(data) <= 1:
        return None
    cleaned = Path(str(raw_path) + '.clean')
    cleaned.write_bytes(data)
    return cleaned


def process_item(root, item, delay, state, done):
    out_name = Path(item['name']).stem + '-5m.tif'
    out_path = Path(root) / PROVIDER / out_name
    if out_name in done and out_path.exists():
        return None
    with state['lock']:
        wait = delay - (time.monotonic() - state['last'])
        if wait > 0:
            time.sleep(wait)
        state['last'] = time.monotonic()
    try:
        fetch(root, PROVIDER, item['name'], item['url'], licence=LICENCE,
              licence_url=LICENCE_URL, terms_checked_utc='2026-09-25')
        raw_path = Path(root) / PROVIDER / item['name']
        cleaned = strip_footer(raw_path)
        if cleaned is None:
            entry = dict(url=item['url'], raw_bytes=raw_path.stat().st_size,
                         raw_sha256=digest(raw_path), derived=None, derived_sha256=None,
                         derived_bytes=0, epoch=item['epoch'],
                         method='empty SH tile (footer only), nothing retained')
            raw_path.unlink()
            Path(str(raw_path) + '.provenance.json').unlink()
            append_journal(state['journal'], entry, state['lock'])
            return entry
        method = 'Area-averaged 5 m grid from the verified DGM1 XYZ lattice'
        stats = reduce_xyz(cleaned, out_path, 1, EPSG)
        cleaned.unlink()
        assert_crs(out_path, EPSG)
        derive_provenance(raw_path, out_path, method)
        entry = manifest_entry(item['url'], raw_path, out_path, method)
        entry['epoch'] = item['epoch']
        if stats['valid_fraction'] == 0:
            entry['method'] = 'fully void tile, nothing retained'
            entry['derived'], entry['derived_sha256'], entry['derived_bytes'] = None, None, 0
            out_path.unlink()
            Path(str(out_path) + '.provenance.json').unlink()
        raw_path.unlink()
        Path(str(raw_path) + '.provenance.json').unlink()
        append_journal(state['journal'], entry, state['lock'])
        return entry
    except Exception as error:
        failure = dict(url=item['url'], name=item['name'], error=f'{type(error).__name__}: {error}')
        append_journal(state['failed'], failure, state['lock'])
        return failure


def fetch_all(root, workers=4, delay=0.5, limit=None, only=None):
    journal = Path(root) / PROVIDER / 'stream-manifest.journal.jsonl'
    failed_path = Path(root) / PROVIDER / 'failed.journal.jsonl'
    journal.parent.mkdir(parents=True, exist_ok=True)
    entries, done = load_journal(journal)
    fetch(root, PROVIDER, INDEX_NAME, INDEX_URL, licence=LICENCE,
          licence_url=LICENCE_URL, terms_checked_utc='2026-09-25')
    items = parse_index((Path(root) / PROVIDER / INDEX_NAME).read_bytes())
    if only:
        items = [item for item in items if item['name'] in only]
    items = items[:limit]
    print(f'{PROVIDER}: {len(items)} tiles indexed')
    state = {'lock': threading.Lock(), 'last': 0.0, 'journal': journal, 'failed': failed_path}
    with ThreadPoolExecutor(max_workers=workers) as pool:
        results = list(pool.map(lambda item: process_item(root, item, delay, state, done), items))
    failures = [r for r in results if r is not None and 'error' in r]
    for result in results:
        if result is not None and 'error' not in result:
            entries.append(result)
    manifest = Path(root) / PROVIDER / 'stream-manifest.json'
    try:
        write_manifest(manifest, entries)
    except ValueError:
        manifest.unlink()
        write_manifest(manifest, entries)
    kept = sorted((e for e in entries if e.get('derived')), key=lambda e: e['derived'])
    sources = Path(root) / PROVIDER / 'country-sources.json'
    value = [dict(path=e['derived'], horizontal_crs=EPSG, vertical_crs=VERTICAL_EPSG,
                  epoch=e.get('epoch', ''), role='national', group=GROUP)
             for e in kept]
    try:
        publish_source_json(root, sources, value)
    except ValueError:
        sources.unlink()
        publish_source_json(root, sources, value)
    with source_budget(Path(root)) as available:
        print(f'{PROVIDER}: {len(kept)} tiles retained, {len(failures)} failed, '
              f'{available / 1e9:.1f} GB budget left')
    if failures:
        raise ValueError(f'{len(failures)} SH tiles failed; see {failed_path}')
    return entries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root')
    parser.add_argument('--workers', type=int, default=4)
    parser.add_argument('--delay', type=float, default=0.5)
    parser.add_argument('--limit', type=int, default=None)
    parser.add_argument('--only', nargs='*', default=None)
    args = parser.parse_args()
    fetch_all(args.root, workers=args.workers, delay=args.delay,
              limit=args.limit, only=args.only)


if __name__ == '__main__':
    main()

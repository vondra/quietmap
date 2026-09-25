"""Fetch single-file DGM for Sachsen-Anhalt, Hamburg, Bremen and Saarland.

These providers publish one archive per state or district instead of per-tile
catalogues. The archives stay retained next to their decoded grids; per-tile
epochs come from each archive's own metadata (ST .meta, HH tile CSV, SL
flight overview, HB file epochs).
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import urllib.parse
import zipfile

from osgeo import gdal
from dgm_reduce import (assert_crs, derive_provenance, normalize_grid, publish_country_sources,
                        reduce_geotiff, reduce_xyz)
from terrain_io import fetch, source_budget

gdal.UseExceptions()
gdal.SetConfigOption('GDAL_PAM_ENABLED', 'NO')

PROVIDERS = {
    'de-hb-dgm5': dict(
        files={'Gitternetz_DGM5_2017_HB_ASCII_XYZ.zip': 'ALS 2017',
               'Gitternetz_DGM5_2015_BHV_ASCII_XYZ.zip': 'ALS 2015'},
        base='https://gdi2.geo.bremen.de/inspire/download/DGM/data',
        licence='Creative Commons Attribution 4.0 International',
        licence_url='https://creativecommons.org/licenses/by/4.0/',
        group='DE-HB-DGM5', epsg=25832, spacing=5),
    'de-st-dgm5': dict(
        files={'DGM5.zip': 'official DGM5, statewide'},
        base=('https://www.geodatenportal.sachsen-anhalt.de/gfds_webshare/download/LVermGeo/'
              'Geodatenportal/Online-Bereitstellung-LVermGeo/DGM'),
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0',
        group='DE-ST-DGM5', epsg=25832, spacing=5),
    'de-hh-dgm1': dict(
        files={'dgm1_hh_2022-04-30.zip': 'ALS 2022, statewide'},
        base='https://daten-hamburg.de/opendata/fernerkundung_hoehenmodelle/dgm',
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0',
        group='DE-HH-DGM1', epsg=25832, spacing=1),
    'de-sl-dgm1': dict(
        files={f'DGM1_tif_{lk}_EPSG-25832_Entstehung-2025.zip': f'ALS 2025, district {lk}'
               for lk in ('MZG', 'NK', 'SB', 'SLS', 'SPK', 'WND')},
        base=('https://www.shop.lvgl.saarland.de/cloud/public.php/dav/files/'
              'NK8ndP55qAqGEZD/OD_DGM1_2025_tif_LK'),
        licence='Datenlizenz Deutschland - Namensnennung - Version 2.0',
        licence_url='https://www.govdata.de/dl-de/by-2-0',
        group='DE-SL-DGM1', epsg=25832, spacing=1),
}

SL_FLIGHT_URL = ('https://www.shop.lvgl.saarland.de/cloud/public.php/dav/files/'
                 'NK8ndP55qAqGEZD/OD_DGM1_2025_tif_LK/' +
                 urllib.parse.quote('Kachelübersicht-Airborne Laserscanning 2025.pdf'))
SL_FLIGHT_NAME = 'Kachelübersicht-Airborne Laserscanning 2025.pdf'
SL_EPOCH = 'ALS 2024-12-28 to 2025-04-19 (8 pts/m², provider flight overview)'


def fetch_only(root, provider):
    config = PROVIDERS[provider]
    for name in config['files']:
        fetch(root, provider, name, f"{config['base']}/{name}", licence=config['licence'],
              licence_url=config['licence_url'], terms_checked_utc='2026-09-25')
    if provider == 'de-sl-dgm1':
        fetch(root, provider, SL_FLIGHT_NAME, SL_FLIGHT_URL, licence=config['licence'],
              licence_url=config['licence_url'], terms_checked_utc='2026-09-25')
    with source_budget(Path(root)) as available:
        print(f'{provider}: fetched, {available / 1e9:.1f} GB budget left')


def publish_listing(root, provider, names):
    """country-sources.json lists derived grids; the retained zips are not sources."""
    config = PROVIDERS[provider]
    entries = [dict(derived=name, epoch=epoch) for name, epoch in names]
    publish_country_sources(root, provider, entries, config['epsg'], 7837,
                            config['group'], 'unknown ALS epoch')


def st_epoch(meta_text):
    """Aktualitaet line of an ST .meta sidecar (latin-1 with umlauts)."""
    for line in meta_text.splitlines():
        if line.startswith('Aktualitaet:'):
            return f"ALS {line.split(':', 1)[1].strip()}"
    raise ValueError('ST .meta sidecar has no Aktualitaet line')


def hh_epochs(csv_text):
    """Map tile name to acquisition month from the HH tile table."""
    rows = [line.split(';') for line in csv_text.splitlines() if line.strip()]
    header = next(i for i, row in enumerate(rows) if row[0] == 'Kachelname')
    return {row[0]: f"ALS {row[1]}" for row in rows[header + 1:] if len(row) > 1 and row[1]}


def band_nodata(path):
    dataset = gdal.Open(str(path))
    nodata = dataset.GetRasterBand(1).GetNoDataValue()
    dataset = None
    if nodata is None:
        raise ValueError(f'{path}: no source nodata declared')
    return nodata


def decode_geotiff_archive(root, provider, members_of, epoch_of, method, workers=4):
    """Decode every tif member of retained zips; 1 m members average to 5 m."""
    config = PROVIDERS[provider]
    names = []
    for name in config['files']:
        raw = Path(root) / provider / name
        with zipfile.ZipFile(raw) as archive:
            members = sorted(members_of(archive))
            if not members:
                raise ValueError(f'{name}: no tif members in the archive')
            epochs = epoch_of(archive)

        def decode_one(member_name):
            out_name = Path(member_name).stem + '-5m.tif'
            out_path = Path(root) / provider / out_name
            if not out_path.exists():
                with zipfile.ZipFile(raw) as archive:
                    payload = archive.read(member_name)
                with tempfile.NamedTemporaryFile(suffix='.tif', delete=False) as handle:
                    handle.write(payload)
                    tmp = handle.name
                if config['spacing'] == 5:
                    normalize_grid(tmp, out_path, assign_epsg=config['epsg'])
                else:
                    reduce_geotiff(tmp, out_path, band_nodata(tmp))
                Path(tmp).unlink()
                assert_crs(out_path, config['epsg'])
                derive_provenance(raw, out_path, f'{method}: {member_name}')
            return out_name, epochs(member_name)

        with ThreadPoolExecutor(max_workers=workers) as pool:
            names.extend(pool.map(decode_one, members))
    names.sort()
    publish_listing(root, provider, names)
    return names


def decode_hb(root, provider):
    config = PROVIDERS[provider]
    names = []
    for name in config['files']:
        raw = Path(root) / provider / name
        with zipfile.ZipFile(raw) as archive:
            members = sorted(n for n in archive.namelist() if n.endswith('.xyz'))
            if not members:
                raise ValueError(f'{name}: no xyz members in the archive')
            for member_name in members:
                out_name = Path(member_name).stem + '-5m.tif'
                out_path = Path(root) / provider / out_name
                if not out_path.exists():
                    with archive.open(member_name) as member:
                        xyz = Path(root) / provider / (Path(member_name).name + '.tmp')
                        xyz.write_bytes(member.read())
                    reduce_xyz(xyz, out_path, config['spacing'], config['epsg'])
                    xyz.unlink()
                    assert_crs(out_path, config['epsg'])
                    derive_provenance(raw, out_path,
                                      f'Decoded {member_name} from the official DGM5 archive')
                names.append((out_name, config['files'][name]))
    names.sort()
    publish_listing(root, provider, names)
    return names


def decode_st(root, provider, workers=4):
    def members_of(archive):
        return [n for n in archive.namelist()
                if n.endswith('.tif') and not Path(n).name.startswith('Thumbs')]

    def epoch_of(archive):
        metas = {n: archive.read(n).decode('latin-1') for n in archive.namelist()
                 if n.endswith('.meta')}
        return lambda member: st_epoch(metas[member.rsplit('.', 1)[0] + '.meta'])

    return decode_geotiff_archive(
        root, provider, members_of, epoch_of,
        'Normalised official DGM5 tile (natively 5 m, land values untouched)',
        workers=workers)


def decode_hh(root, provider, workers=4):
    def members_of(archive):
        return [n for n in archive.namelist() if n.endswith('.tif')]

    def epoch_of(archive):
        tables = [n for n in archive.namelist() if n.endswith('.csv')]
        if len(tables) != 1:
            raise ValueError(f'expected one HH tile table, found {len(tables)}')
        epochs = hh_epochs(archive.read(tables[0]).decode('latin-1'))
        return lambda member: epochs[Path(member).stem]

    return decode_geotiff_archive(
        root, provider, members_of, epoch_of,
        'Area-averaged 5 m grid from the verified DGM1 tile',
        workers=workers)


def decode_sl(root, provider, workers=4):
    def members_of(archive):
        return [n for n in archive.namelist() if n.endswith('.tif')]

    def epoch_of(archive):
        return lambda member: SL_EPOCH

    return decode_geotiff_archive(
        root, provider, members_of, epoch_of,
        'Area-averaged 5 m grid from the verified DGM1 tile',
        workers=workers)


DECODERS = {'de-hb-dgm5': decode_hb, 'de-st-dgm5': decode_st,
            'de-hh-dgm1': decode_hh, 'de-sl-dgm1': decode_sl}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root')
    parser.add_argument('provider', choices=sorted(PROVIDERS))
    parser.add_argument('--fetch-only', action='store_true')
    parser.add_argument('--workers', type=int, default=4)
    args = parser.parse_args()
    fetch_only(args.root, args.provider)
    if not args.fetch_only:
        if args.provider == 'de-hb-dgm5':
            DECODERS[args.provider](args.root, args.provider)
        else:
            DECODERS[args.provider](args.root, args.provider, workers=args.workers)


if __name__ == '__main__':
    main()

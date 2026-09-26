#!/usr/bin/env python3
"""Fetch the Austrian geoland DGM 10 m country raster with a recorded GHA datum."""
import argparse
import json
from pathlib import Path
import shutil
import tempfile
import zipfile

from dem_windows import nodata_tag
from terrain_io import fetch, provenance, publish_json, publish_path, publish_source_json, digest

PROVIDER = 'at-dgm10'
URL = 'https://gis.ktn.gv.at/OGD/Geographie_Planung/ogd-10m-at.zip'
LICENCE = 'CC BY 4.0'
LICENCE_URL = 'https://www.data.gv.at/katalog/dataset/dgm'
ARCHIVE = 'ogd-10m-at.zip'
MEMBER = 'dhm_at_lamb_10m_2018.tif'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    record = fetch(args.output, PROVIDER, ARCHIVE, URL, LICENCE, LICENCE_URL, '2026-09-25',
                   'geoland DGM Oesterreich 10 m (AustriaDEM composite to 2018), MGI Austria Lambert '
                   '(EPSG:31287), Gebrauchshoehen Adria (EPSG:5778); raw bytes retained.')
    archive = args.output / PROVIDER / ARCHIVE
    target = args.output / PROVIDER / MEMBER
    if target.exists():
        derived = provenance(target)
        if derived.get('parent_sha256') != record['sha256']:
            raise ValueError('derived raster references another source archive')
    else:
        with zipfile.ZipFile(archive) as container:
            if set(container.NameToInfo) != {MEMBER, 'dhm_at_lamb_10m_2018.tfw'}:
                raise ValueError('official archive contents changed')
            with tempfile.TemporaryDirectory(dir=target.parent) as staged:
                with container.open(MEMBER) as member, open(Path(staged) / MEMBER, 'wb') as out:
                    shutil.copyfileobj(member, out, 1 << 20)
                publish_path(target, Path(staged) / MEMBER)
        publish_json(str(target) + '.provenance.json', dict(
            record, sha256=digest(target), bytes=target.stat().st_size,
            parent_sha256=record['sha256'],
            notes=('Lossless archive member extraction; 58061x31793 at 10 m, Float32, '
                   'EPSG:31287 + 5778; no resampling.')))
    nodata = nodata_tag(target)
    publish_source_json(args.output, args.output / PROVIDER / 'country-sources.json', [dict(
        path=str(target.resolve()), horizontal_crs='EPSG:31287', vertical_crs=5778,
        epoch='ALS composite to 2018', role='national', group='AT-DGM10',
        nodata=nodata, datum_area_of_interest=[9.3, 46.2, 17.2, 49.2])])
    print(json.dumps({'path': str(target)}))


if __name__ == '__main__':
    main()

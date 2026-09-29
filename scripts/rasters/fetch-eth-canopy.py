#!/usr/bin/env python3
"""Fetch selected ETH GCH 2020 three-degree canopy tiles; 255 remains unavailable."""
import argparse
from urllib.parse import urlencode
from terrain_io import fetch

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output')
    parser.add_argument('latitude', type=int)
    parser.add_argument('longitude', type=int)
    args = parser.parse_args()
    lat, lon = args.latitude, args.longitude
    if lat % 3 or lon % 3 or not (-90 <= lat < 90 and -180 <= lon < 180):
        parser.error('expected the southwest corner of a three-degree tile')
    tile = f'{"N" if lat >= 0 else "S"}{abs(lat):02}{"E" if lon >= 0 else "W"}{abs(lon):03}'
    name = f'ETH_GlobalCanopyHeight_10m_2020_{tile}_Map.tif'
    url = 'https://libdrive.ethz.ch/index.php/s/cO8or7iOe5dT2Rt/download?' + urlencode({'path':'/3deg_cogs','files':name})
    fetch(args.output, 'eth-gch-2020', name, url, 'CC BY 4.0',
          'https://creativecommons.org/licenses/by/4.0/', '2026-09-24',
          'Lang et al. 2023, doi:10.3929/ethz-b-000609802; 2020 canopy top in metres; 255 nodata; foliage only.')

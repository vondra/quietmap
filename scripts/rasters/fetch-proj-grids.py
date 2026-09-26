#!/usr/bin/env python3
"""Fetch the PROJ datum grids that convert national heights to EGM2008."""
import argparse
from pathlib import Path

from terrain_io import fetch

CDN = 'https://cdn.proj.org'
# (file, operation, origin product). All are open-licence per PROJ-data policy.
GRIDS = [
    ('us_nga_egm08_25.tif', 'WGS 84 to EGM2008 height (EPSG:3858)', 'US NGA EGM2008 2.5x2.5 undulation grid'),
    ('at_bev_GV_Hoehengrid_plus_Geoid_V2.tif', 'ETRS89 to GHA height (EPSG:9278)', 'Austria BEV GV height grid + geoid'),
    ('ch_swisstopo_chgeo2004_ETRS89_LN02.tif', 'ETRS89 to LN02 height', 'swisstopo CHGeo2004 ETRS89-LN02'),
    ('nl_nsgi_nlgeo2018.tif', 'ETRS89 to NAP height (EPSG:9283)', 'NSGI NLGEO2018 quasi-geoid'),
    ('be_ign_hBG18.tif', 'ETRS89 to Ostend height (EPSG:9908)', 'IGN Belgium hBG18'),
    ('fr_ign_RAF18b.tif', 'RGF93 v2b to NGF-IGN69 height (EPSG:9786)', 'IGN France RAF18b'),
    ('fr_ign_RAF18.tif', 'RGF93 v2 to NGF-IGN69 height (EPSG:8885)', 'IGN France RAF18'),
    ('fr_ign_RAC23.tif', 'RGF93 v2b to NGF-IGN78 Corsica height (EPSG:10506)', 'IGN France RAC23'),
    ('at_bev_GV_Hoehengrid_V1.tif', 'ETRS89 to GHA height chain (PROJ 9.4 resolution)', 'Austria BEV GV height grid V1'),
    ('at_bev_GEOID_GRS80_Oesterreich.tif', 'ETRS89 to GHA height chain (PROJ 9.4 resolution)', 'Austria BEV GRS80 geoid Austria'),
    ('dk_sdfi_dvr90_2013.tif', 'ETRS89 to DVR90(2013) height (EPSG:10491)', 'SDFI DVR90 2013 realization'),
    ('se_lantmateriet_SWEN17_RH2000.tif', 'SWEREF99 to RH2000 height', 'Lantmateriet SWEN17_RH2000'),
    ('pt_dgt_GeodPT08.tif', 'ETRS89 to Cascais height (EPSG:10544)', 'DGT GeodPT08'),
]
LICENCE = 'permissive open licence (PROJ-data package: public domain/MIT/BSD/CC0/CC-BY/CC-BY-SA)'
LICENCE_URL = 'https://github.com/OSGeo/PROJ-data'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    for name, operation, origin in GRIDS:
        fetch(args.output, 'proj-grids', name, CDN + '/' + name, LICENCE, LICENCE_URL,
              '2026-09-25', f'{origin}; PROJ operation {operation}; enables strict no-ballpark national-to-EGM2008 conversion.')


if __name__ == '__main__':
    main()

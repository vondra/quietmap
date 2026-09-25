#!/usr/bin/env python3
"""Fetch the fixed GEDTM30 v1.2 world COG; its raw samples are metres despite the scale tag."""
import argparse
from terrain_io import fetch

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output')
    args = parser.parse_args()
    name = 'gedtm_rf_m_30m_s_20060101_20151231_go_epsg.4326.3855_v1.2.tif'
    fetch(args.output, 'gedtm30-v1.2', name, 'https://s3.opengeohub.org/global/dtm/v1.2/' + name,
          'CC BY 4.0', 'https://creativecommons.org/licenses/by/4.0/', '2026-09-24',
          'GEDTM30 v1.2 (doi:10.5281/zenodo.18887460); raw metres, EGM2008. Ignore erroneous Scale=0.1.')

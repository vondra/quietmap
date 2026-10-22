"""The water of ESA WorldCover 2021 v200 (10 m; Zanaga et al. 2022, CC BY 4.0) for the ship converter:
each 3 arc-second cell's share of permanent water (class 80, the sea along the coasts with it), one
file a 3 x 3 degree tile named as WorldCover names it by its south-west corner (`N39W003.water`):
3,600 x 3,600 bytes of percent from the north-west corner, row by row, zstd-compressed
(`qm-build dev4 --kinds sources --water OUT_DIR`), beside WorldCover's grid of its tiles: it has no
tile where it has no land, the open sea, all water. A tile at once takes about 3 GB of memory (its
classes and their mask). usage: water.py OUT_DIR [TILES_AT_ONCE]"""

from concurrent.futures import ProcessPoolExecutor
import json
import os
from pathlib import Path
import subprocess
import sys

import numpy as np
from osgeo import gdal

gdal.UseExceptions()

BUCKET = "https://esa-worldcover.s3.eu-central-1.amazonaws.com"
URL = BUCKET + "/v200/2021/map/ESA_WorldCover_10m_2021_v200_{tile}_Map.tif"
GRID = "esa_worldcover_grid.geojson"
WATER = 80
# A tile's side in WorldCover pixels (1/12,000 degree) and in our cells (1/1,200 degree).
PIXELS = 36_000
CELLS = 3_600


def water_percent(classes):
    """Each 10 x 10 block's share of water pixels, in percent (exact: a hundred pixels a cell)."""
    water = (classes == WATER).reshape(CELLS, PIXELS // CELLS, CELLS, PIXELS // CELLS)
    return water.sum(axis=(1, 3), dtype=np.uint8)


def fetch(url, path):
    subprocess.run(["curl", "-sS", "-f", "--retry", "3", "-o", str(path), url], check=True)


def convert(tile, out):
    target = Path(out) / f"{tile}.water"
    if target.exists():
        return tile, "kept"
    download = Path(out) / f".{tile}.tif"
    fetch(URL.format(tile=tile), download)
    dataset = gdal.Open(str(download))
    if (dataset.RasterXSize, dataset.RasterYSize) != (PIXELS, PIXELS):
        raise ValueError(f"{tile}: {dataset.RasterXSize} x {dataset.RasterYSize} pixels")
    percent = water_percent(dataset.GetRasterBand(1).ReadAsArray())
    dataset = None
    download.unlink()
    raw = Path(out) / f".{tile}.raw"
    percent.tofile(raw)
    subprocess.run(["zstd", "-q", "-10", "--rm", "-f", str(raw), "-o", str(target) + ".partial"], check=True)
    os.replace(str(target) + ".partial", target)
    return tile, f"{int(percent.mean())} % water"


def main(out, at_once="6"):
    grid = Path(out) / GRID
    if not grid.exists():
        Path(out).mkdir(parents=True, exist_ok=True)
        fetch(f"{BUCKET}/{GRID}", grid)
    tiles = sorted(feature["properties"]["ll_tile"] for feature in json.load(open(grid))["features"])
    with ProcessPoolExecutor(int(at_once)) as pool:
        for number, (tile, result) in enumerate(pool.map(convert, tiles, [out] * len(tiles)), 1):
            print(f"{number}/{len(tiles)} {tile} {result}", flush=True)


if __name__ == "__main__":
    main(*sys.argv[1:])

"""Official Copernicus coverage, source identity, and native interpolation dependencies."""

from concurrent.futures import ThreadPoolExecutor
import hashlib
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
import urllib.request

Tile = tuple[int, int]
CATALOG_FILE = "catalog.sqlite"


def tile_name(tile: Tile) -> str:
    latitude, longitude = tile
    return f'{"N" if latitude >= 0 else "S"}{abs(latitude):02}{"E" if longitude >= 0 else "W"}{abs(longitude):03}'


def source_name(resolution: int, tile: Tile) -> str:
    name = tile_name(tile)
    code = {30: 10, 90: 30}[resolution]
    return f"Copernicus_DSM_COG_{code}_{name[:3]}_00_{name[3:]}_00_DEM"


def source_path(root: Path, resolution: int, tile: Tile) -> Path:
    name = source_name(resolution, tile)
    return root / f"glo{resolution}" / name / f"{name}.tif"


def parse_inventory(text: str, resolution: int) -> set[Tile]:
    tiles = set()
    code = {30: 10, 90: 30}[resolution]
    for name in text.splitlines():
        match = re.fullmatch(rf"Copernicus_DSM_COG_{code}_([NS])(\d{{2}})_00_([EW])(\d{{3}})_00_DEM", name)
        if match is None:
            raise ValueError(f"Invalid GLO{resolution} inventory entry: {name}")
        ns, latitude, ew, longitude = match.groups()
        tile = (int(latitude) * (-1 if ns == "S" else 1),
                int(longitude) * (-1 if ew == "W" else 1))
        if not (-90 <= tile[0] < 90 and -180 <= tile[1] < 180) or tile in tiles:
            raise ValueError(f"Duplicate or invalid Copernicus tile: {name}")
        tiles.add(tile)
    if not tiles:
        raise ValueError(f"Empty GLO{resolution} inventory")
    return tiles


def read_catalog(root: Path) -> dict[int, set[Tile]]:
    with sqlite3.connect(f"{(root / CATALOG_FILE).resolve().as_uri()}?mode=ro", uri=True) as database:
        inventories = dict(database.execute("SELECT resolution, inventory FROM catalog"))
    if set(inventories) != {30, 90}:
        raise ValueError("DEM catalog requires both official GLO30 and GLO90 inventories")
    catalog = {resolution: parse_inventory(text, resolution) for resolution, text in inventories.items()}
    if not catalog[30] <= catalog[90]:
        raise ValueError("GLO30 inventory is not a subset of worldwide GLO90")
    return catalog


def adjacent_tiles(tiles: set[Tile], north: bool) -> set[Tile]:
    # COGs omit east/south shared nodes. Sampling needs E/S support; publishing
    # a new source changes the W/N neighbors that carry those shared nodes.
    dy, dx = (1, -1) if north else (-1, 1)
    return {(latitude + y, (longitude + x + 180) % 360 - 180)
            for latitude, longitude in tiles for y in (0, dy) for x in (0, dx)
            if -90 <= latitude + y < 90}


def source_gap(catalog: dict[int, set[Tile]]) -> set[Tile]:
    return catalog[90] - catalog[30]


def required_supplement(catalog: dict[int, set[Tile]]) -> set[Tile]:
    return adjacent_tiles(source_gap(catalog), north=False) & catalog[90]


def changed_outputs(catalog: dict[int, set[Tile]]) -> set[Tile]:
    return adjacent_tiles(source_gap(catalog), north=True) & catalog[90]


def fetch_catalog(root: Path) -> None:
    inventories = {}
    for resolution in (30, 90):
        url = f"https://copernicus-dem-{resolution}m.s3.amazonaws.com/tileList.txt"
        raw = urllib.request.urlopen(url, timeout=60).read()
        text = raw.decode("ascii")
        count = len(parse_inventory(text, resolution))
        inventories[resolution] = text
        print(f"[cop-dem] GLO{resolution}: {count} catalog tiles, sha256={hashlib.sha256(raw).hexdigest()}", flush=True)
    if not parse_inventory(inventories[30], 30) <= parse_inventory(inventories[90], 90):
        raise ValueError("Official GLO30 inventory exceeds GLO90 coverage")
    root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".dem-catalog-", dir=root) as directory:
        staged = Path(directory) / CATALOG_FILE
        with sqlite3.connect(staged) as database:
            database.execute("CREATE TABLE catalog (resolution INTEGER PRIMARY KEY CHECK(resolution IN (30,90)), inventory TEXT NOT NULL)")
            database.executemany("INSERT INTO catalog VALUES (?, ?)", inventories.items())
        os.replace(staged, root / CATALOG_FILE)


def download_supplement(root: Path, jobs: int) -> None:
    catalog = read_catalog(root)

    def download(tile: Tile) -> None:
        name = source_name(90, tile)
        url = f"https://copernicus-dem-90m.s3.amazonaws.com/{name}/{name}.tif"
        path = source_path(root, 90, tile)
        request = urllib.request.Request(url, method="HEAD")
        with urllib.request.urlopen(request, timeout=60) as response:
            size = int(response.headers["Content-Length"])
            etag = response.headers["ETag"].strip('"')
        if re.fullmatch(r"[0-9a-f]{32}", etag) is None:
            raise ValueError(f"No single-part source checksum for {url}")
        if path.is_file() and path.stat().st_size == size and hashlib.md5(path.read_bytes()).hexdigest() == etag:
            return
        data = urllib.request.urlopen(url, timeout=60).read()
        if len(data) != size or hashlib.md5(data).hexdigest() != etag:
            raise ValueError(f"Source checksum mismatch: {url}")
        path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".download-", dir=path.parent) as directory:
            staged = Path(directory) / path.name
            staged.write_bytes(data)
            os.replace(staged, path)
        print(f"[cop-dem] GLO90 {tile_name(tile)}: {size} verified bytes", flush=True)

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        list(pool.map(download, sorted(required_supplement(catalog))))


if __name__ == "__main__":
    root = Path(sys.argv[2])
    if sys.argv[1] == "catalog":
        fetch_catalog(root)
    elif sys.argv[1] == "supplement":
        download_supplement(root, int(sys.argv[3]))
    else:
        raise ValueError("Expected catalog or supplement operation")

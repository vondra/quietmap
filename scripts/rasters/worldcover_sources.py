"""Categorical source identity, native coverage, and the shared WorldCover/CCI acoustic proxy."""

import hashlib
import os
from pathlib import Path
import re
import sqlite3
import tempfile
import urllib.request

import numpy as np
import rasterio

BUCKET = "esa-worldcover"
PREFIX = "v200/2021/map/"
CATALOG_FILE = "catalog.sqlite"


def native_tiles(key: str) -> set[tuple[int, int]]:
    match = re.fullmatch(PREFIX + r"ESA_WorldCover_10m_2021_v200_([NS])(\d{2})([EW])(\d{3})_Map.tif", key)
    if match is None:
        raise ValueError(f"Invalid official WorldCover key: {key}")
    ns, latitude, ew, longitude = match.groups()
    lat = int(latitude) * (-1 if ns == "S" else 1)
    lon = int(longitude) * (-1 if ew == "W" else 1)
    if lat % 3 or lon % 3 or not (-90 <= lat <= 87 and -180 <= lon <= 177):
        raise ValueError(f"Invalid WorldCover source footprint: {key}")
    return {(lat + dy, lon + dx) for dy in range(3) for dx in range(3)}


def validate_inventory(rows: list[tuple[str, int]]) -> dict[str, int]:
    result = {}
    for key, size in rows:
        native_tiles(key)
        if key in result or not isinstance(size, int) or size <= 0:
            raise ValueError(f"Duplicate or invalid WorldCover source: {key}")
        result[key] = size
    if not result:
        raise ValueError("Empty official WorldCover inventory")
    return result


def read_catalog(root: Path) -> dict[str, int]:
    with sqlite3.connect(f"{(root / CATALOG_FILE).resolve().as_uri()}?mode=ro", uri=True) as database:
        rows = list(database.execute("SELECT key, bytes FROM worldcover_sources ORDER BY key"))
    return validate_inventory(rows)


def fetch_catalog(root: Path, s3) -> dict[str, int]:
    rows = [(item["Key"], item["Size"])
            for page in s3.get_paginator("list_objects_v2").paginate(Bucket=BUCKET, Prefix=PREFIX)
            for item in page.get("Contents", []) if item["Key"].endswith("_Map.tif")]
    inventory = validate_inventory(rows)
    root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".worldcover-catalog-", dir=root) as staged:
        path = Path(staged) / CATALOG_FILE
        with sqlite3.connect(path) as database:
            database.execute("CREATE TABLE worldcover_sources (key TEXT PRIMARY KEY, bytes INTEGER NOT NULL CHECK(bytes > 0))")
            database.executemany("INSERT INTO worldcover_sources VALUES (?, ?)", sorted(inventory.items()))
        os.replace(path, root / CATALOG_FILE)
    return inventory


def validate_source_files(root: Path, inventory: dict[str, int]) -> None:
    actual = {path.name for path in root.glob("*_Map.tif")}
    expected = {key.rsplit("/", 1)[-1] for key in inventory}
    if actual != expected:
        raise ValueError(f"WorldCover local inventory differs: missing={sorted(expected - actual)}, extra={sorted(actual - expected)}")
    for key, size in inventory.items():
        path = root / key.rsplit("/", 1)[-1]
        if path.stat().st_size != size:
            raise ValueError(f"WorldCover size mismatch: {path}")


# Stanford NatCap's unresampled C3S 2022 class-band derivative, acquired 2026-09-05.
# https://data.naturalcapitalalliance.stanford.edu/dataset/sts-1a410f75622cb976ce1f28a7d5085741da7a52d53c66bae3be4489d6bb47de7f
CCI_FILENAME = "ESA-CCI-LULC-300m-P1Y-2022-v2.1.1.tif"
CCI_URL = "https://data.naturalcapitalalliance.stanford.edu/download/global/esa_CCI/" + CCI_FILENAME
CCI_IDENTITY = "C3S-LC-L4-LCCS-Map-300m-P1Y-2022-v2.1.1"
CCI_SHA256 = "622a005ce66894fbaeaa35f638645093a1a93d70d885288fb6467a42ded9dda9"

# Existing acoustic proxy: permanent snow/firn stays soft; water stays hard
# (ISO 9613-2 section 7.3.1). These are not measured imperviousness percentages.
WORLDCOVER_IMD = {10: 2, 20: 5, 30: 5, 40: 10, 50: 85, 60: 15,
                  70: 0, 80: 100, 90: 5, 95: 2, 100: 5}
# C3S 2021/2022 PUG v1.1 Table 1-2 and Appendix A: crop-dominant mosaics
# remain crops; unspecified mixed natural vegetation uses the shrub/grass proxy.
CCI_WORLDCOVER = {
    **dict.fromkeys((10, 11, 12, 20, 30), 40),
    **dict.fromkeys((40, 100, 110, 120, 121, 122, 130), 30),
    **dict.fromkeys((50, 60, 61, 62, 70, 71, 72, 80, 81, 82, 90, 160), 10),
    **dict.fromkeys((150, 151, 152, 153, 200, 201, 202), 60),
    140: 100, 170: 95, 180: 90, 190: 50, 210: 80, 220: 70,
}


def cci_source_path(root: Path) -> Path:
    return root / "imd-background" / CCI_FILENAME


def validate_cci_source(root: Path) -> str:
    path = cci_source_path(root)
    with path.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if digest != CCI_SHA256:
        raise ValueError(f"CCI background differs from the reviewed source identity: {path}")
    with rasterio.open(path) as source:
        if (source.count != 1 or source.dtypes != ("uint8",)
                or source.shape != (64800, 129600) or source.crs != "EPSG:4326"
                or source.tags().get("NC_GLOBAL#id") != CCI_IDENTITY
                or not np.allclose(tuple(source.transform)[:6],
                                   (1/360, 0, -180, 0, -1/360, 90), rtol=0, atol=1e-12)):
            raise ValueError(f"CCI background has an unsupported native grid or dataset identity: {path}")
    return digest


def download_cci_source(root: Path) -> None:
    path = cci_source_path(root)
    if path.exists():
        validate_cci_source(root)
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".cci-download-", dir=path.parent) as directory:
        staged_root = Path(directory)
        staged = cci_source_path(staged_root)
        staged.parent.mkdir()
        with urllib.request.urlopen(CCI_URL, timeout=60) as response, staged.open("wb") as output:
            expected = int(response.headers["Content-Length"])
            while block := response.read(1024 * 1024):
                output.write(block)
            output.flush()
            os.fsync(output.fileno())
        if staged.stat().st_size != expected:
            raise ValueError("Incomplete CCI background download")
        validate_cci_source(staged_root)
        os.replace(staged, path)


def cci_tile_classes(source, tile: tuple[int, int], grid: int = 3601) -> np.ndarray:
    lat, lon = tile
    intervals = grid - 1
    # Rational global coordinates avoid tile-dependent floating boundary choices.
    rows = np.minimum(((89-lat)*360*intervals + np.arange(grid)*360)//intervals,
                      source.height-1)
    cols = (((lon+180)*360*intervals + np.arange(grid)*360)//intervals) % source.width
    result = np.empty((grid, grid), dtype=np.uint8)
    cuts = [0, *list(np.flatnonzero(np.diff(cols) < 0)+1), grid]
    for begin, end in zip(cuts, cuts[1:]):
        columns = cols[begin:end]
        top, bottom = int(rows.min()), int(rows.max())+1
        left, right = int(columns.min()), int(columns.max())+1
        native = source.read(1, window=((top, bottom), (left, right)))
        result[:, begin:end] = native[np.ix_(rows-top, columns-left)]
    return result


def categorical_imd(worldcover: np.ndarray, cci: np.ndarray | None = None) -> np.ndarray:
    lut = np.full(256, 255, dtype=np.uint8)
    for category, percentage in WORLDCOVER_IMD.items():
        lut[category] = percentage
    result = lut[worldcover]
    missing = result == 255
    if cci is not None and np.any(missing):
        cci_lut = np.full(256, 255, dtype=np.uint8)
        for category, worldcover_category in CCI_WORLDCOVER.items():
            cci_lut[category] = WORLDCOVER_IMD[worldcover_category]
        result[missing] = cci_lut[cci[missing]]
    if np.any(result == 255):
        raise ValueError("IMD nodes remain unclassified by WorldCover and CCI")
    return result


def complete_imd_coverage(root: Path, land: set[tuple[int, int]]) -> tuple[set, set, str]:
    digest = validate_cci_source(root)
    inventory = read_catalog(root)
    tiles = land | set().union(*(native_tiles(key) for key in inventory))
    unknown = set()
    with rasterio.open(cci_source_path(root)) as source:
        for tile in sorted(tiles):
            # Every native background pixel touched by the inclusive arcsecond tile
            # is checked. This pinned globally classified background certifies coverage;
            # the converter independently preserves valid finer WorldCover nodes.
            classes = cci_tile_classes(source, tile, grid=361)
            if not np.all(np.isin(classes, tuple(CCI_WORLDCOVER))):
                unknown.add(tile)
    return tiles - unknown, unknown, digest

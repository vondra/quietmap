"""Official WorldCover source coverage; publication never infers water from a missing local file."""

import os
from pathlib import Path
import re
import sqlite3
import tempfile

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

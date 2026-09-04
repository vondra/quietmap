#!/usr/bin/env python3
"""Validate paired Hansen rasters and write clean GDAL VRT source lists."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ElementTree

import rasterio
from rasterio.errors import RasterioError


HANSEN_TILE_NAME = re.compile(
    r"^Hansen_GFC-[^_]+_(?P<layer>treecover2000|lossyear)_"
    r"(?P<latitude>[0-9]{2})(?P<north_south>[NS])_"
    r"(?P<longitude>[0-9]{3})(?P<east_west>[EW])\.tif$"
)
HANSEN_GRANULE_PIXELS = 40_000
MAX_ERROR_RESPONSE_BYTES = 16 * 1024


def _element_text(root: ElementTree.Element, local_name: str) -> str | None:
    for element in root.iter():
        if element.tag.rsplit("}", 1)[-1] == local_name and element.text:
            return element.text.strip()
    return None


def is_expected_missing_lossyear(path: Path) -> bool:
    """Return true only for a bounded, well-formed NoSuchKey response for path."""
    try:
        if path.stat().st_size > MAX_ERROR_RESPONSE_BYTES:
            return False
        root = ElementTree.parse(path).getroot()
    except (OSError, ElementTree.ParseError):
        return False

    if root.tag.rsplit("}", 1)[-1] != "Error":
        return False
    if _element_text(root, "Code") != "NoSuchKey":
        return False
    described_object = _element_text(root, "Key") or _element_text(root, "Details")
    if not described_object:
        return False
    return described_object == path.name or described_object.endswith(f"/{path.name}")


def _expected_bounds(path: Path, expected_layer: str) -> tuple[float, float, float, float]:
    match = HANSEN_TILE_NAME.fullmatch(path.name)
    if not match or match.group("layer") != expected_layer:
        raise ValueError(f"unexpected Hansen {expected_layer} filename: {path.name}")

    latitude = float(match.group("latitude"))
    if match.group("north_south") == "S":
        latitude = -latitude
    longitude = float(match.group("longitude"))
    if match.group("east_west") == "W":
        longitude = -longitude
    return longitude, latitude - 10.0, longitude + 10.0, latitude


def _validate_hansen_raster(path: Path, expected_layer: str) -> None:
    expected_bounds = _expected_bounds(path, expected_layer)
    with rasterio.open(path) as dataset:
        actual_bounds = tuple(dataset.bounds)
        problems = [
            dataset.driver != "GTiff",
            dataset.width != HANSEN_GRANULE_PIXELS,
            dataset.height != HANSEN_GRANULE_PIXELS,
            dataset.count != 1,
            tuple(dataset.dtypes) != ("uint8",),
            dataset.crs is None or dataset.crs.to_epsg() != 4326,
            any(abs(actual - expected) > 1e-9 for actual, expected in zip(actual_bounds, expected_bounds)),
        ]
    if any(problems):
        raise ValueError(f"unexpected Hansen raster schema or extent: {path}")


def validated_hansen_sources(hansen_dir: Path) -> tuple[list[Path], list[Path], int]:
    """Return valid treecover/loss rasters and the explicit zero-loss count."""
    treecover_dir = hansen_dir / "treecover2000"
    lossyear_dir = hansen_dir / "lossyear"
    treecover_sources = sorted(treecover_dir.glob("*.tif"))
    if not treecover_sources:
        raise ValueError(f"no Hansen treecover2000 GeoTIFFs in {treecover_dir}")

    valid_lossyear_sources: list[Path] = []
    expected_lossyear_paths: set[Path] = set()
    expected_missing_count = 0
    for treecover_path in treecover_sources:
        _validate_hansen_raster(treecover_path, "treecover2000")
        lossyear_path = lossyear_dir / treecover_path.name.replace(
            "_treecover2000_", "_lossyear_", 1
        )
        expected_lossyear_paths.add(lossyear_path)
        if not lossyear_path.is_file():
            raise ValueError(f"missing Hansen lossyear source or NoSuchKey marker: {lossyear_path}")
        try:
            _validate_hansen_raster(lossyear_path, "lossyear")
        except RasterioError as error:
            if not is_expected_missing_lossyear(lossyear_path):
                raise ValueError(f"invalid Hansen lossyear source: {lossyear_path}") from error
            expected_missing_count += 1
        else:
            valid_lossyear_sources.append(lossyear_path)

    actual_lossyear_paths = set(lossyear_dir.glob("*.tif"))
    unexpected_lossyear_paths = sorted(actual_lossyear_paths - expected_lossyear_paths)
    if unexpected_lossyear_paths:
        raise ValueError(f"unpaired Hansen lossyear source: {unexpected_lossyear_paths[0]}")
    if not valid_lossyear_sources:
        raise ValueError(f"no valid Hansen lossyear GeoTIFFs in {lossyear_dir}")
    return treecover_sources, valid_lossyear_sources, expected_missing_count


def _write_gdal_source_list(path: Path, sources: list[Path]) -> None:
    path.write_text("".join(f"{source.resolve()}\n" for source in sources), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("hansen_dir", type=Path)
    parser.add_argument("treecover_list", type=Path)
    parser.add_argument("lossyear_list", type=Path)
    arguments = parser.parse_args()
    try:
        treecover, lossyear, expected_missing = validated_hansen_sources(arguments.hansen_dir)
        _write_gdal_source_list(arguments.treecover_list, treecover)
        _write_gdal_source_list(arguments.lossyear_list, lossyear)
    except (OSError, RasterioError, ValueError) as error:
        print(f"[forest-cont] Hansen source validation failed: {error}", file=sys.stderr)
        return 2
    print(
        f"[forest-cont] Hansen sources: treecover={len(treecover)}, "
        f"lossyear={len(lossyear)}, expected-zero-loss={expected_missing}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

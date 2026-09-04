"""Reproject valid chronological IMD measurements over immutable WorldCover tiles."""

import argparse
from contextlib import ExitStack
import errno
import fcntl
import filecmp
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET

import numpy as np
import rasterio
from rasterio.enums import Resampling
from rasterio.transform import Affine
from rasterio.vrt import WarpedVRT
from rasterio.warp import transform_bounds

GRID = 3601  # Same node-registered 1-degree grid consumed by raster-reader.


def tile_coordinates(name):
    match = re.fullmatch(r"([NS])(\d{2})([EW])(\d{3})", name)
    if not match:
        raise ValueError(f"invalid raster tile: {name}")
    lat = int(match[2]) * (1 if match[1] == "N" else -1)
    lon = int(match[4]) * (1 if match[3] == "E" else -1)
    if not (-90 <= lat < 90 and -180 <= lon < 180):
        raise ValueError(f"out-of-range raster tile: {name}")
    return lat, lon


def tiles_intersecting(bounds):
    west, south, east, north = bounds
    if not all(math.isfinite(v) for v in bounds) or west > east or south > north:
        raise ValueError(f"invalid geographic source bounds: {bounds}")
    half_pixel = 0.5 / (GRID - 1)
    return {
        f"{'N' if lat >= 0 else 'S'}{abs(lat):02d}"
        f"{'E' if lon >= 0 else 'W'}{abs(lon):03d}"
        for lat in range(max(-90, math.floor(south - half_pixel)),
                         min(89, math.floor(north + half_pixel)) + 1)
        for lon in range(max(-180, math.floor(west - half_pixel)),
                         min(179, math.floor(east + half_pixel)) + 1)
    }


def masked_source_vrt(path, output):
    with rasterio.open(path) as source:
        if source.count != 1 or source.dtypes != ("uint8",) or source.crs is None:
            raise ValueError(f"IMD must have one georeferenced byte band: {path}")
        if source.transform.b or source.transform.d:
            raise ValueError(f"rotated IMD source is unsupported: {path}")
        bounds = transform_bounds(source.crs, "EPSG:4326", *source.bounds)
        root = ET.Element("VRTDataset", rasterXSize=str(source.width),
                          rasterYSize=str(source.height))
        ET.SubElement(root, "SRS").text = source.crs.to_wkt()
        ET.SubElement(root, "GeoTransform").text = ",".join(
            str(value) for value in source.transform.to_gdal())
        band = ET.SubElement(root, "VRTRasterBand", dataType="Byte", band="1")
        ET.SubElement(band, "NoDataValue").text = "255"
        data = ET.SubElement(band, "ComplexSource")
        ET.SubElement(data, "SourceFilename", relativeToVRT="0").text = str(path.resolve())
        ET.SubElement(data, "SourceBand").text = "1"
        for tag in ("SrcRect", "DstRect"):
            ET.SubElement(data, tag, xOff="0", yOff="0", xSize=str(source.width),
                          ySize=str(source.height))
        if source.nodata is not None:
            ET.SubElement(data, "NODATA").text = str(source.nodata)
        # Mask special codes BEFORE bilinear interpolation, not after clipping.
        ET.SubElement(data, "LUT").text = "0:0,100:100,101:255,255:255"
        ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)
        return source.crs.to_wkt(), tiles_intersecting(bounds)


def build_mosaics(source_root, temporary):
    sources = sorted(source_root.glob("[0-9][0-9][0-9][0-9]/*.tif"))
    if not sources:
        raise ValueError(f"no year-indexed IMD sources: {source_root}")
    groups = {}
    for index, source in enumerate(sources):
        masked = temporary / f"source-{index}.vrt"
        projection, tiles = masked_source_vrt(source, masked)
        key = (source.parent.name, projection)
        paths, coverage = groups.setdefault(key, ([], set()))
        paths.append(masked)
        coverage.update(tiles)
    mosaics = []
    for index, (_, (paths, coverage)) in enumerate(sorted(groups.items())):
        mosaic = temporary / f"mosaic-{index}.vrt"
        subprocess.run(["gdalbuildvrt", "-q", "-strict", "-srcnodata", "255",
                        "-vrtnodata", "255", str(mosaic), *map(str, paths)], check=True)
        mosaics.append((mosaic, coverage))
    print(json.dumps({"sources": len(sources), "mosaics": len(mosaics)}), flush=True)
    return mosaics


def read_base(path, grid=GRID):
    if not path.exists():
        return np.full((grid, grid), 100, dtype=np.uint8)
    if path.stat().st_size != grid * grid:
        raise ValueError(f"invalid IMD tile length: {path}")
    values = np.fromfile(path, dtype=np.uint8).reshape(grid, grid)
    if np.any(values > 100):
        raise ValueError(f"IMD base values outside 0–100: {path}")
    return values


def overlay_tile(base, mosaics, name):
    lat, lon = tile_coordinates(name)
    grid = base.shape[0]
    step = 1.0 / (grid - 1)
    transform = Affine(step, 0, lon - step / 2, 0, -step, lat + 1 + step / 2)
    result = base.copy()
    for mosaic, coverage in mosaics:
        if name not in coverage:
            continue
        with WarpedVRT(mosaic, crs="EPSG:4326", transform=transform,
                       width=grid, height=grid, src_nodata=255, nodata=255,
                       resampling=Resampling.bilinear) as warped:
            values = warped.read(1)
        # Only WorldCover knows water: earlier IMD=100 also means urban land.
        values[(base == 100) & (values == 0)] = 100
        valid = values <= 100
        result[valid] = values[valid]
    return result


def publish_atomic(destination, content: Path | np.ndarray):
    if destination.exists():
        identical = (filecmp.cmp(content, destination, shallow=False)
                     if isinstance(content, Path)
                     else destination.read_bytes() == content.tobytes())
        if identical:
            return False
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.",
                                                 dir=destination.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            if isinstance(content, Path):
                with content.open("rb") as original:
                    try:
                        fcntl.ioctl(output.fileno(), 0x40049409, original.fileno())  # Linux FICLONE.
                    except OSError as error:
                        if error.errno not in (errno.EOPNOTSUPP, errno.EXDEV, errno.EINVAL, errno.ENOTTY):
                            raise
                        shutil.copyfileobj(original, output)
            else:
                output.write(content.tobytes())
            os.fchmod(output.fileno(), 0o644)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, destination)
        directory = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)
    return True


def convert(source_root, base_root, output_root, selected_tiles=()):
    if not base_root.is_dir() or base_root.resolve() == output_root.resolve():
        raise ValueError("IMD_BASE must exist and differ from IMD_DST")
    base_tiles = {path.stem for path in base_root.glob("*.raw")}
    if not base_tiles:
        raise ValueError("IMD_BASE has no WorldCover raw tiles")
    output_root.mkdir(parents=True, exist_ok=True)
    with (output_root / ".imd-overlay.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with tempfile.TemporaryDirectory(prefix="imd-overlay-") as directory, ExitStack() as stack:
            mosaics = build_mosaics(source_root, Path(directory))
            overlay_tiles = set().union(*(coverage for _, coverage in mosaics))
            targets = base_tiles | overlay_tiles | {p.stem for p in output_root.glob("*.raw")}
            if selected_tiles:
                targets = set(selected_tiles)
            opened = [(stack.enter_context(rasterio.open(path)), coverage)
                      for path, coverage in mosaics]
            changed = 0
            for index, name in enumerate(sorted(targets), 1):
                tile_coordinates(name)
                source, destination = base_root / f"{name}.raw", output_root / f"{name}.raw"
                if name not in overlay_tiles and source.exists():
                    if source.stat().st_size != GRID * GRID:
                        raise ValueError(f"invalid IMD base length: {source}")
                    changed += publish_atomic(destination, source)
                else:
                    values = overlay_tile(read_base(source, GRID), opened, name)
                    if source.exists() or destination.exists() or np.any(values != 100):
                        changed += publish_atomic(destination, values)
                if index % 100 == 0 or index == len(targets):
                    print(json.dumps({"checked_tiles": index, "total_tiles": len(targets),
                                      "changed_tiles": changed}), flush=True)
            return changed


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sources", required=True, type=Path)
    parser.add_argument("--base", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--tile", action="append", default=[])
    arguments = parser.parse_args()
    convert(arguments.sources, arguments.base, arguments.output, arguments.tile)

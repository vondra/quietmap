"""Preserve native Copernicus latitude grids and periodic support for exact node interpolation."""

import argparse
from collections import defaultdict
import copy
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET

from dem_sources import changed_outputs, read_catalog, required_supplement, source_gap, source_path, tile_name
from dem_native_grid import integer_pixel, write_aligned_gap


def make_periodic(mosaic: Path) -> None:
    tree = ET.parse(mosaic)
    dataset = tree.getroot()
    transform = dataset.find("GeoTransform")
    channel = dataset.find("VRTRasterBand")
    if transform is None or transform.text is None or channel is None:
        raise ValueError(f"Incomplete native DEM mosaic: {mosaic}")
    geo = [float(value) for value in transform.text.split(",")]
    if len(geo) != 6 or geo[1] <= 0 or geo[5] >= 0 or geo[2] != 0 or geo[4] != 0:
        raise ValueError(f"Invalid native DEM geotransform: {mosaic}")
    period = integer_pixel(360 / geo[1])
    origin = -180 - geo[1] / 2
    offset = integer_pixel((geo[0] - origin) / geo[1])
    for entry in list(channel):
        destination = entry.find("DstRect")
        source = entry.find("SrcRect")
        if destination is None or source is None:
            continue
        for axis in ["x", "y"]:
            if integer_pixel(float(source.attrib[axis + "Size"])
                             - float(destination.attrib[axis + "Size"])) != 0:
                raise ValueError(f"Native DEM band would resample source pixels: {mosaic}")
            integer_pixel(float(source.attrib[axis + "Off"]))
            integer_pixel(float(destination.attrib[axis + "Off"]))
        destination.set("xOff", str(integer_pixel(float(destination.attrib["xOff"])) + offset))
        if int(destination.attrib["xOff"]) == 0:
            wrapped = copy.deepcopy(entry)
            wrapped_rect = wrapped.find("DstRect")
            if wrapped_rect is None:
                raise ValueError(f"Missing periodic DEM source window: {mosaic}")
            wrapped_rect.set("xOff", str(int(destination.attrib["xOff"]) + period))
            channel.append(wrapped)
    # The repeated +180 node supplies the bilinear kernel beyond the last E179 sample.
    dataset.set("rasterXSize", str(period + 1))
    geo[0] = origin
    transform.text = ", ".join(map(str, geo))
    tree.write(mosaic)


def native_mosaic(sources: list[Path], mosaic: Path) -> None:
    inputs = mosaic.with_suffix(".txt")
    inputs.write_text("".join(f"{path.resolve()}\n" for path in sources))
    subprocess.run(
        ["gdalbuildvrt", "-q", "-strict", "-resolution", "highest",
         "-input_file_list", str(inputs), str(mosaic)], check=True,
    )
    make_periodic(mosaic)


def add_aligned_gap(band: Path, aligned: Path, longitude: int, width: int, height: int) -> None:
    tree = ET.parse(band)
    channel = tree.getroot().find("VRTRasterBand")
    if channel is None:
        raise ValueError(f"Missing native DEM channel: {band}")
    # gdalbuildvrt rejects mixed Float32/Float64 inputs. Preserve the original fine
    # 1:1 sources and add only the aligned gap, without rounding its interpolants.
    channel.set("dataType", "Float64")
    for offset in ([0, 360 * width] if longitude == -180 else [0]):
        source = ET.SubElement(channel, "SimpleSource")
        ET.SubElement(source, "SourceFilename", relativeToVRT="0").text = str(aligned.resolve())
        ET.SubElement(source, "SourceBand").text = "1"
        ET.SubElement(source, "SrcRect", xOff="0", yOff="0", xSize=str(width), ySize=str(height))
        ET.SubElement(source, "DstRect", xOff=str((longitude + 180) * width + offset),
                      yOff="0", xSize=str(width), ySize=str(height))
    tree.write(band)


def build_native_mosaics(source_root: Path, output: Path, coverage_gaps: bool = False) -> None:
    catalog = read_catalog(source_root)
    changed = changed_outputs(catalog)
    selected = changed if coverage_gaps else catalog[90]
    latitudes = {latitude + offset for latitude, _ in selected for offset in (0, -1)}
    groups: dict[int, list[Path]] = defaultdict(list)
    for tile in sorted(catalog[30]):
        path = source_path(source_root, 30, tile)
        if not path.is_file():
            raise ValueError(f"Missing catalogued GLO30 land source: {path}")
        if tile[0] in latitudes:
            groups[tile[0]].append(path)
    for latitude, sources in groups.items():
        native_mosaic(sources, output / f"{latitude}.vrt")

    gaps = {tile for tile in source_gap(catalog) if tile[0] in latitudes}
    if gaps:
        support = [source_path(source_root, 90, tile) for tile in sorted(required_supplement(catalog))]
        for path in support:
            if not path.is_file():
                raise ValueError(f"Missing catalogued GLO90 native support: {path}")
        native = output / "native90.vrt"
        native_mosaic(support, native)
        for latitude, longitude in sorted(gaps):
            band = output / f"{latitude}.vrt"
            if not band.is_file():
                raise ValueError(f"No native GLO30 latitude lattice for gap at {latitude}")
            transform = ET.parse(band).findtext("GeoTransform")
            if transform is None:
                raise ValueError(f"Missing native latitude geotransform: {band}")
            geo = [float(value) for value in transform.split(",")]
            dx, dy = geo[1], -geo[5]
            width, height = integer_pixel(1 / dx), integer_pixel(1 / dy)
            aligned = output / f"{tile_name((latitude, longitude))}.tif"
            write_aligned_gap(native, aligned, latitude, longitude, width, height)
            add_aligned_gap(band, aligned, longitude, width, height)

    for name, tiles in [("tiles.txt", selected), ("changed.txt", changed)]:
        (output / name).write_text("".join(f"{tile_name(tile)}\n" for tile in sorted(tiles)))
    print(f"[cop-dem] Planned {len(selected)}/{len(catalog[90])} land tiles; {len(changed)} coverage-dependent outputs")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_root", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--coverage-gaps", action="store_true")
    arguments = parser.parse_args()
    try:
        build_native_mosaics(arguments.source_root, arguments.output, arguments.coverage_gaps)
    except subprocess.CalledProcessError as error:
        sys.exit(error.returncode)

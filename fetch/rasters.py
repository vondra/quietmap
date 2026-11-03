#!/usr/bin/env python3
"""Normalize an explicit, ordered list of publisher rasters to one GDAL mosaic per manifest input."""
import argparse
import json
import os
from pathlib import Path
import re
import tempfile
import xml.etree.ElementTree as ET

from osgeo import gdal

gdal.UseExceptions()


def land_coverage(catalog, target):
    """Copernicus's published tile catalogue defines land degrees, including polar land."""
    cells = bytearray(360 * 180)
    for name in Path(catalog).read_text().splitlines():
        match = re.fullmatch(r"Copernicus_DSM_COG_(?:30|90)_([NS])(\d+)_00_([EW])(\d+)_00_DEM", name)
        if not match:
            raise ValueError(f"invalid Copernicus coverage tile {name}")
        latitude = int(match[2]) * (-1 if match[1] == "S" else 1)
        longitude = int(match[4]) * (-1 if match[3] == "W" else 1)
        if not (-90 <= latitude < 90 and -180 <= longitude < 180):
            raise ValueError(f"coverage tile outside the geographic grid: {name}")
        cells[(89 - latitude) * 360 + longitude + 180] = 1
    if not any(cells):
        raise ValueError("empty Copernicus land catalogue")
    dataset = gdal.GetDriverByName("GTiff").Create(str(target), 360, 180, 1, gdal.GDT_Byte)
    dataset.SetProjection("EPSG:4326")
    dataset.SetGeoTransform((-180, 1, 0, 90, 0, -1))
    dataset.GetRasterBand(1).WriteRaster(0, 0, 360, 180, bytes(cells))
    dataset.FlushCache()


def validated_missing_loss(path):
    """Recognize an archived absent loss granule; absence is not an observed zero."""
    if path.stat().st_size > 16384 or "_lossyear_" not in path.name:
        return False
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError:
        return False
    described = root.findtext("Key") or root.findtext("Details") or ""
    return root.tag == "Error" and root.findtext("Code") == "NoSuchKey" and (
        described == path.name or described.endswith("/" + path.name)
    )


def convert(recipe, output):
    """A recipe names files; it never discovers a vintage from directory contents."""
    output.mkdir(parents=True, exist_ok=True)
    for entry in recipe:
        identifier = entry["input"]
        if Path(identifier).name != identifier or identifier in (".", ".."):
            raise ValueError("input must be a single manifest identifier")
        target = output / identifier
        if target.exists():
            raise ValueError(f"refusing to replace {target}")
        if "coverage_catalog" in entry:
            if "files" in entry:
                raise ValueError("land coverage needs its catalogue only")
            with tempfile.TemporaryDirectory(dir=output) as workspace:
                temporary = Path(workspace) / "coverage"
                land_coverage(entry["coverage_catalog"], temporary)
                os.link(temporary, target)
            continue
        paths = [str(Path(path).resolve(strict=True)) for path in entry["files"]]
        for marker in entry.get("missing_loss_markers", []):
            if not validated_missing_loss(Path(marker)):
                raise ValueError(f"invalid missing-loss record {marker}")
        if not paths:
            raise ValueError(f"{identifier}: no explicitly selected raster")
        options = dict(strict=True)
        if "crs" in entry:
            options["outputSRS"] = entry["crs"]
        if "bounds" in entry:
            options["outputBounds"] = entry["bounds"]
        if "pixel_size" in entry:
            x_step, y_step = entry["pixel_size"]
            if x_step <= 0 or y_step <= 0:
                raise ValueError("pixel_size must contain two positive steps")
            options.update(resolution="user", xRes=x_step, yRes=y_step)
        if "nodata" in entry:
            options.update(srcNodata=entry["nodata"], VRTNodata=entry["nodata"])
        with tempfile.TemporaryDirectory(dir=output) as workspace:
            temporary = Path(workspace) / "mosaic"
            shifted = {}
            for path, offset in entry.get("longitude_offsets", {}).items():
                path = str(Path(path).resolve(strict=True))
                if path not in paths:
                    raise ValueError("longitude offset names an unselected file")
                native = gdal.Open(path)
                spatial_ref = native.GetSpatialRef()
                if not spatial_ref.IsGeographic():
                    raise ValueError("longitude offsets require a geographic source")
                west, step, rotation, north, tilt, down = native.GetGeoTransform()
                if rotation or tilt:
                    raise ValueError("longitude offsets require a north-up grid")
                wrapper = str(Path(workspace) / str(len(shifted)))
                gdal.Translate(wrapper, native, format="VRT", outputBounds=[
                    west + offset, north, west + offset + native.RasterXSize * step,
                    north + native.RasterYSize * down])
                shifted[path] = wrapper
            dataset = gdal.BuildVRT(str(temporary), [shifted.get(p, p) for p in paths], **options)
            if dataset is None:
                raise ValueError(f"{identifier}: failed mosaic")
            dataset = None
            tree = ET.parse(temporary)
            originals = {str(Path(w).resolve()): p for p, w in shifted.items()}
            for node in tree.findall(".//SourceFilename"):
                source = Path(node.text)
                if node.get("relativeToVRT") == "1":
                    source = temporary.parent / source
                source = str(source.resolve())
                node.text = originals.get(source, source)
                node.set("relativeToVRT", "0")
            # Percentage palettes describe display classes, not averaged measurements.
            for band in tree.findall("VRTRasterBand"):
                for palette in band.findall("ColorTable"):
                    band.remove(palette)
                for interpretation in band.findall("ColorInterp"):
                    interpretation.text = "Gray"
            tree.write(temporary, encoding="utf-8", xml_declaration=True)
            os.link(temporary, target)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--recipe", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    arguments = parser.parse_args()
    convert(json.loads(arguments.recipe.read_text()), arguments.out)

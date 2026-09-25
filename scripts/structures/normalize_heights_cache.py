#!/usr/bin/env python3
"""Normalize one measured building-height inventory into the per-1-degree cache.

Reads the provider's retained download (local file, no network) and writes
`measured_heights_v1` tiles beside any already cached inventories. Heights are
mean roof heights in metres: NRW LoD1 `measuredHeight`, 3DBAG 50th-percentile
roof minus ground (the 70th percentile overstates pitched roofs ~1 m).
One row per building part; the ladder joins by overlap.
"""

import argparse
import json
import math
import os
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))

import pyarrow.parquet as pq
import shapely
from pyproj import Transformer

from measured_heights import CONTRACT_KEY, CONTRACT_VERSION, SCHEMA
from structure_inventory import degree_name, write_official_cache

NS = {"bldg": "http://www.opengis.net/citygml/building/1.0",
      "gml": "http://www.opengis.net/gml"}



def _read_json(path):
    with open(path, encoding="utf-8") as source:
        return json.load(source)

def lowest_ring_lonlat(solid, to_lonlat):
    """The LoD1 solid's ground footprint: its exterior ring of lowest mean z."""
    best, best_z = None, math.inf
    for ring in solid.findall(".//gml:LinearRing", NS):
        pos = ring.find("gml:posList", NS)
        if pos is None or pos.text is None:
            continue
        coords = [float(v) for v in pos.text.split()]
        dim = int(pos.get("srsDimension") or 3)
        xs = coords[0::dim]
        mean_z = sum(coords[2::dim]) / len(xs) if dim == 3 else 0.0
        if mean_z < best_z:
            best_z = mean_z
            lons, lats = to_lonlat.transform(xs, coords[1::dim])
            best = list(zip(lons, lats))
    return best


def read_nrw_lod1(paths):
    """NRW 3D-Gebaeudemodell LoD1 (CityGML 1.0, ETRS89 UTM32): one row per
    building part with its own measuredHeight, else per building."""
    to_lonlat = Transformer.from_crs("EPSG:25832", "EPSG:4326", always_xy=True)
    rows = []
    for path in paths:
        root = ET.parse(path).getroot()
        for building in root.findall(".//bldg:Building", NS):
            parts = building.findall("bldg:consistsOfBuildingPart/bldg:BuildingPart", NS)
            for unit in parts or [building]:
                height = unit.findtext("bldg:measuredHeight", None, NS)
                solid = unit.find("bldg:lod1Solid", NS)
                if height is None or solid is None:
                    continue
                ring = lowest_ring_lonlat(solid, to_lonlat)
                if ring is None or len(ring) < 4:
                    continue
                rows.append((shapely.Polygon(ring), float(height)))
    print(f"[normalize-heights] nrw-lod1: {len(rows)} footprints from {len(paths)} tiles",
          flush=True)
    return rows


def explode_polygons(geometry):
    if geometry["type"] == "Polygon":
        return [geometry["coordinates"]]
    if geometry["type"] == "MultiPolygon":
        return geometry["coordinates"]
    raise SystemExit(f"unsupported height geometry {geometry['type']}")


def read_heights_geojson(paths):
    """Footprint GeoJSON with a `height_m` property per feature (the 3DBAG
    window form: 50th-percentile roof height minus ground, metres). Rows
    without a positive height (failed reconstruction, 56 of 5,966 in the
    Amersfoort/Oosterwolde windows) are skipped, not guessed."""
    rows, skipped = [], 0
    for path in paths:
        for feature in _read_json(path)["features"]:
            height = feature["properties"].get("height_m")
            if height is None or not math.isfinite(height) or height <= 0:
                skipped += 1
                continue
            for polygon in explode_polygons(feature["geometry"]):
                rows.append((shapely.Polygon(polygon[0], polygon[1:]), float(height)))
    print(f"[normalize-heights] heights-geojson: {len(rows)} footprints "
          f"from {len(paths)} files, {skipped} without a height skipped", flush=True)
    return rows


def append_cache(rows, source, as_of, cache_dir):
    by_tile = {}
    for geom, height_m in rows:
        if geom.is_empty:
            continue
        centroid = geom.centroid
        tile = degree_name(math.floor(centroid.y), math.floor(centroid.x))
        columns = by_tile.setdefault(tile, {name: [] for name in SCHEMA.names})
        columns["geometry"].append(shapely.to_wkb(geom))
        columns["height_m"].append(float(height_m))
        columns["source"].append(source)
        columns["as_of"].append(as_of)
    for tile, columns in by_tile.items():
        path = os.path.join(cache_dir, f"{tile}.parquet")
        if os.path.exists(path):
            for row in pq.read_table(path).to_pylist():
                for name in SCHEMA.names:
                    columns[name].append(row[name])
    write_official_cache(by_tile, cache_dir, SCHEMA, CONTRACT_KEY, CONTRACT_VERSION)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--from", dest="provider", required=True,
                    choices=("nrw-lod1-gml", "heights-geojson"))
    ap.add_argument("--input", required=True, nargs="+", help="provider download(s)")
    ap.add_argument("--source", required=True, help="inventory id, e.g. DE-NW-LoD1-2026")
    ap.add_argument("--as-of", required=True, help="inventory vintage YYYY-MM-DD")
    ap.add_argument("--out", required=True, help="cache directory (tiles accumulate)")
    args = ap.parse_args()
    rows = read_nrw_lod1(args.input) if args.provider == "nrw-lod1-gml" \
        else read_heights_geojson(args.input)
    if not rows:
        raise SystemExit(f"{args.input}: no footprints")
    append_cache(rows, args.source, args.as_of, args.out)
    print(f"[normalize-heights] wrote {len(rows)} footprints as {args.source} to {args.out}",
          flush=True)


if __name__ == "__main__":
    main()

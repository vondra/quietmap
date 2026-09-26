#!/usr/bin/env python3
"""Normalize one official noise-barrier inventory into the per-1-degree cache.

Reads the provider's retained download (local file, no network) and writes
`official_barriers_v1` tiles beside any already cached inventories. Heights are
metres above the road; out-of-range measurements fall back to the inventory's
own in-range median and are marked unmeasured. Only standing walls are kept:
proposed, removed and replaced records are not noise protection.
"""

import argparse
import json
import math
import os
import statistics
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))

import shapely
from pyproj import Transformer

import qmgrid
from official_barriers import CONTRACT_KEY, CONTRACT_VERSION, KIND_BERM, KIND_COMBINED, KIND_WALL, SCHEMA
from structure_inventory import accumulate_official_cache, degree_name

FT_TO_M = 0.3048  # exact

# A screen outside 1-12 m above the road is a broken measurement, not a wall:
# GWV_2024 (7,183 joined 3D segments, 2026-09-25) has Scherm p5/p95 1.3/8.2 m;
# the 19-41 m tails sit on viaduct edges and the negatives on mismatched
# carriageways. In-range medians (Wal 1,559, Scherm 4,497, Combinatie 719):
# 2.9, 3.8, 4.9 m. US medians from the same day's fetches: WSDOT 12 ft (302
# distinct walls with a height, rule below), FDOT 14 ft (1,032 constructed
# walls); VDOT carries no height, so the FHWA 2022 length-weighted US mean
# (4.45 m) stands in.
PHYSICAL_RANGE_M = (1.0, 12.0)
GWV_MEDIAN_M = {"Wal": 2.9, "Scherm": 3.8, "Combinatie": 4.9}
GWV_KIND = {"Wal": KIND_BERM, "Scherm": KIND_WALL, "Combinatie": KIND_COMBINED}
WSDOT_MEDIAN_FT = 12.0
FDOT_MEDIAN_FT = 14.0
US_FHWA_MEAN_M = 4.45



def _read_json(path):
    with open(path, encoding="utf-8") as source:
        return json.load(source)

def explode_lines(geometry):
    """GeoJSON LineString/MultiLineString geometry to a list of coord lists,
    dropping null and degenerate (under two positions) lines."""
    if geometry is None:
        return []
    if geometry["type"] == "LineString":
        parts = [geometry["coordinates"]]
    elif geometry["type"] == "MultiLineString":
        parts = geometry["coordinates"]
    else:
        raise SystemExit(f"unsupported barrier geometry {geometry['type']}")
    return [part for part in parts if len(part) >= 2]


def in_range(height_m):
    return PHYSICAL_RANGE_M[0] <= height_m <= PHYSICAL_RANGE_M[1]


def read_gwv(segmenten_path, kantstreep_path):
    """RWS GWV_2024: barrier-top lines plus road-edge reference lines, both
    EPSG:28992 with NAP z. Height is the per-vertex top-minus-reference median."""
    to_lonlat = Transformer.from_crs("EPSG:28992", "EPSG:4326", always_xy=True)
    segmenten = _read_json(segmenten_path)["features"]
    reference = {(f["properties"]["id_gw_vz"], f["properties"]["id_segment"]): f
                 for f in _read_json(kantstreep_path)["features"]}
    rows, substituted = [], 0
    for feature in segmenten:
        props = feature["properties"]
        kind = GWV_KIND.get(props["type"])
        if kind is None:
            raise SystemExit(f"GWV: unknown type {props['type']!r}")
        peer = reference.get((props["id_gw_vz"], props["id_segment"]))
        diffs = None
        if peer is not None:
            top = [c for line in explode_lines(feature["geometry"]) for c in line]
            ref = [c for line in explode_lines(peer["geometry"]) for c in line]
            if len(top) == len(ref):
                diffs = [a[2] - b[2] for a, b in zip(top, ref)
                         if len(a) > 2 and len(b) > 2]
        height, measured = GWV_MEDIAN_M[props["type"]], False
        if diffs:
            median = statistics.median(diffs)
            if in_range(median):
                height, measured = median, True
        if not measured:
            substituted += 1
        for line in explode_lines(feature["geometry"]):
            lons, lats = to_lonlat.transform([c[0] for c in line], [c[1] for c in line])
            rows.append((shapely.LineString(zip(lons, lats)), height, measured, kind))
    print(f"[normalize-barriers] gwv: {len(rows)} lines, {substituted} median-substituted",
          flush=True)
    return rows


def read_wsdot(path):
    """WSDOT noise walls: MinHeightFt is 0 (unknown) on 360 of 742 existing
    walls, so the height is the min/max mean where the minimum is positive,
    else the maximum."""
    rows, substituted = [], 0
    for feature in _read_json(path)["features"]:
        props = feature["properties"]
        minimum, maximum = props.get("MinHeightFt") or 0, props.get("MaxHeightFt") or 0
        feet = (minimum + maximum) / 2 if minimum > 0 else maximum
        height, measured = feet * FT_TO_M, True
        if not (feet > 0 and in_range(height)):
            height, measured, substituted = WSDOT_MEDIAN_FT * FT_TO_M, False, substituted + 1
        for line in explode_lines(feature["geometry"]):
            rows.append((shapely.LineString([(c[0], c[1]) for c in line]),
                         height, measured, KIND_WALL))
    print(f"[normalize-barriers] wsdot: {len(rows)} lines, {substituted} median-substituted",
          flush=True)
    return rows


def read_fdot(path):
    """FDOT GeoPlan noise barriers: FED_HEIGHT is feet (median 14, max 22 over
    1,449 records). Only CONSTRUCTED barriers stand; recommended, removed and
    replaced records are skipped."""
    rows, substituted, skipped = [], 0, 0
    for feature in _read_json(path)["features"]:
        props = feature["properties"]
        if props.get("TYPE") != "CONSTRUCTED BARRIERS":
            skipped += 1
            continue
        feet = props.get("FED_HEIGHT") or 0
        height, measured = feet * FT_TO_M, True
        if not (feet > 0 and in_range(height)):
            height, measured, substituted = FDOT_MEDIAN_FT * FT_TO_M, False, substituted + 1
        for line in explode_lines(feature["geometry"]):
            rows.append((shapely.LineString([(c[0], c[1]) for c in line]),
                         height, measured, KIND_WALL))
    print(f"[normalize-barriers] fdot: {len(rows)} lines, "
          f"{substituted} median-substituted, {skipped} non-standing skipped", flush=True)
    return rows


def read_vdot(path):
    """VDOT 2023 barrier study: geometry only, so every wall takes the FHWA
    2022 length-weighted US mean height and is marked unmeasured."""
    rows = []
    for feature in _read_json(path)["features"]:
        for line in explode_lines(feature["geometry"]):
            rows.append((shapely.LineString([(c[0], c[1]) for c in line]),
                         US_FHWA_MEAN_M, False, KIND_WALL))
    print(f"[normalize-barriers] vdot: {len(rows)} lines at the US mean height", flush=True)
    return rows


READERS = {"gwv": read_gwv, "wsdot": read_wsdot, "fdot": read_fdot, "vdot": read_vdot}


def append_cache(rows, source, as_of, cache_dir):
    # Inventories repeat records: WSDOT publishes 742 existing-wall features for
    # 317 distinct geometries (2026-09-25). One physical wall is one cache row.
    seen, duplicates = set(), 0
    by_tile = {}
    for geom, height_m, measured, kind in rows:
        if geom.is_empty:
            continue
        key = (shapely.to_wkb(geom), round(float(height_m), 3), kind)
        if key in seen:
            duplicates += 1
            continue
        seen.add(key)
        centroid = geom.centroid
        tile = degree_name(math.floor(centroid.y), math.floor(centroid.x))
        columns = by_tile.setdefault(tile, {name: [] for name in SCHEMA.names})
        columns["geometry"].append(shapely.to_wkb(geom))
        columns["height_m"].append(float(height_m))
        columns["measured"].append(bool(measured))
        columns["kind"].append(kind)
        columns["source"].append(source)
        columns["as_of"].append(as_of)
    accumulate_official_cache(by_tile, source, cache_dir, SCHEMA, CONTRACT_KEY,
                              CONTRACT_VERSION)
    if duplicates:
        print(f"[normalize-barriers] {duplicates} duplicate lines dropped", flush=True)
    return len(seen)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--from", dest="provider", required=True, choices=sorted(READERS))
    ap.add_argument("--input", required=True, help="provider download (gwv: gwv_segmenten)")
    ap.add_argument("--input2", help="gwv_hoogte_kantstreep (gwv only)")
    ap.add_argument("--source", required=True, help="inventory id, e.g. NL-RWS-GWV-2024")
    ap.add_argument("--as-of", required=True, help="inventory vintage YYYY-MM-DD")
    ap.add_argument("--out", required=True, help="cache directory (tiles accumulate)")
    args = ap.parse_args()
    if args.provider == "gwv":
        if not args.input2:
            raise SystemExit("gwv needs --input2 gwv_hoogte_kantstreep")
        rows = read_gwv(args.input, args.input2)
    else:
        if args.input2:
            raise SystemExit(f"{args.provider} takes no --input2")
        rows = READERS[args.provider](args.input)
    if not rows:
        raise SystemExit(f"{args.input}: no barrier lines")
    kept = append_cache(rows, args.source, args.as_of, args.out)
    print(f"[normalize-barriers] wrote {kept} lines as {args.source} to {args.out}",
          flush=True)


if __name__ == "__main__":
    main()

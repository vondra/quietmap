#!/usr/bin/env python3
# Height-ladder MATERIALIZER for the vector obstacle store (screening heights).
#
# Deterministically REGENERATES each cell's prepared obstacles.arrow from the
# immutable Overture staging shards plus height rasters — never from its own
# previous output, so a re-run cannot compound state and there is no
# "restore the original tier" problem (gg review 2026-08-09, Codex CRITICAL 1):
#
#   prepared/<cell>/obstacles.arrow = ladder( merge(staging shards), rasters )
#
#   tier 0  mapped per-building height (Overture/OSM)     — staging, never touched
#   tier 1  floors x 3 m                                  — staging, replaced by tier 3 only
#   tier 2  flat 8 m world default                        — staging, replaced by tier 3 or 4
#   tier 3  city/national measured per-building zonal     — regional 1 m raster (IPR Praha)
#   tier 4  areal prior (GHSL ANBH 100 m pixel average)   — global raster, tier-2 rows only
#
# Tier 3: mean of in-footprint pixels >= MEASURED_MIN_M from a regional
# DSM-DTM building-height raster, with a coverage guard so structures absent
# from the city model keep their staging height instead of inheriting a
# neighbour's wall. Zonal eligibility is gated on the stored CENTROID lying
# inside the raster grid — a footprint straddling the raster's outer edge with
# its centroid outside stays on its staging/ANBH height (documented edge
# behavior; the raster edge is the city boundary, acoustically marginal). Tier 4 samples GHS-BUILT-H ANBH at the centroid and
# replaces ONLY the flat 8 m default — floors-derived heights are per-building
# information and win over a 100 m areal average. Tier 4 stays eligible for
# the low-profile 3 m shed cap exactly like tier 2 (noise_compute::low_profile)
# — an areal average knows nothing about the individual shed under it.
#
# Merge order = sorted shard filenames, the same order the Wave-1 promotion
# used and the loaders' shard_paths() sort — regenerating a never-enriched
# cell reproduces the promoted file byte-for-byte except enriched rows.
# A promoted cell with no staging FAILS loudly: leaving its old output while a
# chain step exits zero would falsely certify it. Cells staged but never
# promoted are skipped — promotion policy stays with the world cutover; this
# tool only re-materializes already-promoted cells.
#
# An OSM planet re-extract does NOT touch obstacles.arrow (osm-to-h3r4.sh
# rewrites only its own per-file arrows). The manifest runs the TS face on every
# chain pass. It selects only cells whose adjacent proof no longer matches the
# output, staging, raster, or worker identity; this worker seals the exact
# output inode it published.
#
# Usage:
#   enrich-obstacle-heights.py --h3r4-dir data/prepared/2026/h3r4 \
#     --staging-dir data/enrichment/global/overture-obstacles/h3r4 \
#     --ghsl <ANBH .tif> [--regional <mosaic .vrt|.tif>] \
#     (--cells hex1,hex2,... | --cells-file <one hex per line>)
#     [--proof-manifest <JSON object mapping cell to input SHA-256>]
#
# --cells-file exists because a world run's cell list exceeds ARG_MAX as a
# single argument (gg pass 2); the TS face always writes a manifest file.
# Writes each cell's obstacles.arrow atomically (tmp + rename), schema unchanged.

import argparse
import glob
import json
import math
import os
import sys

import numpy as np
import pyarrow as pa
import pyarrow.ipc as ipc
import shapely
import shapely.ops
from osgeo import gdal
from pyproj import CRS, Transformer
from shapely import wkb as shapely_wkb

gdal.UseExceptions()
gdal.SetCacheMax(512 * 1024 * 1024)  # ANBH point reads cluster; keep blocks hot

MEASURED_MIN_M = 2.0      # zonal pixels below this are "not a building surface here"
COVERAGE_MIN_FRAC = 0.30  # measured pixels must cover this share of the footprint
COVERAGE_MIN_PX = 3
TIER3_CLAMP = (2.5, 250.0)
ANBH_MIN_M = 1.0          # ANBH below this = no better info than the default
ANBH_MAX_VALID = 250.0    # GHSL NoData sentinel is 255 — belt for a missing tag
TIER4_CLAMP = (3.0, 100.0)
HEIGHT_PROOF_FILENAME = "obstacles.height-materialization.json"
HEIGHT_PROOF_VERSION = 1

SCHEMA = pa.schema(
    [
        ("polygon_wkb", pa.binary()),
        ("height_m", pa.float32()),
        ("centroid_lat", pa.float64()),
        ("centroid_lon", pa.float64()),
        # 0/1/2 from the ingest ladder, 3/4 written here — full table above and
        # in noise_compute::low_profile (cap applies to tiers 2 and 4).
        ("height_tier", pa.uint8()),
    ]
)


def transformer_for(ds, path):
    """WGS84 -> the raster's own CRS. Hard-fails on an unreferenced raster —
    the IPR exportImage tiles arrive as a datum-less LOCAL_CS shell until the
    downloader stamps EPSG:5514 (gg review 2026-08-09, Codex CRITICAL 3)."""
    crs = CRS.from_wkt(ds.GetProjection())
    if not (crs.is_projected or crs.is_geographic):
        raise SystemExit(f"{path}: raster CRS is not georeferenced ({crs.name}) — re-run scripts/obstacles/download-height-rasters.sh to stamp it")
    return Transformer.from_crs("EPSG:4326", crs, always_xy=True)


class GlobalPrior:
    """GHS-BUILT-H ANBH: nearest-pixel value at a WGS84 point (windowed reads)."""

    def __init__(self, path):
        self.ds = gdal.Open(path)
        self.band = self.ds.GetRasterBand(1)
        self.gt = self.ds.GetGeoTransform()
        self.nodata = self.band.GetNoDataValue()
        self.w, self.h = self.ds.RasterXSize, self.ds.RasterYSize
        self.tr = transformer_for(self.ds, path)

    def sample(self, lon, lat):
        x, y = self.tr.transform(lon, lat)
        ci = int((x - self.gt[0]) / self.gt[1])
        ri = int((y - self.gt[3]) / self.gt[5])
        if not (0 <= ci < self.w and 0 <= ri < self.h):
            return None
        v = float(self.band.ReadAsArray(ci, ri, 1, 1)[0, 0])
        if not math.isfinite(v) or v >= ANBH_MAX_VALID:
            return None
        if self.nodata is not None and v == self.nodata:
            return None
        return v


class RegionalHeights:
    """Regional 1 m relative-height raster held fully in RAM (a Praha-sized
    mosaic is 37 501 x 28 000 F32 ~ 4.2 GB; zonal over VRT windows would pay
    tile decompression per footprint)."""

    def __init__(self, path):
        ds = gdal.Open(path)
        self.gt = ds.GetGeoTransform()
        self.w, self.h = ds.RasterXSize, ds.RasterYSize
        self.tr = transformer_for(ds, path)
        band = ds.GetRasterBand(1)
        self.arr = band.ReadAsArray().astype(np.float32, copy=False)
        nodata = band.GetNoDataValue()
        if nodata is not None:
            self.arr[self.arr == nodata] = np.nan

    def covers(self, x, y):
        c = (x - self.gt[0]) / self.gt[1]
        r = (y - self.gt[3]) / self.gt[5]
        return 0 <= c < self.w and 0 <= r < self.h

    def zonal_measured_mean(self, geom_wgs84):
        """Mean of in-footprint pixels >= MEASURED_MIN_M, or None when the
        coverage guard says the city model does not know this structure."""
        g = shapely.ops.transform(self.tr.transform, geom_wgs84)
        minx, miny, maxx, maxy = g.bounds
        c0 = max(0, int(math.floor((minx - self.gt[0]) / self.gt[1])))
        c1 = min(self.w, int(math.ceil((maxx - self.gt[0]) / self.gt[1])) + 1)
        r0 = max(0, int(math.floor((maxy - self.gt[3]) / self.gt[5])))
        r1 = min(self.h, int(math.ceil((miny - self.gt[3]) / self.gt[5])) + 1)
        if c1 <= c0 or r1 <= r0:
            return None
        # A malformed continent-scale footprint would mesh-grid gigabytes here
        # (gg pass 2) — no real building needs a 4x4 km window; abstain.
        if (c1 - c0) * (r1 - r0) > 16_000_000:
            return None
        window = self.arr[r0:r1, c0:c1]
        xs = self.gt[0] + (np.arange(c0, c1) + 0.5) * self.gt[1]
        ys = self.gt[3] + (np.arange(r0, r1) + 0.5) * self.gt[5]
        xx, yy = np.meshgrid(xs, ys)
        inside = shapely.contains_xy(g, xx.ravel(), yy.ravel()).reshape(window.shape)
        vals = window[inside]
        vals = vals[np.isfinite(vals)]
        measured = vals[vals >= MEASURED_MIN_M]
        if len(measured) < max(COVERAGE_MIN_PX, COVERAGE_MIN_FRAC * int(inside.sum())):
            return None
        return float(measured.mean())


def read_staging(staging_cell_dir):
    """Merge the cell's staging shards in sorted-filename order — the same
    order promotion used and shard_paths() sorts, so an unenriched regeneration
    reproduces the promoted row order."""
    shards = sorted(glob.glob(os.path.join(staging_cell_dir, "obstacles-*.arrow")))
    if not shards:
        return None
    tables = [ipc.open_file(s).read_all() for s in shards]
    return pa.concat_tables(tables).combine_chunks()


def stable_file_identity(stat_result):
    """JSON-safe identity shared with pipeline/lib/obstacle-height-materialization.ts."""
    return {
        "dev": str(stat_result.st_dev),
        "ino": str(stat_result.st_ino),
        "size": str(stat_result.st_size),
        "mtimeNs": str(stat_result.st_mtime_ns),
        "ctimeNs": str(stat_result.st_ctime_ns),
    }


def fsync_directory(path):
    """Persist a completed rename before publishing a proof that depends on it."""
    directory_fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def write_height_proof(cell, out_path, output_stat, inputs_sha256, before, after):
    """Seal the exact inode this worker published, never a later path lookup.

    If a concurrent promotion replaces the path between output publication and
    this proof write, the proof retains the worker inode and therefore reads as
    stale on the next verification instead of blessing the promoted payload.
    """
    proof_path = os.path.join(os.path.dirname(out_path), HEIGHT_PROOF_FILENAME)
    tmp = f"{proof_path}.tmp.{os.getpid()}"
    proof = {
        "version": HEIGHT_PROOF_VERSION,
        "cell": cell,
        "inputsSha256": inputs_sha256,
        "output": stable_file_identity(output_stat),
        "rows": int(sum(after)),
        "beforeTiers": [int(value) for value in before],
        "afterTiers": [int(value) for value in after],
    }
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(proof, f, sort_keys=True, separators=(",", ":"))
        f.write("\n")
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, proof_path)
    fsync_directory(os.path.dirname(proof_path))


def enrich_cell(cell, h3r4_dir, staging_dir, ghsl, regional, proof_inputs_sha256=None):
    out_path = os.path.join(h3r4_dir, cell, "obstacles.arrow")
    if not os.path.exists(out_path):
        print(f"{cell}: not promoted — skipped (promotion stays with the world cutover)", file=sys.stderr)
        return
    staged = read_staging(os.path.join(staging_dir, cell))
    if staged is None:
        raise RuntimeError(f"{cell}: NO STAGING SHARDS — refusing to leave a promoted cell unmaterialized")
    # Row-count TRIPWIRE, not proof: same-count re-ingested staging passes and
    # is regenerated from — which is the desired refresh after a re-ingest. A
    # count mismatch means promotion and staging visibly disagree; stop and let
    # a human decide which is current.
    n_before = ipc.open_file(out_path).read_all().num_rows
    if staged.num_rows != n_before:
        print(
            f"{cell}: staging rows {staged.num_rows} != promoted rows {n_before} — "
            f"staging and promotion have diverged; refusing to regenerate",
            file=sys.stderr,
        )
        sys.exit(1)

    heights = staged.column("height_m").to_numpy(zero_copy_only=False).copy()
    tiers = staged.column("height_tier").to_numpy(zero_copy_only=False).copy()
    lats = staged.column("centroid_lat").to_numpy(zero_copy_only=False)
    lons = staged.column("centroid_lon").to_numpy(zero_copy_only=False)
    before = np.bincount(tiers, minlength=5)
    assert before[3] == 0 and before[4] == 0, "staging must be pristine tiers 0-2"

    in_regional = np.zeros(len(heights), dtype=bool)
    if regional is not None:
        rx, ry = regional.tr.transform(lons, lats)
        for i in range(len(heights)):
            in_regional[i] = regional.covers(rx[i], ry[i])

    wkbs = staged.column("polygon_wkb").to_pylist()
    stats = {"tier3": 0, "tier4": 0, "abstain": 0}
    for i in range(len(heights)):
        tier = int(tiers[i])
        if tier == 0:
            continue
        if in_regional[i]:
            h = regional.zonal_measured_mean(shapely_wkb.loads(wkbs[i]))
            if h is not None:
                heights[i] = min(max(h, TIER3_CLAMP[0]), TIER3_CLAMP[1])
                tiers[i] = 3
                stats["tier3"] += 1
                continue
            stats["abstain"] += 1
        if tier == 2:
            v = ghsl.sample(lons[i], lats[i])
            if v is not None and v >= ANBH_MIN_M:
                heights[i] = min(max(v, TIER4_CLAMP[0]), TIER4_CLAMP[1])
                tiers[i] = 4
                stats["tier4"] += 1

    # Tier-specific validation (gg pass 2): tiers 0/1/2 pass through from
    # staging and may carry any finite positive height — a mapped supertall is
    # taller than every clamp; only the tiers written HERE have ranges.
    assert np.isfinite(heights).all() and (heights > 0).all()
    for t, (lo, hi) in ((3, TIER3_CLAMP), (4, TIER4_CLAMP)):
        th = heights[tiers == t]
        assert len(th) == 0 or (th.min() >= lo and th.max() <= hi)
    out = pa.table(
        {
            "polygon_wkb": staged.column("polygon_wkb"),
            "height_m": pa.array(heights, pa.float32()),
            "centroid_lat": staged.column("centroid_lat"),
            "centroid_lon": staged.column("centroid_lon"),
            "height_tier": pa.array(tiers, pa.uint8()),
        },
        schema=SCHEMA,
    )
    # Unique suffix (gg pass 2): a fixed ".tmp" would let two concurrent runs
    # interleave writes into one inode and publish corruption via either rename.
    tmp = f"{out_path}.tmp.{os.getpid()}"
    with ipc.new_file(tmp, SCHEMA) as w:
        w.write_table(out)
    after = np.bincount(tiers, minlength=5)
    output_fd = os.open(tmp, os.O_RDONLY)
    try:
        os.fsync(output_fd)
        os.replace(tmp, out_path)
        fsync_directory(os.path.dirname(out_path))
        if proof_inputs_sha256 is not None:
            # fstat holds the inode written above even if another process races
            # an atomic replacement of the path immediately after publication.
            write_height_proof(cell, out_path, os.fstat(output_fd), proof_inputs_sha256, before, after)
    finally:
        os.close(output_fd)
    print(
        f"{cell}: {len(heights)} rows; tiers {list(before)} -> {list(after)}; "
        f"tier3 {stats['tier3']}, tier4 {stats['tier4']}, regional-abstain {stats['abstain']}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--h3r4-dir", required=True)
    ap.add_argument("--staging-dir", required=True)
    ap.add_argument("--ghsl", required=True)
    ap.add_argument("--regional")
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--cells")
    group.add_argument("--cells-file")
    ap.add_argument("--proof-manifest")
    args = ap.parse_args()
    if args.cells_file:
        with open(args.cells_file) as f:
            cells = [line.strip() for line in f if line.strip()]
    else:
        cells = args.cells.split(",")
    proof_inputs = None
    if args.proof_manifest:
        with open(args.proof_manifest, encoding="utf-8") as f:
            proof_inputs = json.load(f)
        if set(proof_inputs) != set(cells):
            raise SystemExit("proof manifest cells do not exactly match the requested cells")
        for cell, digest in proof_inputs.items():
            if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
                raise SystemExit(f"{cell}: invalid proof input SHA-256")
    ghsl = GlobalPrior(args.ghsl)
    regional = RegionalHeights(args.regional) if args.regional else None
    for cell in cells:
        enrich_cell(
            cell,
            args.h3r4_dir,
            args.staging_dir,
            ghsl,
            regional,
            proof_inputs[cell] if proof_inputs is not None else None,
        )


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Download Overture building parquets for 1x1-degree tiles in ONE process: the theme's
parquet footers are read once, then every tile scans only the row groups whose bbox
statistics intersect it, so an empty tile costs a statistics check instead of the
~1-2 GB of footers `overturemaps download` fetched per tile (2026-09-03: 5 tiles/min,
55 MB/s, for the mostly empty polar list).

    download-overture-tiles.py TILE_LIST PARQUET_DIR [INGESTED_LIST]

Writes PARQUET_DIR/<tile>.parquet (same rows and columns as the CLI wrote: rows whose bbox
intersects the tile's degree square); skips tiles already cached or listed in INGESTED_LIST
(an ingested tile's parquet is deleted on purpose and never re-fetched). Tiles of one
latitude row are scanned as one strip of up to BATCH_TILES and split in memory, so the
per-scan cost (a statistics pass over every row group) is paid once per strip."""
import os
import sys
import time

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.dataset as ds
import pyarrow.fs as pafs
import pyarrow.parquet as pq

# Every tile of the obstacle manifest was ingested from this release (2026-09-03); mixing
# releases inside one manifest would make neighbouring tiles disagree.
RELEASE = "2026-08-19.0"
DATASET_PATH = f"overturemaps-us-west-2/release/{RELEASE}/theme=buildings/type=building/"
# Strip width: a dense European tile is ~100 MB in Arrow, so ten of them stay well under
# 2 GB in memory while an empty polar row still amortises the scan tenfold.
BATCH_TILES = 10


def tile_bbox(tile):
    """N50E014 -> (14, 50, 15, 51) as (xmin, ymin, xmax, ymax)."""
    lat = int(tile[1:3]) * (-1 if tile[0] == "S" else 1)
    lon = int(tile[4:7]) * (-1 if tile[3] == "W" else 1)
    return lon, lat, lon + 1, lat + 1


def bbox_filter(xmin, ymin, xmax, ymax):
    return ((pc.field("bbox", "xmin") < xmax) & (pc.field("bbox", "xmax") > xmin)
            & (pc.field("bbox", "ymin") < ymax) & (pc.field("bbox", "ymax") > ymin))


def strips(tiles):
    """Consecutive tiles of one latitude row, at most BATCH_TILES per strip."""
    strip = []
    for tile in sorted(tiles):
        if strip and (tile[:3] != strip[0][:3] or len(strip) == BATCH_TILES):
            yield strip
            strip = []
        strip.append(tile)
    if strip:
        yield strip


def main(list_path, parquet_dir, ingested_path=None):
    tiles = [line.strip() for line in open(list_path) if line.strip()]
    ingested = set()
    if ingested_path and os.path.exists(ingested_path):
        ingested = {line.strip() for line in open(ingested_path)}
    todo = [t for t in tiles if t not in ingested
            and not (os.path.exists(f"{parquet_dir}/{t}.parquet")
                     and os.path.getsize(f"{parquet_dir}/{t}.parquet") > 0)]
    print(f"[overture-tiles] {len(tiles)} selected, {len(todo)} to fetch", flush=True)
    if not todo:
        return 0
    started = time.time()
    s3 = pafs.S3FileSystem(anonymous=True, region="us-west-2")
    dataset = ds.dataset(DATASET_PATH, filesystem=s3, format="parquet")
    fragments = list(dataset.get_fragments())
    for fragment in fragments:
        fragment.ensure_complete_metadata()
    print(f"[overture-tiles] footers of {len(fragments)} files read in "
          f"{time.time() - started:.0f} s", flush=True)
    indexed = ds.FileSystemDataset(fragments, dataset.schema, dataset.format, s3)
    failed = 0
    for strip in strips(todo):
        boxes = [tile_bbox(t) for t in strip]
        try:
            rows = indexed.to_table(filter=bbox_filter(
                min(b[0] for b in boxes), min(b[1] for b in boxes),
                max(b[2] for b in boxes), max(b[3] for b in boxes)))
        except Exception as error:  # one strip must not end the pass; the next run retries
            failed += len(strip)
            print(f"[overture-tiles] {strip[0]}..{strip[-1]} FAILED: {error}",
                  file=sys.stderr, flush=True)
            continue
        for tile, box in zip(strip, boxes):
            out = f"{parquet_dir}/{tile}.parquet"
            tmp = out + ".dl"
            table = ds.dataset(rows).to_table(filter=bbox_filter(*box))
            pq.write_table(table, tmp)
            os.replace(tmp, out)
            print(f"[overture-tiles] {tile} ({table.num_rows} rows, "
                  f"{os.path.getsize(out) // 1024} KiB)", flush=True)
    print(f"[overture-tiles] finished: {len(todo) - failed}/{len(todo)} fetched, "
          f"{failed} failed, {time.time() - started:.0f} s", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    sys.exit(main(*sys.argv[1:]))

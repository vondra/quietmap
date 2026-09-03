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
import ctypes
import os
import sys
import time
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.dataset as ds
import pyarrow.fs as pafs
import pyarrow.parquet as pq

# Every tile of the obstacle manifest was ingested from this release (2026-09-03); mixing
# releases inside one manifest would make neighbouring tiles disagree.
RELEASE = "2026-08-19.0"
DATASET_PATH = f"overturemaps-us-west-2/release/{RELEASE}/theme=buildings/type=building/"
# Strip width: a dense tile is hundreds of MB in Arrow (three strips of ten reached 16 GiB after
# 123 tiles on 2026-09-04); four tiles per strip keep three strips in flight under a few GB
# while an empty polar row still amortises the statistics pass fourfold.
BATCH_TILES = 4
# One strip waits on S3 round trips per row group: measured 2026-09-03 at 21 MB/s of a 55 MB/s
# link and 61 % of one core; three strips in flight fill the link.
STRIP_WORKERS = 3
# Arrow's default (jemalloc) pool kept strip buffers: 26 GB RSS after 800 tiles (measured), so
# the process uses the system allocator and trims it after every strip. Past this size the run
# still ends with exit 3 and the caller starts a fresh process; cached tiles are skipped.
RSS_RESTART_BYTES = 24 << 30
EXIT_RESTART = 3
# The columns ingest-overture-obstacles.py reads (plus id for audits); the theme's other 16
# columns (names, sources, roof attributes) were 90 % of a strip's memory and transfer.
COLUMNS = ["id", "geometry", "bbox", "height", "num_floors", "class", "subtype", "is_underground"]


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
    pa.set_memory_pool(pa.system_memory_pool())
    s3 = pafs.S3FileSystem(anonymous=True, region="us-west-2")
    dataset = ds.dataset(DATASET_PATH, filesystem=s3, format="parquet")
    fragments = list(dataset.get_fragments())
    for fragment in fragments:
        fragment.ensure_complete_metadata()
    print(f"[overture-tiles] footers of {len(fragments)} files read in "
          f"{time.time() - started:.0f} s", flush=True)
    indexed = ds.FileSystemDataset(fragments, dataset.schema, dataset.format, s3)
    def fetch_strip(strip):
        boxes = [tile_bbox(t) for t in strip]
        try:
            rows = indexed.to_table(columns=COLUMNS, filter=bbox_filter(
                min(b[0] for b in boxes), min(b[1] for b in boxes),
                max(b[2] for b in boxes), max(b[3] for b in boxes)))
        except Exception as error:  # one strip must not end the pass; the next run retries
            print(f"[overture-tiles] {strip[0]}..{strip[-1]} FAILED: {error}",
                  file=sys.stderr, flush=True)
            return len(strip)
        for tile, box in zip(strip, boxes):
            out = f"{parquet_dir}/{tile}.parquet"
            tmp = out + ".dl"
            table = ds.dataset(rows).to_table(filter=bbox_filter(*box))
            pq.write_table(table, tmp)
            os.replace(tmp, out)
            print(f"[overture-tiles] {tile} ({table.num_rows} rows, "
                  f"{os.path.getsize(out) // 1024} KiB)", flush=True)
        return 0

    def rss_bytes():
        return int(open("/proc/self/statm").read().split()[1]) * os.sysconf("SC_PAGE_SIZE")

    failed = 0
    pending = list(strips(todo))
    restart = False
    with ThreadPoolExecutor(max_workers=STRIP_WORKERS) as executor:
        inflight = set()
        while inflight or (pending and not restart):
            while pending and not restart and len(inflight) < STRIP_WORKERS:
                inflight.add(executor.submit(fetch_strip, pending.pop(0)))
            done, inflight = wait(inflight, return_when=FIRST_COMPLETED)
            failed += sum(future.result() for future in done)
            pa.default_memory_pool().release_unused()
            ctypes.CDLL("libc.so.6").malloc_trim(0)
            if not restart and rss_bytes() > RSS_RESTART_BYTES:
                restart = True
                print(f"[overture-tiles] {rss_bytes() >> 30} GiB resident, restarting after the "
                      f"strips in flight ({sum(len(s) for s in pending)} tiles left)", flush=True)
    left = sum(len(s) for s in pending)
    print(f"[overture-tiles] finished: {len(todo) - failed - left}/{len(todo)} fetched, "
          f"{failed} failed, {left} left for a fresh process, {time.time() - started:.0f} s",
          flush=True)
    if left:
        return EXIT_RESTART
    return 1 if failed else 0


if __name__ == "__main__":
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    sys.exit(main(*sys.argv[1:]))

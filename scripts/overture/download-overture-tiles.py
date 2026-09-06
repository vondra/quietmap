#!/usr/bin/env python3
"""Stream bbox-selected Overture buildings into resumable one-degree source Parquets."""
import ctypes
import os
import sys
import time
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from contextlib import ExitStack

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.dataset as ds
import pyarrow.fs as pafs
import pyarrow.parquet as pq

# The whole parquet cache comes from this one release so neighbouring tiles never
# disagree; a release bump is a full refetch, never a mix.
RELEASE = "2026-08-19.0"
DATASET_PATH = f"overturemaps-us-west-2/release/{RELEASE}/theme=buildings/type=building/"
# Four adjacent tiles share a footer-statistics scan without retaining a full strip.
BATCH_TILES = 4
# Coalesce small scan pieces; each tile retains less than 64 MiB plus its incoming piece.
TILE_BUFFER_BYTES = 64 << 20
# Five independent streams overlap S3 latency; each disables speculative buffering.
STRIP_WORKERS = 5
# Arrow's default (jemalloc) pool kept strip buffers: 26 GB RSS after 800 tiles (measured), so
# the process uses the system allocator and trims it after every strip. Past this size the run
# still ends with exit 3 and the caller starts a fresh process; cached tiles are skipped.
RSS_RESTART_BYTES = 24 << 30
EXIT_RESTART = 3
# The columns build-structures.py reads (plus id for audits); the theme's other 16
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
    for tile in sorted(set(tiles), key=lambda name: (name[:3], tile_bbox(name)[0])):
        if strip and (tile[:3] != strip[0][:3] or len(strip) == BATCH_TILES
                      or tile_bbox(tile)[0] != tile_bbox(strip[-1])[2]):
            yield strip
            strip = []
        strip.append(tile)
    if strip:
        yield strip


def fetch_strip(indexed, strip, parquet_dir):
    """Publish each complete tile only after its streaming scan and writers finish."""
    boxes = [tile_bbox(tile) for tile in strip]
    outputs = [f"{parquet_dir}/{tile}.parquet" for tile in strip]
    counts = [0] * len(strip)
    try:
        scanner = indexed.scanner(
            columns=COLUMNS, filter=bbox_filter(boxes[0][0], boxes[0][1],
                                               boxes[-1][2], boxes[-1][3]),
            batch_readahead=0, fragment_readahead=1,
            fragment_scan_options=ds.ParquetFragmentScanOptions(
                pre_buffer=False, use_buffered_stream=True),
        )
        with ExitStack() as opened:
            writers = [opened.enter_context(pq.ParquetWriter(out + ".dl", scanner.projected_schema))
                       for out in outputs]
            buffered_tables = [[] for _ in outputs]
            buffered_bytes = [0] * len(outputs)

            def flush_tile(index):
                if buffered_tables[index]:
                    writers[index].write_table(pa.concat_tables(buffered_tables[index]))
                    buffered_tables[index].clear()
                    buffered_bytes[index] = 0

            for batch in scanner.to_batches():
                indexed_batch = ds.dataset(batch)
                for index, box in enumerate(boxes):
                    table = indexed_batch.to_table(filter=bbox_filter(*box))
                    if table.num_rows:
                        buffered_tables[index].append(table)
                        buffered_bytes[index] += table.nbytes
                        counts[index] += table.num_rows
                        if buffered_bytes[index] >= TILE_BUFFER_BYTES:
                            flush_tile(index)
            for index in range(len(outputs)):
                flush_tile(index)
        for tile, out, count in zip(strip, outputs, counts):
            os.replace(out + ".dl", out)
            print(f"[overture-tiles] {tile} ({count} rows, "
                  f"{os.path.getsize(out) // 1024} KiB)", flush=True)
        return 0
    except Exception as error:  # Incomplete .dl files are never accepted as cached tiles.
        print(f"[overture-tiles] {strip[0]}..{strip[-1]} FAILED: {error}",
              file=sys.stderr, flush=True)
        return len(strip)


def main(list_path, parquet_dir):
    with open(list_path) as selected:
        tiles = [line.strip() for line in selected if line.strip()]
    todo = [t for t in tiles
            if not (os.path.exists(f"{parquet_dir}/{t}.parquet")
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

    def rss_bytes():
        with open("/proc/self/statm") as status:
            return int(status.read().split()[1]) * os.sysconf("SC_PAGE_SIZE")

    failed = 0
    pending = list(strips(todo))
    restart = False
    with ThreadPoolExecutor(max_workers=STRIP_WORKERS) as executor:
        inflight = set()
        while inflight or (pending and not restart):
            while pending and not restart and len(inflight) < STRIP_WORKERS:
                inflight.add(executor.submit(fetch_strip, indexed, pending.pop(0), parquet_dir))
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
    if len(sys.argv) != 3:
        sys.exit("usage: download-overture-tiles.py TILE_LIST PARQUET_DIR")
    sys.exit(main(*sys.argv[1:]))

#!/usr/bin/env python3
"""Build the one per-z9 structure table from OSM, Overture and measured heights."""

import argparse
import json
import multiprocessing as mp
import os
from pathlib import Path
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))
import qmgrid
from structure_inputs import GlobalPrior, RegionalHeights, read_overture_parquet
from structure_freshness import file_identity, input_fingerprint
from structure_inventory import overture_sources, world_squares
from structure_merge import build_square, structure_is_fresh
from worker_jobs import available_memory_bytes, cpu_jobs, fit_jobs

# 20 spawn workers peaked the 60 GiB world-build cgroup at ~3 GiB RSS each (2026-09-12).
WORKER_BYTES = 4 << 30

_PREPARED = None
_OVERTURE = None
_GHSL = None
_REGIONAL = None


def build_one(name, prepared_dir, overture_parquet, ghsl, regional):
    square = qmgrid.parse_square_name(name)
    if square is None:
        raise ValueError(f"not a square name: {name}")
    name = qmgrid.square_name(*square)
    square_dir = os.path.join(prepared_dir, name)
    ovt_inputs = [file_identity(source)
                  for _, _, source in overture_sources(overture_parquet, square)]
    inputs = input_fingerprint(square_dir, ovt_inputs, ghsl, regional)
    if structure_is_fresh(os.path.join(square_dir, "structures.arrow"), inputs):
        return None
    ovt, ovt_inputs = read_overture_parquet(overture_parquet, square)
    return build_square(name, prepared_dir, ovt, ovt_inputs, ghsl, regional)


def _init_worker(prepared_dir, overture_parquet, ghsl_path, regional_path):
    global _PREPARED, _OVERTURE, _GHSL, _REGIONAL
    _PREPARED = prepared_dir
    _OVERTURE = overture_parquet
    _GHSL = GlobalPrior(ghsl_path)
    _REGIONAL = RegionalHeights(regional_path) if regional_path else None


def _process_name(name):
    return build_one(name, _PREPARED, _OVERTURE, _GHSL, _REGIONAL)


def accumulate(census, totals):
    if census is None:
        totals["fresh_skip"] += 1
        return
    totals["built"] += 1
    for key in ("osm_only", "both", "overture_only", "walls", "rows", "bytes"):
        totals[key] += census[key]


def emit_progress(done, total, totals):
    print(
        f"[build-structures] {done}/{total}: built={totals['built']} "
        f"fresh-skip={totals['fresh_skip']} both={totals['both']} "
        f"osm-only={totals['osm_only']} overture-only={totals['overture_only']} "
        f"walls={totals['walls']} rows={totals['rows']} bytes={totals['bytes']}",
        flush=True,
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--prepared-dir", required=True)
    ap.add_argument("--overture-parquet", required=True)
    ap.add_argument("--ghsl", required=True)
    ap.add_argument("--regional")
    group = ap.add_mutually_exclusive_group()
    group.add_argument("--squares")
    group.add_argument("--squares-file")
    ap.add_argument("--census-log", help="append one JSON line per built square")
    ap.add_argument("--jobs", type=int, default=None,
                    help="Worker cap (default: all CPUs that fit memory; 1 keeps the serial path)")
    args = ap.parse_args()
    if args.jobs is not None and args.jobs < 1:
        raise ValueError("--jobs must be >= 1")

    if args.squares_file:
        with open(args.squares_file) as source:
            squares = [line.strip() for line in source if line.strip()]
    elif args.squares:
        squares = args.squares.split(",")
    else:
        # Misnamed prepared inputs must not disappear behind complete output coverage.
        for path in (Path(args.prepared_dir) / "z9").glob("*/*"):
            if not path.is_dir():
                continue
            name = f"z9/{path.parent.name}/{path.name}"
            square = qmgrid.parse_square_name(name)
            if square is None or qmgrid.square_name(*square) != name:
                raise ValueError(f"Noncanonical prepared square: {path}")
        squares = world_squares(args.overture_parquet)
    requested = cpu_jobs() if args.jobs is None else args.jobs
    jobs = min(len(squares), fit_jobs(requested, WORKER_BYTES)) if squares else 1
    print(
        f"[build-structures] jobs={jobs} requested={requested} "
        f"memory_bytes={available_memory_bytes()} worker_bytes={WORKER_BYTES}",
        flush=True,
    )
    census_log = open(args.census_log, "a", encoding="utf-8") if args.census_log else None
    totals = {"built": 0, "fresh_skip": 0, "osm_only": 0, "both": 0,
              "overture_only": 0, "walls": 0, "rows": 0, "bytes": 0}

    def consume(census):
        accumulate(census, totals)
        if census is not None and census_log is not None:
            census_log.write(json.dumps(census) + "\n")
            census_log.flush()

    if jobs == 1:
        ghsl = GlobalPrior(args.ghsl)
        regional = RegionalHeights(args.regional) if args.regional else None
        for done, name in enumerate(squares, start=1):
            consume(build_one(name, args.prepared_dir, args.overture_parquet, ghsl, regional))
            if done % 1000 == 0 or done == len(squares):
                emit_progress(done, len(squares), totals)
    else:
        context = mp.get_context("spawn")
        with context.Pool(
            processes=jobs,
            initializer=_init_worker,
            initargs=(args.prepared_dir, args.overture_parquet, args.ghsl, args.regional),
        ) as pool:
            for done, census in enumerate(pool.imap_unordered(_process_name, squares, chunksize=8), start=1):
                consume(census)
                if done % 1000 == 0 or done == len(squares):
                    emit_progress(done, len(squares), totals)
    if census_log is not None:
        census_log.close()
    print(f"[build-structures] DONE {totals}", flush=True)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Build the one per-z9 structure table from OSM, Overture and measured heights."""

import argparse
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))
import qmgrid
from structure_inputs import GlobalPrior, RegionalHeights, read_overture_parquet
from structure_freshness import file_identity, input_fingerprint
from structure_inventory import overture_sources, world_squares
from structure_merge import build_square, structure_is_fresh


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
    args = ap.parse_args()

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
    ghsl = GlobalPrior(args.ghsl)
    regional = RegionalHeights(args.regional) if args.regional else None
    census_log = open(args.census_log, "a", encoding="utf-8") if args.census_log else None
    totals = {"built": 0, "fresh_skip": 0, "osm_only": 0, "both": 0,
              "overture_only": 0, "walls": 0, "rows": 0, "bytes": 0}
    for done, name in enumerate(squares, start=1):
        square = qmgrid.parse_square_name(name)
        if square is None:
            ap.error(f"not a square name: {name}")
        name = qmgrid.square_name(*square)
        square_dir = os.path.join(args.prepared_dir, name)
        ovt_inputs = [file_identity(source)
                      for _, _, source in overture_sources(args.overture_parquet, square)]
        inputs = input_fingerprint(square_dir, ovt_inputs, ghsl, regional)
        if structure_is_fresh(os.path.join(square_dir, "structures.arrow"), inputs):
            census = None
        else:
            ovt, ovt_inputs = read_overture_parquet(args.overture_parquet, square)
            census = build_square(name, args.prepared_dir, ovt, ovt_inputs, ghsl, regional)
        if census is None:
            totals["fresh_skip"] += 1
        else:
            totals["built"] += 1
            for k in ("osm_only", "both", "overture_only", "walls", "rows", "bytes"):
                totals[k] += census[k]
            if census_log is not None:
                census_log.write(json.dumps(census) + "\n")
                census_log.flush()
        if done % 1000 == 0 or done == len(squares):
            print(
                f"[build-structures] {done}/{len(squares)}: built={totals['built']} "
                f"fresh-skip={totals['fresh_skip']} both={totals['both']} "
                f"osm-only={totals['osm_only']} overture-only={totals['overture_only']} "
                f"walls={totals['walls']} rows={totals['rows']} bytes={totals['bytes']}",
                flush=True,
            )
    if census_log is not None:
        census_log.close()
    print(f"[build-structures] DONE {totals}", flush=True)


if __name__ == "__main__":
    main()

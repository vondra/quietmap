#!/usr/bin/env python3
"""Build the one per-z9 structure table from OSM, Overture and measured heights."""

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lib"))
import qmgrid
from structure_inputs import GlobalPrior, RegionalHeights, read_overture_parquet
from structure_merge import build_square


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--prepared-dir", required=True)
    ap.add_argument("--overture-parquet", required=True)
    ap.add_argument("--ghsl", required=True)
    ap.add_argument("--regional")
    group = ap.add_mutually_exclusive_group(required=True)
    group.add_argument("--squares")
    group.add_argument("--squares-file")
    ap.add_argument("--census-log", help="append one JSON line per built square")
    args = ap.parse_args()

    squares = ([line.strip() for line in open(args.squares_file) if line.strip()]
               if args.squares_file else args.squares.split(","))
    ghsl = GlobalPrior(args.ghsl)
    regional = RegionalHeights(args.regional) if args.regional else None
    census_log = open(args.census_log, "a", encoding="utf-8") if args.census_log else None
    totals = {"built": 0, "fresh_skip": 0, "osm_only": 0, "both": 0,
              "overture_only": 0, "walls": 0, "rows": 0, "bytes": 0}
    for done, name in enumerate(squares, start=1):
        square = qmgrid.parse_square_name(name)
        if square is None:
            ap.error(f"not a square name: {name}")
        x, y = square
        ovt, ovt_mtime = read_overture_parquet(args.overture_parquet, (x, y))
        census = build_square(name, args.prepared_dir, ovt, ovt_mtime, ghsl, regional)
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

#!/usr/bin/env python3
"""Benchmark report: reads the JSON lines of cold.py runs (fast, and optionally --exact) and the
dev4 reference answers, and prints per point the cold times (median of the repeats), bytes and
files, the fast-versus-exact difference per layer and the difference to dev4 per layer.

usage: report.py RUNS.jsonl [--exact EXACT.jsonl] [--dev4 DIR_OF_<point>.summary.json.gz]
"""
import argparse
import gzip
import json
import pathlib
import statistics


def load(path):
    runs = {}
    for line in pathlib.Path(path).read_text().splitlines():
        record = json.loads(line)
        runs.setdefault(record["point"], []).append(record)
    return runs


def dev4_layers(directory, point):
    path = pathlib.Path(directory) / f"{point}.summary.json.gz"
    if not path.exists():
        return {}
    answer = json.loads(gzip.decompress(path.read_bytes()))
    return {source["source_type"]: source["lden"] for source in answer["sources"]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("runs")
    parser.add_argument("--exact")
    parser.add_argument("--dev4")
    arguments = parser.parse_args()
    runs = load(arguments.runs)
    exact = load(arguments.exact) if arguments.exact else {}
    header = f"{'point':18} {'first ms':>8} {'full ms':>8} {'first MB':>8} {'full MB':>8} {'files':>5}  layers (fast | fast-exact | fast-dev4)"
    print(header)
    for point, records in runs.items():
        first = statistics.median(r["first_ms"] for r in records)
        full = statistics.median(r["full_ms"] for r in records)
        last = records[-1]
        cells = []
        dev4 = dev4_layers(arguments.dev4, point) if arguments.dev4 else {}
        for layer, level in sorted(last["layers"].items()):
            text = f"{layer} {level:.1f}"
            if point in exact and layer in exact[point][-1]["layers"]:
                text += f" | {level - exact[point][-1]['layers'][layer]:+.2f}"
            if layer in dev4 and dev4[layer] is not None and dev4[layer] > -50:
                text += f" | {level - dev4[layer]:+.1f}"
            cells.append(text)
        print(f"{point:18} {first:8.0f} {full:8.0f} {last['first_mb']:8.1f} {last['full_mb']:8.1f} {last['full_files']:5d}  "
              + "; ".join(cells))


if __name__ == "__main__":
    main()

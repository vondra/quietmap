#!/usr/bin/env python3
"""Benchmark report: reads the JSON lines of cold.py runs (fast, and optionally --exact) and the
dev4 reference answers, and prints per point the cold times (median of the repeats) against the
budget, bytes and files, the fast-versus-exact difference per layer and the difference to dev4
per layer; then the error of every fast layer answer against exact (p95 and max over all runs,
the budget being 0.1 and 0.3 dB).

usage: report.py RUNS.jsonl [--exact EXACT.jsonl] [--dev4 DIR_OF_<point>.summary.json.gz]
                 [--disk nvme|hdd]
"""
import argparse
import gzip
import json
import pathlib
import statistics

# First and full answer (ms) per disk (ARCHITECTURE.md, Budgets).
BUDGET_MS = {"nvme": (200, 500), "hdd": (500, 1500)}
ERROR_BUDGET_DB = (0.1, 0.3)


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
    parser.add_argument("--disk", choices=sorted(BUDGET_MS), default="nvme")
    arguments = parser.parse_args()
    runs = load(arguments.runs)
    exact = load(arguments.exact) if arguments.exact else {}
    first_budget, full_budget = BUDGET_MS[arguments.disk]
    print(f"{'point':18} {'first ms':>9} {'full ms':>9} {'first MB':>8} {'full MB':>8} {'files':>5}  "
          "layers (fast | fast-exact | fast-dev4)")
    errors = []
    for point, records in runs.items():
        first = statistics.median(r["first_ms"] for r in records)
        full = statistics.median(r["full_ms"] for r in records)
        mark = lambda value, budget: f"{value:8.0f}{'*' if value > budget else ' '}"
        last = records[-1]
        reference = exact[point][-1]["layers"] if point in exact else {}
        for record in records:
            for layer, level in record["layers"].items():
                if layer in reference:
                    errors.append((abs(level - reference[layer]), point, layer))
        cells = []
        dev4 = dev4_layers(arguments.dev4, point) if arguments.dev4 else {}
        for layer, level in sorted(last["layers"].items()):
            text = f"{layer} {level:.1f}"
            if layer in reference:
                text += f" | {level - reference[layer]:+.2f}"
            if layer in dev4 and dev4[layer] is not None and dev4[layer] > -50:
                text += f" | {level - dev4[layer]:+.1f}"
            cells.append(text)
        print(f"{point:18} {mark(first, first_budget)} {mark(full, full_budget)} "
              f"{last['first_mb']:8.1f} {last['full_mb']:8.1f} {last['full_files']:5d}  "
              + "; ".join(cells))
    print(f"* over the {arguments.disk} budget ({first_budget} / {full_budget} ms)")
    if errors:
        errors.sort()
        p95 = errors[max(0, -(-95 * len(errors) // 100) - 1)]
        worst = errors[-1]
        verdict = "within" if p95[0] <= ERROR_BUDGET_DB[0] and worst[0] <= ERROR_BUDGET_DB[1] else "OVER"
        print(f"error against exact over {len(errors)} layer answers: p95 {p95[0]:.3f} dB, "
              f"max {worst[0]:.3f} dB ({worst[1]} {worst[2]}): {verdict} the budget "
              f"({ERROR_BUDGET_DB[0]} / {ERROR_BUDGET_DB[1]} dB)")


if __name__ == "__main__":
    main()

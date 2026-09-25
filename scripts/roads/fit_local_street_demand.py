"""Fit S2p to DfT MCU counts in training squares only, with grouped five-fold CV by z9 square."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import subprocess
import tempfile
import zipfile

import numpy as np
import scipy
from scipy.optimize import least_squares

REPO = Path(__file__).resolve().parents[2]
PARAMETER_NAMES = ("residentialUrban", "unclassifiedUrban", "rural", "throughFactor", "demandScale")


def square_hash(prefix, x, y):
    return int(hashlib.sha256(f"{prefix}/{x}/{y}".encode()).hexdigest()[:8], 16) % 5


def training_points(archive):
    """Discard spatial holdouts before reading their traffic values; latest manual non-2020 count."""
    points = {}
    with zipfile.ZipFile(archive) as source:
        with source.open("dft_traffic_counts_aadf.csv") as data:
            for row in csv.DictReader(io.TextIOWrapper(data, encoding="utf-8-sig")):
                if row["road_category"] != "MCU" or row["estimation_method"] != "Counted":
                    continue
                year = int(row["year"])
                if not 2015 <= year <= 2025 or year == 2020:
                    continue
                lat, lon = float(row["latitude"]), float(row["longitude"])
                x = math.floor((lon + 180) / 360 * 512)
                y = math.floor((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) * 256)
                if square_hash("qm-holdout-v1", x, y) == 0:
                    continue
                count = int(row["all_motor_vehicles"] or 0)
                if count <= 0:
                    continue
                identity = row["count_point_id"]
                if identity not in points or points[identity]["year"] < year:
                    points[identity] = dict(id=identity, x=x, y=y, lat=lat, lon=lon, year=year, aadf=count)
    return list(points.values())


def fit(rows):
    if not rows or any(square_hash("qm-holdout-v1", r["x"], r["y"]) == 0 for r in rows):
        raise ValueError("fit requires nonempty training squares only")
    cells = np.array([0 if r["builtUp"] == 2 and r["roadClass"] == 5 else
                      1 if r["builtUp"] == 2 else 2 for r in rows])
    if set(cells) != {0, 1, 2}:
        raise ValueError("all three S2p cells need training data")
    trips = np.array([r["trips"] for r in rows])
    through = np.array([r["through"] for r in rows])
    observed = np.array([r["aadf"] for r in rows])
    initial = np.array([math.log(400)] * 3 + [0.0, 0.0])
    result = least_squares(lambda p: np.log(np.exp(p[cells] + through * p[3]) + np.exp(p[4]) * trips)
                           - np.log(observed), initial)
    if not result.success or not np.isfinite(result.x).all():
        raise ValueError(f"S2p fit failed: {result.message}")
    return dict(zip(PARAMETER_NAMES, np.exp(result.x).tolist()))


def predict(parameters, rows):
    return np.array([(parameters["residentialUrban"] if r["roadClass"] == 5 else parameters["unclassifiedUrban"])
                     if r["builtUp"] == 2 else parameters["rural"] for r in rows]) * np.array([
        parameters["throughFactor"] if r["through"] else 1 for r in rows]) + parameters["demandScale"] * np.array([
            r["trips"] for r in rows])


def cross_validate(rows):
    folds = np.array([square_hash("w3-cv", r["x"], r["y"]) for r in rows])
    predictions = np.zeros(len(rows))
    counts = []
    for fold in range(5):
        train = [r for r, f in zip(rows, folds) if f != fold]
        validate = [r for r, f in zip(rows, folds) if f == fold]
        if not validate:
            raise ValueError(f"empty CV fold {fold}")
        predictions[folds == fold] = predict(fit(train), validate)
        counts.append(len(validate))
    error = 10 * np.log10(predictions / np.array([r["aadf"] for r in rows]))
    return dict(fold_counts=counts, median_db=float(np.median(error)), mae_db=float(np.mean(np.abs(error))),
                share_over_5_db=float(np.mean(np.abs(error) > 5)))


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dft-zip", type=Path, required=True)
    parser.add_argument("--prepared-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--jobs", type=int, choices=range(1, 9), default=4)
    parser.add_argument("--write", action="store_true", help="replace the production parameter JSON")
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    provenance = json.loads(Path(str(args.dft_zip) + ".provenance.json").read_text())
    if sha256(args.dft_zip) != provenance["sha256"]:
        raise ValueError("DfT archive differs from its acquisition provenance")
    points = training_points(args.dft_zip)
    groups = {}
    for point in points:
        groups.setdefault((point["x"], point["y"]), []).append(point)

    def extract(item):
        (x, y), queries = item
        with tempfile.TemporaryDirectory(dir=args.output_dir) as scratch:
            query, output = Path(scratch) / "queries.json", Path(scratch) / "features.json"
            query.write_text(json.dumps(queries))
            subprocess.run([str(REPO / "pipeline/node_modules/.bin/tsx"), str(REPO / "pipeline/local-street-features.ts"),
                            str(args.prepared_dir / f"z9/{x}/{y}"), str(query), str(output)], check=True)
            result = json.loads(output.read_text())
        print(f"z9/{x}/{y}: {len(result['features'])}/{len(queries)} clean points", flush=True)
        return dict(square=f"z9/{x}/{y}", **result)

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        extracted = list(pool.map(extract, sorted(groups.items())))
    by_id = {p["id"]: p for p in points}
    rows = [{**r, "aadf": by_id[r["id"]]["aadf"], "year": by_id[r["id"]]["year"]}
            for square in extracted for r in square["features"]]
    features_path = args.output_dir / "training-features.json"
    features_path.write_text(json.dumps(rows))
    identities = [{k: v for k, v in sq.items() if k != "features"} for sq in extracted]
    (args.output_dir / "input-identities.json").write_text(json.dumps(identities, indent=2) + "\n")
    result = dict(parameters=fit(rows), provenance=dict(
        source_url=provenance["url"], source_sha256=provenance["sha256"], fetched_utc=provenance["fetched_utc"],
        licence=provenance["licence"], attribution="Contains public sector information licensed under OGL v3.0; © Crown copyright, Department for Transport",
        counts="latest positive manual MCU AADF 2015–2025 excluding 2020; residential/unclassified",
        matching="nearest row <=12 m; no other class within 20 m; >30 m from square edge; source 11",
        model="S2p: T[cell] * (through ? c : 1) + k * max_street(routed trips); structures_v5 storeys",
        holdout="qm-holdout-v1/{x}/{y}: sha256 first 8 hex digits modulo 5 == 0 excluded before extraction",
        objective="least squares on ln(AADF); positive parameters via exponential transform",
        cv="five folds by sha256(w3-cv/{x}/{y}) first 8 hex digits modulo 5; no model selection",
        training_counts=len(rows), training_squares=len({(r["x"], r["y"]) for r in rows}),
        scipy_version=scipy.__version__, numpy_version=np.__version__,
        feature_code_sha256=hashlib.sha256(b''.join((REPO / p).read_bytes() for p in (
            'pipeline/local-street-features.ts', 'pipeline/enrich-roads-service-tree.ts',
            'pipeline/lib/service-tree-flow.ts', 'pipeline/lib/service-tree-buildings.ts',
            'pipeline/lib/trip-rates.ts', 'pipeline/lib/country-fleet.ts', 'pipeline/lib/country-fleet.json',
            'pipeline/lib/count-holdout.ts', 'scripts/roads/fit_local_street_demand.py'))).hexdigest(),
        features_sha256=sha256(features_path), inputs_sha256=sha256(args.output_dir / "input-identities.json")),
        cross_validation=cross_validate(rows))
    serialized = json.dumps(result, indent=2) + "\n"
    (args.output_dir / "fit.json").write_text(serialized)
    if args.write:
        (REPO / "pipeline/lib/local-street-demand.json").write_text(serialized)
    print(serialized)


if __name__ == "__main__":
    main()

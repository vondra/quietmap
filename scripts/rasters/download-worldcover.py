#!/usr/bin/env python3
"""Acquire official WorldCover maps and the pinned global CCI IMD background."""
import argparse, os
import boto3
from botocore import UNSIGNED
from botocore.config import Config
from concurrent.futures import ThreadPoolExecutor
import threading
from pathlib import Path
from worldcover_sources import BUCKET, fetch_catalog, validate_source_files, download_cci_source, validate_cci_source


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dest", required=True)
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--catalog-only", action="store_true", help="Refresh official coverage and verify existing source keys/sizes without downloading tiles")
    args = ap.parse_args()
    s3 = boto3.client("s3", region_name="eu-central-1",
                      config=Config(signature_version=UNSIGNED,
                                    retries={"max_attempts": 10}))
    inventory = fetch_catalog(Path(args.dest), s3)
    keys = sorted(inventory.items())
    print(f"tiles: {len(keys)}", flush=True)
    if args.catalog_only:
        validate_source_files(Path(args.dest), inventory)
        validate_cci_source(Path(args.dest))
        return 0
    lock = threading.Lock()
    done = [0]; skipped = [0]; bytes_dl = [0]
    def one(item):
        key, size = item
        local = os.path.join(args.dest, key.rsplit("/", 1)[-1])
        if os.path.isfile(local) and os.path.getsize(local) == size:
            with lock: skipped[0] += 1
            return
        tmp = local + ".part"
        s3.download_file(BUCKET, key, tmp)
        if os.path.getsize(tmp) != size:
            raise RuntimeError(f"short file {key}")
        os.replace(tmp, local)
        with lock:
            done[0] += 1; bytes_dl[0] += size
            if done[0] % 100 == 0:
                print(f"dl={done[0]} skip={skipped[0]} GB={bytes_dl[0]/1e9:.1f}", flush=True)
    with ThreadPoolExecutor(max_workers=args.jobs) as ex:
        list(ex.map(one, keys))
    validate_source_files(Path(args.dest), inventory)
    download_cci_source(Path(args.dest))
    print(f"FINAL dl={done[0]} skip={skipped[0]} GB={bytes_dl[0]/1e9:.1f}", flush=True)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())

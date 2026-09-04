#!/usr/bin/env python3
"""Sync ESA WorldCover v200 2021 3x3-degree Map.tif tiles (unsigned S3, resumable)."""
import argparse, os
import boto3
from botocore import UNSIGNED
from botocore.config import Config
from concurrent.futures import ThreadPoolExecutor
import threading

BUCKET = "esa-worldcover"
PREFIX = "v200/2021/map/"

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dest", required=True)
    ap.add_argument("--jobs", type=int, default=8)
    args = ap.parse_args()
    s3 = boto3.client("s3", region_name="eu-central-1",
                      config=Config(signature_version=UNSIGNED,
                                    retries={"max_attempts": 10}))
    pag = s3.get_paginator("list_objects_v2")
    keys = []
    for page in pag.paginate(Bucket=BUCKET, Prefix=PREFIX):
        for o in page.get("Contents", []):
            if o["Key"].endswith("_Map.tif"):
                keys.append((o["Key"], o["Size"]))
    keys.sort()
    print(f"tiles: {len(keys)}", flush=True)
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
    print(f"FINAL dl={done[0]} skip={skipped[0]} GB={bytes_dl[0]/1e9:.1f}", flush=True)
    return 0

if __name__ == "__main__":
    raise SystemExit(main())

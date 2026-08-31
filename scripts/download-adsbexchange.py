#!/usr/bin/env python3
"""Download adsbexchange free sample days → gzipped readsb day tars, stored
parallel to the adsb.lol cache so the aircraft-extract pipeline reads them
unchanged (identical `traces/{xx}/trace_full_{icao}.json` layout, gzip content).

adsbexchange blocks directory listing, so each day's aircraft set is enumerated
from the 5-minute `readsb-hist` snapshots (every aircraft that transmitted a
position is seen in some snapshot), then every `trace_full_<icao>.json` is
fetched and packed gzipped into `<out>/<year>/<day>/subset.tar`.

Free samples cover the 1st of each month. Resumable: a day whose subset.tar
exists is skipped; a day with mass transient failures is left as `.part`
(not finalized) so a re-run retries it.

  download-adsbexchange.py --last-12 --anchor 2026-06 --out /data/adsbexchange
  download-adsbexchange.py --days 2026-05-01,2026-04-01 --out /data/adsbexchange
"""
from __future__ import annotations
import argparse, os, sys, io, gzip, json, time, tarfile
import concurrent.futures as cf
import requests

try:
    import orjson
    loads = orjson.loads
except Exception:
    loads = json.loads

BASE = "https://samples.adsbexchange.com"
UA = "Mozilla/5.0 (X11; Linux x86_64)"
# One pooled session shared across worker threads — reuses TLS connections
# (urllib3's pool is thread-safe for concurrent GETs), far fewer handshakes
# than per-request urllib and much gentler on Cloudflare's anti-bot.
SESSION = requests.Session()
SESSION.headers["User-Agent"] = UA

def log(msg: str) -> None:
    print(f"[adsbx-dl] {time.strftime('%H:%M:%S')} {msg}", flush=True)

# fetch() return contract: bytes = body (200); b"" = definitively absent (404);
# None = transient failure after retries (429/5xx/network) — caller must NOT
# treat None as "no trace", or a Cloudflare block would silently truncate a day.
def fetch(url: str, timeout: int = 60, retries: int = 5):
    for attempt in range(retries):
        try:
            r = SESSION.get(url, timeout=timeout)
            if r.status_code == 200:
                return r.content
            if r.status_code == 404:
                return b""
            # 403 / 429 / 5xx — back off (caps at 30 s) and retry.
            time.sleep(min(2 ** attempt, 30))
        except Exception:
            time.sleep(min(2 ** attempt, 30))
    return None

def snapshot_times(step_min: int = 5) -> list[str]:
    return [f"{h:02d}{m:02d}00" for h in range(24) for m in range(0, 60, step_min)]

def enumerate_day(day: str, workers: int) -> set[str]:
    """Union of position-having ICAO hexes across the day's 5-min snapshots."""
    y, m, d = day.split("-")
    urls = [f"{BASE}/readsb-hist/{y}/{m}/{d}/{t}Z.json.gz" for t in snapshot_times()]
    hexes: set[str] = set()
    done = 0
    with cf.ThreadPoolExecutor(max_workers=workers) as ex:
        for raw in ex.map(fetch, urls):
            done += 1
            if not raw:  # b"" (404) or None (transient) → skip this snapshot
                continue
            try:
                obj = loads(gzip.decompress(raw))
            except Exception:
                try:
                    obj = loads(raw)
                except Exception:
                    continue
            for a in obj.get("aircraft", []):
                h = a.get("hex")
                if h and not h.startswith("~") and a.get("lat") is not None:
                    hexes.add(h)
            if done % 60 == 0:
                log(f"  {day} enumerate: {done}/{len(urls)} snapshots, {len(hexes)} aircraft")
    return hexes

def download_and_pack(day: str, hexes: list[str], out_tar: str, workers: int):
    """Fetch each trace and stream it gzipped into the day tar (as_completed →
    bounded memory). Returns (packed, missing, errors). A day with mass transient
    errors is left as `.part` (not finalized) so a re-run retries it."""
    y, m, d = day.split("-")
    tmp_tar = out_tar + ".part"
    packed = missing = errors = done = 0
    t0 = time.time()
    with tarfile.open(tmp_tar, "w") as tar, cf.ThreadPoolExecutor(max_workers=workers) as ex:
        futs = {
            ex.submit(fetch, f"{BASE}/traces/{y}/{m}/{d}/{h[-2:]}/trace_full_{h}.json"): h
            for h in hexes
        }
        for fut in cf.as_completed(futs):
            h = futs[fut]
            raw = fut.result()
            done += 1
            if raw is None:
                errors += 1
            elif raw == b"":
                missing += 1
            else:
                gz = gzip.compress(raw)
                info = tarfile.TarInfo(f"traces/{h[-2:]}/trace_full_{h}.json")
                info.size = len(gz)
                tar.addfile(info, io.BytesIO(gz))
                packed += 1
            if done % 5000 == 0:
                rate = done / max(time.time() - t0, 1)
                log(f"  {day} traces: {done}/{len(hexes)} ({packed} ok, {missing} no-trace, {errors} err), {rate:.0f}/s")
    # Mass transient failure ⇒ the day is incomplete. Don't finalize: leaving
    # only `.part` makes the next run re-download it (the skip check keys on
    # subset.tar). A handful of stragglers (<1 %) is acceptable for a noise map.
    if errors > max(50, len(hexes) // 100):
        log(f"  {day} INCOMPLETE: {errors} transient errors — keeping {tmp_tar}, NOT finalizing")
        return packed, missing, errors
    os.replace(tmp_tar, out_tar)
    return packed, missing, errors

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", help="comma-separated YYYY-MM-DD")
    ap.add_argument("--last-12", action="store_true",
                    help="last 12 firsts-of-month ending at --anchor")
    ap.add_argument("--anchor", default=None,
                    help="YYYY-MM — most recent first-of-month to include (with --last-12)")
    ap.add_argument("--out", required=True,
                    help="explicit cache directory for downloaded TAR files")
    ap.add_argument("--workers", type=int, default=12)
    a = ap.parse_args()
    if not a.out.strip():
        ap.error("--out must be a non-empty explicit cache directory")

    if a.last_12:
        if not a.anchor:
            log("ERROR: --last-12 needs --anchor YYYY-MM")
            return 2
        ay, am = (int(x) for x in a.anchor.split("-"))
        days = []
        for i in range(12):
            mm, yy = am - i, ay
            while mm <= 0:
                mm += 12
                yy -= 1
            days.append(f"{yy:04d}-{mm:02d}-01")
    elif a.days:
        days = [s.strip() for s in a.days.split(",") if s.strip()]
    else:
        log("ERROR: pass --days or --last-12 --anchor")
        return 2

    log(f"{len(days)} day(s): {','.join(days)} → {a.out} (workers={a.workers})")
    for day in days:
        ddir = os.path.join(a.out, day.split("-")[0], day)
        os.makedirs(ddir, exist_ok=True)
        out_tar = os.path.join(ddir, "subset.tar")
        if os.path.exists(out_tar) and os.path.getsize(out_tar) > 1_000_000:
            log(f"{day} already packed ({os.path.getsize(out_tar)/1e9:.1f} GB) — skip")
            continue
        t0 = time.time()
        log(f"{day} enumerate aircraft …")
        hexes = sorted(enumerate_day(day, a.workers))
        if not hexes:
            log(f"{day} no aircraft (sample missing?) — skip")
            continue
        log(f"{day} {len(hexes)} aircraft → fetch + pack …")
        packed, missing, errors = download_and_pack(day, hexes, out_tar, a.workers)
        if os.path.exists(out_tar):
            sz = os.path.getsize(out_tar) / 1e9
            log(f"{day} DONE: {packed} packed ({missing} no-trace, {errors} err), {sz:.1f} GB, {time.time()-t0:.0f}s")
    log("all days done")
    return 0

if __name__ == "__main__":
    sys.exit(main())

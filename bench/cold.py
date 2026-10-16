#!/usr/bin/env python3
"""Cold popup benchmark: for every benchmark point, evict every file its click can read from the
page cache (the square files of its tiles of every kind, the flight tracks, the weather table;
posix_fadvise DONTNEED, residency checked with mincore), run `qm-popup` and record the time to the
first and the full answer, bytes and reads, and the levels per layer.

usage: cold.py --prepared DIR --year YYYY [--points bench/points.json] [--only a,b] [--repeat 3]
               [--exact] [--popup target/release/qm-popup]
Prints one JSON line per run. Metadata (directory entries, inodes) stays warm: dropping it needs
root on the host.
"""
import argparse
import ctypes
import json
import math
import mmap
import os
import pathlib
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Every tile kind a click reads (`tiles::Kind`).
KINDS = ("terrain", "terrain-fine", "obstacles", "sources", "aircraft", "aircraft-far", "aircraft-events")
EVICT_RINGS = 4
LIBC = ctypes.CDLL("libc.so.6", use_errno=True)
LIBC.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_ubyte)]


def tile_of(lat, lon, zoom=12):
    n = 2 ** zoom
    x = int((lon + 180.0) / 360.0 * n) % n
    y = int((1.0 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2.0 * n)
    return x, min(max(y, 0), n - 1)


def square_files(year_root, lat, lon):
    """The files of every kind of the z9 squares holding the tiles within EVICT_RINGS of the click's
    (`tiles::square`: `<year root>/<kind>/<x9>_<y9>.<kind>`)."""
    cx, cy = tile_of(lat, lon)
    n = 4096
    squares = set()
    for dy in range(-EVICT_RINGS, EVICT_RINGS + 1):
        for dx in range(-EVICT_RINGS, EVICT_RINGS + 1):
            x, y = (cx + dx) % n, cy + dy
            if 0 <= y < n:
                squares.add((x >> 3, y >> 3))
    for kind in KINDS:
        for x9, y9 in sorted(squares):
            path = year_root / kind / f"{x9}_{y9}.{kind}"
            if path.exists():
                yield path


def click_files(prepared, year, lat, lon):
    """Every file a click at (lat, lon) can read: its squares' files, the flight tracks and the
    weather."""
    yield from square_files(prepared / year, lat, lon)
    yield from sorted((prepared / year / "aircraft-tracks").glob("*.aircraft-tracks"))
    if (prepared / "weather").exists():
        yield prepared / "weather"


LIBC.mmap.restype = ctypes.c_void_p
LIBC.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
LIBC.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
PROT_READ, MAP_SHARED, MAP_FAILED = 1, 1, ctypes.c_void_p(-1).value


def resident_pages(path):
    """Pages of the file in the page cache (mincore over a shared read-only mapping)."""
    size = path.stat().st_size
    if size == 0:
        return 0
    with open(path, "rb") as handle:
        address = LIBC.mmap(None, size, PROT_READ, MAP_SHARED, handle.fileno(), 0)
        if address in (None, MAP_FAILED):
            raise OSError(ctypes.get_errno(), f"mmap {path}")
        try:
            pages = (size + mmap.PAGESIZE - 1) // mmap.PAGESIZE
            vector = (ctypes.c_ubyte * pages)()
            if LIBC.mincore(address, size, vector) != 0:
                raise OSError(ctypes.get_errno(), f"mincore {path}")
            return sum(v & 1 for v in vector)
        finally:
            LIBC.munmap(address, size)


def evict(paths):
    for path in paths:
        with open(path, "rb") as handle:
            os.posix_fadvise(handle.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
    return sum(resident_pages(path) for path in paths)


def run(popup, prepared, year, point, exact):
    command = [str(popup), "--prepared", str(prepared), "--year", year,
               "--lat", str(point["lat"]), "--lon", str(point["lon"])]
    if exact:
        command += ["--exact", "1"]
    started = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    assert process.stdout is not None and process.stderr is not None
    lines = []
    for raw in process.stdout:
        lines.append((time.monotonic() - started, json.loads(raw)))
    error = process.stderr.read()
    if process.wait() != 0:
        raise RuntimeError(f"{point['name']}: {error.strip()}")
    return lines


def summary(point, lines, resident_before, repeat, exact):
    first, last = lines[0][1], lines[-1][1]
    return {
        "point": point["name"],
        "repeat": repeat,
        "exact": exact,
        "resident_pages_before": resident_before,
        "first_ms": first["stats"]["elapsed_ms"],
        "full_ms": last["stats"]["elapsed_ms"],
        "first_wall_ms": round(lines[0][0] * 1000),
        "full_wall_ms": round(lines[-1][0] * 1000),
        "first_mb": round(first["stats"]["bytes"] / 1e6, 2),
        "full_mb": round(last["stats"]["bytes"] / 1e6, 2),
        "first_reads": first["stats"]["reads"],
        "first_read_ms": first["stats"]["read_ms"],
        "full_read_ms": last["stats"]["read_ms"],
        "full_reads": last["stats"]["reads"],
        "rings": last["stats"]["rings"],
        "total_lden": last["total_lden"],
        "first_total_lden": first["total_lden"],
        "layers": {s["source_type"]: s.get("lden_precise", s["lden"]) for s in last["sources"] if s["lden"] is not None},
        "evaluated": {s["source_type"]: [s["evaluated"], s["candidates"]] for s in last["sources"] if s["candidates"]},
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--prepared", required=True)
    parser.add_argument("--year", default="2026")
    parser.add_argument("--points", default=str(ROOT / "bench" / "points.json"))
    parser.add_argument("--only", default="")
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--exact", action="store_true")
    parser.add_argument("--popup", default=str(ROOT / "target" / "release" / "qm-popup"))
    arguments = parser.parse_args()
    prepared = pathlib.Path(arguments.prepared)
    points = json.loads(pathlib.Path(arguments.points).read_text())
    wanted = set(filter(None, arguments.only.split(",")))
    for point in points:
        if wanted and point["name"] not in wanted:
            continue
        paths = list(click_files(prepared, arguments.year, point["lat"], point["lon"]))
        for repeat in range(arguments.repeat):
            resident = evict(paths)
            lines = run(arguments.popup, prepared, arguments.year, point, arguments.exact)
            print(json.dumps(summary(point, lines, resident, repeat, arguments.exact)), flush=True)


if __name__ == "__main__":
    main()

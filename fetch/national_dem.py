#!/usr/bin/env python3
"""Fetch a national terrain model at its native resolution and area-average it locally onto a fixed lattice."""
import argparse
import gzip
import hashlib
import http.client
import json
import math
import os
import re
import resource
import shutil
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

from osgeo import gdal, osr

gdal.UseExceptions()
os.environ.setdefault("GDAL_PAM_ENABLED", "NO")  # never write sidecars next to retained publisher bytes
NODATA = -9999.0
# WCS 2.0 scaling parameters. A server's own resampling of a terrain coverage writes heights the
# publisher does not have: blobs of 100-255 m in Flemish polders, single-pixel 0 m lines across
# Polish mountains, both absent from the native 1 m data at the same places.
SERVER_SCALING = {"SCALESIZE", "SCALEFACTOR", "SCALEAXES", "SCALEEXTENT"}
# A header value's leading number, as GDAL reads it: 21 Polish sheets of 2024-08-23 write
# "cellsize 1.00.000141320112294", which GDAL, averaging them, reads as 1.00.
LEADING_NUMBER = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def unscaled(url):
    """A ground model is requested at the publisher's resolution; scaling happens here, by area."""
    keys = {key.upper() for key, _ in urllib.parse.parse_qsl(urllib.parse.urlsplit(url).query,
                                                             keep_blank_values=True)}
    if keys & SERVER_SCALING:
        raise ValueError(f"server-side scaling refused for a ground model: {sorted(keys & SERVER_SCALING)}")
    return url


def wcs_window(service, coverage, epsg, x0, y0, x1, y1):
    crs = f"http://www.opengis.net/def/crs/EPSG/0/{epsg}"
    return unscaled(f"{service}?service=WCS&version=2.0.1&request=GetCoverage&CRS=EPSG:{epsg}"
                    f"&subset=x,{crs}({x0},{x1})&subset=y,{crs}({y0},{y1})&COVERAGEID={coverage}"
                    f"&FORMAT=image/geotiff&RESPONSE_CRS=EPSG:{epsg}")


def multipart_tiff(body, content_type):
    """WCS 2.0 answers multipart/related: the GML description, then the coverage bytes."""
    if not content_type.startswith("multipart/"):
        return body
    start = min(i for i in (body.find(b"II*\x00"), body.find(b"MM\x00*")) if i >= 0)
    boundary = content_type.split("boundary=", 1)[1].split(";", 1)[0].strip('"')
    end = body.rfind(b"--" + boundary.encode())
    return body[start:end].rstrip(b"\r\n")


def download(url, attempts=4, timeout=600):
    """Bytes and headers, or None for a window the service says lies outside its coverage."""
    for attempt in range(attempts):
        try:
            request = urllib.request.Request(unscaled(url), headers={"User-Agent": "QuietMap terrain fetch"})
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read(), response.headers.get("Content-Type", "")
        except urllib.error.HTTPError as error:
            if error.code == 404 and b"InvalidSubsetting" in error.read(4096):
                return None
            if error.code < 500 or attempt == attempts - 1:
                raise
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.IncompleteRead):
            if attempt == attempts - 1:
                raise
        time.sleep(10 * 2 ** attempt)


def publish(target, payload):
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as staged:
        staged.write(payload)
    os.replace(staged.name, target)


def average_tile(sources, epsg, bounds, step, target):
    """The mean of every valid native pixel inside each lattice cell, weighted by overlap; sources are
    (path, EPSG) pairs, each warped from its own system onto the lattice's."""
    target.parent.mkdir(parents=True, exist_ok=True)
    staged = target.with_name(target.name + ".partial")
    source_datasets = [gdal.Translate("", str(path), format="VRT", outputSRS=f"EPSG:{source_epsg}")
                       for path, source_epsg in sources]
    gdal.Warp(str(staged), source_datasets, format="GTiff",
              dstSRS=f"EPSG:{epsg}", outputBounds=bounds, xRes=step, yRes=step, resampleAlg="average",
              srcNodata=NODATA, dstNodata=NODATA, outputType=gdal.GDT_Float32, errorThreshold=0,
              warpOptions=["NUM_THREADS=1"], creationOptions=["COMPRESS=DEFLATE", "PREDICTOR=3", "TILED=YES"])
    os.replace(staged, target)


def sha256(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def wcs_tile(arguments, x0, y0):
    """One lattice tile from native windows; the raw responses are kept beside their requests."""
    name = arguments.name.format(x0=x0, y0=y0)
    target = arguments.out / name
    if target.exists():
        return json.loads(Path(str(target) + ".provenance.json").read_text())
    windows = []
    for wy in range(y0, y0 + arguments.tile, arguments.window):
        for wx in range(x0, x0 + arguments.tile, arguments.window):
            url = wcs_window(arguments.service, arguments.coverage, arguments.epsg,
                             wx, wy, wx + arguments.window, wy + arguments.window)
            raw = arguments.raw / f"{arguments.coverage}_{wx}_{wy}.tif"
            if not raw.exists():
                answer = download(url)
                if answer is None:
                    windows.append(dict(request_url=url, outside_coverage=True))
                    continue
                publish(raw, multipart_tiff(*answer))
            windows.append(dict(request_url=url, path=str(raw), sha256=sha256(raw)))
    sources = [Path(w["path"]) for w in windows if "path" in w]
    record = dict(tile=name, epsg=arguments.epsg, step=arguments.step, windows=windows,
                  fetched_utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
    if sources:
        average_tile([(path, arguments.epsg) for path in sources], arguments.epsg,
                     [x0, y0, x0 + arguments.tile, y0 + arguments.tile],
                     arguments.step, target)
        record["sha256"] = sha256(target)
        publish(Path(str(target) + ".provenance.json"), (json.dumps(record, indent=1) + "\n").encode())
    return record


def sheet_index(arguments):
    """Every published sheet of the yearly index layers, with its own update date and download URL."""
    rows = []
    for layer in arguments.layers:
        start = 0
        while True:
            url = (f"{arguments.index}?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES={layer}"
                   f"&COUNT=1000&STARTINDEX={start}")
            root = ET.fromstring(download(url)[0])
            members = [m[0] for m in root if m.tag.endswith("member")]
            for feature in members:
                values = {child.tag.split("}")[1]: (child.text or "").strip() for child in feature}
                date = feature.find(".//{*}akt_data//{*}timePosition")
                rows.append(dict(layer=layer, sheet=values.get("godlo"), url=values.get("url_do_pobrania"),
                                 updated=date.text if date is not None else None,
                                 filled=values.get("czy_ark_wypelniony"), resolution=values.get("char_przestrz"),
                                 format=values.get("format"), source=values.get("zrodlo_danych")))
            if len(members) < 1000:
                break
            start += 1000
    publish(arguments.out, (json.dumps(rows, indent=0) + "\n").encode())
    print(json.dumps({"sheets": len(rows)}))


def current_sheets(rows):
    """Per sheet the newest version, over the newest complete one when the newest is partial.

    The list is in drawing order: an older complete sheet comes first, so the newer one covers it.
    """
    versions = {}
    for row in rows:
        if row["url"] and row["updated"]:
            versions.setdefault(row["sheet"], []).append(row)
    chosen = []
    for sheet in sorted(versions):
        ordered = sorted(versions[sheet], key=lambda r: (r["updated"], r["url"]))
        newest = ordered[-1]
        if newest["filled"] != "TAK":
            complete = [r for r in ordered if r["filled"] == "TAK"]
            if complete:
                chosen.append(complete[-1])
        chosen.append(newest)
    return sorted(chosen, key=lambda r: (r["updated"], r["url"]))


def sheet(arguments, row):
    """One native sheet, gzip-retained byte for byte; its header is checked before it is kept."""
    target = arguments.raw / (Path(urllib.parse.urlsplit(row["url"]).path).name + ".gz")
    receipt = Path(str(target) + ".json")
    if receipt.exists():
        return json.loads(receipt.read_text())
    if shutil.disk_usage(arguments.raw).free < arguments.reserve_bytes:
        raise ValueError(f"fewer than {arguments.reserve_bytes} bytes would stay free beside the sheets")
    body, _ = download(row["url"])
    if not body.lstrip().lower().startswith(b"ncols"):
        raise ValueError(f"not an ASCII grid: {row['url']}")
    publish(target, gzip.compress(body, compresslevel=3))
    record = dict(row, sha256=hashlib.sha256(body).hexdigest(), bytes=len(body), path=str(target),
                  fetched_utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
    publish(receipt, (json.dumps(record) + "\n").encode())
    return record


def sheet_extent(path):
    """West, south, east, north of an ASCII grid sheet, from its header as GDAL reads it."""
    with gzip.open(path, "rt") as source:
        header = {}
        while len(header) < 5:
            key, value = source.readline().split()[:2]
            header[key.lower()] = float(LEADING_NUMBER.match(value)[0])
    half = header["cellsize"] / 2 if "xllcenter" in header else 0
    west = header.get("xllcorner", header.get("xllcenter", 0) - half)
    south = header.get("yllcorner", header.get("yllcenter", 0) - half)
    return west, south, west + header["ncols"] * header["cellsize"], south + header["nrows"] * header["cellsize"]


def sheet_system(extent, epsg, zones):
    """A sheet's EPSG and its extent in the lattice's EPSG: a Gauss-Krüger easting carries its zone's
    number in the millions (8,025 of Poland's 67,891 sheets are in its 2000 system, the rest in 1992)."""
    source = zones.get(int(extent[0] // 1_000_000), epsg)
    if source == epsg:
        return source, extent
    systems = []
    for code in (source, epsg):
        system = osr.SpatialReference()
        system.ImportFromEPSG(code)
        system.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
        systems.append(system)
    return source, osr.CoordinateTransformation(*systems).TransformBounds(*extent, 21)


def zone_systems(text):
    """Gauss-Krüger zones and their EPSG codes, from 'ZONE=EPSG,..'."""
    return dict(tuple(map(int, pair.split("="))) for pair in text.split(",") if pair)


def sheet_tiles(arguments):
    """Average the fetched sheets onto lattice tiles; sheets are drawn in their drawing order."""
    # Each sheet stays open through its tile's warp: 304 sheets in one Polish tile, six tiles at once,
    # pass the usual soft limit of 1,024 files, and GDAL then reports a sheet 'not recognized'.
    resource.setrlimit(resource.RLIMIT_NOFILE, (resource.getrlimit(resource.RLIMIT_NOFILE)[1],) * 2)
    receipts = [json.loads(p.read_text()) for p in arguments.raw.glob("*.gz.json")]
    receipts.sort(key=lambda r: (r["updated"], r["url"]))
    tiles = {}
    for record in receipts:
        record["epsg"], (west, south, east, north) = sheet_system(sheet_extent(record["path"]), arguments.epsg,
                                                                  arguments.zones)
        for x0 in range(int(west // arguments.tile) * arguments.tile, math.ceil(east), arguments.tile):
            for y0 in range(int(south // arguments.tile) * arguments.tile, math.ceil(north), arguments.tile):
                tiles.setdefault((x0, y0), []).append(record)

    def work(corner):
        x0, y0 = corner
        target = arguments.out / arguments.name.format(x0=x0, y0=y0)
        if target.exists():
            return target.name
        sheets = tiles[corner]
        average_tile([(f"/vsigzip/{s['path']}", s["epsg"]) for s in sheets], arguments.epsg,
                     [x0, y0, x0 + arguments.tile, y0 + arguments.tile], arguments.step, target)
        record = dict(tile=target.name, sha256=sha256(target), sheets=[dict(url=s["url"], sha256=s["sha256"],
                      updated=s["updated"]) for s in sheets])
        publish(Path(str(target) + ".provenance.json"), (json.dumps(record, indent=1) + "\n").encode())
        return target.name

    with ThreadPoolExecutor(arguments.jobs) as pool:
        for done, name in enumerate(pool.map(work, sorted(tiles)), 1):
            print(json.dumps(dict(done=done, total=len(tiles), tile=name)), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    tiles = commands.add_parser("wcs-tiles", help="native WCS windows averaged onto lattice tiles")
    for flag in ("--service", "--coverage", "--name"):
        tiles.add_argument(flag, required=True)
    for flag in ("--epsg", "--tile", "--window"):
        tiles.add_argument(flag, type=int, required=True)
    tiles.add_argument("--step", type=float, required=True)
    tiles.add_argument("--tiles", type=Path, required=True, help="lines 'x0 y0': lower-left tile corners")
    tiles.add_argument("--raw", type=Path, required=True)
    tiles.add_argument("--out", type=Path, required=True)
    tiles.add_argument("--jobs", type=int, default=4)
    index = commands.add_parser("sheet-index", help="a WFS index of native sheets")
    index.add_argument("--index", required=True)
    index.add_argument("--layers", nargs="+", required=True)
    index.add_argument("--out", type=Path, required=True)
    sheets = commands.add_parser("sheets", help="download the listed native sheets")
    sheets.add_argument("--list", type=Path, required=True, help="the sheet index; its current sheets are fetched")
    sheets.add_argument("--raw", type=Path, required=True)
    sheets.add_argument("--jobs", type=int, default=8)
    sheets.add_argument("--reserve-bytes", type=int, required=True, help="free space kept on the sheets' disk")
    averaged = commands.add_parser("sheet-tiles", help="average fetched sheets onto lattice tiles")
    averaged.add_argument("--raw", type=Path, required=True)
    averaged.add_argument("--out", type=Path, required=True)
    averaged.add_argument("--name", required=True)
    for flag in ("--epsg", "--tile"):
        averaged.add_argument(flag, type=int, required=True)
    averaged.add_argument("--step", type=float, required=True)
    averaged.add_argument("--zones", default="", type=zone_systems,
                          help="ZONE=EPSG,..: a sheet whose easting's millions are a listed zone is in its EPSG")
    averaged.add_argument("--jobs", type=int, default=4)
    arguments = parser.parse_args()
    if arguments.command == "sheet-index":
        return sheet_index(arguments)
    if arguments.command == "sheet-tiles":
        return sheet_tiles(arguments)
    if not 1 <= arguments.jobs <= 16:
        parser.error("jobs must stay between 1 and 16 connections")
    if arguments.command == "wcs-tiles":
        if arguments.tile % arguments.window or arguments.window % arguments.step:
            parser.error("windows must tile a lattice tile and hold whole lattice cells")
        corners = [tuple(int(v) for v in line.split()) for line in arguments.tiles.read_text().splitlines() if line]
        work = lambda corner: wcs_tile(arguments, *corner)
    else:
        corners = current_sheets(json.loads(arguments.list.read_text()))
        work = lambda row: sheet(arguments, row)
    with ThreadPoolExecutor(arguments.jobs) as pool:
        for done, record in enumerate(pool.map(work, corners), 1):
            print(json.dumps(dict(done=done, total=len(corners), item=record.get("tile") or record.get("sheet"))),
                  flush=True)


if __name__ == "__main__":
    main()

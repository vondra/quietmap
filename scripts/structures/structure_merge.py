"""One-to-one OSM/Overture merge preserving emission order and screening identity."""

import os
from hashlib import sha256
from pathlib import Path

import pyarrow as pa
import pyarrow.ipc as ipc
import shapely
from shapely import STRtree

import qmgrid
import structure_contract
import structure_inputs
from structure_contract import (
    SCHEMA, CONTRACT_KEY, CONTRACT_VERSION, KIND_BUILDING, KIND_BARRIER,
    EMISSION_GRID_THRESHOLD_M2,
    load_osm_buildings, load_barriers, wall_grid_poly, wall_centroid_grid,
    validate_square,
)
from structure_inputs import (
    ENVELOPE_FROM_BUILDING_USE, ENVELOPE_DEFAULT, ENVELOPE_OUTDOOR,
    apply_raster_tiers, overture_height_ladder,
)

IOU_MATCH_THRESHOLD = 0.5

def builder_version():
    """A source-code correction invalidates resume even with frozen inputs."""
    digest = sha256()
    for module in (qmgrid, structure_contract, structure_inputs):
        assert module.__file__ is not None
        digest.update(Path(module.__file__).read_bytes())
    for name in ("build-structures.py", "structure_merge.py"):
        digest.update(Path(__file__).with_name(name).read_bytes())
    return digest.hexdigest()


BUILDER_VERSION = builder_version()

def match_pairs(osm_geoms, osm_geom_idx, overture_rows):
    """One-to-one OSM<->Overture assignment; returns {overture_row: osm_row}.
    Proven form (complete qualifying set, greedy by iou/centroid/rows) — kept
    verbatim, geometry source agnostic."""
    from shapely import wkb as shapely_wkb

    if not overture_rows or not osm_geoms:
        return {}
    tree = STRtree(osm_geoms)
    edges = []
    for j, row in enumerate(overture_rows):
        g = row.get("geom")
        if g is None:
            g = shapely_wkb.loads(row["wkb"])
            row["geom"] = g
        c = g.centroid
        for k in tree.query(g, predicate="intersects"):
            og = osm_geoms[k]
            contains = og.covers(c)
            try:
                inter = g.intersection(og).area
            except Exception:
                gg = g if g.is_valid else g.buffer(0)
                oo = og if og.is_valid else og.buffer(0)
                inter = 0.0 if gg.is_empty or oo.is_empty else gg.intersection(oo).area
            iou = 0.0
            if inter > 0.0:
                union = g.area + og.area - inter
                iou = inter / union if union > 0 else 0.0
            if contains or iou >= IOU_MATCH_THRESHOLD:
                edges.append((iou, 1 if contains else 0, j, osm_geom_idx[k]))
    edges.sort(key=lambda e: (-e[0], -e[1], e[2], e[3]))
    matched_ovt, matched_osm, pairs = set(), set(), {}
    for _iou, _centroid_in, j, i in edges:
        if j in matched_ovt or i in matched_osm:
            continue
        matched_ovt.add(j)
        matched_osm.add(i)
        pairs[j] = i
    return pairs

def build_square(name, prepared_dir, overture_rows, overture_mtime, ghsl, regional):
    """Write one square's structures.arrow; return the census dict, or None
    when the square is up to date (idempotent skip)."""
    square = qmgrid.parse_square_name(name)
    if square is None:
        raise ValueError(f"not a square name: {name}")
    x, y = square
    square_dir = os.path.join(prepared_dir, "z9", str(x), str(y))
    if not os.path.isdir(square_dir):
        raise SystemExit(
            f"{name}: no prepared square directory {square_dir} — the structure table "
            f"follows the prepared inventory and must not extend it"
        )
    overture_rows = overture_rows or []
    out_path = os.path.join(square_dir, "structures.arrow")
    if os.path.exists(out_path):
        out_mtime = os.path.getmtime(out_path)
        inputs = [os.path.join(square_dir, n) for n in ("buildings.arrow", "barriers.arrow")]
        mtimes = [os.path.getmtime(p) for p in inputs if os.path.exists(p)]
        if overture_mtime is not None:
            mtimes.append(overture_mtime)
        mtimes.append(ghsl.mtime)
        if regional is not None:
            mtimes.append(regional.mtime)
        with ipc.open_file(out_path) as output_file:
            output_meta = output_file.schema.metadata or {}
        if (max(mtimes) <= out_mtime
                and output_meta.get(b"builder_version") == BUILDER_VERSION.encode()
                and output_meta.get(CONTRACT_KEY.encode()) == CONTRACT_VERSION.encode()
                and output_meta.get(b"grid") == b"z30"):
            return None  # idempotent: no input is newer than the output
    osm = load_osm_buildings(os.path.join(square_dir, "buildings.arrow"))
    barriers = load_barriers(os.path.join(square_dir, "barriers.arrow"))

    osm_geoms, osm_geom_idx, osm_geom_by_row = [], [], {}
    for i, g in enumerate(osm["shapely"]):
        if g is None or g.is_empty:
            continue
        osm_geoms.append(g)
        osm_geom_idx.append(i)
        osm_geom_by_row[i] = g
    pairs = match_pairs(osm_geoms, osm_geom_idx, overture_rows)
    osm_to_ovt = {i: j for j, i in pairs.items()}

    osm_only = {}
    n_osm = len(osm["osm_id"])
    matched_osm = set(pairs.values())
    raster_rows = list(overture_rows)
    for i in range(n_osm):
        if i in matched_osm:
            continue
        h, tier = overture_height_ladder(osm["height"][i], osm["floors"][i])
        gx, gy = osm["centroid_gx"][i], osm["centroid_gy"][i]
        clon, clat = qmgrid.grid_to_lonlat(gx, gy)
        row = {"height_m": h, "tier": tier,
               "clat": clat, "clon": clon, "osm_row": i}
        row["geom"] = osm_geom_by_row.get(i)
        osm_only[i] = row
        if row["geom"] is not None:
            raster_rows.append(row)
    stats = {"tier3": 0, "tier4": 0, "abstain": 0}
    apply_raster_tiers(raster_rows, regional, ghsl, stats)

    out = {f: [] for f in SCHEMA.names}
    n_both = 0
    n_osm_only_geom = sum(
        1 for i in osm_only if osm_geom_by_row.get(i) is not None
    )
    osm_only_geom_counter = 0
    wall_counter = 0

    def snap_geom(geom):
        """Shapely polygon -> snapped grid ring (exterior of the largest part).
        v2 stores one ring per row; holes were already dropped at extract."""
        if geom.geom_type == "MultiPolygon":
            geom = max(geom.geoms, key=lambda p: p.area)
        return [qmgrid.lonlat_to_grid(x, y)
                for x, y in shapely.get_coordinates(geom.exterior)]

    def emit(i_osm, ovt, ordinal):
        if ovt is not None:
            ring = snap_geom(ovt_geom(ovt))
            geom_blob = qmgrid.encode_grid_poly(ring)
            height_m, tier = ovt["height_m"], ovt["tier"]
            envelope = ovt["envelope"]
            cgx, cgy = qmgrid.lonlat_to_grid(ovt["clon"], ovt["clat"])
        else:
            geom_blob = osm["geom"][i_osm]
            r = osm_only[i_osm]  # every unmatched OSM row laddered above
            height_m, tier = r["height_m"], r["tier"]
            envelope = ENVELOPE_FROM_BUILDING_USE.get(
                osm["building_use"][i_osm], ENVELOPE_DEFAULT
            )
            cgx, cgy = osm["centroid_gx"][i_osm], osm["centroid_gy"][i_osm]
        out["kind"].append(KIND_BUILDING)
        out["geom"].append(geom_blob)
        out["height_m"].append(height_m)
        out["height_tier"].append(tier)
        out["envelope_class"].append(envelope)
        out["centroid_gx"].append(cgx)
        out["centroid_gy"].append(cgy)
        for c in ("osm_id", "building_type", "building_use", "height", "floors",
                  "name", "addr_street", "addr_housenumber", "area_m2",
                  "opening_hours_frac", "source_id"):
            out[c].append(osm[c][i_osm] if i_osm is not None else None)
        emission_geom = None
        if i_osm is not None:
            osm_blob = osm["geom"][i_osm]
            area = osm["area_m2"][i_osm]
            needs_poly = osm_blob is not None and (
                area is None or not (area > 0.0) or area > EMISSION_GRID_THRESHOLD_M2
            )
            if needs_poly and osm_blob != geom_blob:
                emission_geom = osm_blob
        out["emission_geom"].append(emission_geom)
        if i_osm is not None and ovt is not None:
            out["emission_centroid_gx"].append(osm["centroid_gx"][i_osm])
            out["emission_centroid_gy"].append(osm["centroid_gy"][i_osm])
        else:
            out["emission_centroid_gx"].append(None)
            out["emission_centroid_gy"].append(None)
        out["segment_idx"].append(None)
        out["screening_ordinal"].append(ordinal)

    def ovt_geom(ovt):
        g = ovt.get("geom")
        if g is None:
            from shapely import wkb as shapely_wkb
            g = shapely_wkb.loads(ovt["wkb"])
            ovt["geom"] = g
        return g

    for i in range(n_osm):
        j = osm_to_ovt.get(i)
        if j is not None:
            n_both += 1
            emit(i, overture_rows[j], j)
        else:
            has_geom = osm_geom_by_row.get(i) is not None
            ordinal = None
            if has_geom:
                ordinal = len(overture_rows) + osm_only_geom_counter
                osm_only_geom_counter += 1
            emit(i, None, ordinal)
    matched_ovt = set(pairs.keys())
    n_ovt_only = 0
    for j, row in enumerate(overture_rows):
        if j in matched_ovt:
            continue
        n_ovt_only += 1
        emit(None, row, j)

    # Walls: one row per micro-segment, grid polyline, mapped-or-default height.
    for b in barriers:
        out["kind"].append(KIND_BARRIER)
        out["geom"].append(wall_grid_poly(
            b["start_gx"], b["start_gy"], b["end_gx"], b["end_gy"]))
        h = b["height"]
        out["height_m"].append(h)
        out["height_tier"].append(b["height_tier"])
        out["envelope_class"].append(ENVELOPE_OUTDOOR)
        cgx, cgy = wall_centroid_grid(
            b["start_gx"], b["start_gy"], b["end_gx"], b["end_gy"])
        out["centroid_gx"].append(cgx)
        out["centroid_gy"].append(cgy)
        out["osm_id"].append(b["osm_id"])
        for c in ("building_type", "building_use", "height", "floors", "name",
                  "addr_street", "addr_housenumber", "area_m2",
                  "opening_hours_frac", "source_id", "emission_geom",
                  "emission_centroid_gx", "emission_centroid_gy"):
            out[c].append(None)
        out["segment_idx"].append(b["segment_idx"])
        out["screening_ordinal"].append(
            len(overture_rows) + n_osm_only_geom + wall_counter
        )
        wall_counter += 1

    meta = dict(SCHEMA.metadata or {})
    meta[CONTRACT_KEY] = CONTRACT_VERSION
    meta["grid"] = "z30"
    meta["builder_version"] = BUILDER_VERSION
    meta["building_rows"] = str(n_osm + n_ovt_only)
    meta["barrier_rows"] = str(len(barriers))
    schema = SCHEMA.with_metadata(meta)
    table = pa.table(out, schema=schema)

    validate_square(name, osm, table)

    tmp = f"{out_path}.tmp.{os.getpid()}"
    with ipc.new_file(tmp, schema) as w:
        # Sequential 4096-row chunks, no spatial re-sort: the emission stream is
        # the buildings.arrow subsequence and must not be reordered.
        for batch in table.to_batches(max_chunksize=4096):
            w.write_batch(batch)
    fd = os.open(tmp, os.O_RDONLY)
    try:
        os.fsync(fd)
        os.replace(tmp, out_path)
        dir_fd = os.open(square_dir, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        os.close(fd)
    return {
        "square": name,
        "osm_rows": n_osm,
        "both": n_both,
        "osm_only": n_osm - n_both,
        "overture_only": n_ovt_only,
        "walls": len(barriers),
        "rows": table.num_rows,
        "tier3": stats["tier3"],
        "tier4": stats["tier4"],
        "regional_abstain": stats["abstain"],
        "bytes": os.path.getsize(out_path),
    }

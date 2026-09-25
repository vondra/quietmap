//! Vector structure loading for the popup.
//!
//! Each query assembles an [`ObstacleSet`] from the prebuilt per-square
//! indexes (`structures.qoix`, see `square_obstacle_index`) covering the query
//! source envelope. One `structures.arrow` per square carries BOTH screening
//! stocks — buildings (kind 0, polygons) and noise walls (kind 1, polyline
//! microsegments, indexed as [`ObstacleKind::Barrier`] edges) — and, in its
//! OSM-attributed rows, the input of the low-profile height cap's lookup. This
//! file owns how that table becomes an index ([`build_obstacle_index_from_arrow_bytes`]:
//! dense ids in `screening_ordinal` order, the capped heights) and the
//! containment probes the popup runs on the assembled set.
//!
//! **All-or-error.** Any selected square whose index can neither be mapped nor
//! built in process aborts the whole load: a partial set would silently
//! under-screen the path.
//! Emptiness is not a gap: a 0-row table is the answer "nothing stands here".

use std::io::Cursor;
use std::path::Path;

use arrow::array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, UInt32Array, UInt8Array,
};
use arrow::ipc::reader::FileReader;
use grid::Square;
use noise_compute::envelope::EnvelopeClass;
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};

use square_store::grid_cols::{
    col_binary, col_f32, col_i32, col_u8, decode_geom, polygons_wkb, ring_lonlat,
};
use square_store::store::{STRUCTURE_KIND_BARRIER, STRUCTURE_KIND_BUILDING};
use square_store::structure_contract;

use crate::square_obstacle_index::{
    load_square_obstacle_index_or_build_it_in_process, STRUCTURES_ARROW,
};

fn square_dir(prepared_year_dir: &Path, square: Square) -> std::path::PathBuf {
    prepared_year_dir.join(grid::square_name(square))
}

/// Assemble the query's [`ObstacleSet`], or fail when vector coverage cannot
/// be proved complete.
pub fn load_obstacle_set(
    prepared_year_dir: &Path,
    lat: f64,
    lon: f64,
) -> Result<ObstacleSet, String> {
    use rayon::prelude::*;
    // Obstacles screen surface propagation, so the surface reach selects them;
    // the wider cruise owner radius has no obstacles to offer. Independent
    // squares map their indexes concurrently.
    let indexes = crate::query::surface_squares_within_reach(lat, lon)?
        .into_par_iter()
        .filter_map(|square| {
            load_square_obstacle_index_or_build_it_in_process(
                &square_dir(prepared_year_dir, square),
                square,
            )
                .map_err(|e| format!("structure_store: {e}"))
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Zero edges is a legitimate answer: a 0-row table is the finished sweep
    // saying there is nothing here. A file that exists HAS been asked and HAS
    // answered; treating its emptiness as a fault would take whole countries
    // silent the moment an Overture release rejects their heights (raised in
    // review, 2026-08-30).
    Ok(ObstacleSet { indexes })
}

/// Tallest structure the height probe will report (m) — comfortably above the
/// tallest building on Earth (828 m), so the bisection's upper bracket is
/// never the answer in practice.
const MAX_PROBE_HEIGHT_M: f32 = 1_000.0;
/// Height-probe resolution (m). Store heights are metre-scale (mapped values,
/// the 3 m low-profile cap, the 8 m default), so 5 cm is far below anything a
/// popup would display.
const HEIGHT_PROBE_RESOLUTION_M: f32 = 0.05;

/// Height of the tallest vector footprint containing the point, regardless of
/// envelope class (the building-height hover).
///
/// Runs on the already-loaded query set — zero extra I/O. The height comes out
/// of the containment test itself: `ObstacleIndex::contains_built(…, min_h)`
/// answers "inside a footprint TALLER than `min_h`", which is monotone in
/// `min_h`, so the tallest containing footprint is the threshold where it
/// flips — ~15 in-memory probes, with the exact polygon test (and its
/// hole/overlap semantics) every other containment probe uses.
pub fn point_inside_obstacle(set: &ObstacleSet, lat: f64, lon: f64) -> Option<f32> {
    let mut seen: Vec<(u32, u32, f32)> = Vec::new();
    let mut inside = |min_h: f32| {
        set.indexes
            .iter()
            .any(|i| i.contains_built(lat, lon, min_h, &mut seen))
    };
    // `min_height_m = 0` admits every indexed footprint (the builder already
    // drops height ≤ 0) — "inside any obstacle polygon", the mask's rule.
    if !inside(0.0) {
        return None;
    }
    let (mut lo, mut hi) = (0.0f32, MAX_PROBE_HEIGHT_M);
    while hi - lo > HEIGHT_PROBE_RESOLUTION_M {
        let mid = 0.5 * (lo + hi);
        if inside(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(0.5 * (lo + hi))
}

/// The rings of one footprint of a square's `structures.arrow`, found among the
/// batches whose envelope holds `(lat, lon)` (a point inside the footprint).
pub fn footprint_polygons_at(
    structures: &square_store::store::LazyArrow,
    footprint_id: u32,
    lat: f64,
    lon: f64,
) -> Result<grid::poly::GridPolygons, String> {
    for batch in structures.batches_within(lat, lon, 0.0)? {
        let ordinals = square_store::grid_cols::col_u32(&batch, "screening_ordinal")
            .ok_or("structures.arrow: missing screening_ordinal")?;
        if let Some(row) = (0..batch.num_rows())
            .find(|&row| !ordinals.is_null(row) && ordinals.value(row) == footprint_id)
        {
            return col_binary(&batch, "geom")
                .filter(|column| !column.is_null(row))
                .and_then(|column| grid::poly::decode_grid_polygons(column.value(row)))
                .ok_or_else(|| format!("structures.arrow: footprint {footprint_id} has no rings"));
        }
    }
    Err(format!(
        "structures.arrow: footprint {footprint_id} not found at ({lat:.6}, {lon:.6})"
    ))
}

/// Every enclosed building footprint the square's index holds, as (index id,
/// rings): the buildings the façade-exposure stage writes one row for. The
/// same admission as [`build_obstacle_index_from_arrow_bytes`]: a geometry,
/// a positive screening height and an enclosed envelope class.
pub fn enclosed_building_footprints(
    bytes: &[u8],
    structures_arrow: &Path,
) -> Result<Vec<(u32, grid::poly::GridPolygons)>, String> {
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", structures_arrow.display()))?;
    let mut footprints = Vec::new();
    for batch in reader {
        let batch =
            batch.map_err(|e| format!("arrow batch {}: {e}", structures_arrow.display()))?;
        let heights = structure_contract::heights(&batch)?;
        let (Some(kinds), Some(classes), Some(ordinals)) = (
            col_u8(&batch, "kind"),
            col_u8(&batch, "envelope_class"),
            square_store::grid_cols::col_u32(&batch, "screening_ordinal"),
        ) else {
            return Err(format!("{}: missing structure columns", structures_arrow.display()));
        };
        for row in 0..batch.num_rows() {
            if kinds.value(row) != STRUCTURE_KIND_BUILDING
                || heights.value(row) <= 0
                || !EnvelopeClass::from_u8(classes.value(row)).is_enclosed()
                || ordinals.is_null(row)
            {
                continue;
            }
            if let Some(polygons) = col_binary(&batch, "geom")
                .filter(|column| !column.is_null(row))
                .and_then(|column| grid::poly::decode_grid_polygons(column.value(row)))
            {
                footprints.push((ordinals.value(row), polygons));
            }
        }
    }
    footprints.sort_unstable_by_key(|(id, _)| *id);
    Ok(footprints)
}

/// Hover-only winner over every visible footprint, including Outdoor-class
/// carports and roof structures, which the building exposure
/// (`ObstacleSet::enclosed_footprint_at`) treats as open ground.
pub fn point_inside_footprint(
    set: &ObstacleSet,
    lat: f64,
    lon: f64,
) -> Option<(EnvelopeClass, f32)> {
    let mut seen = Vec::new();
    set.indexes
        .iter()
        .filter_map(|index| index.containing_footprint(lat, lon, 0.0, &mut seen))
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.cmp(&a.2)))
        .map(|(class, height, _)| (class, height))
}

/// The low-profile cap's lookup, read from the SAME structures.arrow the index
/// is built from: kind=0 rows with a valid `osm_id` are the OSM building stock
/// (the merge's emission rows — the old buildings.arrow subsequence), matched
/// at their emission centroid where the merge kept one, else the screening
/// centroid. The rule itself lives in [`noise_compute::low_profile`] (shared
/// with the tile painter's loader, so popup and tiles cap the same footprints).
///
fn low_profile_from_structures(bytes: &[u8], label: &Path) -> Result<LowProfileLookup, String> {
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", label.display()))?;
    structure_contract::validate_schema(reader.schema().as_ref())?;
    let mut lookup = LowProfileLookup::default();
    for batch in reader {
        let batch = batch.map_err(|e| format!("arrow batch {}: {e}", label.display()))?;
        let (Some(kinds), Some(osm_ids), Some(cgxs), Some(cgys), Some(types), Some(areas)) = (
            batch
                .column_by_name("kind")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("osm_id")
                .and_then(|c| c.as_any().downcast_ref::<Int64Array>()),
            batch
                .column_by_name("centroid_gx")
                .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
            batch
                .column_by_name("centroid_gy")
                .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
            batch
                .column_by_name("building_type")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("area_m2")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>()),
        ) else {
            return Err(format!(
                "{}: missing current structure emission columns",
                label.display()
            ));
        };
        // The merge moves an OSM-matched row's screening centroid to the
        // Overture footprint's; the OSM one survives as emission_centroid_*.
        let egxs = batch
            .column_by_name("emission_centroid_gx")
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
        let egys = batch
            .column_by_name("emission_centroid_gy")
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
        // An emission-only row (a school ground, a retail zone, a car park below
        // the ground) has no screening geometry. It must not seed the cap: a
        // garage class beneath a real building would pull that building's
        // screening height down to one floor.
        let geoms = batch.column_by_name("geom");
        for i in 0..batch.num_rows() {
            if kinds.value(i) != STRUCTURE_KIND_BUILDING
                || osm_ids.is_null(i)
                || types.is_null(i)
                || areas.is_null(i)
                || geoms.is_none_or(|geom| geom.is_null(i))
            {
                continue;
            }
            let (gx, gy) = match (egxs, egys) {
                (Some(egxs), Some(egys)) if !egxs.is_null(i) && !egys.is_null(i) => {
                    (egxs.value(i), egys.value(i))
                }
                _ if !cgxs.is_null(i) && !cgys.is_null(i) => (cgxs.value(i), cgys.value(i)),
                _ => continue,
            };
            let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
            lookup.insert_if_low(types.value(i), lat, lon, areas.value(i));
        }
    }
    Ok(lookup)
}

#[cfg(test)]
mod low_profile_seed_tests {
    use super::*;
    use crate::structure_test_fixture as fx;

    const LAT: f64 = 50.0755;
    const LON: f64 = 14.4378;

    fn garage(stands: bool) -> fx::StructureRow {
        fx::StructureRow {
            kind: square_store::store::STRUCTURE_KIND_BUILDING,
            // A row that stands carries screening geometry; an emission-only row
            // (a car park below the ground, a school ground) carries none.
            ring_lonlat: stands.then(|| fx::square_ring_lonlat(LAT, LON)),
            // Production: a row that stands carries an ordinal, an emission-only
            // row carries none.
            screening_ordinal: stands.then_some(0),
            height_m: if stands { 3 } else { 0 },
            envelope_class: 1,
            centroid_lonlat: Some((LON, LAT)),
            osm_id: Some(7),
            building_type: Some(7), // garage: the low-profile class
            area_m2: Some(450.0),
            emission_ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
            ..Default::default()
        }
    }

    fn lookup(stands: bool) -> noise_compute::low_profile::LowProfileLookup {
        let batch = fx::structure_batch(&[garage(stands)], true);
        let mut bytes = Vec::new();
        {
            let mut writer =
                arrow::ipc::writer::FileWriter::try_new(&mut bytes, &batch.schema()).unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }
        low_profile_from_structures(&bytes, Path::new("fixture")).unwrap()
    }

    /// The cap pulls a matched Overture footprint down to one floor when a
    /// garage of the same size sits on it. An emission-only row is not on it —
    /// nothing stands there — so it must not pull a real building's wall down.
    #[test]
    fn only_a_garage_that_stands_caps_a_real_building() {
        // A height that is not per building is what the cap exists for.
        let tall = 18.0;
        assert_eq!(lookup(true).capped_height(tall, false, LAT, LON, 450.0), 3.0);
        assert_eq!(lookup(false).capped_height(tall, false, LAT, LON, 450.0), tall);
    }
}

/// Metric origin of one square's index: the square centre, so the cache entry
/// is query-independent; crossings project the ray per call, so mixed origins
/// across a set are fine.
fn square_center_latlon(square: Square) -> (f64, f64) {
    use grid::{EARTH_CIRCUMFERENCE_M, WEB_MERCATOR_RADIUS_M, Z9_TILES_PER_AXIS};
    use std::f64::consts::PI;
    let axis = f64::from(Z9_TILES_PER_AXIS);
    let lon = (f64::from(square.x) + 0.5) / axis * 360.0 - 180.0;
    let half = EARTH_CIRCUMFERENCE_M / 2.0;
    let y_m = half - (f64::from(square.y) + 0.5) / axis * EARTH_CIRCUMFERENCE_M;
    let lat = (2.0 * (y_m / WEB_MERCATOR_RADIUS_M).exp().atan() - PI / 2.0).to_degrees();
    (lat, lon)
}

/// Build one square's index from its final `structures.arrow` bytes — the
/// pipeline step's builder (`square_obstacle_index::write_square_obstacle_index`).
///
/// Ids are dense in `screening_ordinal` order, one per geometry-carrying row,
/// buildings and walls sharing the one counter.
pub fn build_obstacle_index_from_arrow_bytes(
    square: Square,
    bytes: &[u8],
    structures_arrow: &Path,
) -> Result<ObstacleIndex, String> {
    let (origin_lat, origin_lon) = square_center_latlon(square);
    let mut builder = ObstacleIndex::builder(origin_lat, origin_lon);
    // The cap lookup must be complete before ANY row is capped: the match is
    // spatial, so a first-rows-only lookup would miss neighbours further down
    // the file. Two streaming passes over the one in-memory read keep the old
    // loader's memory shape (a dense metro square's table runs to ~1 GB).
    let low_profile = low_profile_from_structures(bytes, structures_arrow)?;
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| format!("arrow open {}: {e}", structures_arrow.display()))?;
    let mut batches = Vec::new();
    for batch in reader {
        batches
            .push(batch.map_err(|e| format!("arrow batch {}: {e}", structures_arrow.display()))?);
    }
    let batch_heights: Vec<_> = batches
        .iter()
        .map(structure_contract::heights)
        .collect::<Result<_, _>>()?;
    // The index inserts rows in `screening_ordinal` order (its dense ids follow
    // the sort): the engine's exact-δ tie resolution is scan-order sensitive,
    // and the migration's ordinals reproduce the legacy obstacles.arrow order.
    let mut index_rows: Vec<(u32, usize, usize)> = Vec::new();
    for (batch_idx, batch) in batches.iter().enumerate() {
        let geom = batch
            .column_by_name("geom")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| format!("{}: missing geom", structures_arrow.display()))?;
        let ordinals = batch
            .column_by_name("screening_ordinal")
            .and_then(|c| c.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| format!("{}: missing screening_ordinal", structures_arrow.display()))?;
        for i in 0..batch.num_rows() {
            if geom.is_null(i) {
                continue; // a geometry-less row screens nothing (the schema allows null)
            }
            if ordinals.is_null(i) {
                return Err(format!(
                    "{}: row {i} has geometry but no screening_ordinal",
                    structures_arrow.display()
                ));
            }
            index_rows.push((ordinals.value(i), batch_idx, i));
        }
    }
    index_rows.sort_unstable_by_key(|&(ordinal, _, _)| ordinal);
    let mut next_id: u32 = 0;
    for &(ordinal, batch_idx, i) in &index_rows {
        // The façade-exposure rows name footprints by `screening_ordinal`; the
        // index names them by id. The producer numbers geometry rows densely,
        // so the two are one number.
        if ordinal != next_id {
            return Err(format!(
                "{}: screening_ordinal {ordinal} where {next_id} was due (ordinals must be dense)",
                structures_arrow.display()
            ));
        }
        let batch = &batches[batch_idx];
        let kinds = batch
            .column_by_name("kind")
            .and_then(|c| c.as_any().downcast_ref::<UInt8Array>())
            .ok_or_else(|| format!("{}: missing kind", structures_arrow.display()))?;
        let geom = batch
            .column_by_name("geom")
            .and_then(|c| c.as_any().downcast_ref::<BinaryArray>())
            .ok_or_else(|| format!("{}: missing geom", structures_arrow.display()))?;
        let heights = batch_heights[batch_idx];
        let id = next_id;
        match kinds.value(i) {
            STRUCTURE_KIND_BUILDING => {
                let (polygons, height, _, _) = building_geometry_and_height(
                    batch,
                    i,
                    f32::from(heights.value(i)),
                    &low_profile,
                )
                .map_err(|error| format!("{}: {error}", structures_arrow.display()))?;
                let class = batch
                    .column_by_name("envelope_class")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>())
                    .filter(|a| !a.is_null(i))
                    .map(|a| EnvelopeClass::from_u8(a.value(i)))
                    .unwrap_or(EnvelopeClass::Default);
                // Grid rings reach the index through the same WKB ingestion
                // every other loader uses, so the envelope class travels with
                // the footprint exactly as before.
                let wkb = polygons_wkb(&polygons);
                builder.add_polygon_wkb(&wkb, height, ObstacleKind::Building, id, class);
            }
            // Walls keep their mapped height: the cap is a building-only
            // correction (noise_compute::low_profile caps typology/GHSL heights), and
            // add_polyline never clamps to the building height ceiling.
            STRUCTURE_KIND_BARRIER => {
                let ring = decode_geom(Some(geom.value(i))).ok_or_else(|| {
                    format!(
                        "{}: row {i} invalid wall geometry",
                        structures_arrow.display()
                    )
                })?;
                if ring.len() < 2 {
                    return Err(format!(
                        "{}: row {i} geom is not a wall microsegment",
                        structures_arrow.display()
                    ));
                }
                let pts: Vec<(f64, f64)> = ring_lonlat(&ring)
                    .into_iter()
                    .map(|(lon, lat)| (lat, lon))
                    .collect();
                builder.add_polyline(&pts, f32::from(heights.value(i)), ObstacleKind::Barrier, id);
            }
            other => {
                return Err(format!(
                    "{}: unknown structure kind {other} at row {i}",
                    structures_arrow.display()
                ));
            }
        }
        next_id = next_id.wrapping_add(1);
    }
    Ok(builder.build())
}

/// Screening topology and height are identical for the index and display.
fn building_geometry_and_height(
    batch: &arrow::record_batch::RecordBatch,
    row: usize,
    raw_height: f32,
    low_profile: &LowProfileLookup,
) -> Result<(grid::poly::GridPolygons, f32, u8, bool), String> {
    let polygons = col_binary(batch, "geom")
        .filter(|column| !column.is_null(row))
        .and_then(|column| grid::poly::decode_grid_polygons(column.value(row)))
        .ok_or_else(|| format!("row {row} has invalid building topology"))?;
    let height_source = col_u8(batch, "height_source")
        .filter(|column| !column.is_null(row))
        .ok_or_else(|| format!("row {row} has no height_source"))?
        .value(row);
    let mut height = raw_height;
    if let (Some(gx), Some(gy)) = (col_i32(batch, "centroid_gx"), col_i32(batch, "centroid_gy")) {
        if !gx.is_null(row) && !gy.is_null(row) {
            let (lon, lat) =
                square_store::grid_cols::grid_cell_lonlat(gx.value(row), gy.value(row));
            let area = col_f32(batch, "area_m2")
                .filter(|column| !column.is_null(row))
                .map(|column| column.value(row))
                .unwrap_or_else(|| {
                    // Preserve the existing cap's largest-exterior area fallback;
                    // every part and hole still reaches containment and screening.
                    polygons
                        .iter()
                        .filter_map(|rings| grid::poly::ring_area_m2(&rings[0]))
                        .fold(0.0, f64::max) as f32
                });
            height = low_profile.capped_height(
                raw_height,
                structure_contract::height_is_per_building(height_source),
                lat,
                lon,
                area,
            );
        }
    }
    Ok((polygons, height, height_source, height < raw_height))
}

/// One logical footprint with all polygon rings in lat/lon and as-used height.
#[derive(serde::Serialize)]
pub struct FootprintView {
    /// Polygon parts, each with exterior first and then holes.
    #[serde(rename = "p")]
    pub polygons: Vec<Vec<Vec<(f64, f64)>>>,
    #[serde(rename = "h")]
    pub height_m: f32,
    #[serde(rename = "t")]
    pub height_source: u8,
    #[serde(rename = "c")]
    pub capped: bool,
}

/// Footprints within the existing padded-centroid display gate, at as-used heights.
/// Enumerate every owner in that gate, including the viewport interior.
///
/// This overlay draws what the engine screens with, so it follows the physics
/// loader's rule rather than a softer one: a square that is provably empty
/// contributes nothing, and anything ELSE that stops us reading it is an error.
/// Returning an empty list on a broken shard would paint a transparent tile,
/// and on a noise map an absent building is indistinguishable from a quiet
/// place (raised in review, 2026-08-30).
pub fn footprints_in_bbox(
    prepared_year_dir: &Path,
    south: f64,
    west: f64,
    north: f64,
    east: f64,
) -> Result<Vec<FootprintView>, String> {
    let longitude_span = if west > east {
        east + 360.0 - west
    } else {
        east - west
    };
    let center_lon = west + longitude_span / 2.0;
    let pad = 0.01;
    let squares = grid::bounds::BoundedSquares::from_degrees(
        south - pad,
        west - pad,
        north + pad,
        west + longitude_span + pad,
    )
    .ok_or_else(|| "invalid footprint query bounds".to_string())?;
    let mut out = Vec::new();
    for square in squares.iter() {
        let path = square_dir(prepared_year_dir, square).join(STRUCTURES_ARROW);
        if !path.is_file() {
            continue; // nothing built here — nothing to draw
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
        // A square whose cap cannot be read must not contribute footprints at
        // their uncapped height: a wrong number here is worse than an error.
        let low_profile = low_profile_from_structures(&bytes, &path).map_err(|e| {
            format!(
                "structure_store: low-profile cap for {}: {e}",
                grid::square_name(square)
            )
        })?;
        let reader = FileReader::try_new(Cursor::new(&bytes), None)
            .map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
        for batch in reader {
            let batch = batch.map_err(|e| format!("structure_store: {}: {e}", path.display()))?;
            let heights = structure_contract::heights(&batch)?;
            let (Some(kinds), Some(geom), Some(cgxs), Some(cgys)) = (
                batch
                    .column_by_name("kind")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
                batch
                    .column_by_name("geom")
                    .and_then(|c| c.as_any().downcast_ref::<BinaryArray>()),
                batch
                    .column_by_name("centroid_gx")
                    .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
                batch
                    .column_by_name("centroid_gy")
                    .and_then(|c| c.as_any().downcast_ref::<Int32Array>()),
            ) else {
                continue;
            };
            for i in 0..batch.num_rows() {
                // Walls are not footprints; the overlay draws buildings only.
                if kinds.value(i) != STRUCTURE_KIND_BUILDING {
                    continue;
                }
                if geom.is_null(i) || heights.is_null(i) || cgxs.is_null(i) || cgys.is_null(i) {
                    continue;
                }
                let (clon, clat) =
                    square_store::grid_cols::grid_cell_lonlat(cgxs.value(i), cgys.value(i));
                if clat < south - pad
                    || clat > north + pad
                    || grid::geo::wrapped_longitude_delta(center_lon, clon).abs()
                        > longitude_span / 2.0 + pad
                {
                    continue;
                }
                let (polygons, height_m, height_source, capped) = building_geometry_and_height(
                    &batch,
                    i,
                    f32::from(heights.value(i)),
                    &low_profile,
                )
                .map_err(|error| format!("{}: {error}", path.display()))?;
                out.push(FootprintView {
                    polygons: polygons
                        .iter()
                        .map(|rings| {
                            rings
                                .iter()
                                .map(|ring| {
                                    ring_lonlat(ring)
                                        .into_iter()
                                        .map(|(lon, lat)| (lat, lon))
                                        .collect()
                                })
                                .collect()
                        })
                        .collect(),
                    height_m,
                    height_source,
                    capped,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "structure_producer_tests.rs"]
mod producer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure_test_fixture as fx;
    use std::sync::Arc;
    use tempfile::TempDir;

    const LAT: f64 = 50.0;
    const LON: f64 = 14.25;

    fn prague() -> Square {
        grid::square_of(LAT, LON)
    }

    fn house_row() -> fx::StructureRow {
        fx::StructureRow {
            kind: STRUCTURE_KIND_BUILDING,
            ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
            height_m: 12,
            height_source: 0,
            envelope_class: 1, // Residential
            centroid_lonlat: Some((LON + 0.0001, LAT + 0.0001)),
            osm_id: Some(7),
            building_type: Some(1),
            area_m2: Some(450.0),
            ..Default::default()
        }
    }

    fn obstacle_set(year: &Path) -> ObstacleSet {
        load_obstacle_set(year, LAT, LON).unwrap()
    }

    #[test]
    fn verified_structure_bytes_build_without_reopening_the_label() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("structures.arrow");
        fx::write_structure_file(&path, &[house_row()], true);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let index = build_obstacle_index_from_arrow_bytes(prague(), &bytes, &path).unwrap();
        let set = ObstacleSet {
            indexes: vec![Arc::new(index)],
        };
        let winner = set.enclosed_footprint_at(LAT + 0.0001, LON + 0.0001).unwrap();
        assert_eq!(winner.class, EnvelopeClass::Residential);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn enclosed_winner_names_its_square_row_class_and_height() {
        let tmp = TempDir::new().unwrap();
        fx::write_square_structures(tmp.path(), prague(), &[house_row()]);
        let set = obstacle_set(tmp.path());
        assert!(!set.indexes.is_empty());
        let winner = set
            .enclosed_footprint_at(LAT + 0.0001, LON + 0.0001)
            .expect("click inside the footprint must be enclosed");
        assert_eq!(winner.class, EnvelopeClass::Residential);
        assert_eq!(winner.key.square(), prague());
        assert_eq!(winner.key.id, 0);
        assert!(
            (winner.height_m - 12.0).abs() < 0.5,
            "h={}",
            winner.height_m
        );
        assert!(set.enclosed_footprint_at(LAT + 0.5, LON + 0.5).is_none());
        let bytes = std::fs::read(fx::square_dir(tmp.path(), prague()).join(STRUCTURES_ARROW)).unwrap();
        let footprints = enclosed_building_footprints(&bytes, Path::new("fixture")).unwrap();
        assert_eq!(footprints.len(), 1);
        assert_eq!(footprints[0].0, winner.key.id);
        let square = square_store::store::load_square(&fx::square_dir(tmp.path(), prague())).unwrap();
        let rings = footprint_polygons_at(&square.structures, 0, LAT + 0.0001, LON + 0.0001).unwrap();
        assert_eq!(rings, footprints[0].1);
    }

    #[test]
    fn hover_winner_names_outdoor_footprints_that_building_exposure_ignores() {
        let tmp = TempDir::new().unwrap();
        let mut row = house_row();
        row.envelope_class = 0; // Outdoor carport
        fx::write_square_structures(tmp.path(), prague(), &[row]);
        let set = obstacle_set(tmp.path());
        let (class, _) = point_inside_footprint(&set, LAT + 0.0001, LON + 0.0001)
            .expect("hover must see the carport");
        assert_eq!(class, EnvelopeClass::Outdoor);
        assert!(set.enclosed_footprint_at(LAT + 0.0001, LON + 0.0001).is_none());
        let bytes = std::fs::read(fx::square_dir(tmp.path(), prague()).join(STRUCTURES_ARROW)).unwrap();
        assert!(enclosed_building_footprints(&bytes, Path::new("fixture")).unwrap().is_empty());
    }

    #[test]
    fn wall_rows_index_without_becoming_footprints() {
        let tmp = TempDir::new().unwrap();
        fx::write_square_structures(
            tmp.path(),
            prague(),
            &[fx::StructureRow {
                kind: STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(LON, LAT), (LON + 0.001, LAT + 0.001)]),
                height_m: 3,
                height_source: 0,
                envelope_class: 0,
                centroid_lonlat: Some((LON + 0.0005, LAT + 0.0005)),
                osm_id: Some(11),
                segment_idx: Some(2),
                ..Default::default()
            }],
        );
        let set = obstacle_set(tmp.path());
        assert!(!set.indexes.is_empty());
        // A wall is not a footprint: neither probe fires on its midpoint.
        assert!(point_inside_footprint(&set, LAT + 0.0005, LON + 0.0005).is_none());
        let fps =
            footprints_in_bbox(tmp.path(), LAT - 0.01, LON - 0.01, LAT + 0.01, LON + 0.01).unwrap();
        assert!(fps.is_empty());
    }

    #[test]
    fn dateline_wall_stays_local_after_structures_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let square = grid::square_of(0.0, -180.0);
        fx::write_square_structures(
            tmp.path(),
            square,
            &[fx::StructureRow {
                kind: STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(179.999, 0.0), (-179.999, 0.0)]),
                height_m: 3,
                height_source: 0,
                envelope_class: 0,
                centroid_lonlat: Some((-180.0, 0.0)),
                osm_id: Some(11),
                segment_idx: Some(0),
                ..Default::default()
            }],
        );

        let set = load_obstacle_set(tmp.path(), 0.0, 179.9).unwrap();
        assert_eq!(set.edge_count(), 1);
        let view = set
            .indexes
            .iter()
            .map(|index| index.gpu_view())
            .find(|view| !view.edges_xyxyh.is_empty())
            .expect("wall index");
        let wall_length_m = f64::from((view.edges_xyxyh[2] - view.edges_xyxyh[0]).abs());
        assert!(
            (wall_length_m - 222.64).abs() < 0.5,
            "wall length {wall_length_m} m"
        );
        // The grid spans the wall (to the next cell edge), not the world: a
        // seam wall read as world-spanning would grid 40 000 km.
        let grid_span_m = view.cols as f64 * view.cell_m;
        assert!(
            grid_span_m <= wall_length_m + view.cell_m,
            "short wall's grid spans {grid_span_m} m"
        );

        for lon in [179.8, -179.8] {
            let mut hits = Vec::new();
            set.crossings(-0.01, lon, 0.01, lon, &mut hits);
            assert!(
                hits.is_empty(),
                "phantom world-spanning wall at {lon}: {hits:?}"
            );
        }
        for lon in [179.9995, -179.9995] {
            let mut hits = Vec::new();
            set.crossings(-0.01, lon, 0.01, lon, &mut hits);
            assert_eq!(hits.len(), 1, "seam wall missing at {lon}: {hits:?}");
            assert_eq!(hits[0].kind, ObstacleKind::Barrier);
            assert_eq!(hits[0].height_m, 3.0);
            assert!((hits[0].t - 0.5).abs() < 0.001, "t={}", hits[0].t);
        }
    }

    #[test]
    fn footprints_carry_as_used_height_and_ring() {
        let tmp = TempDir::new().unwrap();
        fx::write_square_structures(tmp.path(), prague(), &[house_row()]);
        let fps =
            footprints_in_bbox(tmp.path(), LAT - 0.01, LON - 0.01, LAT + 0.01, LON + 0.01).unwrap();
        assert_eq!(fps.len(), 1);
        assert!(
            (fps[0].height_m - 12.0).abs() < 0.5,
            "h={}",
            fps[0].height_m
        );
        assert_eq!(fps[0].height_source, 0);
        assert!(!fps[0].capped);
        assert_eq!(fps[0].polygons[0][0].len(), 5);
        assert!((fps[0].polygons[0][0][0].0 - LAT).abs() < 0.0001);
        assert!((fps[0].polygons[0][0][0].1 - LON).abs() < 0.0001);
    }

    #[test]
    fn viewport_interior_footprints_are_not_limited_to_corner_and_center_owners() {
        let tmp = TempDir::new().unwrap();
        let mut row = house_row();
        row.ring_lonlat = Some(fx::square_ring_lonlat(2.0, 5.0));
        row.centroid_lonlat = Some((5.0001, 2.0001));
        fx::write_square_structures(tmp.path(), grid::square_of(2.0001, 5.0001), &[row]);
        let footprints = footprints_in_bbox(tmp.path(), 0.0, 0.0, 10.0, 10.0).unwrap();
        assert_eq!(footprints.len(), 1);
        assert_eq!(footprints[0].height_m, 12.0);
        assert!(footprints_in_bbox(tmp.path(), 0.0, 0.0, 1.0, 1.0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn dateline_footprint_survives_both_overlay_tile_queries() {
        for centroid_lon in [-179.9998, 179.9998] {
            let tmp = TempDir::new().unwrap();
            let mut row = house_row();
            row.centroid_lonlat = Some((centroid_lon, 10.0005));
            row.ring_lonlat = Some(vec![
                (179.999, 10.0),
                (-179.999, 10.0),
                (-179.999, 10.001),
                (179.999, 10.001),
                (179.999, 10.0),
            ]);
            fx::write_square_structures(tmp.path(), grid::square_of(10.0005, centroid_lon), &[row]);
            for (west, east) in [(179.99, 180.0), (-180.0, -179.99), (179.99, -179.99)] {
                let footprints = footprints_in_bbox(tmp.path(), 9.99, west, 10.01, east).unwrap();
                assert_eq!(
                    footprints.len(),
                    1,
                    "centroid {centroid_lon}, bbox {west}..{east}"
                );
                assert_eq!(footprints[0].height_m, 12.0);
                assert_eq!(footprints[0].polygons[0][0].len(), 5);
            }
            assert!(
                footprints_in_bbox(tmp.path(), 9.99, 179.96, 10.01, 179.97)
                    .unwrap()
                    .is_empty(),
                "longitude padding must remain bounded"
            );
        }
    }

    #[test]
    fn missing_square_dir_is_empty_not_an_error() {
        let tmp = TempDir::new().unwrap();
        let set = load_obstacle_set(tmp.path(), LAT, LON).unwrap();
        assert!(set.indexes.is_empty());
    }

    /// The contract gate sits in the pipeline step: an unstamped table is
    /// neither blocked nor indexed, so no popup can ever map one built from it.
    #[test]
    fn unstamped_table_gets_no_index() {
        let tmp = TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_structure_file(&dir.join("structures.arrow"), &[house_row()], false);
        let unstamped = std::fs::read(dir.join("structures.arrow")).unwrap();
        let err =
            crate::structures_finalize::finalize_square_structures(&dir, prague()).unwrap_err();
        assert!(err.contains("structures_contract mismatch"), "got: {err}");
        assert!(!dir.join("structures.qoix").exists());
        assert_eq!(std::fs::read(dir.join("structures.arrow")).unwrap(), unstamped);
    }

    #[test]
    fn square_center_is_query_independent_and_sane() {
        let (lat, lon) = square_center_latlon(prague());
        assert!((lon - 14.24).abs() < 0.4, "lon={lon}");
        assert!((lat - 50.1).abs() < 0.4, "lat={lat}");
    }
}

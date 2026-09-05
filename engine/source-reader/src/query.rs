//! Pure point-query logic: Arrow batches → typed noise-source views.
//! Sits below the NAPI glue in `lib.rs` (the popup engine entry point) and
//! holds the zero-copy collection that both `collect_sources_at_point` and
//! `query_noise_at_point` delegate to, plus the segment top-K cap applied to
//! the popup's trace output. No global `STORE`/`RASTERS` state here — callers
//! hand in pre-loaded square data.
//!
//! Grid port: on-disk coordinates are z30 int32 grid cells (`start_gx` …,
//! `centroid_gx/gy`, `geom` grid rings). They are converted to lon/lat floats
//! at the read edge via `square_store::grid_cols`, exactly once per value.
//! `normalize::prepare_*` takes the decoded grid rings directly; the debug
//! `polygon_wkb` strings stay WKB-shaped (synthesized from the grid ring) so
//! the `query_buildings` / `query_leisure` JSON keeps its contract.

use std::path::Path;

use arrow::array::Array;
use square_store::grid_cols::{
    col_binary, col_bool, col_f32, col_i16, col_i32, col_i64, col_str, col_u16, col_u8,
    decode_geom, grid_cell_lonlat, ring_lonlat,
};
use square_store::store::{load_square, SquareData, STRUCTURE_KIND_BUILDING};

const BUILDING_QUERY_RADIUS_M: f64 = 2_000.0;
const INDUSTRIAL_QUERY_RADIUS_M: f64 = 5_000.0;
// Existing row gates bound accepted line midpoints by 1.5 times their reach.
pub(crate) const LINE_MIDPOINT_REACH_FACTOR: f64 = 1.5;

#[derive(Debug)]
pub struct PointQueryData {
    pub roads: Vec<noise_compute::types::RoadSegment>,
    pub railways: Vec<noise_compute::types::RailSegment>,
    /// M4/M5 per-row baked admins, aligned by index with `roads`/`railways`
    /// (`None` entries = pre-bake rows → receiver fallback). Installed into
    /// the kernels' thread-local channels around `compute_at_point*` (see
    /// `lib.rs::query_noise_impl`) — the segment structs are codever-SHARED
    /// and cannot carry the field.
    pub road_admins: Vec<Option<noise_compute::admin::Admin>>,
    pub rail_admins: Vec<Option<noise_compute::admin::Admin>>,
    pub buildings: Vec<noise_compute::types::PointSource>,
    pub industrial: Vec<noise_compute::types::PointSource>,
    /// v6 aircraft popup arrows. Rows are consumed via typed views in
    /// `compute_aircraft_v6` — no AircraftSegment synthesis happens here.
    pub aircraft_airborne_batches: Vec<arrow::record_batch::RecordBatch>,
    pub aircraft_cruise_batches: Vec<arrow::record_batch::RecordBatch>,
    /// `airport_traffic.arrow` per-microsegment sparse counters.
    pub aircraft_airport_traffic_batches: Vec<arrow::record_batch::RecordBatch>,
    /// `airport_lines.arrow` OSM ids and refs used to label runway and
    /// taxiway segment traces.
    pub airport_lines_batches: Vec<arrow::record_batch::RecordBatch>,
    pub n_days: u16,
}

// NACE codes are written directly into industrial.arrow by enrichment scripts.

/// Select every owner in the existing surface-source midpoint gates.
/// Airborne and cruise are support-copied and consumed only from the receiver cell.
pub fn squares_within_reach(lat: f64, lng: f64) -> Result<Vec<grid::Square>, String> {
    let radius = noise_compute::constants::RAILWAY_REACH_CEILING
        .max(noise_compute::constants::ROAD_MAX_RADIUS[0])
        .max(noise_compute::constants::GROUND_OPS_RUNWAY_MAX_RADIUS)
        .max(BUILDING_QUERY_RADIUS_M)
        .max(INDUSTRIAL_QUERY_RADIUS_M);
    squares_within_radius(lat, lng, radius * LINE_MIDPOINT_REACH_FACTOR)
}

pub fn squares_within_radius(
    lat: f64,
    lng: f64,
    radius_m: f64,
) -> Result<Vec<grid::Square>, String> {
    if !lat.is_finite()
        || lat.abs() > 90.0
        || !lng.is_finite()
        || !radius_m.is_finite()
        || radius_m < 0.0
    {
        return Err("invalid source query position or radius".to_string());
    }
    let lng = grid::geo::normalize_longitude(lng);
    let (latitude_radius, longitude_radius) = grid::geo::reach_box_half_extents_deg(lat, radius_m);
    let bounds = grid::bounds::BoundedSquares::from_degrees(
        (lat - latitude_radius).next_down(),
        (lng - longitude_radius).next_down(),
        (lat + latitude_radius).next_up(),
        (lng + longitude_radius).next_up(),
    )
    .ok_or_else(|| "invalid source query bounds".to_string())?;
    Ok(bounds.iter().collect())
}

/// Directory `<prepared_year>/z9/<x>/<y>` for one square.
pub fn square_dir(prepared_year_dir: &Path, square: grid::Square) -> std::path::PathBuf {
    prepared_year_dir
        .join("z9")
        .join(square.x.to_string())
        .join(square.y.to_string())
}

pub fn collect_sources_at_point(
    prepared_year_dir: &Path,
    lat: f64,
    lng: f64,
) -> Result<PointQueryData, String> {
    let squares = squares_within_reach(lat, lng)?;
    let loaded: Vec<SquareData> = squares
        .iter()
        .map(|sq| load_square(&square_dir(prepared_year_dir, *sq)))
        .collect::<Result<_, _>>()?;
    let refs: Vec<_> = squares.into_iter().zip(&loaded).collect();

    collect_from_square_data(&refs, lat, lng)
}

/// Shared source collection logic. Takes pre-loaded square data.
/// Both `collect_sources_at_point` and `query_noise_at_point` delegate here.
///
/// NACE codes are read directly from industrial.arrow nace_4digit column.
pub fn collect_from_square_data(
    square_data: &[(grid::Square, &SquareData)],
    lat: f64,
    lng: f64,
) -> Result<PointQueryData, String> {
    let mut all_roads = Vec::new();
    let mut all_railways = Vec::new();
    let mut all_road_admins = Vec::new();
    let mut all_rail_admins = Vec::new();
    let mut all_buildings = Vec::new();
    let mut all_industrial = Vec::new();
    let mut all_airborne_batches: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let mut all_cruise_batches: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let mut all_airport_traffic_batches: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let receiver_square = grid::square_of(lat, lng);
    let mut n_days_from_metadata: Option<u16> = None;
    // Prune aircraft batches per square ONCE; the collection below consumes the
    // result. Airborne shares the row/segment axis envelope. Cruise has a
    // rep_len-dependent centroid radius, not the airborne envelope;
    // airport traffic's row accept is a planar circle.
    let airborne_gate = airborne_envelope_gate(lat, lng);
    let per_square_aircraft: Vec<(
        Vec<arrow::record_batch::RecordBatch>,
        Vec<arrow::record_batch::RecordBatch>,
        Vec<arrow::record_batch::RecordBatch>,
    )> = square_data
        .iter()
        .map(|(square, data)| {
            Ok((
                if *square == receiver_square {
                    data.aircraft_airborne.batches_where(&airborne_gate)?
                } else {
                    Vec::new()
                },
                if *square == receiver_square {
                    data.aircraft_cruise.batches_all()?
                } else {
                    Vec::new()
                },
                data.aircraft_airport_traffic.batches_within(
                    lat,
                    lng,
                    noise_compute::constants::GROUND_OPS_RUNWAY_MAX_RADIUS,
                )?,
            ))
        })
        .collect::<Result<_, String>>()?;
    // The label lookup needs every selected airport-line row, but only
    // when nearby airport traffic exists. Keep the files footer-only for all
    // other clicks.
    let all_airport_lines_batches = if per_square_aircraft
        .iter()
        .any(|(_, _, traffic)| !traffic.is_empty())
    {
        let mut batches = Vec::new();
        for (_, data) in square_data {
            batches.extend(data.airport_lines.batches_all()?);
        }
        batches
    } else {
        Vec::new()
    };
    // Support copies belong only to the receiver cell; ground rows retain their
    // owner cells. The file stamp is the sampling window even when no row is near.
    for (square, data) in square_data {
        for arrow in [
            (*square == receiver_square).then_some(&data.aircraft_airborne),
            (*square == receiver_square).then_some(&data.aircraft_cruise),
            Some(&data.aircraft_airport_traffic),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(schema) = arrow.schema() {
                let days = schema
                    .metadata()
                    .get("n_days")
                    .and_then(|value| value.parse::<u16>().ok())
                    .filter(|days| *days > 0)
                    .ok_or_else(|| {
                        "aircraft file has no valid n_days sampling window".to_string()
                    })?;
                n_days_from_metadata =
                    Some(n_days_from_metadata.map_or(days, |value| value.max(days)));
            }
        }
    }
    let n_days = n_days_from_metadata.unwrap_or(365);

    for ((_, data), (airborne_batches, cruise_batches, airport_traffic_batches)) in
        square_data.iter().zip(per_square_aircraft)
    {
        // The batch gate must cover the configured railway ceiling; each row's
        // exact reach is applied downstream after its emission is known.
        let railway_batches = data.railways.batches_within(
            lat,
            lng,
            noise_compute::constants::RAILWAY_REACH_CEILING,
        )?;
        let railways = query_railways_from_batches(
            &railway_batches,
            lat,
            lng,
            noise_compute::constants::RAILWAY_REACH_CEILING,
        );
        // Receiver-square admin for the C1 per-region period model. Only the scaled
        // counts / speed of `norm` feed `RailSegment` here; `compute_railways`
        // re-resolves the same admin for emission + reach, so this is for
        // signature consistency (and harmless if the table is uninitialised).
        // M5: a row with baked columns overrides this with its own admin.
        let rail_admin = noise_compute::admin::admin_for_latlng(lat, lng);
        for r in railways {
            let norm = noise_compute::normalize::normalize_rail(
                noise_compute::normalize::RawRailInput {
                    rail_type: r.rail_type,
                    usage: r.usage,
                    maxspeed: r.maxspeed,
                    service: r.service,
                    highspeed: r.highspeed,
                    trains_passenger: r.trains_passenger,
                    trains_freight: r.trains_freight,
                    parallel_divisor: r.parallel_divisor,
                },
                r.admin.unwrap_or(rail_admin),
            );
            let trains_passenger_source: u8 = if r.trains_passenger > 0 { 0 } else { 1 };
            let trains_freight_source: u8 = if r.trains_freight > 0 { 0 } else { 1 };
            let speed_source: u8 = if r.maxspeed > 0 {
                0
            } else if r.highspeed {
                1
            } else {
                2
            };

            all_railways.push(noise_compute::types::RailSegment {
                osm_id: r.osm_id,
                segment_idx: r.segment_idx,
                start_lat: r.start_lat,
                start_lon: r.start_lon,
                end_lat: r.end_lat,
                end_lon: r.end_lon,
                length_m: r.length_m,
                rail_type: r.rail_type,
                usage: r.usage,
                maxspeed: r.maxspeed,
                trains_passenger: norm.scaled_passenger_per_day,
                trains_freight: norm.scaled_freight_per_day,
                speed_kmh: norm.speed_kmh,
                track_count: 1,
                name: r.name.clone(),
                rail_ref: r.rail_ref.clone(),
                bridge: r.bridge,
                tunnel: r.tunnel,
                service: r.service > 0,
                highspeed: r.highspeed,
                parallel_divisor: r.parallel_divisor.max(1),
                speed_source,
                trains_passenger_source,
                trains_freight_source,
                source_id: r.source_id,
                dist_m: r.dist_m,
                cp_lat: r.cp_lat,
                cp_lon: r.cp_lon,
                fraction: r.fraction,
            });
            all_rail_admins.push(r.admin);
        }

        let road_batches =
            data.roads
                .batches_within(lat, lng, noise_compute::constants::ROAD_MAX_RADIUS[0])?;
        let roads = query_roads_from_batches(
            &road_batches,
            lat,
            lng,
            noise_compute::constants::ROAD_MAX_RADIUS[0],
        );
        for r in roads {
            all_roads.push(noise_compute::types::RoadSegment {
                osm_id: r.osm_id,
                segment_idx: r.segment_idx,
                start_lat: r.start_lat,
                start_lon: r.start_lon,
                end_lat: r.end_lat,
                end_lon: r.end_lon,
                length_m: r.length_m,
                road_class: r.road_class,
                speed_limit: r.speed_limit,
                speed_taper: r.speed_taper,
                surface_type: r.surface_type,
                oneway: r.oneway,
                lanes: r.lanes,
                aadt_light: r.aadt_light,
                aadt_medium: r.aadt_medium,
                aadt_heavy: r.aadt_heavy,
                aadt_moto: r.aadt_moto,
                source_id: r.source_id,
                name: r.name.clone(),
                road_ref: r.road_ref.clone(),
                bridge: r.bridge,
                tunnel: r.tunnel,
                access: r.access,
                junction: r.junction,
                built_up: r.built_up,
                dist_m: r.dist_m,
                cp_lat: r.cp_lat,
                cp_lon: r.cp_lon,
                fraction: r.fraction,
            });
            all_road_admins.push(r.admin);
        }

        let building_batches = data
            .structures
            .batches_within(lat, lng, BUILDING_QUERY_RADIUS_M)?;
        let buildings =
            query_buildings_from_batches(&building_batches, lat, lng, BUILDING_QUERY_RADIUS_M);
        for b in buildings {
            let display_name = if !b.name.is_empty() {
                b.name.clone()
            } else if !b.addr_street.is_empty() {
                if !b.addr_housenumber.is_empty() {
                    format!("{} {}", b.addr_street, b.addr_housenumber)
                } else {
                    b.addr_street.clone()
                }
            } else {
                String::new()
            };

            let prepared_points = noise_compute::normalize::prepare_building_points(
                noise_compute::normalize::RawBuildingInput {
                    centroid_lat: b.centroid_lat,
                    centroid_lon: b.centroid_lon,
                    height_m: b.height,
                    floors: b.floors,
                    building_type: b.building_type,
                    area_m2: (b.area_m2 > 0.0).then_some(b.area_m2 as f64),
                    polygon_grid: &b.polygon_grid,
                },
            );
            for prepared in prepared_points {
                let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                all_buildings.push(prepared.with_metadata(
                    b.osm_id,
                    b.building_type,
                    display_name.clone(),
                    b.polygon_grid.clone(),
                    pt_dist,
                ));
            }
        }

        // Leisure AREA sources fold into the building/settlement layer
        // (settlement v2 phase 2): same point-source compute, tagged with
        // `source_type = LEISURE_TYPE_BASE + sport` so the popup names a padel
        // court correctly (see source_names::building_type_name).
        let leisure_batches = data
            .leisure
            .batches_within(lat, lng, BUILDING_QUERY_RADIUS_M)?;
        let leisure =
            query_leisure_from_batches(&leisure_batches, lat, lng, BUILDING_QUERY_RADIUS_M);
        for lz in leisure {
            let source_type = noise_compute::types::LEISURE_TYPE_BASE.saturating_add(lz.sport);
            let prepared_points = noise_compute::normalize::prepare_leisure_points(
                noise_compute::normalize::RawLeisureInput {
                    centroid_lat: lz.centroid_lat,
                    centroid_lon: lz.centroid_lon,
                    sport: lz.sport,
                    area_m2: (lz.area_m2 > 0.0).then_some(lz.area_m2 as f64),
                    polygon_grid: &lz.polygon_grid,
                },
            );
            for prepared in prepared_points {
                let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                all_buildings.push(prepared.with_metadata(
                    lz.osm_id,
                    source_type,
                    lz.name.clone(),
                    lz.polygon_grid.clone(),
                    pt_dist,
                ));
            }
        }

        for batch in &data
            .industrial
            .batches_within(lat, lng, INDUSTRIAL_QUERY_RADIUS_M)?
        {
            let n = batch.num_rows();
            let (Some(cgx), Some(cgy)) =
                (col_i32(batch, "centroid_gx"), col_i32(batch, "centroid_gy"))
            else {
                continue;
            };
            let stype = col_u8(batch, "source_type");
            let hub_h = col_f32(batch, "hub_height");
            let power = col_f32(batch, "rated_power_kw");
            let ind_name = col_str(batch, "name");
            let geom_col = col_binary(batch, "geom");
            let area_col = col_f32(batch, "area_m2");

            for i in 0..n {
                let (c_lon, c_lat) = grid_cell_lonlat(cgx.value(i), cgy.value(i));
                let dist = grid::geo::flat_dist(lat, lng, c_lat, c_lon);
                if dist > INDUSTRIAL_QUERY_RADIUS_M {
                    continue;
                }
                // I-07 dedup: skip a same-site duplicate row the enricher suppressed
                // (kept parity with the heatmap loader — both must skip it).
                if batch
                    .column_by_name("suppressed")
                    .and_then(|c| c.as_any().downcast_ref::<arrow::array::UInt8Array>())
                    .map(|a| a.value(i))
                    .unwrap_or(0)
                    != 0
                {
                    continue;
                }

                let st = stype.map(|a| a.value(i)).unwrap_or(0);
                let iname = ind_name.map(|a| a.value(i).to_string()).unwrap_or_default();
                let osm_id = col_i64(batch, "osm_id").map(|a| a.value(i)).unwrap_or(0);
                // Wind rows (st == 10) are pure points: no polygon, matching
                // the heatmap loader.
                let polygon_grid: grid::poly::GridRing = if st == 10 {
                    Vec::new()
                } else {
                    geom_col
                        .filter(|g| !g.is_null(i))
                        .and_then(|g| decode_geom(Some(g.value(i))))
                        .unwrap_or_default()
                };
                let area_m2 = area_col.and_then(|a| {
                    let v = a.value(i);
                    if v > 0.0 {
                        Some(v as f64)
                    } else {
                        None
                    }
                });

                let sub = col_u8(batch, "site_subtype")
                    .map(|a| a.value(i))
                    .unwrap_or(0);
                let prepared_points = noise_compute::normalize::prepare_industrial_points(
                    noise_compute::normalize::RawIndustrialInput {
                        centroid_lat: c_lat,
                        centroid_lon: c_lon,
                        source_type: st,
                        site_subtype: sub,
                        hub_height_m: hub_h.and_then(|a| {
                            let value = a.value(i);
                            if value > 0.0 {
                                Some(value)
                            } else {
                                None
                            }
                        }),
                        rated_power_kw: power.and_then(|a| {
                            let value = a.value(i);
                            if value > 0.0 {
                                Some(value)
                            } else {
                                None
                            }
                        }),
                        area_m2,
                        polygon_grid: &polygon_grid,
                        nace_4digit: batch
                            .column_by_name("nace_4digit")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::UInt16Array>())
                            .map(|a| a.value(i))
                            .filter(|&v| v > 0),
                    },
                );
                // Dataset stamp (GEM / E-PRTR / …) → popup provenance tooltip.
                // `with_metadata` leaves source_id at 0 (shared with the
                // building path, which has no stamp column).
                let row_source_id = col_u16(batch, "source_id").map(|a| a.value(i)).unwrap_or(0);
                for prepared in prepared_points {
                    let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                    let mut ps = prepared.with_metadata(
                        osm_id,
                        st,
                        iname.clone(),
                        polygon_grid.clone(),
                        pt_dist,
                    );
                    ps.source_id = row_source_id;
                    all_industrial.push(ps);
                }
            }
        }

        // Aircraft popup arrows: bbox-gated above (per_square_aircraft);
        // per-row reach prune + emission contract live inside
        // compute_aircraft_v6. RecordBatch clones are refcount bumps on
        // Arc-backed Arrow buffers, not data copies.
        all_airborne_batches.extend(airborne_batches);
        all_cruise_batches.extend(cruise_batches);
        all_airport_traffic_batches.extend(airport_traffic_batches);
    }

    Ok(PointQueryData {
        roads: all_roads,
        railways: all_railways,
        road_admins: all_road_admins,
        rail_admins: all_rail_admins,
        buildings: all_buildings,
        industrial: all_industrial,
        aircraft_airborne_batches: all_airborne_batches,
        aircraft_cruise_batches: all_cruise_batches,
        aircraft_airport_traffic_batches: all_airport_traffic_batches,
        airport_lines_batches: all_airport_lines_batches,
        n_days,
    })
}

/// Road segment query result (references into mmap'd data, minimal copy).
#[derive(serde::Serialize)]
pub struct RoadResult {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub road_class: u8,
    pub speed_limit: u8,
    /// Taper-graded effective speed (0 = none; absent column reads 0).
    pub speed_taper: u8,
    pub surface_type: u8,
    pub oneway: bool,
    pub lanes: u8,
    pub name: String,
    #[serde(rename = "ref")]
    pub road_ref: String,
    pub bridge: bool,
    pub tunnel: bool,
    pub access: u8,
    pub junction: u8,
    pub built_up: u8,
    pub aadt_light: i32,
    pub aadt_medium: i32,
    pub aadt_heavy: i32,
    pub aadt_moto: i32,
    pub source_id: u16,
    pub dist_m: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub fraction: f64,
    /// M4: the row's own baked admin when its batch carried the M3 triplet
    /// (`None` = no columns → receiver-admin fallback in the kernel). Engine
    /// side-channel — never on the wire (`RoadSegment` is codever-SHARED and
    /// cannot carry it, so `query.rs` aligns these with the segment vec).
    #[serde(skip_serializing)]
    pub admin: Option<noise_compute::admin::Admin>,
}

/// Scan road batches, filter by distance, return results.
pub fn query_roads_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<RoadResult> {
    let mut results = Vec::new();

    // Admin resolved once per popup call — lat/lng is the query centre.
    // Falls back to UNKNOWN → WORLD_DEFAULT when the table isn't loaded.
    let admin = noise_compute::admin::admin_for_latlng(lat, lon);

    for batch in batches {
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id");
        let seg_idx = col_i16(batch, "segment_idx");
        let sgx = col_i32(batch, "start_gx");
        let sgy = col_i32(batch, "start_gy");
        let egx = col_i32(batch, "end_gx");
        let egy = col_i32(batch, "end_gy");
        let len = col_f32(batch, "length_m");
        let rclass = col_u8(batch, "road_class");
        let speed = col_u8(batch, "speed_limit");
        // Absent on pre-taper arrows → 0 = none (the taper step writes it).
        let speed_taper_col = col_u8(batch, "speed_taper");
        let surface = col_u8(batch, "surface_type");
        let ow = col_bool(batch, "oneway");
        let lanes = col_u8(batch, "lanes");
        let name = col_str(batch, "name");
        let road_ref = col_str(batch, "ref");
        let bridge_col: Option<&arrow::array::BooleanArray> = batch
            .column_by_name("bridge")
            .and_then(|c| c.as_any().downcast_ref());
        let tunnel_col: Option<&arrow::array::BooleanArray> = batch
            .column_by_name("tunnel")
            .and_then(|c| c.as_any().downcast_ref());
        let access_col = col_u8(batch, "access");
        let junction_col = col_u8(batch, "junction");
        // Absent on pre-migration arrows → 0 = unknown → the legacy speed table.
        let built_up_col = col_u8(batch, "built_up");
        let aadt_l = col_i32(batch, "aadt_light");
        let aadt_m = col_i32(batch, "aadt_medium");
        let aadt_h = col_i32(batch, "aadt_heavy");
        let aadt_mo = col_i32(batch, "aadt_moto");
        // Single `source_id` column; provenance via
        // `noise_compute::sources::provenance_of(source_id)`.
        let source_id_col = col_u16(batch, "source_id");
        // M3 baked admin triplet (all-or-none at bake time). The `country_iso`
        // column's PRESENCE is the fallback switch: a present 0 bakes
        // `Admin::UNKNOWN` (WORLD defaults, NO receiver fallback); only an
        // ABSENT column takes the receiver admin. Tolerant reads — a
        // wrong-typed column reads as absent (the bake hard-fails instead).
        let country_iso_col = col_u16(batch, "country_iso");
        let city_id_col = col_u16(batch, "city_id");
        let continent_col = col_u8(batch, "continent");

        // All required columns must be present
        let (Some(osm_id), Some(sgx), Some(sgy), Some(egx), Some(egy)) =
            (osm_id, sgx, sgy, egx, egy)
        else {
            continue;
        };

        for i in 0..n {
            // Grid endpoints → lon/lat once per row; the gates below run on
            // floats exactly as before.
            let (s_lon, s_lat) = grid_cell_lonlat(sgx.value(i), sgy.value(i));
            let (e_lon, e_lat) = grid_cell_lonlat(egx.value(i), egy.value(i));
            // Cheap bbox reject FIRST, before the per-row normalize cascade.
            // ~99% of rows are far from the popup point (popup hits ~1-2 k of
            // ~900 k road segments per ring); running normalize_road on all
            // of them was the dominant cost in collect_from_square_data
            // (~160 ms warm). max_radius is the upper bound — final accept
            // uses effective_radius after normalize.
            let mid_lat = (s_lat + e_lat) * 0.5;
            let dlat = (lat - mid_lat).abs() * grid::geo::M_PER_DEG_LAT;
            if dlat > max_radius * LINE_MIDPOINT_REACH_FACTOR {
                continue;
            }
            let mid_lon = grid::geo::wrapped_longitude_midpoint(s_lon, e_lon);
            let dlon = grid::geo::wrapped_longitude_delta(mid_lon, lon).abs()
                * grid::geo::m_per_deg_lon(mid_lat.to_radians());
            if dlon > max_radius * LINE_MIDPOINT_REACH_FACTOR {
                continue;
            }

            let source_id = source_id_col.map(|a| a.value(i)).unwrap_or(0);
            // The row's own baked admin when the column is present (M4), else
            // `None` → the receiver admin (pre-bake behaviour, unchanged).
            let row_admin = country_iso_col.map(|iso| {
                noise_compute::defaults::baked_admin(
                    iso.value(i),
                    city_id_col.map(|c| c.value(i)).unwrap_or(0),
                    continent_col.map(|c| c.value(i)).unwrap_or(0),
                )
            });
            let raw = noise_compute::normalize::RawRoadInput {
                road_class: rclass.map(|a| a.value(i)).unwrap_or(0),
                speed_limit: speed.map(|a| a.value(i)).unwrap_or(0),
                speed_taper: speed_taper_col.map(|a| a.value(i)).unwrap_or(0),
                surface_type: surface.map(|a| a.value(i)).unwrap_or(0),
                oneway: ow.map(|a| a.value(i)).unwrap_or(false),
                lanes: lanes.map(|a| a.value(i)).unwrap_or(0),
                aadt_light: aadt_l.map(|a| a.value(i)).unwrap_or(0),
                aadt_medium: aadt_m.map(|a| a.value(i)).unwrap_or(0),
                aadt_heavy: aadt_h.map(|a| a.value(i)).unwrap_or(0),
                aadt_moto: aadt_mo.map(|a| a.value(i)).unwrap_or(0),
                provenance: noise_compute::sources::provenance_of(source_id),
                tunnel: tunnel_col.map(|a| a.value(i)).unwrap_or(false),
                access: access_col.map(|a| a.value(i)).unwrap_or(0),
                junction: junction_col.map(|a| a.value(i)).unwrap_or(0),
                built_up: built_up_col.map(|a| a.value(i)).unwrap_or(0),
            };
            let Some(norm) =
                noise_compute::normalize::normalize_road(raw, row_admin.unwrap_or(admin))
            else {
                continue;
            };
            let effective_radius = max_radius.min(norm.max_distance_m);

            // Tighter bbox reject using effective_radius (per-class).
            if dlat > effective_radius * LINE_MIDPOINT_REACH_FACTOR
                || dlon > effective_radius * LINE_MIDPOINT_REACH_FACTOR
            {
                continue;
            }

            // Exact closest point on segment
            let cp = grid::geo::closest_point_on_segment(lat, lon, s_lat, s_lon, e_lat, e_lon);
            if cp.dist_m > effective_radius {
                continue;
            }

            results.push(RoadResult {
                osm_id: osm_id.value(i),
                segment_idx: seg_idx.map(|a| a.value(i)).unwrap_or(0),
                start_lat: s_lat,
                start_lon: s_lon,
                end_lat: e_lat,
                end_lon: e_lon,
                // Derive from the endpoints when the column is missing or
                // ZERO, exactly as the tile loaders do
                // (tile-painter/src/source_loader_road.rs). Taking 0.0 here made
                // the popup and the tiles disagree in ONE DIRECTION: the arc
                // pre-gate is `length_m > min_span_rad * dist`, which at the
                // shipped `min_span_rad = 0.0` reads `0.0 > 0.0` = false, so the
                // popup silently skipped arc screening and fell back to the
                // closest-point verdict for that segment while the tile
                // arc-screened it. Same row, same physics, two answers — and the
                // popup is the lane the owner clicks. (Review 2026-08-04.)
                length_m: len
                    .map(|a| a.value(i))
                    .filter(|l| *l > 0.0)
                    .unwrap_or_else(|| grid::geo::flat_dist(s_lat, s_lon, e_lat, e_lon) as f32),
                road_class: raw.road_class,
                speed_limit: raw.speed_limit,
                speed_taper: raw.speed_taper,
                surface_type: raw.surface_type,
                oneway: raw.oneway,
                lanes: raw.lanes,
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                road_ref: road_ref.map(|a| a.value(i).to_string()).unwrap_or_default(),
                bridge: bridge_col.map(|a| a.value(i)).unwrap_or(false),
                tunnel: raw.tunnel,
                access: raw.access,
                junction: raw.junction,
                built_up: raw.built_up,
                aadt_light: raw.aadt_light,
                aadt_medium: raw.aadt_medium,
                aadt_heavy: raw.aadt_heavy,
                aadt_moto: raw.aadt_moto,
                source_id,
                dist_m: cp.dist_m,
                cp_lat: cp.lat,
                cp_lon: cp.lon,
                fraction: cp.fraction,
                admin: row_admin,
            });
        }
    }

    results
}

#[derive(serde::Serialize)]
pub struct BuildingResult {
    pub osm_id: i64,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub height: f32,
    pub floors: u8,
    pub area_m2: f32,
    pub building_type: u8,
    pub building_use: u8,
    pub name: String,
    pub addr_street: String,
    pub addr_housenumber: String,
    pub polygon_wkb: String,
    pub dist_m: f64,
    /// Original OSM grid ring (`emission_geom`); drives the emission
    /// compute. Never on the wire.
    #[serde(skip_serializing)]
    pub polygon_grid: grid::poly::GridRing,
}

/// Railway segment query result.
#[derive(serde::Serialize)]
pub struct RailResult {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub rail_type: u8,
    pub usage: u8,
    pub maxspeed: u16,
    pub name: String,
    pub rail_ref: String,
    pub bridge: bool,
    pub tunnel: bool,
    pub service: u8,
    pub highspeed: bool,
    pub trains_passenger: i32,
    pub trains_freight: i32,
    pub parallel_divisor: u8,
    pub source_id: u16,
    pub dist_m: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub fraction: f64,
    /// M5: the row's own baked admin when its batch carried the M3 triplet
    /// (`None` = no columns → receiver-admin fallback in the kernel). Engine
    /// side-channel — never on the wire (see `RoadResult::admin`).
    #[serde(skip_serializing)]
    pub admin: Option<noise_compute::admin::Admin>,
}

pub fn query_railways_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<RailResult> {
    let mut results = Vec::new();

    for batch in batches {
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id");
        let sgx = col_i32(batch, "start_gx");
        let sgy = col_i32(batch, "start_gy");
        let egx = col_i32(batch, "end_gx");
        let egy = col_i32(batch, "end_gy");

        let (Some(osm_id), Some(sgx), Some(sgy), Some(egx), Some(egy)) =
            (osm_id, sgx, sgy, egx, egy)
        else {
            continue;
        };

        let seg_idx = col_i16(batch, "segment_idx");
        let len = col_f32(batch, "length_m");
        let rtype = col_u8(batch, "rail_type");
        let usage = col_u8(batch, "usage");
        let maxspd = col_u16(batch, "maxspeed");
        let name = col_str(batch, "name");
        let rail_ref = col_str(batch, "ref");
        let bridge_col = col_bool(batch, "bridge");
        let tunnel_col = col_bool(batch, "tunnel");
        let service_col = col_u8(batch, "service");
        let highspeed_col = col_bool(batch, "highspeed");
        let trains_pax = col_i32(batch, "trains_passenger");
        let trains_frt = col_i32(batch, "trains_freight");
        let par_div = col_u8(batch, "parallel_divisor");
        let source_id_col = col_u16(batch, "source_id");
        // M3 baked admin triplet — the rail mirror of the road reads above
        // (M5: the row's own ISO drives the kernel's EU/world split).
        let country_iso_col = col_u16(batch, "country_iso");
        let city_id_col = col_u16(batch, "city_id");
        let continent_col = col_u8(batch, "continent");

        for i in 0..n {
            let (s_lon, s_lat) = grid_cell_lonlat(sgx.value(i), sgy.value(i));
            let (e_lon, e_lat) = grid_cell_lonlat(egx.value(i), egy.value(i));

            let mid_lat = (s_lat + e_lat) / 2.0;
            let mid_lon = grid::geo::wrapped_longitude_midpoint(s_lon, e_lon);
            let dlat = (lat - mid_lat).abs() * grid::geo::M_PER_DEG_LAT;
            if dlat > max_radius * LINE_MIDPOINT_REACH_FACTOR {
                continue;
            }
            let dlon = grid::geo::wrapped_longitude_delta(mid_lon, lon).abs()
                * grid::geo::m_per_deg_lon(mid_lat.to_radians());
            if dlon > max_radius * LINE_MIDPOINT_REACH_FACTOR {
                continue;
            }

            let cp = grid::geo::closest_point_on_segment(lat, lon, s_lat, s_lon, e_lat, e_lon);
            if cp.dist_m > max_radius {
                continue;
            }

            results.push(RailResult {
                osm_id: osm_id.value(i),
                segment_idx: seg_idx.map(|a| a.value(i)).unwrap_or(0),
                start_lat: s_lat,
                start_lon: s_lon,
                end_lat: e_lat,
                end_lon: e_lon,
                // Derive from the endpoints when the column is missing or
                // ZERO, exactly as the tile loaders do
                // (tile-painter/src/source_loader_road.rs). Taking 0.0 here made
                // the popup and the tiles disagree in ONE DIRECTION: the arc
                // pre-gate is `length_m > min_span_rad * dist`, which at the
                // shipped `min_span_rad = 0.0` reads `0.0 > 0.0` = false, so the
                // popup silently skipped arc screening and fell back to the
                // closest-point verdict for that segment while the tile
                // arc-screened it. Same row, same physics, two answers — and the
                // popup is the lane the owner clicks. (Review 2026-08-04.)
                length_m: len
                    .map(|a| a.value(i))
                    .filter(|l| *l > 0.0)
                    .unwrap_or_else(|| grid::geo::flat_dist(s_lat, s_lon, e_lat, e_lon) as f32),
                rail_type: rtype.map(|a| a.value(i)).unwrap_or(0),
                usage: usage.map(|a| a.value(i)).unwrap_or(0),
                maxspeed: maxspd.map(|a| a.value(i)).unwrap_or(0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                rail_ref: rail_ref.map(|a| a.value(i).to_string()).unwrap_or_default(),
                bridge: bridge_col.map(|a| a.value(i)).unwrap_or(false),
                tunnel: tunnel_col.map(|a| a.value(i)).unwrap_or(false),
                service: service_col.map(|a| a.value(i)).unwrap_or(0),
                highspeed: highspeed_col.map(|a| a.value(i)).unwrap_or(false),
                trains_passenger: trains_pax.map(|a| a.value(i)).unwrap_or(0),
                trains_freight: trains_frt.map(|a| a.value(i)).unwrap_or(0),
                parallel_divisor: par_div.map(|a| a.value(i)).unwrap_or(1),
                source_id: source_id_col.map(|a| a.value(i)).unwrap_or(0),
                dist_m: cp.dist_m,
                cp_lat: cp.lat,
                cp_lon: cp.lon,
                fraction: cp.fraction,
                admin: country_iso_col.map(|iso| {
                    noise_compute::emission::railway::baked_admin(
                        iso.value(i),
                        city_id_col.map(|c| c.value(i)).unwrap_or(0),
                        continent_col.map(|c| c.value(i)).unwrap_or(0),
                    )
                }),
            });
        }
    }
    results
}

/// Building emission rows of the merged structure table: kind=0 rows with a
/// valid `osm_id`, in file order — the old buildings.arrow subsequence with
/// the same values. The emission position is `emission_centroid_*` where the
/// merge kept the OSM centroid (matched rows screen at the Overture one), else
/// `centroid_*`; emission_geom always contains the original OSM ring or null.
pub fn query_buildings_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<BuildingResult> {
    let mut results = Vec::new();

    for batch in batches {
        let n = batch.num_rows();
        let kind = col_u8(batch, "kind");
        let osm_id = col_i64(batch, "osm_id");
        let cgx = col_i32(batch, "centroid_gx");
        let cgy = col_i32(batch, "centroid_gy");

        let (Some(kind), Some(osm_id), Some(cgx), Some(cgy)) = (kind, osm_id, cgx, cgy) else {
            continue;
        };

        let egx = col_i32(batch, "emission_centroid_gx");
        let egy = col_i32(batch, "emission_centroid_gy");
        let height = col_f32(batch, "height");
        let floors = col_u8(batch, "floors");
        let area = col_f32(batch, "area_m2");
        let btype = col_u8(batch, "building_type");
        let buse = col_u8(batch, "building_use");
        let name = col_str(batch, "name");
        let street = col_str(batch, "addr_street");
        let house = col_str(batch, "addr_housenumber");
        let emission_geom = col_binary(batch, "emission_geom");

        for i in 0..n {
            if kind.value(i) != STRUCTURE_KIND_BUILDING || osm_id.is_null(i) {
                continue;
            }
            let present_grid = |col: Option<&arrow::array::Int32Array>, j: usize| {
                col.filter(|a| !a.is_null(j)).map(|a| a.value(j))
            };
            let (e_lon, e_lat) = match (present_grid(egx, i), present_grid(egy, i)) {
                (Some(gx), Some(gy)) => grid_cell_lonlat(gx, gy),
                _ => grid_cell_lonlat(cgx.value(i), cgy.value(i)),
            };
            let dist = grid::geo::flat_dist(lat, lon, e_lat, e_lon);
            if dist > max_radius {
                continue;
            }

            let polygon_grid: grid::poly::GridRing = emission_geom
                .filter(|a| !a.is_null(i))
                .and_then(|a| decode_geom(Some(a.value(i))))
                .unwrap_or_default();
            let polygon_wkb = hex_encode(&grid_ring_to_wkb_polygon(&ring_lonlat(&polygon_grid)));

            let opt_f32 = |col: Option<&arrow::array::Float32Array>| {
                col.filter(|a| !a.is_null(i)).map(|a| a.value(i))
            };
            let opt_u8 = |col: Option<&arrow::array::UInt8Array>| {
                col.filter(|a| !a.is_null(i)).map(|a| a.value(i))
            };
            let opt_str = |col: Option<&arrow::array::StringArray>| {
                col.filter(|a| !a.is_null(i))
                    .map(|a| a.value(i).to_string())
                    .unwrap_or_default()
            };
            results.push(BuildingResult {
                osm_id: osm_id.value(i),
                centroid_lat: e_lat,
                centroid_lon: e_lon,
                height: opt_f32(height).unwrap_or(0.0),
                floors: opt_u8(floors).unwrap_or(0),
                area_m2: opt_f32(area).unwrap_or(0.0),
                building_type: opt_u8(btype).unwrap_or(0),
                building_use: opt_u8(buse).unwrap_or(0),
                name: opt_str(name),
                addr_street: opt_str(street),
                addr_housenumber: opt_str(house),
                polygon_wkb,
                dist_m: dist,
                polygon_grid,
            });
        }
    }

    results
}

/// One `leisure.arrow` row near the receiver (settlement v2 phase 2).
#[derive(serde::Serialize)]
pub struct LeisureResult {
    pub osm_id: i64,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    /// `emission::leisure` class id (PITCH/PADEL/…).
    pub sport: u8,
    pub area_m2: f32,
    pub name: String,
    pub polygon_wkb: String,
    pub dist_m: f64,
    /// Decoded grid ring; drives the emission compute. Never on the wire.
    #[serde(skip_serializing)]
    pub polygon_grid: grid::poly::GridRing,
}

pub fn query_leisure_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<LeisureResult> {
    let mut results = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let (Some(osm_id), Some(cgx), Some(cgy)) = (
            col_i64(batch, "osm_id"),
            col_i32(batch, "centroid_gx"),
            col_i32(batch, "centroid_gy"),
        ) else {
            continue;
        };
        let sport = col_u8(batch, "sport");
        let area = col_f32(batch, "area_m2");
        let name = col_str(batch, "name");
        let geom = col_binary(batch, "geom");

        for i in 0..n {
            let (c_lon, c_lat) = grid_cell_lonlat(cgx.value(i), cgy.value(i));
            let dist = grid::geo::flat_dist(lat, lon, c_lat, c_lon);
            if dist > max_radius {
                continue;
            }
            let polygon_grid: grid::poly::GridRing = geom
                .filter(|a| !a.is_null(i))
                .and_then(|a| decode_geom(Some(a.value(i))))
                .unwrap_or_default();
            results.push(LeisureResult {
                osm_id: osm_id.value(i),
                centroid_lat: c_lat,
                centroid_lon: c_lon,
                sport: sport.map(|a| a.value(i)).unwrap_or(0),
                area_m2: area.map(|a| a.value(i)).unwrap_or(0.0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                polygon_wkb: hex_encode(&grid_ring_to_wkb_polygon(&ring_lonlat(&polygon_grid))),
                dist_m: dist,
                polygon_grid,
            });
        }
    }
    results
}

/// Encode a lon/lat ring as little-endian WKB Polygon (single ring) so the
/// debug `polygon_wkb` strings keep their contract while the compute path
/// reads the grid ring directly.
fn grid_ring_to_wkb_polygon(ring_lonlat: &[(f64, f64)]) -> Vec<u8> {
    let mut wkb = Vec::with_capacity(9 + 4 + ring_lonlat.len() * 16 + 16);
    wkb.push(1);
    wkb.extend_from_slice(&3u32.to_le_bytes());
    wkb.extend_from_slice(&1u32.to_le_bytes());
    let closed = ring_lonlat.len() + 1;
    wkb.extend_from_slice(&(closed as u32).to_le_bytes());
    for &(lon, lat) in ring_lonlat {
        wkb.extend_from_slice(&lon.to_le_bytes());
        wkb.extend_from_slice(&lat.to_le_bytes());
    }
    if let Some(&(lon, lat)) = ring_lonlat.first() {
        wkb.extend_from_slice(&lon.to_le_bytes());
        wkb.extend_from_slice(&lat.to_le_bytes());
    }
    wkb
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(feature = "node")]
pub(crate) fn apply_segment_top_k_with_cap(
    traces: &mut noise_compute::types::TraceCollector,
    per_kind_cap: usize,
) -> noise_compute::types::SegmentTracesSummary {
    use noise_compute::types::{LayerKind, SegmentTracesSummary};

    let mut summary = SegmentTracesSummary {
        total_count: traces.segments.len() as u32,
        truncated: false,
        ..Default::default()
    };

    // Aircraft segments split into 3 sub-tabs by `aircraft_subtype`
    // (1 = ground / 2 = airborne / 3 = cruise) for top-K budgeting:
    // each sub-tab carries its own slice of segments at the popup-
    // global cap. Subtype 1 emitted by
    // `noise-compute::compute::aircraft_v6::airport_traffic::run`'s
    // `emit_segment_traces` (one SegmentTrace per microsegment).
    let aircraft_subtype_bucket = |seg: &noise_compute::types::SegmentTrace| -> Option<u8> {
        if seg.kind != LayerKind::Aircraft {
            return None;
        }
        match seg.aircraft_subtype {
            1 => Some(1),
            2 => Some(2),
            3 => Some(3),
            _ => None,
        }
    };

    let mut per_kind_total: std::collections::HashMap<LayerKind, u32> =
        std::collections::HashMap::new();
    let mut aircraft_ground_total = 0u32;
    let mut aircraft_cruise_total = 0u32;
    for seg in &traces.segments {
        *per_kind_total.entry(seg.kind).or_insert(0) += 1;
        match aircraft_subtype_bucket(seg) {
            Some(1) => aircraft_ground_total += 1,
            // Airborne subseg total comes from `traces.airborne_above_cutoff`
            // (maintained by airborne::scatter). With the bounded min-heap
            // most candidates never reach `traces.segments`, so the Vec
            // length is no longer a valid denominator.
            Some(2) => {}
            Some(3) => aircraft_cruise_total += 1,
            _ => {}
        }
    }
    summary.road_total = *per_kind_total.get(&LayerKind::Road).unwrap_or(&0);
    summary.railway_total = *per_kind_total.get(&LayerKind::Railway).unwrap_or(&0);
    summary.aircraft_ground_total = aircraft_ground_total;
    summary.building_total = *per_kind_total.get(&LayerKind::Building).unwrap_or(&0);
    summary.industrial_total = *per_kind_total.get(&LayerKind::Industrial).unwrap_or(&0);
    summary.aircraft_airborne_total = traces.airborne_above_cutoff;
    summary.aircraft_cruise_total = aircraft_cruise_total;

    traces.segments.sort_unstable_by(|a, b| {
        b.received_lden
            .full
            .partial_cmp(&a.received_lden.full)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut per_kind: std::collections::HashMap<LayerKind, u32> = std::collections::HashMap::new();
    let mut aircraft_ground_count = 0u32;
    let mut aircraft_airborne_subseg_count = 0u32;
    let mut aircraft_cruise_count = 0u32;
    // `retain_mut` drops over-cap traces in place — avoids the
    // intermediate `kept` Vec allocation. Drop cost itself is
    // unchanged (each over-cap trace still pays cascade dealloc on
    // Box<PropagationBreakdown> + inner Vec<f32>) and remains the
    // hot spot — at LKPR ≈ 100 ms with n_in ≈ 4 k. Real fix is
    // capping at per-source emission (e.g. airport_traffic ground
    // ops capped to 150 in `aircraft_v6/airport_traffic.rs`).
    let mut truncated = false;
    traces.segments.retain_mut(|seg| {
        let cap_ok = match aircraft_subtype_bucket(seg) {
            Some(1) => {
                if (aircraft_ground_count as usize) < per_kind_cap {
                    aircraft_ground_count += 1;
                    true
                } else {
                    false
                }
            }
            Some(2) => {
                if (aircraft_airborne_subseg_count as usize) < per_kind_cap {
                    aircraft_airborne_subseg_count += 1;
                    true
                } else {
                    false
                }
            }
            Some(3) => {
                if (aircraft_cruise_count as usize) < per_kind_cap {
                    aircraft_cruise_count += 1;
                    true
                } else {
                    false
                }
            }
            _ => {
                let count = per_kind.entry(seg.kind).or_insert(0);
                if (*count as usize) < per_kind_cap {
                    *count += 1;
                    true
                } else {
                    false
                }
            }
        };
        if !cap_ok {
            truncated = true;
        }
        cap_ok
    });
    summary.truncated = truncated;

    summary.road_count = *per_kind.get(&LayerKind::Road).unwrap_or(&0);
    summary.railway_count = *per_kind.get(&LayerKind::Railway).unwrap_or(&0);
    summary.aircraft_ground_count = aircraft_ground_count;
    summary.building_count = *per_kind.get(&LayerKind::Building).unwrap_or(&0);
    summary.industrial_count = *per_kind.get(&LayerKind::Industrial).unwrap_or(&0);
    summary.aircraft_airborne_count = aircraft_airborne_subseg_count;
    summary.aircraft_cruise_count = aircraft_cruise_count;
    // Airborne pre-capping in `airborne::scatter` drops most above-cutoff
    // candidates before they reach `traces.segments`, so the cap-loop
    // above never sees them and can't flip `truncated` for that case.
    // Detect it here by comparing total above-cutoff vs returned count.
    if traces.airborne_above_cutoff > summary.aircraft_airborne_count {
        summary.truncated = true;
    }

    summary
}

/// Batch-level replica of the airborne kernel's row prefilter
/// (`noise-compute/src/compute/aircraft_v6/airborne/mod.rs`), including its
/// f32 envelope rounding and conservative treatment of wide aggregate bounds.
fn airborne_envelope_gate(lat: f64, lng: f64) -> impl Fn(&arrow_batching::RowBbox) -> bool {
    let envelope = noise_compute::emission::aircraft::AirborneEnvelope::new(lat, lng);
    move |bb| envelope.intersects_bbox(*bb)
}

#[cfg(test)]
mod square_query_tests {
    use super::*;
    use crate::structure_test_fixture as fx;

    /// Click point and its square: Prague (50.0, 14.25) is z9/276/173.
    const LAT: f64 = 50.0;
    const LON: f64 = 14.25;

    fn prague() -> grid::Square {
        grid::square_of(LAT, LON)
    }

    #[test]
    fn source_envelope_contains_own_square_and_high_latitude_rail() {
        let squares = squares_within_reach(LAT, LON).unwrap();
        assert!(squares.contains(&prague()));
        assert_eq!(prague(), grid::Square { x: 276, y: 173 });
        let receiver_lat = 81.82379430564337;
        let source_lat = 81.92318633602197;
        let tmp = tempfile::TempDir::new().unwrap();
        let source_square = grid::square_of(source_lat, 0.0);
        let dir = fx::square_dir(tmp.path(), source_square);
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_railways_file(
            &dir.join("railways.arrow"),
            &[fx::FixtureRail {
                osm_id: 901,
                start: (0.0, source_lat),
                end: (0.001, source_lat),
                rail_type: 0,
                maxspeed: 160,
            }],
        );
        let collected = collect_sources_at_point(tmp.path(), receiver_lat, 0.0).unwrap();
        assert_eq!(collected.railways.len(), 1);
        assert_eq!(collected.railways[0].osm_id, 901);
        assert!(collected.railways[0].dist_m < noise_compute::constants::RAILWAY_REACH_CEILING);
    }

    #[test]
    fn square_dir_layout_is_z9_x_y() {
        let year = std::path::Path::new("/prepared/2026");
        assert_eq!(
            square_dir(year, grid::Square { x: 276, y: 173 }),
            std::path::PathBuf::from("/prepared/2026/z9/276/173")
        );
    }

    fn year_with_road() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_roads_file(
            &dir.join("roads.arrow"),
            &[fx::FixtureRoad {
                osm_id: 123,
                start: (LON, LAT),
                end: (LON + 0.002, LAT + 0.001),
                road_class: 2,
                speed_limit: 50,
                lanes: 2,
                name: "Test Street".to_string(),
            }],
        );
        tmp
    }

    #[test]
    fn grid_road_row_collects_with_lonlat_geometry() {
        let tmp = year_with_road();
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert_eq!(data.roads.len(), 1);
        let r = &data.roads[0];
        assert_eq!(r.osm_id, 123);
        assert_eq!(r.name, "Test Street");
        assert!((r.start_lon - LON).abs() < 0.001, "slon={}", r.start_lon);
        assert!((r.start_lat - LAT).abs() < 0.001, "slat={}", r.start_lat);
        assert_eq!(data.road_admins.len(), 1);
        assert_eq!(data.road_admins[0], None);
    }

    #[test]
    fn far_road_row_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_roads_file(
            &dir.join("roads.arrow"),
            &[fx::FixtureRoad {
                osm_id: 9,
                start: (LON + 5.0, LAT + 5.0),
                end: (LON + 5.002, LAT + 5.001),
                road_class: 2,
                speed_limit: 50,
                lanes: 2,
                name: String::new(),
            }],
        );
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert!(data.roads.is_empty());
    }

    #[test]
    fn rail_row_collects_with_grid_geometry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_railways_file(
            &dir.join("railways.arrow"),
            &[fx::FixtureRail {
                osm_id: 77,
                start: (LON, LAT),
                end: (LON + 0.002, LAT),
                rail_type: 0,
                maxspeed: 160,
            }],
        );
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert_eq!(data.railways.len(), 1);
        assert_eq!(data.railways[0].osm_id, 77);
        assert_eq!(data.railways[0].maxspeed, 160);
        assert_eq!(data.rail_admins.len(), 1);
    }

    #[test]
    fn antimeridian_road_and_rail_survive_the_midpoint_prefilter() {
        let (lat, lon) = (0.0, -180.0);
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), grid::square_of(lat, lon));
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_roads_file(
            &dir.join("roads.arrow"),
            &[fx::FixtureRoad {
                osm_id: 1,
                start: (179.999, lat),
                end: (-179.999, lat),
                road_class: 2,
                speed_limit: 50,
                lanes: 2,
                name: "Dateline Road".to_string(),
            }],
        );
        fx::write_railways_file(
            &dir.join("railways.arrow"),
            &[fx::FixtureRail {
                osm_id: 2,
                start: (179.999, lat),
                end: (-179.999, lat),
                rail_type: 0,
                maxspeed: 80,
            }],
        );

        let data = collect_sources_at_point(tmp.path(), lat, lon).unwrap();
        assert_eq!(data.roads.len(), 1);
        assert_eq!(data.railways.len(), 1);
    }

    fn building_row(osm_id: i64) -> fx::StructureRow {
        fx::StructureRow {
            kind: square_store::store::STRUCTURE_KIND_BUILDING,
            ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
            height_m: 12,
            height_tier: 0,
            envelope_class: 1,
            centroid_lonlat: Some((LON + 0.0001, LAT + 0.0001)),
            osm_id: Some(osm_id),
            building_type: Some(1),
            area_m2: Some(450.0),
            ..Default::default()
        }
    }

    #[test]
    fn building_rows_feed_emission_and_walls_do_not() {
        let tmp = tempfile::TempDir::new().unwrap();
        fx::write_square_structures(
            tmp.path(),
            prague(),
            &[
                building_row(55),
                // kind=1 wall: never an emission row.
                fx::StructureRow {
                    kind: square_store::store::STRUCTURE_KIND_BARRIER,
                    ring_lonlat: Some(vec![(LON, LAT), (LON + 0.001, LAT + 0.001)]),
                    height_m: 3,
                    height_tier: 0,
                    envelope_class: 0,
                    centroid_lonlat: Some((LON + 0.0005, LAT + 0.0005)),
                    osm_id: Some(66),
                    segment_idx: Some(0),
                    ..Default::default()
                },
                // kind=0 without osm_id: screening-only, never emission.
                fx::StructureRow {
                    kind: square_store::store::STRUCTURE_KIND_BUILDING,
                    ring_lonlat: Some(fx::square_ring_lonlat(LAT + 0.001, LON + 0.001)),
                    height_m: 8,
                    height_tier: 2,
                    envelope_class: 5,
                    centroid_lonlat: Some((LON + 0.001, LAT + 0.001)),
                    ..Default::default()
                },
            ],
        );
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert_eq!(data.buildings.len(), 1);
        assert_eq!(data.buildings[0].osm_id, 55);
    }

    #[test]
    fn emission_overrides_win_over_screening_geometry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut row = building_row(56);
        row.emission_centroid_lonlat = Some((LON + 0.0002, LAT + 0.0002));
        row.emission_ring_lonlat = Some(fx::square_ring_lonlat(LAT + 0.0002, LON + 0.0002));
        fx::write_square_structures(tmp.path(), prague(), &[row]);
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert_eq!(data.buildings.len(), 1);
        let pt = &data.buildings[0];
        assert!((pt.lon - (LON + 0.0002)).abs() < 0.0002, "lon={}", pt.lon);
    }

    #[test]
    fn leisure_folds_into_buildings_with_sport_tag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_leisure_file(
            &dir.join("leisure.arrow"),
            &[fx::FixtureLeisure {
                osm_id: 88,
                centroid: (LON, LAT),
                sport: 3,
                name: "Court".to_string(),
            }],
        );
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert_eq!(data.buildings.len(), 1);
        assert_eq!(
            data.buildings[0].source_type,
            noise_compute::types::LEISURE_TYPE_BASE + 3
        );
    }

    #[test]
    fn industrial_row_collects() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_industrial_file(
            &dir.join("industrial.arrow"),
            &[fx::FixtureIndustrial {
                osm_id: 99,
                centroid: (LON, LAT),
                source_type: 0,
                name: "Plant".to_string(),
            }],
        );
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert_eq!(data.industrial.len(), 1);
        assert_eq!(data.industrial[0].osm_id, 99);
    }

    #[test]
    fn unstamped_structures_fail_loud() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_structure_file(&dir.join("structures.arrow"), &[building_row(1)], false);
        let err = collect_sources_at_point(tmp.path(), LAT, LON).unwrap_err();
        assert!(err.contains("structures_contract mismatch"), "got: {err}");
    }

    #[test]
    fn empty_tree_collects_nothing_with_default_n_days() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
        assert!(data.roads.is_empty());
        assert!(data.buildings.is_empty());
        assert_eq!(data.n_days, 365);
    }

    fn wall_row(osm_id: i64, seg_idx: i16, lat: f64, lon: f64) -> fx::StructureRow {
        fx::StructureRow {
            kind: square_store::store::STRUCTURE_KIND_BARRIER,
            ring_lonlat: Some(vec![(lon, lat), (lon + 0.001, lat + 0.001)]),
            height_m: 3,
            height_tier: 0,
            envelope_class: 0,
            centroid_lonlat: Some((lon + 0.0005, lat + 0.0005)),
            osm_id: Some(osm_id),
            segment_idx: Some(seg_idx),
            ..Default::default()
        }
    }

    #[test]
    fn wall_listing_preserves_provenance_and_ignores_buildings() {
        let batch = fx::structure_batch(&[
            wall_row(7, -3, LAT, LON),
            wall_row(7, 4, LAT + 0.01, LON + 0.01),
            building_row(42),
        ]);
        let results =
            square_store::barriers::query_barriers_from_batches(&[batch], LAT, LON, 200_000.0)
                .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].segment_idx, -3);
        assert_eq!(results[1].segment_idx, 4);
        assert!((results[0].start_lon - LON).abs() < 0.001);
    }

    #[test]
    fn identical_wall_dupes_merge() {
        let batch = fx::structure_batch(&[wall_row(7, -3, LAT, LON), wall_row(7, -3, LAT, LON)]);
        let results =
            square_store::barriers::query_barriers_from_batches(&[batch], LAT, LON, 200_000.0)
                .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn wall_row_without_segment_idx_fails_closed() {
        let mut row = wall_row(7, -3, LAT, LON);
        row.segment_idx = None;
        let batch = fx::structure_batch(&[row]);
        let err = square_store::barriers::query_barriers_from_batches(&[batch], LAT, LON, 1_000.0)
            .unwrap_err();
        assert!(err.contains("segment_idx"), "got: {err}");
    }

    #[test]
    fn reference_square_gate_reads_rows_strictly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = fx::square_dir(tmp.path(), prague());
        std::fs::create_dir_all(&dir).unwrap();
        fx::write_roads_file(
            &dir.join("roads.arrow"),
            &[fx::FixtureRoad {
                osm_id: 1,
                start: (LON, LAT),
                end: (LON + 0.001, LAT),
                road_class: 2,
                speed_limit: 50,
                lanes: 2,
                name: String::new(),
            }],
        );
        let name = grid::square_name(prague());
        assert_eq!(
            square_store::store::validate_reference_square(tmp.path(), &name).unwrap(),
            1
        );
        assert!(
            square_store::store::validate_reference_square(tmp.path(), "z9/../escape").is_err()
        );
    }
}

#[cfg(test)]
mod airborne_gate_tests {
    #[test]
    fn corner_batch_passes_axis_envelope_but_not_a_circle() {
        let keep = super::airborne_envelope_gate(0.0, 0.0);
        // Codex /gg corner case: bbox ~20.4 km away point-to-point but both
        // axis distances ~14.4 km < 16 km — the kernel's row filter keeps
        // such rows, so the batch gate must too.
        let bb = [0.13, 0.13, 0.14, 0.14];
        assert!(keep(&bb));
        assert!(arrow_batching::point_to_bbox_distance_m(0.0, 0.0, &bb) > 16_000.0);
        // Far on both axes → pruned.
        assert!(!keep(&[1.0, 1.0, 1.1, 1.1]));
    }

    #[test]
    fn antimeridian_receiver_prunes_distant_longitudes_but_keeps_crossing_batches() {
        let keep = super::airborne_envelope_gate(0.0, 179.95);
        assert!(keep(&[0.0, -179.99, 0.1, -179.98]));
        assert!(!keep(&[0.0, -81.0, 0.1, -80.0]));
        assert!(!keep(&[5.0, -179.99, 5.1, -179.98]));
        assert!(super::airborne_envelope_gate(0.001, 179.5)(&[
            0.0, -179.0, 0.0, 179.0
        ]));
    }
}

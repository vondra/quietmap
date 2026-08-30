//! Pure point-query logic: Arrow batches → typed noise-source views.
//! Sits below the NAPI glue in `lib.rs` (the popup engine entry point) and
//! holds the zero-copy collection that both `collect_sources_at_point` and
//! `query_noise_at_point` delegate to, plus the segment top-K cap applied to
//! the popup's trace output. No global `STORE`/`RASTERS` state here — callers
//! hand in pre-loaded hex data.

use std::path::Path;

use crate::geo;
use crate::hex_store::{
    self, canonicalize_barrier_results, hex_encode, load_hex, query_barriers_from_batches,
    query_buildings_from_batches, query_leisure_from_batches, query_railways_from_batches,
    query_roads_from_batches, BarrierResult,
};

const BUILDING_QUERY_RADIUS_M: f64 = 2_000.0;
const INDUSTRIAL_QUERY_RADIUS_M: f64 = 5_000.0;

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
    pub barriers: Vec<noise_compute::types::Barrier>,
    pub n_days: u16,
}

// NACE codes are written directly into industrial.arrow by enrichment scripts.

pub fn collect_sources_at_point(
    h3r4_dir: &Path,
    lat: f64,
    lng: f64,
) -> Result<PointQueryData, String> {
    let hex_ids = geo::grid_disk_r4(lat, lng);
    let loaded: Vec<hex_store::HexData> = hex_ids
        .iter()
        .map(|id| load_hex(&h3r4_dir.join(id).to_string_lossy()))
        .collect::<Result<_, _>>()?;
    let refs: Vec<&hex_store::HexData> = loaded.iter().collect();

    collect_from_hex_data(&refs, lat, lng)
}

/// Shared source collection logic. Takes pre-loaded hex data.
/// Both `collect_sources_at_point` and `query_noise_at_point` delegate here.
///
/// NACE codes are read directly from industrial.arrow nace_4digit column.
pub fn collect_from_hex_data(
    hex_data: &[&hex_store::HexData],
    lat: f64,
    lng: f64,
) -> Result<PointQueryData, String> {
    let mut all_roads = Vec::new();
    let mut all_railways = Vec::new();
    let mut all_road_admins = Vec::new();
    let mut all_rail_admins = Vec::new();
    let mut all_buildings = Vec::new();
    let mut all_industrial = Vec::new();
    let mut all_barrier_results: Vec<BarrierResult> = Vec::new();
    let mut all_airborne_batches: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let mut all_cruise_batches: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let mut all_airport_traffic_batches: Vec<arrow::record_batch::RecordBatch> = Vec::new();
    let mut date_ids = std::collections::HashSet::new();
    let mut n_days_from_metadata: Option<u16> = None;
    // Prune aircraft batches per hex ONCE; the collection below consumes the
    // result. Airborne/cruise use the kernel-identical AXIS envelope (a
    // circular gate is narrower and would drop corner batches the kernel's
    // row prefilter accepts — Codex /gg 2026-07-10); airport_traffic's row
    // accept is a planar circle, so the slack-carrying circular gate fits.
    let airborne_gate = airborne_envelope_gate(lat, lng);
    let per_hex_aircraft: Vec<(
        Vec<arrow::record_batch::RecordBatch>,
        Vec<arrow::record_batch::RecordBatch>,
        Vec<arrow::record_batch::RecordBatch>,
    )> = hex_data
        .iter()
        .map(|data| {
            (
                data.aircraft_airborne.batches_where(&airborne_gate),
                data.aircraft_cruise.batches_where(&airborne_gate),
                data.aircraft_airport_traffic.batches_within(
                    lat,
                    lng,
                    noise_compute::constants::GROUND_OPS_RUNWAY_MAX_RADIUS,
                ),
            )
        })
        .collect();
    // The label lookup needs every airport-line row in the ring, but only
    // when nearby airport traffic exists. Keep the files footer-only for all
    // other clicks.
    let all_airport_lines_batches = if per_hex_aircraft
        .iter()
        .any(|(_, _, traffic)| !traffic.is_empty())
    {
        hex_data
            .iter()
            .flat_map(|data| data.airport_lines.batches_all())
            .collect()
    } else {
        Vec::new()
    };
    // n_days is FILE-level schema metadata — read it without decoding any
    // batch. The legacy date_id fallback (pre-metadata extracts) must scan
    // the FULL files, never the pruned lists: a pruned scan would shrink the
    // divisor to "days with flights near this click" and inflate Lden
    // (Gemini /gg 2026-07-10; unreachable today — bboxes imply n_days — but
    // the invariant must not live in two places).
    for data in hex_data {
        for arrow in [
            &data.aircraft_airborne,
            &data.aircraft_cruise,
            &data.aircraft_airport_traffic,
        ] {
            if let Some(md) = arrow.schema().and_then(|s| s.metadata().get("n_days")) {
                if let Ok(v) = md.parse::<u16>() {
                    n_days_from_metadata =
                        Some(n_days_from_metadata.map(|m| m.max(v)).unwrap_or(v));
                }
            }
        }
    }
    if n_days_from_metadata.is_none() {
        for data in hex_data {
            for arrow in [
                &data.aircraft_airborne,
                &data.aircraft_cruise,
                &data.aircraft_airport_traffic,
            ] {
                for batch in arrow.batches_all() {
                    if let Some(did) = batch
                        .column_by_name("date_id")
                        .and_then(|c| c.as_any().downcast_ref::<arrow::array::Int16Array>())
                    {
                        for i in 0..did.len() {
                            date_ids.insert(did.value(i));
                        }
                    }
                }
            }
        }
    }
    let n_days = n_days_from_metadata.unwrap_or({
        if date_ids.is_empty() {
            365
        } else {
            date_ids.len() as u16
        }
    });

    for (data, (airborne_batches, cruise_batches, airport_traffic_batches)) in
        hex_data.iter().zip(per_hex_aircraft)
    {
        // Spatial candidate pre-filter: load every rail row within the WIDEST
        // possible per-row reach (the clamp ceiling, 10 km), then `compute_railways`
        // applies each row's exact `rail_reach_m` cutoff. Pre-filtering at the old
        // 7 km blanket would silently drop a loud HS corridor 8-10 km out before
        // its honest reach could admit it. The batch gate uses the SAME ceiling.
        let railway_batches =
            data.railways
                .batches_within(lat, lng, noise_compute::constants::RAILWAY_REACH_CEILING);
        let railways = query_railways_from_batches(
            &railway_batches,
            lat,
            lng,
            noise_compute::constants::RAILWAY_REACH_CEILING,
        );
        // Receiver-hex admin for the C1 per-region period model. Only the scaled
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
                .batches_within(lat, lng, noise_compute::constants::ROAD_MAX_RADIUS[0]);
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
            .buildings
            .batches_within(lat, lng, BUILDING_QUERY_RADIUS_M);
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
                    polygon_wkb: &b.polygon_wkb,
                },
            );
            for prepared in prepared_points {
                let pt_dist = crate::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                all_buildings.push(prepared.with_metadata(
                    b.osm_id,
                    b.building_type,
                    display_name.clone(),
                    b.polygon_wkb.clone(),
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
            .batches_within(lat, lng, BUILDING_QUERY_RADIUS_M);
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
                    polygon_wkb: &lz.polygon_wkb,
                },
            );
            for prepared in prepared_points {
                let pt_dist = crate::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                all_buildings.push(prepared.with_metadata(
                    lz.osm_id,
                    source_type,
                    lz.name.clone(),
                    lz.polygon_wkb.clone(),
                    pt_dist,
                ));
            }
        }

        for batch in &data
            .industrial
            .batches_within(lat, lng, INDUSTRIAL_QUERY_RADIUS_M)
        {
            let n = batch.num_rows();
            let clat: Option<&arrow::array::Float64Array> = batch
                .column_by_name("centroid_lat")
                .and_then(|c| c.as_any().downcast_ref());
            let clon: Option<&arrow::array::Float64Array> = batch
                .column_by_name("centroid_lon")
                .and_then(|c| c.as_any().downcast_ref());
            let (Some(clat), Some(clon)) = (clat, clon) else {
                continue;
            };
            let stype: Option<&arrow::array::UInt8Array> = batch
                .column_by_name("source_type")
                .and_then(|c| c.as_any().downcast_ref());
            let hub_h: Option<&arrow::array::Float32Array> = batch
                .column_by_name("hub_height")
                .and_then(|c| c.as_any().downcast_ref());
            let power: Option<&arrow::array::Float32Array> = batch
                .column_by_name("rated_power_kw")
                .and_then(|c| c.as_any().downcast_ref());
            let ind_name: Option<&arrow::array::StringArray> = batch
                .column_by_name("name")
                .and_then(|c| c.as_any().downcast_ref());
            let wkb_col: Option<&arrow::array::BinaryArray> = batch
                .column_by_name("polygon_wkb")
                .and_then(|c| c.as_any().downcast_ref());
            let area_col: Option<&arrow::array::Float32Array> = batch
                .column_by_name("area_m2")
                .and_then(|c| c.as_any().downcast_ref());

            for i in 0..n {
                let c_lat = clat.value(i);
                let c_lon = clon.value(i);
                let dist = crate::geo::flat_dist(lat, lng, c_lat, c_lon);
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
                let osm_id = batch
                    .column_by_name("osm_id")
                    .and_then(|c| c.as_any().downcast_ref::<arrow::array::Int64Array>())
                    .map(|a| a.value(i))
                    .unwrap_or(0);
                let wkb_hex = if st == 10 {
                    String::new()
                } else {
                    wkb_col.map(|a| hex_encode(a.value(i))).unwrap_or_default()
                };
                let area_m2 = area_col.and_then(|a| {
                    let v = a.value(i);
                    if v > 0.0 {
                        Some(v as f64)
                    } else {
                        None
                    }
                });

                let sub = batch
                    .column_by_name("site_subtype")
                    .and_then(|c| c.as_any().downcast_ref::<arrow::array::UInt8Array>())
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
                        polygon_wkb: &wkb_hex,
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
                let row_source_id = batch
                    .column_by_name("source_id")
                    .and_then(|c| c.as_any().downcast_ref::<arrow::array::UInt16Array>())
                    .map(|a| a.value(i))
                    .unwrap_or(0);
                for prepared in prepared_points {
                    let pt_dist = crate::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                    let mut ps =
                        prepared.with_metadata(osm_id, st, iname.clone(), wkb_hex.clone(), pt_dist);
                    ps.source_id = row_source_id;
                    all_industrial.push(ps);
                }
            }
        }

        // 10 km matches the road source radius — barriers along the full source→receiver
        // path are needed for screening, not just near the receiver. The
        // half-segment slack keeps a wall that CROSSES a 10 km path near its far
        // end in the set: the crossing is inside 10 km, its midpoint (what the
        // radius filters on) can be a half-segment beyond.
        const BARRIER_RADIUS_M: f64 =
            10_000.0 + noise_compute::types::BARRIER_SEGMENT_MAX_HALF_LEN_M;
        let barrier_batches = data.barriers.batches_within(lat, lng, BARRIER_RADIUS_M);
        let barriers = query_barriers_from_batches(&barrier_batches, lat, lng, BARRIER_RADIUS_M)?;
        all_barrier_results.extend(barriers);

        // Aircraft popup arrows: bbox-gated above (per_hex_aircraft);
        // per-row reach prune + emission contract live inside
        // compute_aircraft_v6. RecordBatch clones are refcount bumps on
        // Arc-backed Arrow buffers, not data copies.
        all_airborne_batches.extend(airborne_batches);
        all_cruise_batches.extend(cruise_batches);
        all_airport_traffic_batches.extend(airport_traffic_batches);
    }

    let mut all_barriers: Vec<_> = canonicalize_barrier_results(all_barrier_results)?
        .into_iter()
        .map(|b| noise_compute::types::Barrier {
            osm_id: b.osm_id,
            segment_idx: b.segment_idx,
            height_m: b.height,
            start_lat: b.start_lat,
            start_lon: b.start_lon,
            end_lat: b.end_lat,
            end_lon: b.end_lon,
            dist_m: b.dist_m,
        })
        .collect();

    all_barriers.sort_unstable_by(|a, b| {
        a.dist_m
            .partial_cmp(&b.dist_m)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

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
        barriers: all_barriers,
        n_days,
    })
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
/// (`noise-compute/src/compute/aircraft_v6/airborne/mod.rs`): an AXIS-ALIGNED
/// envelope at the 16 km reach cap with the antimeridian wrap guard. A
/// circular gate would be strictly narrower — a bbox corner passes both axis
/// tests at up to reach·√2 point distance, and the kernel keeps such rows
/// (off-segment CPA on the unclamped extension) — so it would drop audible
/// batches (Codex /gg 2026-07-10). Batch bbox ⊇ row bboxes, so envelope
/// overlap here is implied whenever any contained row overlaps.
fn airborne_envelope_gate(lat: f64, lng: f64) -> impl Fn(&arrow_batching::RowBbox) -> bool {
    use noise_compute::emission::aircraft;
    let reach = aircraft::AIRCRAFT_MAX_HORIZONTAL_REACH_M;
    let radius_lat_deg = aircraft::meters_to_lat_deg(reach);
    let radius_lon_deg = aircraft::meters_to_lon_deg(lat, reach);
    let env_min_lat = lat - radius_lat_deg;
    let env_max_lat = lat + radius_lat_deg;
    let env_min_lon = lng - radius_lon_deg;
    let env_max_lon = lng + radius_lon_deg;
    // Same wrap rule as the kernel: an envelope reaching past ±180° turns
    // the longitude prune off (stored bboxes are normalized to [-180, 180]).
    let lon_prune_active = env_min_lon >= -180.0 && env_max_lon <= 180.0;
    move |bb: &arrow_batching::RowBbox| {
        if bb[2] < env_min_lat || bb[0] > env_max_lat {
            return false;
        }
        !(lon_prune_active && (bb[3] < env_min_lon || bb[1] > env_max_lon))
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
    fn antimeridian_receiver_disables_longitude_prune_keeps_latitude_prune() {
        let keep = super::airborne_envelope_gate(0.0, 179.95);
        assert!(keep(&[0.0, -179.9, 0.1, -179.8]));
        assert!(!keep(&[5.0, -179.9, 5.1, -179.8]));
    }
}

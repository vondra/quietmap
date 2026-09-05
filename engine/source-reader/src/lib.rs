//! source-reader: mmap'd Arrow IPC reader for noise popup.
//! Zero-copy: data stays in mmap'd pages, queries iterate directly over Arrow columns.

// mimalloc handles popup's many small short-lived allocs (SegmentTrace
// + Box<PropagationBreakdown> + inner Vec<f32>) faster than glibc malloc
// — Microsoft Research benchmarks ~2× speedup for similar workloads.
// At LKPR the per-popup drop cascade (~6 k traces × ~10 inner allocs)
// is the hot spot remaining in apply_segment_top_k_with_cap.
#[cfg(feature = "node")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod aircraft_v6;
pub mod query;
pub mod structure_store;
#[cfg(test)]
mod structure_test_fixture;
#[cfg(feature = "node")]
pub mod wire;

// Re-export the pure point-query API at the crate root so its paths
// (`source_reader::PointQueryData`, `collect_sources_at_point`, …) are
// unchanged after the lib.rs/query.rs split, and so the `#[napi]` wrappers
// below resolve `collect_from_square_data` / `apply_segment_top_k_with_cap`.
pub use query::*;

#[cfg(feature = "node")]
use napi::{Error, Status};
#[cfg(feature = "node")]
use napi_derive::napi;
#[cfg(feature = "node")]
use std::collections::HashMap;
#[cfg(feature = "node")]
use std::sync::RwLock;

#[cfg(feature = "node")]
use square_store::store::SquareData;

#[cfg(feature = "node")]
static STORE: std::sync::LazyLock<RwLock<SquareStore>> =
    std::sync::LazyLock::new(|| RwLock::new(SquareStore::new()));

#[cfg(feature = "node")]
static RASTERS: std::sync::OnceLock<raster_reader::RealRasters> = std::sync::OnceLock::new();
/// Data root (`…/data/prepared`) captured at `source_init` — the vector
/// obstacle loader keeps its on-disk index cache under it (geodata-v2 1.4).
static DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
/// The live `…/prepared/2026` dir — the structure root: every prepared
/// square carries its own `structures.arrow` under `z9/<x>/<y>/` beside its
/// other arrows.
static YEAR_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The square root, or the one error that explains an unset one. Buildings
/// are vector-only, so a query without this root has no answer to give.
#[cfg(feature = "node")]
fn year_dir() -> napi::Result<&'static std::path::Path> {
    YEAR_DIR
        .get()
        .map(|p| p.as_path())
        .ok_or_else(|| Error::new(Status::GenericFailure, "source_init was never called"))
}

// NACE codes are now baked into industrial.arrow (nace_4digit UInt16 column).
// No global lookup needed at runtime.

#[cfg(feature = "node")]
struct SquareStore {
    squares: HashMap<String, SquareData>,
    prepared_dir: String,
}

/// RAII clearer for the M4/M5 per-row admin channels: a plain
/// clear-after-compute pair lets a kernel unwind leave a stale vec on the
/// surviving napi worker thread (the next query of equal row count would
/// silently inherit the previous query's countries). The guard clears on
/// scope exit either way.
#[cfg(feature = "node")]
struct RowAdminGuard;

#[cfg(feature = "node")]
impl Drop for RowAdminGuard {
    fn drop(&mut self) {
        noise_compute::defaults::set_road_row_admins(None);
        noise_compute::emission::railway::set_rail_row_admins(None);
    }
}

#[cfg(feature = "node")]
impl SquareStore {
    fn new() -> Self {
        SquareStore {
            squares: HashMap::new(),
            prepared_dir: String::new(),
        }
    }
}

/// Make every square in `square_names` resident, loading the missing ones IN
/// PARALLEL and OUTSIDE the store lock. Cold loads used to run
/// sequentially (9 squares × ~10 files) under a held write lock — the whole
/// point of a shared store (all pool workers read one cache since
/// 2026-07-10) is that one visitor's cold load must neither serialize with
/// nor block everyone else's warm queries. First insert wins on a race —
/// the duplicate load is dropped, which is rare and harmless. The requested
/// set is one cache transaction: a load error fails the query and inserts none.
#[cfg(feature = "node")]
fn ensure_squares_parallel(square_names: &[String]) -> napi::Result<()> {
    let missing: Vec<String> = {
        let store = STORE.read().expect("square store poisoned");
        square_names
            .iter()
            .filter(|id| !store.squares.contains_key(id.as_str()))
            .cloned()
            .collect()
    };
    if missing.is_empty() {
        return Ok(());
    }
    let prepared_dir = STORE
        .read()
        .expect("square store poisoned")
        .prepared_dir
        .clone();
    let loaded: Result<Vec<(String, SquareData)>, String> = std::thread::scope(|scope| {
        let handles: Vec<_> = missing
            .iter()
            .map(|name| {
                let dir = format!("{prepared_dir}/{}", name_to_rel(name));
                scope.spawn(move || {
                    square_store::store::load_square(std::path::Path::new(&dir))
                        .map_err(|error| format!("failed to load square {name}: {error}"))
                })
            })
            .collect();
        missing
            .iter()
            .cloned()
            .zip(handles)
            .map(|(name, handle)| {
                handle
                    .join()
                    .expect("square load panicked")
                    .map(|data| (name, data))
            })
            .collect()
    });
    let loaded = loaded.map_err(|error| Error::new(Status::GenericFailure, error))?;
    let mut store = STORE.write().expect("square store poisoned");
    for (id, data) in loaded {
        store.squares.entry(id).or_insert(data);
    }
    Ok(())
}

#[cfg(all(test, feature = "node"))]
mod square_cache_tests;

/// `z9/276/173` → `z9/276/173` (already the relative path under the prepared
/// year dir). Kept as a function so a naming change lands in one place.
#[cfg(feature = "node")]
fn name_to_rel(name: &str) -> &str {
    name
}

#[cfg(feature = "node")]
#[napi]
pub fn source_init(prepared_dir: String) -> napi::Result<String> {
    let mut store = STORE
        .write()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    // The pool workers share ONE library instance (single addon path since
    // 2026-07-10), so every worker spawn/recycle calls source_init on the
    // SAME store — re-init with an unchanged dir must keep the shared cache,
    // not clear it out from under the other workers.
    if store.prepared_dir == prepared_dir {
        return Ok(format!(
            "source-reader already initialized: {prepared_dir} ({} squares cached, shared store)",
            store.squares.len()
        ));
    }
    store.prepared_dir = prepared_dir.clone();
    store.squares.clear();

    // Rasters live at `<prepared>/2026/rasters/`; admin records at
    // `<prepared>/2026/admin/<square_id>/admin.bin`; squares at
    // `<prepared>/2026/z9/<x>/<y>/`.
    let year_path = std::path::Path::new(&prepared_dir);
    let data_dir = year_path.parent().unwrap_or(std::path::Path::new("."));
    let rasters = raster_reader::RealRasters::new(year_path);
    let has_dem = rasters.has_data();
    RASTERS.set(rasters).ok();
    DATA_DIR.set(data_dir.to_path_buf()).ok();
    YEAR_DIR.set(year_path.to_path_buf()).ok();

    // NACE codes are baked into industrial.arrow — no global JSON needed

    // Admin for the defaults cascade (plan v5 §F.3): each queried square's own
    // `<prepared>/2026/admin/<square_id>/admin.bin`, read on first use. A square
    // without one simply leaves the cascade in its WORLD arm.
    noise_compute::admin::set_admin_square_directory(&year_path.join("admin"));

    Ok(format!(
        "source-reader initialized: {prepared_dir} (DEM: {})",
        if has_dem { "loaded" } else { "stub" },
    ))
}

/// Strictly parse one known non-empty roads archive for runtime readiness.
/// Unlike popup queries, this does not read from or write to `STORE`, so a
/// readiness probe can never pin a partially rewritten square in the cache.
#[cfg(feature = "node")]
#[napi]
pub fn source_validate_reference(prepared_dir: String, square_name: String) -> napi::Result<u32> {
    let rows = square_store::store::validate_reference_square(
        std::path::Path::new(&prepared_dir),
        &square_name,
    )
    .map_err(|error| Error::new(Status::GenericFailure, error))?;
    u32::try_from(rows).map_err(|_| {
        Error::new(
            Status::GenericFailure,
            format!("reference roads row count exceeds u32: {rows}"),
        )
    })
}

#[cfg(feature = "node")]
fn reach_square_names(lat: f64, lng: f64) -> Vec<String> {
    squares_within_reach(lat, lng)
        .iter()
        .map(|sq| grid::square_name(*sq))
        .collect()
}

#[cfg(feature = "node")]
#[napi]
pub fn query_roads(lat: f64, lng: f64, max_radius_m: f64) -> napi::Result<String> {
    let square_names = reach_square_names(lat, lng);
    ensure_squares_parallel(&square_names)?;
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;

    let mut all_results = Vec::new();
    for name in &square_names {
        let Some(data) = store.squares.get(name.as_str()) else {
            continue;
        };
        let road_batches = data
            .roads
            .batches_within(lat, lng, max_radius_m)
            .map_err(|error| Error::new(Status::GenericFailure, error))?;
        let mut results = query_roads_from_batches(&road_batches, lat, lng, max_radius_m);
        all_results.append(&mut results);
    }

    Ok(serde_json::to_string(&all_results).unwrap())
}

#[cfg(feature = "node")]
#[napi]
pub fn query_buildings(lat: f64, lng: f64, max_radius_m: f64) -> napi::Result<String> {
    let square_names = reach_square_names(lat, lng);
    ensure_squares_parallel(&square_names)?;
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;

    let mut all_results = Vec::new();
    for name in &square_names {
        let Some(data) = store.squares.get(name.as_str()) else {
            continue;
        };
        let building_batches = data
            .structures
            .batches_within(lat, lng, max_radius_m)
            .map_err(|error| Error::new(Status::GenericFailure, error))?;
        let mut results = query_buildings_from_batches(&building_batches, lat, lng, max_radius_m);
        all_results.append(&mut results);
    }

    Ok(serde_json::to_string(&all_results).unwrap())
}

#[cfg(feature = "node")]
#[napi]
/// Obstacle footprints intersecting a bbox with their AS-USED heights (after
/// the low-profile cap) — the building-height debug overlay's data source,
/// so the map shows exactly what the propagation model screens with. JSON:
/// [{o: [[lat,lon]…], h, t, c}] (o = outer ring, h = height m, t = height
/// tier 0 mapped/1 floors/2 default/3 city-measured zonal/4 ANBH areal prior
/// — see noise_compute::low_profile, c = low-profile-capped).
pub fn query_obstacle_footprints(
    south: f64,
    west: f64,
    north: f64,
    east: f64,
) -> napi::Result<String> {
    let fps = structure_store::footprints_in_bbox(year_dir()?, south, west, north, east)
        .map_err(|e| Error::new(Status::GenericFailure, e))?;
    let rows: Vec<serde_json::Value> = fps
        .iter()
        .map(|f| {
            serde_json::json!({
                "o": f.outer.iter().map(|(la, lo)| [la, lo]).collect::<Vec<_>>(),
                "h": f.height_m,
                "t": f.tier,
                "c": f.capped,
            })
        })
        .collect();
    Ok(serde_json::to_string(&rows).unwrap())
}

/// Map the engine's envelope class to the small plain-language vocabulary
/// used by the building hover tooltip. Kept outside `structure_store.rs` so
/// changing display wording does not rotate its disk-index cache version.
fn building_type_from_envelope(class: noise_compute::envelope::EnvelopeClass) -> &'static str {
    match class {
        noise_compute::envelope::EnvelopeClass::Outdoor => "carport/roof structure",
        noise_compute::envelope::EnvelopeClass::Residential => "house",
        noise_compute::envelope::EnvelopeClass::Commercial => "office",
        noise_compute::envelope::EnvelopeClass::Industrial => "industrial hall",
        noise_compute::envelope::EnvelopeClass::Historic => "historic building",
        noise_compute::envelope::EnvelopeClass::Default => "building",
    }
}

/// Return the vector obstacle containing a point, if any. This is intentionally
/// a containment-only query: it reuses the exact obstacle set and enclosed
/// winner selection used by the popup and heatmap, without running noise
/// collection or propagation.
#[cfg(feature = "node")]
#[napi]
pub fn query_building_at(lat: f64, lng: f64) -> napi::Result<String> {
    let data_dir = DATA_DIR
        .get()
        .map(|p| p.as_path())
        .unwrap_or_else(|| std::path::Path::new("."));
    // A missing obstacle store is an error, not an empty answer. It used to
    // return {"status":"unavailable"} inside an HTTP 200, which reads to a
    // visitor exactly like "there is no building here".
    let set = structure_store::load_obstacle_set(year_dir()?, data_dir, lat, lng)
        .map_err(|e| Error::new(Status::GenericFailure, e))?;
    let result = match structure_store::point_inside_footprint(&set, lat, lng) {
        None => serde_json::Value::Null,
        Some((class, height)) => serde_json::json!({
            "height_m": height,
            "building_type": building_type_from_envelope(class),
        }),
    };
    Ok(serde_json::to_string(&result).unwrap())
}

#[cfg(test)]
mod building_type_tests {
    use super::building_type_from_envelope;
    use noise_compute::envelope::EnvelopeClass;

    #[test]
    fn building_type_labels_match_popup_language() {
        assert_eq!(
            building_type_from_envelope(EnvelopeClass::Outdoor),
            "carport/roof structure"
        );
        assert_eq!(
            building_type_from_envelope(EnvelopeClass::Residential),
            "house"
        );
        assert_eq!(
            building_type_from_envelope(EnvelopeClass::Commercial),
            "office"
        );
        assert_eq!(
            building_type_from_envelope(EnvelopeClass::Industrial),
            "industrial hall"
        );
        assert_eq!(
            building_type_from_envelope(EnvelopeClass::Historic),
            "historic building"
        );
        assert_eq!(
            building_type_from_envelope(EnvelopeClass::Default),
            "building"
        );
    }
}

#[cfg(feature = "node")]
#[napi]
pub fn query_barriers(lat: f64, lng: f64, max_radius_m: f64) -> napi::Result<String> {
    let square_names = reach_square_names(lat, lng);
    ensure_squares_parallel(&square_names)?;
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;

    let mut all_results = Vec::new();
    for name in &square_names {
        let Some(data) = store.squares.get(name.as_str()) else {
            continue;
        };
        let barrier_batches = data
            .structures
            .batches_within(lat, lng, max_radius_m)
            .map_err(|error| Error::new(Status::GenericFailure, error))?;
        let mut results = square_store::barriers::query_barriers_from_batches(
            &barrier_batches,
            lat,
            lng,
            max_radius_m,
        )
        .map_err(|error| Error::new(Status::GenericFailure, error))?;
        all_results.append(&mut results);
    }

    let all_results = square_store::barriers::canonicalize_barrier_results(all_results)
        .map_err(|error| Error::new(Status::GenericFailure, error))?;

    Ok(serde_json::to_string(&all_results).unwrap())
}

#[cfg(feature = "node")]
#[napi]
pub fn reload_hexes(hex_ids: Vec<String>) -> napi::Result<u32> {
    let mut store = STORE
        .write()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    let mut n = 0u32;
    for hex_id in &hex_ids {
        store.squares.remove(hex_id);
        n += 1;
    }
    Ok(n)
}

/// Compute full noise at a point using noise-compute engine.
/// Returns JSON with total Lden, per-source breakdown, top contributors.
#[cfg(feature = "node")]
#[napi]
pub fn query_noise_at_point(lat: f64, lng: f64) -> napi::Result<String> {
    query_noise_impl(lat, lng, SEGMENT_TOP_K_PER_KIND)
}

/// Variant of `query_noise_at_point` with a much higher per-kind segment cap
/// (1000 instead of 150). Called from the popup's "Show all" button — the
/// fully-unfiltered airborne set at an airport is millions of segments, far
/// beyond what a browser can parse or what NAPI's string return can carry.
#[cfg(feature = "node")]
#[napi]
pub fn query_noise_at_point_unfiltered(lat: f64, lng: f64) -> napi::Result<String> {
    query_noise_impl(lat, lng, SEGMENT_TOP_K_PER_KIND_FULL)
}

#[cfg(feature = "node")]
fn query_noise_impl(lat: f64, lng: f64, top_k_per_kind: usize) -> napi::Result<String> {
    // Per-stage timing probes (env-gated: `POPUP_TIMING=1` to enable). Inline
    // `Instant::now()` is cheaper and less destructive than perf/flamegraph
    // for popup-scale work, and lets us watch one number per stage land in
    // the Fastify log per request.
    let timing_on = std::env::var("POPUP_TIMING").as_deref() == Ok("1");
    let t_start = std::time::Instant::now();

    let square_names = reach_square_names(lat, lng);
    // Load missing squares in parallel WITHOUT holding the store lock, then
    // collect under a read lock — concurrent popups on other workers keep
    // running against the shared cache during a cold load.
    ensure_squares_parallel(&square_names)?;
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    let square_refs: Vec<&square_store::store::SquareData> = square_names
        .iter()
        .filter_map(|id| store.squares.get(id.as_str()))
        .collect();

    // Resolve airport_summary.arrow path: sibling of the squares tree under
    // `aircraft/` (Stage 2C v5 reduce output). Missing file means
    // the popup returns zero airport-level counts.
    let airport_summary_pathbuf = std::path::Path::new(&store.prepared_dir)
        .join("aircraft")
        .join("airport_summary.arrow");

    let stub = StubRasters;
    let real_rasters = RASTERS.get();
    let rasters: &dyn noise_compute::types::RasterSampler = match real_rasters {
        Some(r) => r,
        None => &stub,
    };
    // Sample receiver elevation up-front so the aircraft kernels see a
    // real ground reference. With the stub rasters (offline tests),
    // elevation is 0.0.
    let elevation = rasters.elevation(lat, lng);
    let t_load = t_start.elapsed();
    let sources = collect_from_square_data(&square_refs, lat, lng)
        .map_err(|error| Error::new(Status::GenericFailure, error))?;
    let t_collect = t_start.elapsed() - t_load;
    drop(store);

    let config = noise_compute::types::ComputeConfig {
        n_days: sources.n_days,
        ..Default::default()
    };

    let n_airborne = sources
        .aircraft_airborne_batches
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    let n_cruise = sources
        .aircraft_cruise_batches
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    let n_traffic = sources
        .aircraft_airport_traffic_batches
        .iter()
        .map(|b| b.num_rows())
        .sum::<usize>();
    let n_aircraft = n_airborne + n_cruise + n_traffic;
    let n_roads = sources.roads.len();
    let n_railways = sources.railways.len();

    // Vector obstacles: the exact building crossings screening runs on, built
    // per query from the ring obstacle shards. There is no other building
    // representation, so a store that will not load fails the query.
    let data_dir = DATA_DIR
        .get()
        .ok_or_else(|| Error::new(Status::GenericFailure, "source_init was never called"))?;
    let obstacle_set = structure_store::load_obstacle_set(year_dir()?, data_dir, lat, lng)
        .map_err(|e| Error::new(Status::GenericFailure, e))?;
    // Select the enclosed footprint winner once; it supplies the effective
    // envelope delta for the aggregate indoor estimate while traces stay at
    // façade values.
    let inside_envelope = structure_store::point_inside_enclosed(&obstacle_set, lat, lng);
    // Search outward in one-metre cardinal steps using the same containment
    // rule. The ≤100 m shift stays inside the loaded ring, so sources need
    // no reload.
    let (facade_lat, facade_lng) = if inside_envelope.is_some() {
        let step_lat = 1.0 / grid::geo::M_PER_DEG_LAT;
        let step_lon = 1.0 / grid::geo::m_per_deg_lon(lat.to_radians());
        let mut outside = None;
        {
            let set = &obstacle_set;
            for distance in 1..=100 {
                for (dy, dx) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
                    let candidate = (
                        lat + dy * distance as f64 * step_lat,
                        lng + dx * distance as f64 * step_lon,
                    );
                    if structure_store::point_inside_enclosed(set, candidate.0, candidate.1)
                        .is_none()
                    {
                        outside = Some(candidate);
                        break;
                    }
                }
                if outside.is_some() {
                    break;
                }
            }
        }
        outside.unwrap_or((lat, lng))
    } else {
        (lat, lng)
    };
    // 1.4b: with a loaded store, the receiver reflection probe answers from
    // exact footprints too (the popup twin of the pipeline rx_refl pre-bake)
    // — one wrapped sampler serves EVERY popup kernel.
    let vector_refl = Some(&obstacle_set).map(|set| {
        noise_compute::propagation::obstacle_index::VectorReflectionSampler {
            inner: rasters,
            set,
        }
    });
    let rasters: &dyn noise_compute::types::RasterSampler = match &vector_refl {
        Some(w) => w,
        None => rasters,
    };
    let receiver = noise_compute::types::Receiver::new(
        facade_lat,
        facade_lng,
        rasters.elevation(facade_lat, facade_lng),
    );

    let mut traces = noise_compute::types::TraceCollector::new();
    // M4/M5: hand the per-row baked admins to the kernels through their
    // thread-local channels (RoadSegment/RailSegment are codever-SHARED and
    // cannot carry the field). The guard clears on scope exit INCLUDING a
    // kernel unwind — napi-rs turns a caught panic into a JS throw, and a
    // stale vec on the surviving worker thread would paint the previous
    // click's countries onto the next query's segments.
    noise_compute::defaults::set_road_row_admins(Some(sources.road_admins));
    noise_compute::emission::railway::set_rail_row_admins(Some(sources.rail_admins));
    let _row_admin_guard = RowAdminGuard;
    let mut result = noise_compute::compute_at_point_with_traces(
        &receiver,
        &sources.roads,
        &sources.railways,
        &sources.buildings,
        &sources.industrial,
        &obstacle_set,
        rasters,
        &config,
        Some(&mut traces),
    );
    drop(_row_admin_guard);
    aircraft_v6::add_v6_aircraft_to_result(
        &mut result,
        &mut traces,
        &receiver,
        &sources.aircraft_airborne_batches,
        &sources.aircraft_cruise_batches,
        &sources.aircraft_airport_traffic_batches,
        &sources.airport_lines_batches,
        Some(airport_summary_pathbuf.as_path()),
        rasters,
        &obstacle_set,
        sources.n_days,
        top_k_per_kind,
    )
    .map_err(|e| Error::new(Status::GenericFailure, e))?;
    let t_compute = t_start.elapsed() - t_load - t_collect;

    let summary = apply_segment_top_k_with_cap(&mut traces, top_k_per_kind);
    result.segments = std::mem::take(&mut traces.segments);
    result.segments_meta = Some(summary);

    // Stamp wrapper timings before serializing so the popup JSON carries
    // the full per-component breakdown for the frontend debug overlay.
    if let Some(t) = result.timings.as_mut() {
        t.load_ms = t_load.as_secs_f64() * 1000.0;
        t.collect_ms = t_collect.as_secs_f64() * 1000.0;
    }
    let facade_lden = result.total.lden_db;
    let indoor = inside_envelope.and_then(|winner| {
        winner
            .effective_class
            .delta_db()
            .map(|delta| (winner.stored_class, delta))
    });
    // Inside a building the popup publishes the indoor estimate in every level
    // row, the same quantity the painted tile stores per layer.
    noise_compute::present::project_result_to_indoor_display(
        &mut result,
        indoor.map(|(_, delta)| delta),
    );
    let wire_result = wire::build_wire_result(
        result,
        lat,
        lng,
        elevation,
        indoor.map(|(class, delta)| (class, delta, facade_lden)),
    );
    let json = serde_json::to_string(&wire_result).unwrap();
    let t_total = t_start.elapsed();

    if timing_on {
        eprintln!(
            "popup-timing total={:.0}ms load={:.0}ms collect={:.0}ms compute={:.0}ms json={:.0}ms (rd={} rl={} ac={})",
            t_total.as_secs_f64() * 1000.0,
            t_load.as_secs_f64() * 1000.0,
            t_collect.as_secs_f64() * 1000.0,
            t_compute.as_secs_f64() * 1000.0,
            (t_total - t_load - t_collect - t_compute).as_secs_f64() * 1000.0,
            n_roads,
            n_railways,
            n_aircraft,
        );
    }

    Ok(json)
}

#[cfg(feature = "node")]
const SEGMENT_TOP_K_PER_KIND: usize = 150;

/// Upper bound for the "Show all" response. Higher than the default cap but
/// still bounded — NAPI's string return cannot carry the full airport payload
/// (millions of segments) and browsers can't parse it either.
#[cfg(feature = "node")]
const SEGMENT_TOP_K_PER_KIND_FULL: usize = 1000;

/// Stub raster sampler — flat terrain, no buildings, no vegetation.
/// Used as fallback when DEM/raster tiles are not available on disk.
#[cfg(feature = "node")]
struct StubRasters;

#[cfg(feature = "node")]
use noise_compute::types::RasterSampler;

#[cfg(feature = "node")]
impl RasterSampler for StubRasters {
    fn elevation(&self, _lat: f64, _lon: f64) -> f64 {
        200.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        0.5
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

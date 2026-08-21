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
pub mod geo;
pub mod hex_store;
pub mod obstacle_store;
pub mod query;
#[cfg(feature = "node")]
pub mod wire;

// Re-export the pure point-query API at the crate root so its paths
// (`source_reader::PointQueryData`, `collect_sources_at_point`, …) are
// unchanged after the lib.rs/query.rs split, and so the `#[napi]` wrappers
// below resolve `collect_from_hex_data` / `apply_segment_top_k_with_cap`.
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
use hex_store::HexData;
use hex_store::{
    load_hex, query_barriers_from_batches, query_buildings_from_batches, query_roads_from_batches,
};

#[cfg(feature = "node")]
static STORE: std::sync::LazyLock<RwLock<HexStore>> =
    std::sync::LazyLock::new(|| RwLock::new(HexStore::new()));

#[cfg(feature = "node")]
static RASTERS: std::sync::OnceLock<raster_reader::RealRasters> = std::sync::OnceLock::new();
/// Data root (`…/data/prepared`) captured at `source_init` — the vector
/// obstacle loader resolves its staging tree relative to it (geodata-v2 1.4).
static DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
/// The live `…/prepared/{year}/h3r4` dir — the PROMOTED obstacle root
/// (post-Wave-1 shards live beside each cell's arrows).
static H3R4_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

// NACE codes are now baked into industrial.arrow (nace_4digit UInt16 column).
// No global lookup needed at runtime.

#[cfg(feature = "node")]
struct HexStore {
    hexes: HashMap<String, HexData>,
    h3r4_dir: String,
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
impl HexStore {
    fn new() -> Self {
        HexStore {
            hexes: HashMap::new(),
            h3r4_dir: String::new(),
        }
    }
}

/// Make every hex in `hex_ids` resident, loading the missing ones IN
/// PARALLEL and OUTSIDE the store lock. Cold loads used to run
/// sequentially (7 hexes × ~12 files) under a held write lock — the whole
/// point of a shared store (all pool workers read one cache since
/// 2026-07-10) is that one visitor's cold load must neither serialize with
/// nor block everyone else's warm queries. First insert wins on a race —
/// the duplicate load is dropped, which is rare and harmless.
#[cfg(feature = "node")]
fn ensure_hexes_parallel(hex_ids: &[String]) {
    let missing: Vec<String> = {
        let store = STORE.read().expect("hex store poisoned");
        hex_ids
            .iter()
            .filter(|id| !store.hexes.contains_key(id.as_str()))
            .cloned()
            .collect()
    };
    if missing.is_empty() {
        return;
    }
    let h3r4_dir = STORE.read().expect("hex store poisoned").h3r4_dir.clone();
    let loaded: Vec<(String, HexData)> = std::thread::scope(|scope| {
        let handles: Vec<_> = missing
            .iter()
            .map(|hex_id| {
                let dir = format!("{h3r4_dir}/{hex_id}");
                scope.spawn(move || match load_hex(&dir) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("  source-reader: failed to load hex {hex_id}: {e}");
                        HexData::empty()
                    }
                })
            })
            .collect();
        missing
            .iter()
            .cloned()
            .zip(
                handles
                    .into_iter()
                    .map(|h| h.join().expect("hex load panicked")),
            )
            .collect()
    });
    let mut store = STORE.write().expect("hex store poisoned");
    for (id, data) in loaded {
        store.hexes.entry(id).or_insert(data);
    }
}

#[cfg(feature = "node")]
#[napi]
pub fn source_init(h3r4_dir: String) -> napi::Result<String> {
    let mut store = STORE
        .write()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    // The pool workers share ONE library instance (single addon path since
    // 2026-07-10), so every worker spawn/recycle calls source_init on the
    // SAME store — re-init with an unchanged dir must keep the shared cache,
    // not clear it out from under the other workers.
    if store.h3r4_dir == h3r4_dir {
        return Ok(format!(
            "source-reader already initialized: {h3r4_dir} ({} hexes cached, shared store)",
            store.hexes.len()
        ));
    }
    store.h3r4_dir = h3r4_dir.clone();
    store.hexes.clear();

    // Rasters are at data/prepared/{dem,rasters}/ — two levels up from data/prepared/{year}/h3r4/
    let h3r4_path = std::path::Path::new(&h3r4_dir);
    let data_dir = h3r4_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(std::path::Path::new("."));
    let rasters = raster_reader::RealRasters::new(data_dir);
    let has_dem = rasters.has_data();
    RASTERS.set(rasters).ok();
    DATA_DIR.set(data_dir.to_path_buf()).ok();
    H3R4_DIR.set(h3r4_path.to_path_buf()).ok();

    // NACE codes are baked into industrial.arrow — no global JSON needed

    // Admin table for the defaults cascade (plan v5 §F.3). File lives
    // at data/prepared/h3r4-admin.bin (year-independent, like rasters/DEM).
    // Soft-fail — missing file just leaves the cascade in WORLD arm.
    let admin_bin = noise_compute::admin::default_admin_path(h3r4_path);
    let admin_status = match noise_compute::admin::init_admin_table(&admin_bin) {
        Ok(n) => format!("loaded {n} entries"),
        Err(e) => format!("unavailable ({e})"),
    };

    Ok(format!(
        "source-reader initialized: {h3r4_dir} (DEM: {}; admin: {admin_status})",
        if has_dem { "loaded" } else { "stub" },
    ))
}

/// Strictly parse one known non-empty roads archive for runtime readiness.
/// Unlike popup queries, this does not read from or write to `STORE`, so a
/// readiness probe can never pin a partially rewritten H3 cell in the cache.
#[cfg(feature = "node")]
#[napi]
pub fn source_validate_reference(h3r4_dir: String, hex_id: String) -> napi::Result<u32> {
    let rows = hex_store::validate_reference_roads(std::path::Path::new(&h3r4_dir), &hex_id)
        .map_err(|error| Error::new(Status::GenericFailure, error))?;
    u32::try_from(rows).map_err(|_| {
        Error::new(
            Status::GenericFailure,
            format!("reference roads row count exceeds u32: {rows}"),
        )
    })
}

#[cfg(feature = "node")]
#[napi]
pub fn query_roads(lat: f64, lng: f64, max_radius_m: f64) -> napi::Result<String> {
    let hex_ids = geo::grid_disk_r4(lat, lng);
    ensure_hexes_parallel(&hex_ids);
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;

    let mut all_results = Vec::new();
    for hex_id in &hex_ids {
        let Some(data) = store.hexes.get(hex_id.as_str()) else {
            continue;
        };
        let road_batches = data.roads.batches_within(lat, lng, max_radius_m);
        let mut results = query_roads_from_batches(&road_batches, lat, lng, max_radius_m);
        all_results.append(&mut results);
    }

    Ok(serde_json::to_string(&all_results).unwrap())
}

#[cfg(feature = "node")]
#[napi]
pub fn query_buildings(lat: f64, lng: f64, max_radius_m: f64) -> napi::Result<String> {
    let hex_ids = geo::grid_disk_r4(lat, lng);
    ensure_hexes_parallel(&hex_ids);
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;

    let mut all_results = Vec::new();
    for hex_id in &hex_ids {
        let Some(data) = store.hexes.get(hex_id.as_str()) else {
            continue;
        };
        let building_batches = data.buildings.batches_within(lat, lng, max_radius_m);
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
    let h3r4 = H3R4_DIR.get().map(|p| p.as_path());
    let data_dir = DATA_DIR
        .get()
        .map(|p| p.as_path())
        .unwrap_or_else(|| std::path::Path::new("."));
    let fps = obstacle_store::footprints_in_bbox(h3r4, data_dir, south, west, north, east);
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

#[cfg(feature = "node")]
#[napi]
pub fn query_barriers(lat: f64, lng: f64, max_radius_m: f64) -> napi::Result<String> {
    let hex_ids = geo::grid_disk_r4(lat, lng);
    ensure_hexes_parallel(&hex_ids);
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;

    let mut all_results = Vec::new();
    for hex_id in &hex_ids {
        let Some(data) = store.hexes.get(hex_id.as_str()) else {
            continue;
        };
        let barrier_batches = data.barriers.batches_within(lat, lng, max_radius_m);
        let mut results = query_barriers_from_batches(&barrier_batches, lat, lng, max_radius_m)
            .map_err(|error| Error::new(Status::GenericFailure, error))?;
        all_results.append(&mut results);
    }

    let all_results = hex_store::canonicalize_barrier_results(all_results)
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
        store.hexes.remove(hex_id);
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

    let hex_ids = geo::grid_disk_r4(lat, lng);
    // Load missing hexes in parallel WITHOUT holding the store lock, then
    // collect under a read lock — concurrent popups on other workers keep
    // running against the shared cache during a cold load.
    ensure_hexes_parallel(&hex_ids);
    let store = STORE
        .read()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    let hex_refs: Vec<&hex_store::HexData> = hex_ids
        .iter()
        .filter_map(|id| store.hexes.get(id.as_str()))
        .collect();

    // Resolve airport_summary.arrow path: sibling of h3r4_dir under
    // `aircraft/` (Stage 2C v5 reduce output). Missing file →
    // popup returns zero airport-level counts (per Codex C4).
    let airport_summary_pathbuf = std::path::Path::new(&store.h3r4_dir)
        .parent()
        .map(|p| p.join("aircraft").join("airport_summary.arrow"));

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
    let sources = collect_from_hex_data(&hex_refs, lat, lng)
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

    // Vector obstacles (geodata-v2, QM_VECTOR_BUILDINGS=1): exact building
    // crossings replace the raster building channel in screening. Built per
    // query from the ring-1 obstacle shards; None keeps the raster path.
    let obstacle_set = if noise_compute::propagation::obstacle_index::vector_buildings_enabled() {
        DATA_DIR.get().and_then(|d| {
            obstacle_store::load_obstacle_set(H3R4_DIR.get().map(|p| p.as_path()), d, lat, lng)
        })
    } else {
        None
    };
    // Fix 4 (popup half): does the receiver stand INSIDE a footprint? The
    // heatmap masks such pixels to no-data, so the popup must be able to say
    // so. The shared winner also supplies the effective envelope delta for the
    // aggregate indoor display estimate; source traces remain façade values.
    let inside_envelope = obstacle_set
        .as_ref()
        .and_then(|set| obstacle_store::point_inside_enclosed(set, lat, lng));
    // A façade receiver is the first cardinal metre step outside the same
    // enclosed-containment query used by paint. The source selection was made
    // at the click and a one-metre shift never changes its R4 ring.
    let (facade_lat, facade_lng) = if inside_envelope.is_some() {
        let step_lat = 1.0 / noise_compute::constants::M_PER_DEG_LAT;
        let step_lon = 1.0 / noise_compute::constants::m_per_deg_lon(lat.to_radians());
        let mut outside = None;
        if let Some(set) = obstacle_set.as_ref() {
            for distance in 1..=100 {
                for (dy, dx) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
                    let candidate = (
                        lat + dy * distance as f64 * step_lat,
                        lng + dx * distance as f64 * step_lon,
                    );
                    if obstacle_store::point_inside_enclosed(set, candidate.0, candidate.1)
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
    // — one wrapped sampler serves EVERY popup kernel, raster otherwise.
    let vector_refl = obstacle_set.as_ref().map(|set| {
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
    // click's countries onto the next query's segments (/gg M4/M5 #2).
    noise_compute::defaults::set_road_row_admins(Some(sources.road_admins));
    noise_compute::emission::railway::set_rail_row_admins(Some(sources.rail_admins));
    let _row_admin_guard = RowAdminGuard;
    let mut result = noise_compute::compute_at_point_with_traces(
        &receiver,
        &sources.roads,
        &sources.railways,
        &sources.buildings,
        &sources.industrial,
        &sources.barriers,
        obstacle_set.as_ref(),
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
        &sources.synth_airport_lines_batches,
        airport_summary_pathbuf.as_deref(),
        rasters,
        &sources.barriers,
        obstacle_set.as_ref(),
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
            .map(|delta| (winner.stored_class, winner.height_m, delta))
    });
    if let Some((_, _, delta)) = indoor {
        wire::attenuate_total_for_indoor_display(&mut result, delta);
    }
    let wire_result = wire::build_wire_result(
        result,
        lat,
        lng,
        elevation,
        indoor.map(|(class, height, delta)| (class, height, delta, facade_lden)),
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
    fn building_height(&self, _: f64, _: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _: f64, _: f64) -> f64 {
        0.5
    }
    fn building_enclosure(&self, _: f64, _: f64) -> f64 {
        0.0
    }
}

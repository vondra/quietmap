//! source-reader: lazy Arrow IPC reader for noise popup.
//! Decoded batches are retained only for the current and concurrently active areas.

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
pub mod square_obstacle_index;
pub mod structure_store;
pub mod structures_finalize;
#[cfg(test)]
mod structure_test_fixture;
pub mod surface_corner_preview;
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
use std::collections::{HashMap, HashSet};
#[cfg(feature = "node")]
use std::sync::{Arc, RwLock};

#[cfg(feature = "node")]
use square_store::store::SquareData;

#[cfg(feature = "node")]
static STORE: std::sync::LazyLock<RwLock<SquareStore>> =
    std::sync::LazyLock::new(|| RwLock::new(SquareStore::new()));

#[cfg(feature = "node")]
static RASTERS: std::sync::OnceLock<raster_reader::RealRasters> = std::sync::OnceLock::new();
/// The live `…/prepared/2026` dir — the structure root: every prepared
/// square carries its own `structures.arrow` and `structures.qoix` under
/// `z9/<x>/<y>/` beside its other arrows.
#[cfg(feature = "node")]
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
    squares: HashMap<String, Arc<SquareData>>,
    prepared_dir: String,
}

#[cfg(feature = "node")]
impl SquareStore {
    fn new() -> Self {
        SquareStore {
            squares: HashMap::new(),
            prepared_dir: String::new(),
        }
    }

    /// Pin cache hits while the caller loads misses outside the global lock.
    /// These Arcs close the scan-to-insert race: a disjoint acquisition sees
    /// their strong counts and cannot evict a hit before this caller returns.
    fn pin_cached(&self, square_names: &[String]) -> (Vec<Arc<SquareData>>, Vec<String>) {
        let mut pinned = Vec::new();
        let mut missing = Vec::new();
        for id in square_names {
            match self.squares.get(id.as_str()) {
                Some(data) => pinned.push(Arc::clone(data)),
                None => missing.push(id.clone()),
            }
        }
        (pinned, missing)
    }

    /// Keep the requested working set plus every concurrently pinned square.
    fn retain_working_set(&mut self, square_names: &[String]) {
        let requested: HashSet<&str> = square_names.iter().map(String::as_str).collect();
        self.squares
            .retain(|id, data| requested.contains(id.as_str()) || Arc::strong_count(data) > 1);
    }

    /// Clone every requested square before evicting anything. The returned
    /// Arcs keep the query valid without holding the global lock.
    fn pin_working_set(&mut self, square_names: &[String]) -> Vec<Arc<SquareData>> {
        let acquired: Vec<_> =
            square_names
                .iter()
                .map(|id| {
                    Arc::clone(self.squares.get(id.as_str()).expect(
                        "a successful working-set load must contain every requested square",
                    ))
                })
                .collect();
        self.retain_working_set(square_names);
        acquired
    }
}

/// Load outside the store lock, then atomically pin the complete requested set.
/// Successful acquisition drops old areas that no active query has pinned, so
/// decoded Arrow bodies grow with concurrent working sets rather than process
/// history. First insert wins on a race; a load error changes nothing.
#[cfg(feature = "node")]
fn acquire_squares_parallel(square_names: &[String]) -> napi::Result<Vec<Arc<SquareData>>> {
    let (cached_pins, missing, prepared_dir) = {
        let store = STORE.read().expect("square store poisoned");
        let (cached_pins, missing) = store.pin_cached(square_names);
        (cached_pins, missing, store.prepared_dir.clone())
    };
    if missing.is_empty() {
        let mut store = STORE.write().expect("square store poisoned");
        let acquired = store.pin_working_set(square_names);
        drop(cached_pins);
        return Ok(acquired);
    }
    let workers = std::thread::available_parallelism().map_or(1, usize::from);
    let loaded: Result<Vec<(String, SquareData)>, String> = std::thread::scope(|scope| {
        let handles: Vec<_> = missing
            .chunks(missing.len().div_ceil(workers))
            .map(|names| {
                let prepared_dir = &prepared_dir;
                scope.spawn(move || {
                    names
                        .iter()
                        .map(|name| {
                            let dir = format!("{prepared_dir}/{name}");
                            square_store::store::load_square(std::path::Path::new(&dir))
                                .map(|data| (name.clone(), data))
                                .map_err(|error| format!("failed to load square {name}: {error}"))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("square load panicked"))
            .collect::<Result<Vec<_>, _>>()
            .map(|chunks| chunks.into_iter().flatten().collect())
    });
    let loaded = loaded.map_err(|error| Error::new(Status::GenericFailure, error))?;
    let mut store = STORE.write().expect("square store poisoned");
    for (id, data) in loaded {
        store.squares.entry(id).or_insert_with(|| Arc::new(data));
    }
    let acquired = store.pin_working_set(square_names);
    drop(cached_pins);
    Ok(acquired)
}

/// Drop decoded source batches from completed, unrelated queries. Active
/// callers remain pinned, and the next exact acquire still owns correctness.
#[cfg(feature = "node")]
fn prune_source_cache(square_names: &[String]) -> napi::Result<()> {
    let mut store = STORE
        .write()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    store.retain_working_set(square_names);
    Ok(())
}

#[cfg(all(test, feature = "node"))]
mod square_cache_tests;

#[cfg(feature = "node")]
#[napi]
pub fn source_init(prepared_dir: String) -> napi::Result<String> {
    if prepared_dir.is_empty() {
        return Err(Error::new(
            Status::InvalidArg,
            "prepared directory cannot be empty",
        ));
    }
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
    if !store.prepared_dir.is_empty() {
        return Err(Error::new(
            Status::InvalidArg,
            "prepared directory cannot change within one native process; start a new process",
        ));
    }
    store.prepared_dir = prepared_dir.clone();

    // Native raster windows share `<prepared>/2026/z9/<x>/<y>/` with vector
    // shards: a data file, a 0-byte ocean file, or an error at first use.
    let year_path = std::path::Path::new(&prepared_dir);
    RASTERS.set(raster_reader::RealRasters::new(year_path)).ok();
    YEAR_DIR.set(year_path.to_path_buf()).ok();

    // NACE codes are baked into industrial.arrow — no global JSON needed

    noise_compute::square_country_city::set_square_country_city_prepared_directory(year_path);

    Ok(format!("source-reader initialized: {prepared_dir}"))
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
fn source_square_names(squares: Result<Vec<grid::Square>, String>) -> napi::Result<Vec<String>> {
    squares
        .map(|squares| squares.into_iter().map(grid::square_name).collect())
        .map_err(|error| Error::new(Status::InvalidArg, error))
}

#[cfg(feature = "node")]
#[napi]
/// Obstacle footprints intersecting a bbox with their AS-USED heights (after
/// the low-profile cap) — the building-height debug overlay's data source,
/// so the map shows exactly what the propagation model screens with. JSON:
/// [{p: [polygon rings…], h, t, c}] (rings are [lat,lon] vertices, exterior
/// first, then holes; h = height m, t = height
/// tier 0 mapped/1 floors/2 default/3 city-measured zonal/4 ANBH areal prior
/// — see noise_compute::low_profile, c = low-profile-capped).
pub fn query_obstacle_footprints(
    south: f64,
    west: f64,
    north: f64,
    east: f64,
) -> napi::Result<String> {
    prune_source_cache(&[])?;
    let fps = structure_store::footprints_in_bbox(year_dir()?, south, west, north, east)
        .map_err(|e| Error::new(Status::GenericFailure, e))?;
    Ok(serde_json::to_string(&fps).unwrap())
}

/// Map the engine's envelope class to the small plain-language vocabulary
/// used by the building hover tooltip. Kept outside `structure_store.rs` so
/// changing display wording does not rotate its disk-index cache version.
#[cfg(any(feature = "node", test))]
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
/// winner selection used by the popup, without running noise
/// collection or propagation.
#[cfg(feature = "node")]
#[napi]
pub fn query_building_at(lat: f64, lng: f64) -> napi::Result<String> {
    prune_source_cache(&[])?;
    // A missing obstacle store is an error, not an empty answer. It used to
    // return {"status":"unavailable"} inside an HTTP 200, which reads to a
    // visitor exactly like "there is no building here".
    let set = structure_store::load_obstacle_set(year_dir()?, lat, lng)
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

    let initial_square_names = source_square_names(squares_within_reach(lat, lng))?;
    prune_source_cache(&initial_square_names)?;
    let mut obstacle_set = structure_store::load_obstacle_set(year_dir()?, lat, lng)
        .map_err(|error| Error::new(Status::GenericFailure, error))?;
    let (facade_lat, facade_lng, inside_envelope) =
        structure_store::locate_facade_receiver(&obstacle_set, lat, lng);
    if (facade_lat, facade_lng) != (lat, lng) {
        obstacle_set = structure_store::load_obstacle_set(year_dir()?, facade_lat, facade_lng)
            .map_err(|error| Error::new(Status::GenericFailure, error))?;
    }

    let square_names = source_square_names(squares_within_reach(facade_lat, facade_lng))?;
    // The returned Arcs pin this whole query without holding the global lock.
    // Concurrent popups can load or reuse their own working sets while this
    // one decodes batches and computes.
    let squares = acquire_squares_parallel(&square_names)?;
    let square_refs: Vec<_> = square_names
        .iter()
        .zip(&squares)
        .map(|(id, data)| {
            (
                grid::parse_square_name(id).expect("canonical square name"),
                data.as_ref(),
            )
        })
        .collect();

    let t_load = t_start.elapsed();
    let sources = collect_from_square_data(&square_refs, facade_lat, facade_lng)
        .map_err(|error| Error::new(Status::GenericFailure, error))?;
    let t_collect = t_start.elapsed() - t_load;

    let real_rasters = RASTERS
        .get()
        .ok_or_else(|| Error::new(Status::GenericFailure, "source_init was never called"))?;
    let checked = raster_reader::CheckedRasters::new(real_rasters);
    let rasters: &dyn noise_compute::types::RasterSampler = &checked;
    let elevation = rasters.elevation(lat, lng);
    checked
        .ensure_valid()
        .map_err(|error| Error::new(Status::GenericFailure, error.to_string()))?;

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

    // 1.4b: with a loaded store, the receiver reflection probe answers from
    // exact footprints too (the popup twin of the pipeline rx_refl pre-bake)
    // — one wrapped sampler serves EVERY popup kernel.
    let vector_refl = noise_compute::propagation::obstacle_index::VectorReflectionSampler {
        inner: rasters,
        set: &obstacle_set,
    };
    let rasters: &dyn noise_compute::types::RasterSampler = &vector_refl;
    let receiver = noise_compute::types::Receiver::new(
        facade_lat,
        facade_lng,
        rasters.elevation(facade_lat, facade_lng),
    );

    let mut traces = noise_compute::types::TraceCollector::new();
    let mut result = noise_compute::compute_at_point(
        &receiver,
        &sources.roads,
        &sources.railways,
        &sources.buildings,
        &sources.industrial,
        &obstacle_set,
        rasters,
        Some(&mut traces),
    );
    let t_ground = t_start.elapsed() - t_load - t_collect;
    aircraft_v6::add_v6_aircraft_to_result(
        &mut result,
        &mut traces,
        &receiver,
        &sources.aircraft_airborne_batches,
        &sources.aircraft_cruise_batches,
        &sources.aircraft_airport_traffic_batches,
        &sources.airport_lines_batches,
        &sources.airport_summary,
        rasters,
        &obstacle_set,
        sources.n_days,
        top_k_per_kind,
    )
    .map_err(|e| Error::new(Status::GenericFailure, e))?;
    checked
        .ensure_valid()
        .map_err(|error| Error::new(Status::GenericFailure, error.to_string()))?;
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
    // row, derived from the outdoor facade level.
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
            "popup-timing total={:.0}ms load={:.0}ms collect={:.0}ms compute={:.0}ms (ground={:.0}ms air={:.0}ms) json={:.0}ms (rd={} rl={} ac={})",
            t_total.as_secs_f64() * 1000.0,
            t_load.as_secs_f64() * 1000.0,
            t_collect.as_secs_f64() * 1000.0,
            t_compute.as_secs_f64() * 1000.0,
            t_ground.as_secs_f64() * 1000.0,
            (t_compute - t_ground).as_secs_f64() * 1000.0,
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

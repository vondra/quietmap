//! source-reader: mmap'd Arrow IPC reader for noise popup.
//! Zero-copy: data stays in mmap'd pages, queries iterate directly over Arrow columns.

// mimalloc handles popup's many small short-lived allocs (SegmentTrace
// + Box<PropagationBreakdown> + inner Vec<f32>) faster than glibc malloc
// — Microsoft Research benchmarks ~2× speedup for similar workloads.
// At LKPR the per-popup drop cascade (~6 k traces × ~10 inner allocs)
// is the hot spot remaining in apply_segment_top_k_with_cap.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod aircraft_v6;
pub mod geo;
pub mod hex_store;
pub mod popup;
pub mod query;
#[cfg(feature = "node")]
mod result_cache;
pub mod structure_store;
#[cfg(test)]
mod structure_test_fixture;
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
use hex_store::{load_hex, HexData};

/// Loaded hexes, shared by every pool worker (one library instance per
/// process); filled once per data dir, never invalidated inside a process.
#[cfg(feature = "node")]
static STORE: std::sync::LazyLock<RwLock<HashMap<String, std::sync::Arc<HexData>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

#[cfg(feature = "node")]
static RASTERS: std::sync::OnceLock<raster_reader::RealRasters> = std::sync::OnceLock::new();
/// Data root (`…/data/prepared`) captured at `source_init` — the vector
/// obstacle loader keeps its on-disk index cache under it (geodata-v2 1.4).
#[cfg(feature = "node")]
static DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
/// The live `…/prepared/{year}/h3r4` dir — the structure root: every prepared
/// cell carries its own `structures.arrow` beside its other arrows.
#[cfg(feature = "node")]
static H3R4_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The obstacle root, or the one error that explains an unset one. Buildings
/// are vector-only, so a query without this root has no answer to give.
#[cfg(feature = "node")]
fn h3r4_dir() -> napi::Result<&'static std::path::Path> {
    H3R4_DIR
        .get()
        .map(|p| p.as_path())
        .ok_or_else(|| Error::new(Status::GenericFailure, "source_init was never called"))
}

// NACE codes are now baked into industrial.arrow (nace_4digit UInt16 column).
// No global lookup needed at runtime.

/// Make every hex in `hex_ids` resident, loading the missing ones IN
/// PARALLEL and OUTSIDE the store lock. Cold loads used to run
/// sequentially (7 hexes × ~12 files) under a held write lock — the whole
/// point of a shared store (all pool workers read one cache since
/// 2026-07-10) is that one visitor's cold load must neither serialize with
/// nor block everyone else's warm queries. First insert wins on a race —
/// the duplicate load is dropped, which is rare and harmless.
#[cfg(feature = "node")]
fn ensure_hexes_parallel(hex_ids: &[String]) -> napi::Result<()> {
    let missing: Vec<String> = {
        let store = STORE.read().expect("hex store poisoned");
        hex_ids
            .iter()
            .filter(|id| !store.contains_key(id.as_str()))
            .cloned()
            .collect()
    };
    if missing.is_empty() {
        return Ok(());
    }
    let h3r4_dir = h3r4_dir()?;
    let loaded: Vec<(String, HexData)> = std::thread::scope(|scope| {
        let handles: Vec<_> = missing
            .iter()
            .map(|hex_id| {
                let dir = format!("{}/{hex_id}", h3r4_dir.display());
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
        store.entry(id).or_insert_with(|| std::sync::Arc::new(data));
    }
    Ok(())
}

#[cfg(feature = "node")]
#[napi]
pub fn source_init(h3r4_dir: String) -> napi::Result<String> {
    // The write lock serializes concurrent inits from pool workers.
    let store = STORE
        .write()
        .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
    let h3r4_path = std::path::Path::new(&h3r4_dir);
    // The pool workers share ONE library instance (single addon path since
    // 2026-07-10), so every worker spawn/recycle calls source_init on the
    // SAME store — re-init with an unchanged dir keeps the shared cache;
    // another dir in the same process has no data path and is refused.
    if let Some(current) = H3R4_DIR.get() {
        if current != h3r4_path {
            return Err(Error::new(
                Status::GenericFailure,
                format!(
                    "source-reader already initialized with {}, not {h3r4_dir}",
                    current.display()
                ),
            ));
        }
        return Ok(format!(
            "source-reader already initialized: {h3r4_dir} ({} hexes cached, shared store)",
            store.len()
        ));
    }

    // Rasters are at data/prepared/{dem,rasters}/ — two levels up from data/prepared/{year}/h3r4/
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

    // Admin for the defaults cascade (plan v5 §F.3): each queried cell's own
    // prepared/{year}/h3r4/<cell>/admin.bin, read on first use. A cell without
    // one simply leaves the cascade in its WORLD arm.
    noise_compute::admin::set_admin_h3r4_directory(h3r4_path);

    Ok(format!(
        "source-reader initialized: {h3r4_dir} (DEM: {})",
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
    let fps = structure_store::footprints_in_bbox(h3r4_dir()?, south, west, north, east)
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
#[cfg(feature = "node")]
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
    let set = structure_store::load_obstacle_set(h3r4_dir()?, data_dir, lat, lng)
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

#[cfg(all(test, feature = "node"))]
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

/// The popup's default answer for a point: total Lden, per-source breakdown,
/// top contributors and the segment summary — without the segment list,
/// which is 97 % of the bytes (3.8 of 3.95 MB in Prague) and which the
/// Segments tab fetches through `query_noise_segments`.
#[cfg(feature = "node")]
#[napi]
pub fn query_noise_at_point(lat: f64, lng: f64) -> napi::Result<String> {
    cached_point(lat, lng, false)
}

/// The same result with its segment list (top 150 per kind).
#[cfg(feature = "node")]
#[napi]
pub fn query_noise_segments(lat: f64, lng: f64) -> napi::Result<String> {
    cached_point(lat, lng, true)
}

/// The popup's "Show all": a fresh compute with a much higher per-kind cap
/// (1000 instead of 150), pressed once per click and not cached — at 18 MB
/// a result it would crowd out the default answers. The fully-unfiltered
/// airborne set at an airport is millions of segments, far beyond what a
/// browser can parse or what NAPI's string return can carry.
#[cfg(feature = "node")]
#[napi]
pub fn query_noise_at_point_unfiltered(lat: f64, lng: f64) -> napi::Result<String> {
    let result = compute_point(lat, lng, popup::SEGMENT_TOP_K_PER_KIND_FULL)?;
    Ok(serde_json::to_string(&result).unwrap())
}

/// Recent default-cap results, shared by every pool worker. Hex data never
/// changes inside a process (the store fills once per data dir), so a hit is
/// what a recompute would return. Measured 2026-09-07 on dev3: 32 cached
/// Prague points add 277 MB of anonymous RSS (8.7 MB each; their JSON is
/// 3.95 MB) — the bound for one process.
#[cfg(feature = "node")]
static RESULT_CACHE: result_cache::ResultCache<wire::WireResult> =
    result_cache::ResultCache::new(32);

/// One click asks the same point twice (card, then Segments tab) and repeat
/// clicks land on the same coordinates: compute once, serialize per request.
#[cfg(feature = "node")]
fn cached_point(lat: f64, lng: f64, with_segments: bool) -> napi::Result<String> {
    let key = (lat.to_bits(), lng.to_bits());
    if let Some(json) = RESULT_CACHE.get_with(key, |r| serialize_view(r, with_segments)) {
        return Ok(json);
    }
    let mut result = compute_point(lat, lng, popup::SEGMENT_TOP_K_PER_KIND)?;
    let json = serialize_view(&mut result, with_segments);
    RESULT_CACHE.put(key, result);
    Ok(json)
}

/// The result with or without its segment list; the segment summary stays
/// in both (serde skips an empty list).
#[cfg(feature = "node")]
fn serialize_view(result: &mut wire::WireResult, with_segments: bool) -> String {
    if with_segments {
        return serde_json::to_string(result).unwrap();
    }
    let segments = std::mem::take(&mut result.segments);
    let json = serde_json::to_string(result).unwrap();
    result.segments = segments;
    json
}

/// The popup's point computation over the shared store: make the ring
/// resident, then run [`popup::compute_point`] on it.
#[cfg(feature = "node")]
fn compute_point(lat: f64, lng: f64, top_k_per_kind: usize) -> napi::Result<wire::WireResult> {
    let t_start = std::time::Instant::now();
    let hex_ids = geo::grid_disk_r4(lat, lng);
    // Load missing hexes in parallel WITHOUT holding the store lock, then
    // take the ring's hexes out under a read lock held for microseconds,
    // not across collect: the lock prefers writers, so a reader that held
    // it through a seconds-long collect made every other popup queue behind
    // the next cold load's insert (Sahara 3.5 s in an 8-way run vs 12 ms
    // alone, 2026-09-06).
    ensure_hexes_parallel(&hex_ids)?;
    let hex_arcs: Vec<std::sync::Arc<HexData>> = {
        let store = STORE
            .read()
            .map_err(|e| Error::new(Status::GenericFailure, format!("{e}")))?;
        hex_ids
            .iter()
            .filter_map(|id| store.get(id.as_str()).cloned())
            .collect()
    };
    let hex_refs: Vec<&HexData> = hex_arcs.iter().map(|a| a.as_ref()).collect();
    let stub = StubRasters;
    let rasters: &dyn noise_compute::types::RasterSampler = match RASTERS.get() {
        Some(r) => r,
        None => &stub,
    };
    let data_dir = DATA_DIR
        .get()
        .ok_or_else(|| Error::new(Status::GenericFailure, "source_init was never called"))?;
    let load_ms = t_start.elapsed().as_secs_f64() * 1000.0;
    let mut result = popup::compute_point(
        &hex_refs,
        lat,
        lng,
        top_k_per_kind,
        h3r4_dir()?,
        data_dir,
        rasters,
    )
    .map_err(|e| Error::new(Status::GenericFailure, e))?;
    if let Some(t) = result.timings.as_mut() {
        t.load_ms = load_ms;
    }
    Ok(result)
}

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

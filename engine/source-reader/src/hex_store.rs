//! Load Arrow IPC File-format files via mmap, decoding record batches LAZILY.
//!
//! Files written after the popup-batch-pruning change (commit 70a1bd2c) carry
//! `qm_batch_bboxes` schema metadata: queries decode only the batches whose
//! bbox lies within the source class's audibility radius of the click
//! (docs/dev/popup-batch-pruning.md). Old single-batch files have no key and
//! decode in full — same rows as before, just on first touch instead of at
//! load time. Decoded batches are cached per slot (OnceLock), so the shared
//! store's warm path is unchanged and RAM is strictly <= the old eager load.

use arrow::array::*;
use arrow::datatypes::DataType;
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use memmap2::Mmap;
use noise_compute::propagation::streaming_reduction::SourceId64;
use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// One arrow file, opened (footer + schema only) but not decoded. Missing or
/// unreadable files behave as empty. `batch_bboxes` is `Some` ONLY when the
/// metadata entry count matches the file's batch count — any mismatch (old
/// files, enrichment rewrites that re-chunked) degrades to load-all, never to
/// wrong pruning.
pub struct LazyArrow {
    mmap: Option<Arc<Mmap>>,
    schema: Option<arrow::datatypes::SchemaRef>,
    batch_bboxes: Option<Vec<arrow_batching::RowBbox>>,
    slots: Vec<OnceLock<Option<RecordBatch>>>,
}

impl LazyArrow {
    pub fn empty() -> Self {
        LazyArrow {
            mmap: None,
            schema: None,
            batch_bboxes: None,
            slots: Vec::new(),
        }
    }

    /// Open `path`: mmap + IPC footer + schema. NO batch bodies are decoded
    /// here — `ensure_hexes_parallel` calls this for whole rings, and cold
    /// clicks must not pay for batches they will prune.
    pub fn open(path: &Path) -> Self {
        if !path.exists() {
            return Self::empty();
        }
        let Ok(file) = File::open(path) else {
            return Self::empty();
        };
        let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
            return Self::empty();
        };
        let mmap = Arc::new(mmap);
        let reader = match FileReader::try_new(Cursor::new(mmap.as_ref().as_ref()), None) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  source-reader: failed to read {}: {}", path.display(), e);
                return Self::empty();
            }
        };
        let schema = reader.schema();
        let num_batches = reader.num_batches();
        let batch_bboxes = schema
            .metadata()
            .get(arrow_batching::QM_BATCH_BBOXES_KEY)
            .and_then(|v| arrow_batching::parse_batch_bboxes(v))
            .filter(|b| b.len() == num_batches);
        LazyArrow {
            mmap: Some(mmap),
            schema: Some(schema),
            batch_bboxes,
            slots: (0..num_batches).map(|_| OnceLock::new()).collect(),
        }
    }

    /// File-level schema (None for missing/unreadable files) — metadata like
    /// contracts and `n_days` is readable WITHOUT decoding any batch.
    pub fn schema(&self) -> Option<&arrow::datatypes::SchemaRef> {
        self.schema.as_ref()
    }

    /// Decode batch `i` on first touch; a decode error caches as None (skip),
    /// matching the old eager loader's silent-drop of unreadable batches.
    fn batch(&self, i: usize) -> Option<&RecordBatch> {
        let mmap = self.mmap.as_ref()?;
        self.slots[i]
            .get_or_init(|| {
                let mut reader =
                    FileReader::try_new(Cursor::new(mmap.as_ref().as_ref()), None).ok()?;
                reader.set_index(i).ok()?;
                reader.next()?.ok()
            })
            .as_ref()
    }

    /// Every batch of the file (compat path + consumers that must see all
    /// rows, e.g. the airport-lines anchor extractor).
    pub fn batches_all(&self) -> Vec<RecordBatch> {
        (0..self.slots.len())
            .filter_map(|i| self.batch(i).cloned())
            .collect()
    }

    /// Batches whose bbox passes `keep` — the generic prune gate. Files
    /// without valid bbox metadata return everything. The predicate MUST be
    /// a superset of the class's row-level accept (batch bbox ⊇ row bboxes),
    /// or pruning drops audible sources.
    pub fn batches_where(
        &self,
        keep: impl Fn(&arrow_batching::RowBbox) -> bool,
    ) -> Vec<RecordBatch> {
        let Some(bboxes) = &self.batch_bboxes else {
            return self.batches_all();
        };
        (0..self.slots.len())
            .filter(|&i| keep(&bboxes[i]))
            .filter_map(|i| self.batch(i).cloned())
            .collect()
    }

    /// Circular gate for classes whose row-level accept is a planar
    /// distance ≤ radius. GATE_RADIUS_SLACK covers the metric mismatch:
    /// the gate's haversine measures ~111195 m/°lat while the row filters
    /// use flat 110540 m/°lat (`geo::flat_dist`), i.e. the gate sees a
    /// source up to ~0.6% FARTHER than the row filter does — without slack
    /// a row at 9,999 m planar (10,058 m haversine) would lose its batch
    /// (Codex /gg 2026-07-10). 2% strictly covers the mismatch + f32 bbox
    /// rounding; over-admitting a borderline batch costs one decode.
    pub fn batches_within(&self, lat: f64, lon: f64, radius_m: f64) -> Vec<RecordBatch> {
        const GATE_RADIUS_SLACK: f64 = 1.02;
        self.batches_where(|bb| {
            arrow_batching::point_to_bbox_distance_m(lat, lon, bb) <= radius_m * GATE_RADIUS_SLACK
        })
    }
}

/// All source data for one H3 res-4 hex — lazily-decoded Arrow IPC files.
pub struct HexData {
    /// Eagerly-loaded small files keep their mmaps alive here; LazyArrow
    /// fields own their mmap internally.
    _mmaps: Vec<Arc<Mmap>>,
    pub roads: LazyArrow,
    pub railways: LazyArrow,
    pub buildings: LazyArrow,
    pub barriers: LazyArrow,
    pub industrial: LazyArrow,
    /// Leisure AREA sources (`leisure.arrow`, settlement v2 phase 2) — sports
    /// pitch / playground / pool / beer garden. Folded into the building
    /// (settlement) layer in the popup.
    pub leisure: LazyArrow,
    pub aircraft_airborne: LazyArrow,
    pub aircraft_cruise: LazyArrow,
    /// Per-microsegment sparse traffic counters
    /// (`airport_traffic.arrow`). See [`add_v6_aircraft_to_result`].
    pub aircraft_airport_traffic: LazyArrow,
    /// OSM aeroway microsegments (`airport_lines.arrow`). Carries
    /// `osm_id` + `segment_idx` + nullable `ref` (e.g. "RWY 06/24")
    /// per microsegment. Source-reader uses this to map per-row
    /// airport_traffic.arrow rows back to their OSM way identifier so
    /// the popup can label SegmentTraces "LKPR RWY 06/24" instead of
    /// the generic "LKPR runway-roll". The same batches feed the
    /// per-osm_id runway-end anchor extractor. EAGER on purpose: the
    /// file is tiny and its consumers (labels + anchors) reason about
    /// runways possibly far from the click — pruning buys nothing.
    pub airport_lines_batches: Vec<RecordBatch>,
    /// Stage 1.5 DBSCAN-discovered airstrips
    /// (`synth_airport_lines.arrow`). Same microsegment shape as the
    /// real OSM lines but with `osm_id: UInt64` (high-bit-set)
    /// and an explicit `airport_key` column. Fed to the runway-end
    /// anchor extractor alongside the real lines so auto-discovered
    /// strips contribute to the airborne airport-context gate.
    /// EAGER for the same reason as `airport_lines_batches`.
    pub synth_airport_lines_batches: Vec<RecordBatch>,
}

impl HexData {
    pub fn empty() -> Self {
        HexData {
            _mmaps: vec![],
            roads: LazyArrow::empty(),
            railways: LazyArrow::empty(),
            buildings: LazyArrow::empty(),
            barriers: LazyArrow::empty(),
            industrial: LazyArrow::empty(),
            leisure: LazyArrow::empty(),
            aircraft_airborne: LazyArrow::empty(),
            aircraft_cruise: LazyArrow::empty(),
            aircraft_airport_traffic: LazyArrow::empty(),
            airport_lines_batches: vec![],
            synth_airport_lines_batches: vec![],
        }
    }
}

/// Load all source data from a hex directory. Only footers + schemas are
/// read here; batch bodies decode lazily at query time (`batches_within`).
pub fn load_hex(dir: &str) -> Result<HexData, String> {
    let path = Path::new(dir);
    if !path.exists() {
        return Ok(HexData::empty());
    }

    let mut mmaps = Vec::new();

    let buildings = LazyArrow::open(&path.join("buildings.arrow"));
    let leisure = LazyArrow::open(&path.join("leisure.arrow"));
    // settlement v2 phase 2: buildings re-numbered building_type + added columns,
    // leisure is a NEW file. A stale v1 buildings.arrow would feed OLD type ids
    // through the NEW emission profiles → silently wrong popup numbers. Fail loud
    // (Convention-B per-file contract); the fix is re-extract. Schema-level —
    // no batch decode needed.
    check_contract(
        &buildings,
        "buildings_contract",
        BUILDINGS_CONTRACT_V2,
        "buildings.arrow",
    )?;
    check_contract(
        &leisure,
        "leisure_contract",
        LEISURE_CONTRACT_V1,
        "leisure.arrow",
    )?;

    let airport_lines_batches = load_arrow_mmap(&path.join("airport_lines.arrow"), &mut mmaps);
    // Stage 1.5's synth strips live alongside the real OSM lines in
    // each R4 dir. Optional — older extracts predate Stage 1.5; the
    // loader returns an empty vec when the file is absent.
    let synth_airport_lines_batches =
        load_arrow_mmap(&path.join("synth_airport_lines.arrow"), &mut mmaps);

    Ok(HexData {
        _mmaps: mmaps,
        roads: LazyArrow::open(&path.join("roads.arrow")),
        railways: LazyArrow::open(&path.join("railways.arrow")),
        buildings,
        barriers: LazyArrow::open(&path.join("barriers.arrow")),
        industrial: LazyArrow::open(&path.join("industrial.arrow")),
        leisure,
        aircraft_airborne: LazyArrow::open(&path.join("airborne.arrow")),
        aircraft_cruise: LazyArrow::open(&path.join("cruise.arrow")),
        aircraft_airport_traffic: LazyArrow::open(&path.join("airport_traffic.arrow")),
        airport_lines_batches,
        synth_airport_lines_batches,
    })
}

/// settlement v2 phase 2 per-file contracts (source of truth:
/// `osm-extract::finalize`). Mirrored here so the popup rejects a stale
/// buildings.arrow whose `building_type` ids predate the re-numbering.
pub const BUILDINGS_CONTRACT_V2: &str = "buildings_v2";
pub const LEISURE_CONTRACT_V1: &str = "leisure_v1";

/// Verify a settlement arrow's file-level schema carries the expected
/// per-file contract stamp (Convention-B). Missing file passes. Fails loud
/// on mismatch so `load_hex` never serves mis-profiled buildings.
fn check_contract(arrow: &LazyArrow, key: &str, expected: &str, label: &str) -> Result<(), String> {
    let Some(schema) = arrow.schema() else {
        return Ok(());
    };
    let c = schema.metadata().get(key).map(String::as_str);
    if c != Some(expected) {
        return Err(format!(
            "{label} {key} mismatch (expected {expected}, got {c:?}) — \
             re-extract OSM (settlement v2 phase 2)"
        ));
    }
    Ok(())
}

/// Mmap an Arrow IPC File and return its RecordBatches (zero-copy).
fn load_arrow_mmap(path: &Path, mmaps: &mut Vec<Arc<Mmap>>) -> Vec<RecordBatch> {
    if !path.exists() {
        return vec![];
    }

    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let mmap = match unsafe { Mmap::map(&file) } {
        Ok(m) => Arc::new(m),
        Err(_) => return vec![],
    };

    // FileReader can read from a Cursor over the mmap'd bytes
    let cursor = Cursor::new(mmap.as_ref().as_ref());
    let reader = match FileReader::try_new(cursor, None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  source-reader: failed to read {}: {}", path.display(), e);
            return vec![];
        }
    };

    let batches: Vec<RecordBatch> = reader.filter_map(|r| r.ok()).collect();

    // Keep mmap alive
    mmaps.push(mmap);

    batches
}

/// Strictly parse the committed readiness cell's roads archive without adding
/// it to the process-wide H3 cache. The normal popup loader is deliberately
/// tolerant of missing/corrupt optional files and converts Arrow errors to an
/// empty batch list; readiness needs the opposite contract for its one known
/// non-empty reference file.
pub fn validate_reference_roads(h3r4_dir: &Path, hex_id: &str) -> Result<usize, String> {
    // H3 indexes are fixed-width hexadecimal strings. Besides catching caller
    // mistakes, this keeps the native helper confined below h3r4_dir.
    if hex_id.len() != 15 || !hex_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid H3 reference cell: {hex_id:?}"));
    }

    let path = h3r4_dir.join(hex_id).join("roads.arrow");
    let file =
        File::open(&path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let reader = FileReader::try_new(file, None)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let schema = reader.schema();
    for (name, expected) in [
        ("osm_id", DataType::Int64),
        ("start_lat", DataType::Float64),
        ("start_lon", DataType::Float64),
        ("end_lat", DataType::Float64),
        ("end_lon", DataType::Float64),
    ] {
        let field = schema.field_with_name(name).map_err(|_| {
            format!(
                "{} roads schema is missing required column {name}",
                path.display()
            )
        })?;
        if field.data_type() != &expected {
            return Err(format!(
                "{} roads schema column {name} must have type {expected:?}, got {:?}",
                path.display(),
                field.data_type()
            ));
        }
    }

    let mut rows = 0usize;
    for (batch_index, batch) in reader.enumerate() {
        let batch = batch.map_err(|error| {
            format!(
                "failed to read {} batch {batch_index}: {error}",
                path.display()
            )
        })?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or_else(|| format!("row count overflow in {}", path.display()))?;
    }
    if rows == 0 {
        return Err(format!("{} contains no road rows", path.display()));
    }
    Ok(rows)
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
    /// R7 taper graded effective speed (0 = none; absent column reads 0).
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
    batches: &[RecordBatch],
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
        let slat = col_f64(batch, "start_lat");
        let slon = col_f64(batch, "start_lon");
        let elat = col_f64(batch, "end_lat");
        let elon = col_f64(batch, "end_lon");
        let len = col_f32(batch, "length_m");
        let rclass = col_u8(batch, "road_class");
        let speed = col_u8(batch, "speed_limit");
        // Absent on pre-taper arrows → 0 = none (R7 taper writes it).
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
        let (Some(osm_id), Some(slat), Some(slon), Some(elat), Some(elon)) =
            (osm_id, slat, slon, elat, elon)
        else {
            continue;
        };

        for i in 0..n {
            // Cheap bbox reject FIRST, before the per-row normalize cascade.
            // ~99% of rows are far from the popup point (popup hits ~1-2 k of
            // ~900 k road segments per R4 ring); running normalize_road on all
            // of them was the dominant cost in collect_from_hex_data
            // (~160 ms warm). max_radius is the upper bound — final accept
            // uses effective_radius after normalize.
            let s_lat = slat.value(i);
            let e_lat = elat.value(i);
            let mid_lat = (s_lat + e_lat) * 0.5;
            let dlat = (lat - mid_lat).abs() * 110_540.0;
            if dlat > max_radius * 1.5 {
                continue;
            }
            let s_lon = slon.value(i);
            let e_lon = elon.value(i);
            let mid_lon = (s_lon + e_lon) * 0.5;
            let dlon = (lon - mid_lon).abs() * 111_320.0 * mid_lat.to_radians().cos();
            if dlon > max_radius * 1.5 {
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
            if dlat > effective_radius * 1.5 || dlon > effective_radius * 1.5 {
                continue;
            }

            // Exact closest point on segment
            let cp = crate::geo::closest_point_on_segment(lat, lon, s_lat, s_lon, e_lat, e_lon);
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
                    .unwrap_or_else(|| {
                        noise_compute::propagation::geo::flat_dist(s_lat, s_lon, e_lat, e_lon)
                            as f32
                    }),
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
    batches: &[RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<RailResult> {
    let mut results = Vec::new();

    for batch in batches {
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id");
        let slat = col_f64(batch, "start_lat");
        let slon = col_f64(batch, "start_lon");
        let elat = col_f64(batch, "end_lat");
        let elon = col_f64(batch, "end_lon");

        let (Some(osm_id), Some(slat), Some(slon), Some(elat), Some(elon)) =
            (osm_id, slat, slon, elat, elon)
        else {
            continue;
        };

        let seg_idx = col_i16(batch, "segment_idx");
        let len = col_f32(batch, "length_m");
        let rtype = col_u8(batch, "rail_type");
        let usage = col_u8(batch, "usage");
        let maxspd = col_u16_or_u8(batch, "maxspeed");
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
            let s_lat = slat.value(i);
            let s_lon = slon.value(i);
            let e_lat = elat.value(i);
            let e_lon = elon.value(i);

            let mid_lat = (s_lat + e_lat) / 2.0;
            let mid_lon = (s_lon + e_lon) / 2.0;
            let dlat = (lat - mid_lat).abs() * 110_540.0;
            if dlat > max_radius * 1.5 {
                continue;
            }
            let dlon = (lon - mid_lon).abs() * 111_320.0 * mid_lat.to_radians().cos();
            if dlon > max_radius * 1.5 {
                continue;
            }

            let cp = crate::geo::closest_point_on_segment(lat, lon, s_lat, s_lon, e_lat, e_lon);
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
                    .unwrap_or_else(|| {
                        noise_compute::propagation::geo::flat_dist(s_lat, s_lon, e_lat, e_lon)
                            as f32
                    }),
                rail_type: rtype.map(|a| a.value(i)).unwrap_or(0),
                usage: usage.map(|a| a.value(i)).unwrap_or(0),
                maxspeed: maxspd.as_ref().map(|a| a.value(i)).unwrap_or(0),
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

pub fn query_buildings_from_batches(
    batches: &[RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<BuildingResult> {
    let mut results = Vec::new();

    for batch in batches {
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id");
        let clat = col_f64(batch, "centroid_lat");
        let clon = col_f64(batch, "centroid_lon");

        let (Some(osm_id), Some(clat), Some(clon)) = (osm_id, clat, clon) else {
            continue;
        };

        let height = col_f32(batch, "height");
        let floors = col_u8(batch, "floors");
        let area = col_f32(batch, "area_m2");
        let btype = col_u8(batch, "building_type");
        let buse = col_u8(batch, "building_use");
        let name = col_str(batch, "name");
        let street = col_str(batch, "addr_street");
        let house = col_str(batch, "addr_housenumber");
        let wkb = col_binary(batch, "polygon_wkb");

        for i in 0..n {
            let c_lat = clat.value(i);
            let c_lon = clon.value(i);
            let dist = crate::geo::flat_dist(lat, lon, c_lat, c_lon);
            if dist > max_radius {
                continue;
            }

            results.push(BuildingResult {
                osm_id: osm_id.value(i),
                centroid_lat: c_lat,
                centroid_lon: c_lon,
                height: height.map(|a| a.value(i)).unwrap_or(0.0),
                floors: floors.map(|a| a.value(i)).unwrap_or(0),
                area_m2: area.map(|a| a.value(i)).unwrap_or(0.0),
                building_type: btype.map(|a| a.value(i)).unwrap_or(0),
                building_use: buse.map(|a| a.value(i)).unwrap_or(0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                addr_street: street.map(|a| a.value(i).to_string()).unwrap_or_default(),
                addr_housenumber: house.map(|a| a.value(i).to_string()).unwrap_or_default(),
                polygon_wkb: wkb.map(|a| hex_encode(a.value(i))).unwrap_or_default(),
                dist_m: dist,
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
}

pub fn query_leisure_from_batches(
    batches: &[RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<LeisureResult> {
    let mut results = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let (Some(osm_id), Some(clat), Some(clon)) = (
            col_i64(batch, "osm_id"),
            col_f64(batch, "centroid_lat"),
            col_f64(batch, "centroid_lon"),
        ) else {
            continue;
        };
        let sport = col_u8(batch, "sport");
        let area = col_f32(batch, "area_m2");
        let name = col_str(batch, "name");
        let wkb = col_binary(batch, "polygon_wkb");

        for i in 0..n {
            let c_lat = clat.value(i);
            let c_lon = clon.value(i);
            let dist = crate::geo::flat_dist(lat, lon, c_lat, c_lon);
            if dist > max_radius {
                continue;
            }
            results.push(LeisureResult {
                osm_id: osm_id.value(i),
                centroid_lat: c_lat,
                centroid_lon: c_lon,
                sport: sport.map(|a| a.value(i)).unwrap_or(0),
                area_m2: area.map(|a| a.value(i)).unwrap_or(0.0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                polygon_wkb: wkb.map(|a| hex_encode(a.value(i))).unwrap_or_default(),
                dist_m: dist,
            });
        }
    }
    results
}

/// One `barriers.arrow` row for the popup lane: the wall microsegment's
/// geometry (both endpoints — what `path_effects` intersects the ray with) plus
/// its midpoint, which is the point `dist_m` is measured to.
#[derive(Debug, serde::Serialize)]
pub struct BarrierResult {
    pub osm_id: i64,
    #[serde(skip_serializing)]
    pub segment_idx: i16,
    pub height: f32,
    /// Segment midpoint (`dist_m`'s reference point).
    pub lat: f64,
    pub lon: f64,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub dist_m: f64,
}

pub fn query_barriers_from_batches(
    batches: &[RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Result<Vec<BarrierResult>, String> {
    let mut results = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id")
            .ok_or_else(|| "barriers.arrow missing required osm_id column".to_string())?;
        let segment_idx = col_i16(batch, "segment_idx")
            .ok_or_else(|| "barriers.arrow missing required segment_idx column".to_string())?;
        let height = col_f32(batch, "height");
        let slat = col_f64(batch, "start_lat")
            .ok_or_else(|| "barriers.arrow missing required start_lat column".to_string())?;
        let slon = col_f64(batch, "start_lon")
            .ok_or_else(|| "barriers.arrow missing required start_lon column".to_string())?;
        let elat = col_f64(batch, "end_lat")
            .ok_or_else(|| "barriers.arrow missing required end_lat column".to_string())?;
        let elon = col_f64(batch, "end_lon")
            .ok_or_else(|| "barriers.arrow missing required end_lon column".to_string())?;

        for i in 0..n {
            SourceId64::wall(osm_id.value(i), segment_idx.value(i)).map_err(|error| {
                format!(
                    "invalid barriers.arrow provenience ({}, {}): {error:?}",
                    osm_id.value(i),
                    segment_idx.value(i)
                )
            })?;
            let mid_lat = (slat.value(i) + elat.value(i)) / 2.0;
            let mid_lon = (slon.value(i) + elon.value(i)) / 2.0;
            let dist = crate::geo::flat_dist(lat, lon, mid_lat, mid_lon);
            if dist > max_radius {
                continue;
            }

            results.push(BarrierResult {
                osm_id: osm_id.value(i),
                segment_idx: segment_idx.value(i),
                height: height.map(|a| a.value(i)).unwrap_or(3.0),
                lat: mid_lat,
                lon: mid_lon,
                start_lat: slat.value(i),
                start_lon: slon.value(i),
                end_lat: elat.value(i),
                end_lon: elon.value(i),
                dist_m: dist,
            });
        }
    }

    canonicalize_barrier_results(results)
}

/// Stable-dedupe exact repeated emissions and reject one ID naming two shapes.
pub fn canonicalize_barrier_results(
    results: Vec<BarrierResult>,
) -> Result<Vec<BarrierResult>, String> {
    let mut seen = std::collections::BTreeMap::new();
    let mut unique = Vec::with_capacity(results.len());
    for result in results {
        let source_id = SourceId64::wall(result.osm_id, result.segment_idx)
            .map_err(|error| format!("invalid barrier provenience: {error:?}"))?;
        let geometry_bits = [
            result.start_lat.to_bits(),
            result.start_lon.to_bits(),
            result.end_lat.to_bits(),
            result.end_lon.to_bits(),
            u64::from(result.height.to_bits()),
        ];
        match seen.entry(source_id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(geometry_bits);
                unique.push(result);
            }
            std::collections::btree_map::Entry::Occupied(entry)
                if *entry.get() == geometry_bits => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(format!(
                    "barrier provenience ({}, {}) names different geometry",
                    result.osm_id, result.segment_idx
                ));
            }
        }
    }
    Ok(unique)
}

pub fn col_i64<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int64Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_i32<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int32Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_i16<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Int16Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_f64<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Float64Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_f32<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a Float32Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u8<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt8Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u16<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt16Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_u32<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a UInt32Array> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_bool<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a BooleanArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}

/// Width-tolerant accessor for columns migrated u8 → u16 (rail `maxspeed`,
/// 2026-06: 300+ km/h overflowed u8). Old arrows keep UInt8 until the next
/// world OSM re-extract; this reader is the migration. Module-private —
/// a one-column shim, not part of the general `col_*` accessor surface.
enum ColU16OrU8<'a> {
    U16(&'a UInt16Array),
    U8(&'a UInt8Array),
}

impl ColU16OrU8<'_> {
    fn value(&self, i: usize) -> u16 {
        match self {
            Self::U16(a) => a.value(i),
            Self::U8(a) => a.value(i) as u16,
        }
    }
}

fn col_u16_or_u8<'a>(b: &'a RecordBatch, name: &str) -> Option<ColU16OrU8<'a>> {
    let col = b.column_by_name(name)?.as_any();
    if let Some(a) = col.downcast_ref::<UInt16Array>() {
        return Some(ColU16OrU8::U16(a));
    }
    col.downcast_ref::<UInt8Array>().map(ColU16OrU8::U8)
}
pub fn col_str<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}
pub fn col_binary<'a>(b: &'a RecordBatch, name: &str) -> Option<&'a BinaryArray> {
    b.column_by_name(name)?.as_any().downcast_ref()
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod barrier_provenance_tests {
    use super::*;
    use arrow::datatypes::{Field, Schema};

    fn batch(
        osm_ids: Vec<i64>,
        segment_indices: Vec<i16>,
        start_latitudes: Vec<f64>,
    ) -> RecordBatch {
        let rows = osm_ids.len();
        let end_latitudes: Vec<_> = start_latitudes.iter().map(|value| value + 0.001).collect();
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("osm_id", DataType::Int64, false),
                Field::new("segment_idx", DataType::Int16, false),
                Field::new("start_lat", DataType::Float64, false),
                Field::new("start_lon", DataType::Float64, false),
                Field::new("end_lat", DataType::Float64, false),
                Field::new("end_lon", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(osm_ids)),
                Arc::new(Int16Array::from(segment_indices)),
                Arc::new(Float64Array::from(start_latitudes)),
                Arc::new(Float64Array::from(vec![14.0; rows])),
                Arc::new(Float64Array::from(end_latitudes)),
                Arc::new(Float64Array::from(vec![14.001; rows])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn barrier_loaders_preserve_osm_id_and_segment_idx() {
        let results = query_barriers_from_batches(
            &[batch(vec![7, 7], vec![-3, 4], vec![50.0, 50.0])],
            50.0,
            14.0,
            1_000.0,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].segment_idx, -3);
        assert_eq!(results[1].segment_idx, 4);
    }

    #[test]
    fn barrier_authority_dedupes_identical_and_rejects_conflicting_provenience() {
        let identical = query_barriers_from_batches(
            &[batch(vec![7, 7], vec![-3, -3], vec![50.0, 50.0])],
            50.0,
            14.0,
            1_000.0,
        )
        .unwrap();
        assert_eq!(identical.len(), 1);

        let conflicting = query_barriers_from_batches(
            &[batch(vec![7, 7], vec![-3, -3], vec![50.0, 50.5])],
            50.0,
            14.0,
            100_000.0,
        )
        .unwrap_err();
        assert!(conflicting.contains("names different geometry"));
    }

    #[test]
    fn missing_barrier_segment_idx_fails_closed() {
        let mut missing = batch(vec![7], vec![-3], vec![50.0]);
        missing.remove_column(1);
        let error = query_barriers_from_batches(&[missing], 50.0, 14.0, 1_000.0).unwrap_err();
        assert!(error.contains("missing required segment_idx"));
    }
}

#[cfg(test)]
mod synth_load_tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::ipc::writer::FileWriter;
    use std::sync::Arc;

    /// Write a minimal synth_airport_lines.arrow file at `path`.
    /// Schema matches `aircraft-extract::arrow_schemas::synth_airport_lines_schema`
    /// for the column subset Step 1's runway-end resolver consults
    /// (start_lat / start_lon / end_lat / end_lon / aeroway_type +
    /// osm_id / segment_idx for grouping).
    fn write_synth(path: &Path) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("osm_id", DataType::UInt64, false),
            Field::new("segment_idx", DataType::UInt16, false),
            Field::new("airport_key", DataType::Utf8, false),
            Field::new("start_lat", DataType::Float64, false),
            Field::new("start_lon", DataType::Float64, false),
            Field::new("end_lat", DataType::Float64, false),
            Field::new("end_lon", DataType::Float64, false),
            Field::new("length_m", DataType::Float32, false),
            Field::new("heading_deg", DataType::Float32, false),
            Field::new("aeroway_type", DataType::UInt8, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let osm_id = UInt64Array::from(vec![1u64 << 63]);
        let seg_idx = UInt16Array::from(vec![0u16]);
        let key = StringArray::from(vec!["auto-x"]);
        let slat = Float64Array::from(vec![50.0_f64]);
        let slon = Float64Array::from(vec![14.0_f64]);
        let elat = Float64Array::from(vec![50.001_f64]);
        let elon = Float64Array::from(vec![14.0_f64]);
        let len = Float32Array::from(vec![100.0_f32]);
        let head = Float32Array::from(vec![0.0_f32]);
        let atype = UInt8Array::from(vec![7u8]);
        let name = StringArray::from(vec!["synth"]);
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(osm_id),
                Arc::new(seg_idx),
                Arc::new(key),
                Arc::new(slat),
                Arc::new(slon),
                Arc::new(elat),
                Arc::new(elon),
                Arc::new(len),
                Arc::new(head),
                Arc::new(atype),
                Arc::new(name),
            ],
        )
        .unwrap();
        let f = File::create(path).unwrap();
        let mut w = FileWriter::try_new(f, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    fn write_reference_roads(path: &Path, batch_rows: &[usize]) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("start_lat", DataType::Float64, false),
            Field::new("start_lon", DataType::Float64, false),
            Field::new("end_lat", DataType::Float64, false),
            Field::new("end_lon", DataType::Float64, false),
        ]));
        let f = File::create(path).unwrap();
        let mut w = FileWriter::try_new(f, &schema).unwrap();
        for &rows in batch_rows {
            let osm_ids = Int64Array::from_iter_values((0..rows).map(|row| row as i64));
            let coordinates = || Float64Array::from_iter_values((0..rows).map(|row| row as f64));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(osm_ids),
                    Arc::new(coordinates()),
                    Arc::new(coordinates()),
                    Arc::new(coordinates()),
                    Arc::new(coordinates()),
                ],
            )
            .unwrap();
            w.write(&batch).unwrap();
        }
        w.finish().unwrap();
    }

    fn write_wrong_schema_roads(path: &Path) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("start_lat", DataType::Float32, false),
            Field::new("start_lon", DataType::Float64, false),
            Field::new("end_lat", DataType::Float64, false),
            Field::new("end_lon", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Float32Array::from(vec![50.0])),
                Arc::new(Float64Array::from(vec![14.0])),
                Arc::new(Float64Array::from(vec![50.1])),
                Arc::new(Float64Array::from(vec![14.1])),
            ],
        )
        .unwrap();
        let f = File::create(path).unwrap();
        let mut w = FileWriter::try_new(f, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn load_hex_reads_synth_airport_lines_when_present() {
        let dir = tempfile::tempdir().unwrap();
        write_synth(&dir.path().join("synth_airport_lines.arrow"));
        let hex = load_hex(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(hex.synth_airport_lines_batches.len(), 1);
        assert_eq!(hex.synth_airport_lines_batches[0].num_rows(), 1);
    }

    #[test]
    fn load_hex_returns_empty_synth_batches_when_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let hex = load_hex(dir.path().to_str().unwrap()).unwrap();
        assert!(hex.synth_airport_lines_batches.is_empty());
    }

    #[test]
    fn validate_reference_roads_strictly_reads_all_batches_without_cache() {
        let root = tempfile::tempdir().unwrap();
        let hex_id = "841e309ffffffff";
        let hex_dir = root.path().join(hex_id);
        std::fs::create_dir(&hex_dir).unwrap();
        write_reference_roads(&hex_dir.join("roads.arrow"), &[1, 1]);

        assert_eq!(validate_reference_roads(root.path(), hex_id).unwrap(), 2);
    }

    #[test]
    fn validate_reference_roads_rejects_corrupt_or_empty_arrow() {
        let root = tempfile::tempdir().unwrap();
        let hex_id = "841e309ffffffff";
        let hex_dir = root.path().join(hex_id);
        std::fs::create_dir(&hex_dir).unwrap();
        let roads = hex_dir.join("roads.arrow");

        std::fs::write(&roads, b"not an Arrow IPC file").unwrap();
        assert!(validate_reference_roads(root.path(), hex_id)
            .unwrap_err()
            .contains("failed to read"));

        write_reference_roads(&roads, &[0]);
        assert!(validate_reference_roads(root.path(), hex_id)
            .unwrap_err()
            .contains("contains no road rows"));
    }

    #[test]
    fn validate_reference_roads_rejects_wrong_query_schema() {
        let root = tempfile::tempdir().unwrap();
        let hex_id = "841e309ffffffff";
        let hex_dir = root.path().join(hex_id);
        std::fs::create_dir(&hex_dir).unwrap();
        let roads = hex_dir.join("roads.arrow");
        write_wrong_schema_roads(&roads);

        let error = validate_reference_roads(root.path(), hex_id).unwrap_err();
        assert!(error.contains("start_lat"), "unexpected error: {error}");
        assert!(error.contains("Float64"), "unexpected error: {error}");
    }

    #[test]
    fn validate_reference_roads_rejects_path_traversal() {
        let root = tempfile::tempdir().unwrap();
        assert!(validate_reference_roads(root.path(), "../roads.arrow")
            .unwrap_err()
            .contains("invalid H3 reference cell"));
    }

    /// `col_u16_or_u8` must read both the new UInt16 rail `maxspeed`
    /// column and legacy UInt8 arrows (pre-2026-06 extracts) — the
    /// tolerant reader IS the schema migration.
    #[test]
    fn col_u16_or_u8_reads_both_widths() {
        let one_col = |field: Field, arr: ArrayRef| {
            RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![arr]).unwrap()
        };
        let new = one_col(
            Field::new("maxspeed", DataType::UInt16, false),
            Arc::new(UInt16Array::from(vec![300u16])),
        );
        let legacy = one_col(
            Field::new("maxspeed", DataType::UInt8, false),
            Arc::new(UInt8Array::from(vec![120u8])),
        );
        assert_eq!(col_u16_or_u8(&new, "maxspeed").unwrap().value(0), 300);
        assert_eq!(col_u16_or_u8(&legacy, "maxspeed").unwrap().value(0), 120);
        assert!(col_u16_or_u8(&new, "missing").is_none());
    }
}

/// M4/M5 read-side gates: the baked M3 triplet (`country_iso`/`city_id`/
/// `continent`) surfaces as the row's own admin on the query results; an
/// absent column yields `None` (receiver fallback in the kernel).
#[cfg(test)]
mod baked_admin_tests {
    use super::*;
    use arrow::datatypes::{Field, Schema};
    use noise_compute::admin::{Admin, Continent};
    use std::sync::Arc;

    const TH: Admin = Admin {
        continent: Continent::Asia,
        country_iso: *b"TH",
        city_id: 0,
    };
    const CZ: Admin = Admin {
        continent: Continent::Europe,
        country_iso: *b"CZ",
        city_id: 0,
    };

    fn append_triplet(
        mut cols: Vec<(&'static str, ArrayRef)>,
        triplet: Option<(u16, u16, u8)>,
    ) -> RecordBatch {
        if let Some((iso, city, cont)) = triplet {
            cols.push(("country_iso", Arc::new(UInt16Array::from(vec![iso]))));
            cols.push(("city_id", Arc::new(UInt16Array::from(vec![city]))));
            cols.push(("continent", Arc::new(UInt8Array::from(vec![cont]))));
        }
        let fields: Vec<Field> = cols
            .iter()
            .map(|(n, a)| Field::new(*n, a.data_type().clone(), false))
            .collect();
        let arrs: Vec<ArrayRef> = cols.into_iter().map(|(_, a)| a).collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrs).unwrap()
    }

    fn road_batch(triplet: Option<(u16, u16, u8)>) -> RecordBatch {
        append_triplet(
            vec![
                ("osm_id", Arc::new(Int64Array::from(vec![1i64]))),
                ("start_lat", Arc::new(Float64Array::from(vec![50.0]))),
                ("start_lon", Arc::new(Float64Array::from(vec![14.0]))),
                ("end_lat", Arc::new(Float64Array::from(vec![50.0]))),
                ("end_lon", Arc::new(Float64Array::from(vec![14.002]))),
                ("road_class", Arc::new(UInt8Array::from(vec![3u8]))),
                ("speed_limit", Arc::new(UInt8Array::from(vec![50u8]))),
            ],
            triplet,
        )
    }

    fn rail_batch(triplet: Option<(u16, u16, u8)>) -> RecordBatch {
        append_triplet(
            vec![
                ("osm_id", Arc::new(Int64Array::from(vec![1i64]))),
                ("start_lat", Arc::new(Float64Array::from(vec![50.0]))),
                ("start_lon", Arc::new(Float64Array::from(vec![14.0]))),
                ("end_lat", Arc::new(Float64Array::from(vec![50.0]))),
                ("end_lon", Arc::new(Float64Array::from(vec![14.002]))),
                ("rail_type", Arc::new(UInt8Array::from(vec![0u8]))),
                ("maxspeed", Arc::new(UInt16Array::from(vec![120u16]))),
                ("trains_passenger", Arc::new(Int32Array::from(vec![100i32]))),
                ("trains_freight", Arc::new(Int32Array::from(vec![40i32]))),
            ],
            triplet,
        )
    }

    #[test]
    fn road_row_carries_baked_admin_or_none_when_absent() {
        let baked = query_roads_from_batches(
            &[road_batch(Some((
                u16::from_le_bytes(*b"TH"),
                0,
                Continent::Asia as u8,
            )))],
            50.0,
            14.001,
            10_000.0,
        );
        assert_eq!(baked.len(), 1);
        assert_eq!(baked[0].admin, Some(TH), "baked TH row → its own admin");

        let plain = query_roads_from_batches(&[road_batch(None)], 50.0, 14.001, 10_000.0);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].admin, None, "no triplet → receiver fallback");

        let zero = query_roads_from_batches(&[road_batch(Some((0, 0, 0)))], 50.0, 14.001, 10_000.0);
        assert_eq!(
            zero[0].admin,
            Some(Admin::UNKNOWN),
            "present \\0\\0 → UNKNOWN, never a receiver fallback"
        );
    }

    #[test]
    fn rail_row_carries_baked_admin_or_none_when_absent() {
        let baked = query_railways_from_batches(
            &[rail_batch(Some((
                u16::from_le_bytes(*b"CZ"),
                0,
                Continent::Europe as u8,
            )))],
            50.0,
            14.001,
            10_000.0,
        );
        assert_eq!(baked.len(), 1);
        assert_eq!(baked[0].admin, Some(CZ));

        let plain = query_railways_from_batches(&[rail_batch(None)], 50.0, 14.001, 10_000.0);
        assert_eq!(plain[0].admin, None);
    }

    /// The admin is an engine side-channel only — the popup wire JSON
    /// (`query_roads`) must stay byte-shaped as before.
    #[test]
    fn admin_field_never_serializes() {
        let r = query_roads_from_batches(
            &[road_batch(Some((
                u16::from_le_bytes(*b"TH"),
                0,
                Continent::Asia as u8,
            )))],
            50.0,
            14.001,
            10_000.0,
        )
        .into_iter()
        .next()
        .unwrap();
        let v = serde_json::to_value(&r).unwrap();
        assert!(
            v.get("admin").is_none(),
            "wire JSON must not grow an admin key: {v}"
        );
    }
}

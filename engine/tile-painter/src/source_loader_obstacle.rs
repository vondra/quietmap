//! Load the vector obstacle store for a region (geodata-v2 1.5) — the
//! pipeline twin of `source-reader::obstacle_store`; keep loading policy in
//! lockstep (the barrier loader ↔ popup precedent).
//!
//! Reads per-H3R4-cell `obstacles*.arrow` shards (Overture footprints +
//! heights, `scripts/obstacles/ingest-overture-obstacles.py`) into per-cell
//! [`ObstacleIndex`]es (origin = cell centre) shared across the region's
//! tile batches as an [`ObstacleSet`]. Roots per cell, first hit wins: the
//! PROMOTED tree (`h3r4_dir/<cell>/`, post-Wave-1) then the enrichment
//! staging tree; `QM_OBSTACLES_DIR` overrides (tests).
//!
//! ALL-OR-ERROR: a missing ring cell fails the whole region because a partial
//! index would silently omit buildings where coverage is absent;
//! `QM_OBSTACLES_ALLOW_PARTIAL=1` admits missing halo NEIGHBOURS at staging
//! frontiers for dev A/B, but never the region's own cell (popup's
//! query-cell rule). A shard READ/PARSE error — including a failed
//! directory listing — is a hard `Err` that fails the region build: a
//! pipeline must never silently paint with different physics than requested
//! (the popup has its own visitor-facing loading policy).
//! EXCEPTION (ingested-empty proof): a shard-less cell whose every
//! overlapped 1-degree tile is listed in the world ingest manifest
//! (`.ingested-tiles`, see `obstacle_ingest_coverage`) was provably swept
//! by the current Overture ingest and contributed zero footprints — it is
//! EMPTY, not missing, and vector mode proceeds
//! without it.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use arrow::array::{Array, BinaryArray, Float32Array, Float64Array, UInt8Array};
use arrow::ipc::reader::FileReader;
use h3o::{CellIndex, LatLng};
use noise_compute::envelope::{effective_envelope_class, EnvelopeClass};
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use noise_compute::wkb;

/// A region's vector obstacles — the only building geometry there is.
pub struct ObstacleData {
    set: Arc<ObstacleSet>,
}

impl ObstacleData {
    /// The region set. Always present: a region that could not load its
    /// obstacles never gets this far.
    pub fn set(&self) -> &ObstacleSet {
        &self.set
    }

    /// Retain the region geometry beyond this loader value's lifetime.
    pub fn shared_set(&self) -> Arc<ObstacleSet> {
        Arc::clone(&self.set)
    }

    /// Load per-cell indexes for the region's ring.
    ///
    /// Every failure is an `Err`. There is no second building representation to
    /// fall back to, so "we could not read the obstacles" can only mean "do not
    /// paint this region" — painting it anyway would publish a quiet map of a
    /// loud place, and nothing downstream could tell the difference.
    ///
    /// `region_r4` is the cell being PAINTED: even under
    /// `QM_OBSTACLES_ALLOW_PARTIAL=1` it must be ingested — partial mode
    /// admits a missing halo NEIGHBOUR at a staging frontier, never a missing
    /// centre. Same rule as the popup store's query-cell requirement.
    pub fn load_for_r4s(h3r4_dir: &Path, region_r4: u64, r4_hexes: &[u64]) -> Result<Self> {
        let allow_partial = std::env::var("QM_OBSTACLES_ALLOW_PARTIAL").is_ok_and(|v| v == "1");
        if renderer_evidence_requires_vector_mode() && allow_partial {
            bail!("renderer evidence forbids QM_OBSTACLES_ALLOW_PARTIAL");
        }
        if renderer_evidence_requires_vector_mode()
            && std::env::var_os("QM_OBSTACLES_DIR").is_some()
        {
            bail!("renderer evidence requires canonical promoted obstacles, not QM_OBSTACLES_DIR");
        }
        let manifest = ingest_manifest(h3r4_dir);
        let mut indexes = Vec::new();
        for &r4 in r4_hexes {
            let cell = CellIndex::try_from(r4).context("invalid r4 hex")?;
            let Some(dir) = cell_dir(h3r4_dir, cell)? else {
                if allow_partial && r4 != region_r4 {
                    // Never silent: this is the one path that paints with
                    // KNOWN-incomplete geometry, so it must leave a trace an
                    // operator can find in a log they already read.
                    eprintln!(
                        "[obstacles] QM_OBSTACLES_ALLOW_PARTIAL: painting {region_r4:015x} \
                         WITHOUT halo cell {cell} — incomplete screening, dev A/B only"
                    );
                    continue;
                }
                if manifest.is_some_and(|m| m.covers_cell(cell)) {
                    // INGESTED-EMPTY, proven by the world ingest manifest: the
                    // sweep covered this cell and it contributed zero
                    // footprints. Empty is an answer; missing is not, and this
                    // manifest is the only thing that tells them apart.
                    continue;
                }
                bail!(
                    "[obstacles] {} cell {cell} has no shard and the ingest manifest does not \
                     prove it empty — buildings are vector-only, so this region cannot be \
                     painted (QM_OBSTACLES_ALLOW_PARTIAL=1 admits missing halo neighbours \
                     for dev A/B)",
                    if r4 == region_r4 { "REGION" } else { "ring" },
                );
            };
            let low_profile = load_low_profile(h3r4_dir, cell)?;
            indexes.push(Arc::new(build_cell_index(cell, &dir, &low_profile)?));
        }
        let set = ObstacleSet { indexes };
        if set.edge_count() == 0 {
            if renderer_evidence_requires_vector_mode() {
                bail!("renderer evidence requires positive vector mode; obstacle ring is empty");
            }
            // Zero edges is a legitimate answer: ocean, desert, and any cell
            // whose staged shard indexed no footprint. A shard that exists HAS
            // answered, and calling its emptiness a fault would black out whole
            // countries the day an Overture release rejects their heights.
            eprintln!(
                "[obstacles] vector mode: 0 edges across {} cells",
                r4_hexes.len()
            );
        } else {
            eprintln!(
                "[obstacles] vector mode: {} edges across {} cells",
                set.edge_count(),
                set.indexes.len()
            );
        }
        Ok(ObstacleData { set: Arc::new(set) })
    }
}

impl ObstacleData {
    /// The tile's interior estimate, baked from this region's footprints.
    pub fn interior_estimate(
        &self,
        tile: &raster_reader::fused_tile_z13::FusedTileZ13,
    ) -> InteriorEstimate {
        InteriorEstimate::bake(tile, self.set())
    }
}

fn renderer_evidence_requires_vector_mode() -> bool {
    std::env::var(crate::renderer_evidence::RENDERER_EVIDENCE_FLAG).as_deref() == Ok("1")
}

/// Pre-bake one tile's `rx_refl_db` from the 150 × 150 m nine-probe vector
/// enclosure (`noise_compute::…::enclosure_db`; SPEC §4.9). The single bake shared by
/// the CPU builder, the GPU runner, and the independent parity fixture (gg review 2026-07-28:
/// three hand-copies drift).
pub fn bake_tile_vector_rx_refl(
    tile: &mut raster_reader::fused_tile_z13::FusedTileZ13,
    set: &ObstacleSet,
) {
    use noise_compute::constants::ENCLOSURE_RADIUS_M;
    use noise_compute::propagation::obstacle_index::enclosure_db;
    use raster_reader::fused_tile_z13::TILE_PX;
    for py in 0..TILE_PX {
        let lat = tile.rx_lat[py];
        for px in 0..TILE_PX {
            tile.rx_refl_db[py * TILE_PX + px] =
                enclosure_db(set, lat, tile.rx_lon[px], ENCLOSURE_RADIUS_M) as f32;
        }
    }
}

/// Minimum footprint height whose interior masks a receiver. `0.0` means
/// EVERY indexed footprint counts — the index builder already drops
/// `height_m <= 0`, so this reads exactly as "inside any obstacle polygon".
/// Deliberately NOT the enclosure probe's 5 m gate: that one answers a
/// different question (does this receiver stand in a built-up canyon), while
/// this one answers "is this point indoors", which a 3 m garage also is.
const INTERIOR_MASK_MIN_HEIGHT_M: f32 = 0.0;

/// Classify the receiver lattice once per tile. The result is an effective
/// class raster, not a boolean mask: OUTDOOR footprints remain ordinary
/// receivers and every enclosed class carries the ΔL `InteriorEstimate::apply` uses. The
/// source `envelope_class` in Arrow remains unchanged; the height-aware
/// effective class exists only in this paint-time raster.
///
/// WHY classify at all: END / CNOSSOS strategic mapping puts receivers on
/// FACADES, never indoors. A receiver inside a footprint must therefore use a
/// façade donor and an explicit display estimate, not its self-screened value.
///
/// Vector-only by construction: every painted region has an [`ObstacleSet`].
/// A coarse raster could not answer "inside this footprint" and would blank
/// an entire cell around every house.
///
/// A footprint smaller than one pixel (~12 m at the base zoom) rarely covers
/// a pixel centre and so rarely masks anything. Accepted, no special
/// handling: the mask is a display-semantics correction, not physics.
fn bake_tile_envelope_classes(
    tile: &raster_reader::fused_tile_z13::FusedTileZ13,
    set: &ObstacleSet,
) -> Vec<u8> {
    use raster_reader::fused_tile_z13::TILE_PX;
    let mut classes =
        vec![noise_compute::envelope::EnvelopeClass::Outdoor as u8; TILE_PX * TILE_PX];
    // One scratch vec for the whole tile — `contains_built` clears it per
    // probe (same reuse the 9-probe `enclosure_db` does).
    let mut seen: Vec<(u32, u32, f32)> = Vec::new();
    for py in 0..TILE_PX {
        let lat = tile.rx_lat[py];
        for px in 0..TILE_PX {
            let lon = tile.rx_lon[px];
            let winner = set
                .indexes
                .iter()
                .enumerate()
                .filter_map(|(index_ordinal, index)| {
                    index
                        .containing_enclosed(lat, lon, INTERIOR_MASK_MIN_HEIGHT_M, &mut seen)
                        .map(|(class, height, footprint_ordinal)| {
                            (class, height, index_ordinal, footprint_ordinal)
                        })
                })
                .max_by(|a, b| {
                    a.1.total_cmp(&b.1)
                        .then_with(|| b.2.cmp(&a.2))
                        .then_with(|| b.3.cmp(&a.3))
                });
            if let Some((class, height, _, _)) = winner {
                classes[py * TILE_PX + px] = effective_envelope_class(class, height) as u8;
            }
        }
    }
    classes
}

/// One tile's building-interior display estimate: the effective envelope
/// class of every receiver pixel plus, for every enclosed pixel, its façade
/// donor — the nearest OUTDOOR lattice pixel of the SAME tile (exact integer
/// EDT, [`nearest_site_offsets`]). Baked once per tile and applied to every
/// layer of that tile by all three writers (the aircraft runner bakes the
/// identical map from the identical lattice), so energy composition commutes
/// with the envelope loss.
///
/// WHY per tile and not a 3×3 tile halo: a halo donor needs the neighbour
/// tiles' painted bytes, i.e. the full propagation kernel on a ring of tiles
/// the painter never writes (measured cost and the seam consequence: SPEC
/// "Donor transform (per tile)"). A tile's bytes depend only on that tile's
/// own inputs again.
pub struct InteriorEstimate {
    classes: Vec<u8>,
    /// Donor pixel ordinal per receiver pixel; [`NO_DONOR`] outside enclosed
    /// footprints and for an enclosed pixel of a tile without any outdoor
    /// pixel (then `apply` leaves `NO_DATA`).
    donors: Vec<u32>,
}

impl InteriorEstimate {
    /// Classify the tile's lattice against the region's footprints and bake
    /// the donor map (vector regions only — see
    /// [`ObstacleData::interior_estimate`]).
    pub fn bake(tile: &raster_reader::fused_tile_z13::FusedTileZ13, set: &ObstacleSet) -> Self {
        Self::from_classes(bake_tile_envelope_classes(tile, set))
    }

    /// The donor map for an effective-class raster: sites are outdoor pixels,
    /// queries are enclosed pixels. Exact FH transform, integer tie-break
    /// (smaller site x, then smaller site y) — SPEC "Donor transform".
    pub fn from_classes(classes: Vec<u8>) -> Self {
        use raster_reader::fused_tile_z13::TILE_PX;
        assert_eq!(classes.len(), INTERIOR_TILE_CELLS);
        let donors = nearest_site_offsets(
            TILE_PX,
            0..TILE_PX,
            0..TILE_PX,
            |x, y| EnvelopeClass::from_u8(classes[y * TILE_PX + x]) == EnvelopeClass::Outdoor,
            |x, y| {
                EnvelopeClass::from_u8(classes[y * TILE_PX + x])
                    .delta_db()
                    .is_some()
            },
        );
        Self { classes, donors }
    }

    /// Effective envelope class per receiver pixel (row-major, `TILE_PX²`).
    pub fn classes(&self) -> &[u8] {
        &self.classes
    }

    /// Donor pixel ordinal per receiver pixel ([`NO_DONOR`] outside enclosed
    /// footprints, or enclosed without any outdoor pixel in the tile).
    pub fn donors(&self) -> &[u32] {
        &self.donors
    }

    /// Resident bytes, for the GPU lane's pipeline byte gate.
    pub fn heap_bytes(&self) -> u64 {
        (self.classes.capacity() + self.donors.capacity() * std::mem::size_of::<u32>()) as u64
    }

    /// Rewrite every enclosed pixel of one layer's collapsed tile with
    /// `max(0, L_facade − ΔL)`; a missing or `NO_DATA` donor stays `NO_DATA`.
    /// In place is safe: a donor is always an OUTDOOR pixel, and outdoor
    /// pixels are never rewritten here, so `cells[donor]` is still the
    /// collapsed façade value whenever it is read.
    pub fn apply(&self, cells: &mut [u8]) {
        use crate::wire_hm3::{dequantise_lden, quantise_lden, NO_DATA};
        assert_eq!(cells.len(), self.classes.len());
        for (index, class) in self.classes.iter().enumerate() {
            let Some(delta) = EnvelopeClass::from_u8(*class).delta_db() else {
                continue;
            };
            let donor = self.donors[index];
            let facade = if donor == NO_DONOR {
                f64::NEG_INFINITY
            } else {
                dequantise_lden(cells[donor as usize])
            };
            let indoor = noise_compute::envelope::indoor_level_db(facade, delta);
            cells[index] = if indoor.is_finite() {
                quantise_lden(indoor)
            } else {
                NO_DATA
            };
        }
    }
}

const INTERIOR_TILE_CELLS: usize =
    raster_reader::fused_tile_z13::TILE_PX * raster_reader::fused_tile_z13::TILE_PX;

/// "No site reachable" marker in [`nearest_site_offsets`] output: `side ≤
/// u16::MAX` keeps every real ordinal below it.
pub const NO_DONOR: u32 = u32::MAX;

/// `INF` is larger than every squared distance in a `TILE_PX²` receiver
/// window. The EDT deliberately stays in signed integer space: no floating
/// rounding is allowed to decide which façade wins a tie.
const EDT_INF: i32 = i32::MAX / 4;

/// Exact Felzenszwalb–Huttenlocher nearest-site transform for a rectangular
/// query window. The only O(N) scratch is one squared-distance grid, one
/// nearest-site-y grid, and the two lower-envelope arrays. The closure is
/// queried directly instead of materialising an outdoor-point list. Returns,
/// per query-window pixel (row-major), the nearest site's ordinal
/// `y * side + x` in the `side` frame, or [`NO_DONOR`]. Public so census
/// tooling can replay other window shapes against the same exact transform
/// and tie-break.
pub fn nearest_site_offsets(
    side: usize,
    query_x: std::ops::Range<usize>,
    query_y: std::ops::Range<usize>,
    is_site: impl Fn(usize, usize) -> bool,
    is_query: impl Fn(usize, usize) -> bool,
) -> Vec<u32> {
    assert!(side > 0 && side <= u16::MAX as usize);
    assert!(query_x.end <= side && query_y.end <= side);
    let query_width = query_x.end - query_x.start;
    let mut g = vec![EDT_INF; side * side];
    let mut sy = vec![u16::MAX; side * side];

    // Column pass: two sweeps give the exact 1D transform. The backward
    // sweep replaces only a strictly shorter candidate, so a vertical tie
    // keeps the smaller site y as required by the amended tie-break.
    for x in 0..side {
        let mut nearest_y: Option<usize> = None;
        for y in 0..side {
            if is_site(x, y) {
                nearest_y = Some(y);
            }
            if let Some(site_y) = nearest_y {
                let distance = y - site_y;
                let index = y * side + x;
                g[index] = (distance * distance) as i32;
                sy[index] = site_y as u16;
            }
        }
        nearest_y = None;
        for y in (0..side).rev() {
            if is_site(x, y) {
                nearest_y = Some(y);
            }
            if let Some(site_y) = nearest_y {
                let distance = site_y - y;
                let index = y * side + x;
                let candidate = (distance * distance) as i32;
                if candidate < g[index] {
                    g[index] = candidate;
                    sy[index] = site_y as u16;
                }
            }
        }
    }

    let mut envelope_vertices = vec![0i32; side];
    let mut envelope_starts = vec![0i32; side];
    let mut donors = vec![NO_DONOR; query_width * (query_y.end - query_y.start)];

    // Row pass: the lower envelope of the column parabolas. `div_euclid`
    // implements floor division for negative numerators; +1 is the first
    // integer where the right parabola is strictly closer. Querying with
    // `<=` keeps the left (smaller x) site at an exact tie.
    for y in 0..side {
        let mut envelope_len = 0usize;
        for q in 0..side {
            if g[y * side + q] >= EDT_INF {
                continue;
            }
            if envelope_len == 0 {
                envelope_vertices[0] = q as i32;
                envelope_starts[0] = i32::MIN;
                envelope_len = 1;
                continue;
            }
            let separator = |p: usize, q: usize| -> i32 {
                let numerator = (q as i64) * (q as i64) - (p as i64) * (p as i64)
                    + i64::from(g[y * side + q])
                    - i64::from(g[y * side + p]);
                let denominator = 2 * (q as i64 - p as i64);
                (numerator.div_euclid(denominator) + 1) as i32
            };
            let mut start = separator(envelope_vertices[envelope_len - 1] as usize, q);
            while envelope_len > 0 && start <= envelope_starts[envelope_len - 1] {
                envelope_len -= 1;
                if envelope_len > 0 {
                    start = separator(envelope_vertices[envelope_len - 1] as usize, q);
                }
            }
            if envelope_len == 0 {
                envelope_vertices[0] = q as i32;
                envelope_starts[0] = i32::MIN;
                envelope_len = 1;
            } else {
                envelope_vertices[envelope_len] = q as i32;
                envelope_starts[envelope_len] = start;
                envelope_len += 1;
            }
        }
        if envelope_len == 0 {
            continue;
        }
        let mut envelope_index = 0usize;
        for x in 0..side {
            while envelope_index + 1 < envelope_len
                && envelope_starts[envelope_index + 1] <= x as i32
            {
                envelope_index += 1;
            }
            if !(query_x.start..query_x.end).contains(&x)
                || !(query_y.start..query_y.end).contains(&y)
                || !is_query(x, y)
            {
                continue;
            }
            let site_x = envelope_vertices[envelope_index] as usize;
            let site_y = sy[y * side + site_x];
            if site_y == u16::MAX {
                continue;
            }
            let output_index = (y - query_y.start) * query_width + (x - query_x.start);
            donors[output_index] = (site_y as usize * side + site_x) as u32;
        }
    }
    donors
}

/// Read this cell's `buildings.arrow` into the low-profile cap's lookup — the
/// Arrow half of [`noise_compute::low_profile`], which carries the rule itself
/// (shared with the popup's loader, which reads the same file differently).
/// A missing file, or an older schema without the four columns, means no
/// capping — never a hard error for a correction layer.
fn load_low_profile(h3r4_dir: &Path, cell: CellIndex) -> Result<LowProfileLookup> {
    let path = h3r4_dir.join(cell.to_string()).join("buildings.arrow");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LowProfileLookup::default())
        }
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let reader = FileReader::try_new(Cursor::new(bytes), None)
        .with_context(|| format!("arrow open {}", path.display()))?;
    let mut lookup = LowProfileLookup::default();
    for batch in reader {
        let batch = batch.with_context(|| format!("arrow batch {}", path.display()))?;
        let (Some(lats), Some(lons), Some(types), Some(areas)) = (
            batch
                .column_by_name("centroid_lat")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>()),
            batch
                .column_by_name("centroid_lon")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>()),
            batch
                .column_by_name("building_type")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>()),
            batch
                .column_by_name("area_m2")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>()),
        ) else {
            return Ok(LowProfileLookup::default());
        };
        for i in 0..batch.num_rows() {
            if lats.is_null(i) || lons.is_null(i) || types.is_null(i) || areas.is_null(i) {
                continue;
            }
            lookup.insert_if_low(types.value(i), lats.value(i), lons.value(i), areas.value(i));
        }
    }
    Ok(lookup)
}

fn staging_root(h3r4_dir: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("QM_OBSTACLES_DIR") {
        return PathBuf::from(dir);
    }
    // h3r4_dir = <root>/data/prepared/{year}/h3r4 → <root>/data/enrichment/…
    h3r4_dir
        .ancestors()
        .nth(3)
        .map(|d| d.join("enrichment/global/overture-obstacles/h3r4"))
        .unwrap_or_else(|| PathBuf::from("data/enrichment/global/overture-obstacles/h3r4"))
}

/// World ingest proof for shard-less cells. `QM_OBSTACLES_DIR` keeps the
/// manifest next to that override (tests); otherwise walk he84 / NAS layouts.
fn ingest_manifest(
    h3r4_dir: &Path,
) -> Option<&'static noise_compute::propagation::obstacle_ingest_coverage::IngestManifest> {
    if std::env::var("QM_OBSTACLES_DIR").is_ok() {
        return noise_compute::propagation::obstacle_ingest_coverage::IngestManifest::load_cached(
            &staging_root(h3r4_dir)
                .parent()
                .map(|p| p.join(".ingested-tiles"))
                .unwrap_or_else(|| PathBuf::from(".ingested-tiles")),
        );
    }
    noise_compute::propagation::obstacle_ingest_coverage::load_for_h3r4(h3r4_dir)
}

fn cell_dir(h3r4_dir: &Path, cell: CellIndex) -> Result<Option<PathBuf>> {
    if std::env::var("QM_OBSTACLES_DIR").is_err() {
        let promoted = h3r4_dir.join(cell.to_string());
        if !shard_paths(&promoted)?.is_empty() {
            return Ok(Some(promoted));
        }
    }
    let staged = staging_root(h3r4_dir).join(cell.to_string());
    Ok((!shard_paths(&staged)?.is_empty()).then_some(staged))
}

/// Sorted shard listing — deterministic obstacle ordinals per on-disk state.
/// A missing directory is a legitimate "not ingested" (`Ok(empty)`); any
/// OTHER I/O failure is a hard error — a permission or disk fault must not
/// read as "cell missing" and silently activate an incomplete index
/// (gg review 2026-07-28).
fn shard_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read_dir {}", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("read_dir entry in {}", dir.display()))?;
        let p = entry.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("obstacles") && n.ends_with(".arrow"))
        {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

/// Outer-ring area (m², shoelace on a local equirectangular projection) —
/// the low-profile cap's footprint-comparability check. Uses the SAME parser
/// the index builder consumes, so the two can never disagree on geometry.
fn build_cell_index(
    cell: CellIndex,
    dir: &Path,
    low_profile: &LowProfileLookup,
) -> Result<ObstacleIndex> {
    let centre = LatLng::from(cell);
    let mut builder = ObstacleIndex::builder(centre.lat(), centre.lng());
    let mut next_id: u32 = 0;
    let mut capped = 0usize;
    let shards = shard_paths(dir)?;
    if shards.is_empty() {
        bail!("shard dir emptied under us: {}", dir.display());
    }
    for path in shards {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let reader = FileReader::try_new(Cursor::new(bytes), None)
            .with_context(|| format!("arrow open {}", path.display()))?;
        for batch in reader {
            let batch = batch.with_context(|| format!("arrow batch {}", path.display()))?;
            let (Some(wkb), Some(heights)) = (
                batch
                    .column_by_name("polygon_wkb")
                    .and_then(|c| c.as_any().downcast_ref::<BinaryArray>()),
                batch
                    .column_by_name("height_m")
                    .and_then(|c| c.as_any().downcast_ref::<Float32Array>()),
            ) else {
                bail!("{}: missing polygon_wkb/height_m", path.display());
            };
            // Older staging shards lack tier/centroid — then nothing is
            // capped (tier unknowable), matching pre-fix behavior.
            let tiers = batch
                .column_by_name("height_tier")
                .and_then(|c| c.as_any().downcast_ref::<UInt8Array>());
            let clats = batch
                .column_by_name("centroid_lat")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
            let clons = batch
                .column_by_name("centroid_lon")
                .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
            for i in 0..batch.num_rows() {
                if wkb.is_null(i) || heights.is_null(i) {
                    bail!("{}: null row {i}", path.display());
                }
                let mut height = heights.value(i);
                if let (Some(tiers), Some(clats), Some(clons)) = (tiers, clats, clons) {
                    if !tiers.is_null(i) && !clats.is_null(i) && !clons.is_null(i) {
                        let capped_h = low_profile.capped_height(
                            height,
                            tiers.value(i),
                            clats.value(i),
                            clons.value(i),
                            wkb::outer_ring_area_m2(wkb.value(i)),
                        );
                        if capped_h < height {
                            capped += 1;
                        }
                        height = capped_h;
                    }
                }
                let class = batch
                    .column_by_name("envelope_class")
                    .and_then(|c| c.as_any().downcast_ref::<UInt8Array>())
                    .filter(|a| !a.is_null(i))
                    .map(|a| EnvelopeClass::from_u8(a.value(i)))
                    .unwrap_or(EnvelopeClass::Default);
                builder.add_polygon_wkb(
                    wkb.value(i),
                    height,
                    ObstacleKind::Building,
                    next_id,
                    class,
                );
                next_id = next_id.wrapping_add(1);
            }
        }
    }
    if capped > 0 {
        eprintln!("[obstacles] {cell}: {capped} defaulted heights capped to low-profile 3 m");
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Float32Array;
    use h3o::{LatLng, Resolution};

    /// One tiny valid shard: a single closed ~20 m square footprint at
    /// (lat, lon), WKB little-endian Polygon, 1 ring × 5 points.
    fn write_shard(dir: &Path, lat: f64, lon: f64) {
        std::fs::create_dir_all(dir).unwrap();
        let schema = arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("polygon_wkb", arrow::datatypes::DataType::Binary, false),
            arrow::datatypes::Field::new("height_m", arrow::datatypes::DataType::Float32, false),
        ]);
        let mut wkb: Vec<u8> = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
        for (dlon, dlat) in [
            (0.0, 0.0),
            (3e-4, 0.0),
            (3e-4, 2e-4),
            (0.0, 2e-4),
            (0.0, 0.0),
        ] {
            wkb.extend_from_slice(&f64::to_le_bytes(lon + dlon));
            wkb.extend_from_slice(&f64::to_le_bytes(lat + dlat));
        }
        let batch = arrow::record_batch::RecordBatch::try_new(
            Arc::new(schema.clone()),
            vec![
                Arc::new(arrow::array::BinaryArray::from_vec(vec![&wkb])),
                Arc::new(Float32Array::from(vec![9.0_f32])),
            ],
        )
        .unwrap();
        let file = std::fs::File::create(dir.join("obstacles-TEST.arrow")).unwrap();
        let mut w = arrow::ipc::writer::FileWriter::try_new(file, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    /// The whole pipeline loading policy in ONE test (env vars are process
    /// globals; a single body keeps the assertions order-independent):
    /// full ring loads; a missing halo neighbour fails strict mode but loads
    /// under partial; a missing region cell fails even under
    /// partial (the popup's query-cell rule); a corrupt shard → hard Err.
    /// Fix 4's decision matrix on a REAL base-zoom receiver lattice (z12/512 px
    /// over Praha, ~12 m/px) with synthetic footprints: a 200 m block with a
    /// 60 m courtyard, plus a 3 m garage 600 m east.
    /// * annulus pixel → enclosed class (interiors use a façade donor);
    /// * COURTYARD centre → OUTDOOR (a hole shares its outer ring's id, so
    ///   the crossing-parity test reads it as outdoors — an open yard IS a
    ///   receiver);
    /// * open ground between the two footprints → not masked;
    /// * the 3 m garage → enclosed (the class raster's height gate is 0, not the
    ///   enclosure probe's 5 m — a garage interior is still an interior);
    /// * masked area ≈ the footprint area in pixels;
    /// * `apply` attenuates exactly those cells from a finite façade donor.
    #[test]
    fn interior_envelope_matrix() {
        use crate::scatter_band::{lat_to_py, lon_to_px};
        use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};
        use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};

        // Stub rasters (missing dir → every sample is the store default); the
        // Class geometry reads only the tile's receiver lat/lon lattice.
        let rasters = raster_reader::RealRasters::new(Path::new("/nonexistent-qm-interior-mask"));
        let (tx, ty) = crate::grid::lat_lon_to_tile(12, 50.08, 14.43);
        let tile = FusedTileZ13::build(12, tx, ty, 0.0, &rasters);

        let mid = TILE_PX / 2;
        let (clat, clon) = (tile.rx_lat[mid], tile.rx_lon[mid]);
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(clat.to_radians());
        // Axis-aligned square `half` metres around a (north, east) offset.
        let square = |north_m: f64, east_m: f64, half: f64| {
            vec![
                (clat + d_lat(north_m - half), clon + d_lon(east_m - half)),
                (clat + d_lat(north_m - half), clon + d_lon(east_m + half)),
                (clat + d_lat(north_m + half), clon + d_lon(east_m + half)),
                (clat + d_lat(north_m + half), clon + d_lon(east_m - half)),
            ]
        };
        let px_of = |north_m: f64, east_m: f64| {
            lat_to_py(&tile.bbox, clat + d_lat(north_m)) * TILE_PX
                + lon_to_px(&tile.bbox, clon + d_lon(east_m))
        };

        let mut b = ObstacleIndex::builder(clat, clon);
        b.add_ring(&square(0.0, 0.0, 100.0), 12.0, ObstacleKind::Building, 0);
        b.add_ring(&square(0.0, 0.0, 30.0), 12.0, ObstacleKind::Building, 0); // courtyard
        b.add_ring(&square(0.0, 600.0, 20.0), 3.0, ObstacleKind::Building, 1); // garage
        let set = ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        };

        let estimate = InteriorEstimate::bake(&tile, &set);
        let classes = estimate.classes();
        assert_eq!(classes.len(), TILE_PX * TILE_PX);
        let indoor = |i: usize| classes[i] != noise_compute::envelope::EnvelopeClass::Outdoor as u8;
        assert!(
            indoor(px_of(70.0, 0.0)),
            "block interior (annulus) is enclosed"
        );
        assert!(indoor(px_of(0.0, -70.0)), "block interior, other side");
        assert!(
            !indoor(px_of(0.0, 0.0)),
            "courtyard is open ground, not enclosed"
        );
        assert!(!indoor(px_of(0.0, 300.0)), "open ground between footprints");
        assert!(!indoor(px_of(400.0, 0.0)), "open ground north of the block");
        assert!(
            indoor(px_of(0.0, 600.0)),
            "a 3 m garage interior is enclosed"
        );
        assert_eq!(
            EnvelopeClass::from_u8(classes[px_of(70.0, 0.0)]),
            EnvelopeClass::Default,
            "the tall unclassified block keeps DEFAULT in the effective raster"
        );
        assert_eq!(
            EnvelopeClass::from_u8(classes[px_of(0.0, 600.0)]),
            EnvelopeClass::Industrial,
            "the short unclassified garage uses the lightweight 20 dB delta"
        );

        // ~12.26 m/px at this lat ⇒ (200² − 60²) + 20² m² ≈ 245 px.
        let masked = classes.iter().filter(|&&v| v != 0).count();
        assert!(
            (180..=320).contains(&masked),
            "masked area {masked} px is not the ~245 px the footprints cover"
        );

        let mut cells = vec![100u8; TILE_PX * TILE_PX];
        estimate.apply(&mut cells);
        for (i, &class) in classes.iter().enumerate() {
            assert_eq!(
                cells[i],
                if class == noise_compute::envelope::EnvelopeClass::Outdoor as u8 {
                    100
                } else {
                    // The façade cells contain 50 dB; the effective class
                    // chooses 25 dB for the 12 m block and 20 dB for the 3 m
                    // unclassified garage.
                    let delta = EnvelopeClass::from_u8(class).delta_db().unwrap();
                    crate::wire_hm3::quantise_lden(50.0 - delta)
                },
                "cell {i}"
            );
        }
    }

    #[test]
    fn felzenszwalb_edt_matches_bruteforce_on_diamond_ties() {
        // The four cardinal sites produce equal-distance diamond ties around
        // the centre. The amended product tie-break is smaller absolute x,
        // then smaller absolute y, not whichever site happened to be visited
        // first by the transform.
        let side = 7;
        let sites = [(0usize, 3usize), (6, 3), (3, 0), (3, 6)];
        let transformed = nearest_site_offsets(
            side,
            0..side,
            0..side,
            |x, y| sites.contains(&(x, y)),
            |_, _| true,
        );

        for y in 0..side {
            for x in 0..side {
                let expected = sites
                    .iter()
                    .map(|&(site_x, site_y)| {
                        let dx = x as i64 - site_x as i64;
                        let dy = y as i64 - site_y as i64;
                        (dx * dx + dy * dy, site_x, site_y)
                    })
                    .min()
                    .map(|(_, site_x, site_y)| (site_y * side + site_x) as u32)
                    .unwrap();
                assert_eq!(transformed[y * side + x], expected, "pixel ({x}, {y})");
            }
        }

        // (3, 3) is equidistant from the four cardinal sites; smaller x then
        // smaller y selects (0, 3) exactly.
        assert_eq!(transformed[3 * side + 3], (3 * side) as u32);
        assert!(
            nearest_site_offsets(side, 0..side, 0..side, |_, _| false, |_, _| true)
                .into_iter()
                .all(|site| site == NO_DONOR)
        );
    }

    /// The per-tile donor contract: an enclosed block touching the tile's west
    /// seam (its globally nearest façade would lie in the neighbour tile) is
    /// served from THIS tile's outdoor pixels — no neighbour bytes exist. Every
    /// outdoor byte is untouched, every enclosed byte is `façade − ΔL` of an
    /// outdoor in-tile donor, and a donor `NO_DATA` façade stays `NO_DATA`.
    #[test]
    fn interior_estimate_is_tile_self_contained_at_seams() {
        use crate::wire_hm3::{dequantise_lden, quantise_lden, NO_DATA};
        use raster_reader::fused_tile_z13::TILE_PX;

        let outdoor = EnvelopeClass::Outdoor as u8;
        let mut classes = vec![outdoor; TILE_PX * TILE_PX];
        // A 4-px-wide, 12-px-tall residential block flush with x = 0 and a
        // 3-px commercial block flush with the south-east corner.
        for y in 100..112 {
            for x in 0..4 {
                classes[y * TILE_PX + x] = EnvelopeClass::Residential as u8;
            }
        }
        for y in TILE_PX - 3..TILE_PX {
            for x in TILE_PX - 3..TILE_PX {
                classes[y * TILE_PX + x] = EnvelopeClass::Commercial as u8;
            }
        }
        let estimate = InteriorEstimate::from_classes(classes.clone());

        // Façade bytes vary with position so the donor choice is observable;
        // the row right of the west block at y = 105 is silent.
        let mut cells: Vec<u8> = (0..TILE_PX * TILE_PX)
            .map(|i| ((i % 97) + 40) as u8)
            .collect();
        cells[105 * TILE_PX + 4] = NO_DATA;
        let before = cells.clone();
        estimate.apply(&mut cells);

        for (i, &class) in classes.iter().enumerate() {
            let (x, y) = (i % TILE_PX, i / TILE_PX);
            if class == outdoor {
                assert_eq!(cells[i], before[i], "outdoor pixel ({x}, {y}) rewritten");
                continue;
            }
            let delta = EnvelopeClass::from_u8(class).delta_db().unwrap();
            let donor = estimate.donors[i];
            assert_ne!(
                donor, NO_DONOR,
                "enclosed pixel ({x}, {y}) has an in-tile donor"
            );
            let donor = donor as usize;
            assert_eq!(
                classes[donor], outdoor,
                "donor of ({x}, {y}) is not outdoor"
            );
            let expected = if before[donor] == NO_DATA {
                NO_DATA
            } else {
                quantise_lden(noise_compute::envelope::indoor_level_db(
                    dequantise_lden(before[donor]),
                    delta,
                ))
            };
            assert_eq!(cells[i], expected, "enclosed pixel ({x}, {y})");
        }
        // West-seam block, mid row: the nearest outdoor pixel is column x = 4
        // of the same row (distance 1..4; the block's top/bottom rows are 6–7
        // away), never "across the seam" — and that façade is silent here, so
        // the whole row of the block stays silent indoors.
        assert_eq!(estimate.donors[105 * TILE_PX], (105 * TILE_PX + 4) as u32);
        for x in 0..4 {
            assert_eq!(
                cells[105 * TILE_PX + x],
                NO_DATA,
                "silent façade stays silent indoors"
            );
        }
        // South-east corner block: donors come from the tile, x = 508 or y = 508.
        let corner = estimate.donors[(TILE_PX - 1) * TILE_PX + TILE_PX - 1] as usize;
        assert!(corner % TILE_PX == TILE_PX - 4 || corner / TILE_PX == TILE_PX - 4);
    }

    /// A tile without a single outdoor pixel has no façade anywhere: every
    /// enclosed byte becomes `NO_DATA` instead of borrowing a neighbour.
    #[test]
    fn interior_estimate_without_outdoor_pixels_is_no_data() {
        use crate::wire_hm3::NO_DATA;
        use raster_reader::fused_tile_z13::TILE_PX;
        let estimate =
            InteriorEstimate::from_classes(vec![EnvelopeClass::Default as u8; TILE_PX * TILE_PX]);
        let mut cells = vec![90u8; TILE_PX * TILE_PX];
        estimate.apply(&mut cells);
        assert!(cells.iter().all(|&c| c == NO_DATA));
    }

    #[test]
    fn loading_policy_matrix() {
        let tmp = std::env::temp_dir().join(format!("qm-obst-pipe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("QM_OBSTACLES_DIR", tmp.to_str().unwrap());
        std::env::remove_var("QM_OBSTACLES_ALLOW_PARTIAL");

        let region = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let ring: Vec<u64> = region
            .grid_disk::<Vec<_>>(1)
            .into_iter()
            .map(u64::from)
            .collect();
        let h3r4 = tmp.join("unused-h3r4");
        let last = *ring.last().unwrap(); // a halo neighbour (grid_disk puts the centre first)
        assert_ne!(last, u64::from(region));

        // All but one halo cell ingested.
        for &r4 in &ring {
            if r4 == last {
                continue;
            }
            let c = CellIndex::try_from(r4).unwrap();
            let centre = LatLng::from(c);
            write_shard(&tmp.join(c.to_string()), centre.lat(), centre.lng());
        }

        assert!(
            ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "a missing halo neighbour must fail the region: there is no second \
             building representation to fall back to"
        );

        std::env::set_var("QM_OBSTACLES_ALLOW_PARTIAL", "1");
        let partial = ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        let set = partial.set(); // partial mode admits a staging frontier
        assert_eq!(set.indexes.len(), ring.len() - 1);
        assert!(set.edge_count() >= 4 * (ring.len() - 1));

        // Missing REGION cell: even partial mode must refuse.
        std::fs::remove_dir_all(tmp.join(region.to_string())).unwrap();
        assert!(
            ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "a missing REGION cell must fail even in partial mode"
        );

        // Corrupt shard in an ingested cell: hard error.
        let centre = LatLng::from(region);
        write_shard(&tmp.join(region.to_string()), centre.lat(), centre.lng());
        std::fs::write(
            tmp.join(region.to_string()).join("obstacles-BAD.arrow"),
            b"garbage",
        )
        .unwrap();
        assert!(
            ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "corrupt shard must fail the region build"
        );

        // ── Ingested-empty proof: a shard-less halo cell whose degree tiles
        // are all listed in the world ingest manifest passes strict mode;
        // remove the manifest and the same ring fails.
        // Isolated root so its manifest can never collide with the matrix's
        // own (the manifest sits in the PARENT of QM_OBSTACLES_DIR).
        std::env::remove_var("QM_OBSTACLES_ALLOW_PARTIAL");
        let mroot = std::env::temp_dir().join(format!(
            "qm-obst-manifest-test-{}-{region}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&mroot);
        let obst_root = mroot.join("obst");
        std::fs::create_dir_all(&obst_root).unwrap();
        for &r4 in &ring {
            if r4 == last {
                continue;
            }
            let c = CellIndex::try_from(r4).unwrap();
            let centre = LatLng::from(c);
            write_shard(&obst_root.join(c.to_string()), centre.lat(), centre.lng());
        }
        let missing = CellIndex::try_from(last).unwrap();
        let mut names = String::new();
        let (lat_min, lat_max, lon_min, lon_max) = missing_boundary_bbox(missing);
        for lat in lat_min.floor() as i32..=(lat_max - f64::EPSILON).floor() as i32 {
            for lon in lon_min.floor() as i32..=(lon_max - f64::EPSILON).floor() as i32 {
                names.push_str(&format!(
                    "{}{:02}{}{:03}\n",
                    if lat >= 0 { 'N' } else { 'S' },
                    lat.abs(),
                    if lon >= 0 { 'E' } else { 'W' },
                    lon.abs()
                ));
            }
        }
        std::fs::write(mroot.join(".ingested-tiles"), names).unwrap();
        std::env::set_var("QM_OBSTACLES_DIR", obst_root.to_str().unwrap());
        let covered = ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        // A manifest-proven empty halo cell is EMPTY, not missing: the region loads.
        assert_eq!(covered.set().indexes.len(), ring.len() - 1);
        // Manifest gone → the same absent cell is no longer provably empty, and
        // nothing else can answer for it.
        std::fs::remove_file(mroot.join(".ingested-tiles")).unwrap();
        assert!(
            ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "without the manifest a missing halo neighbour is unproven, so the region fails"
        );

        std::env::remove_var("QM_OBSTACLES_DIR");
        std::env::remove_var("QM_OBSTACLES_ALLOW_PARTIAL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&mroot);
    }

    /// Lat/lon bbox of a cell boundary — test twin of the private helper in
    /// `obstacle_ingest_coverage` (kept local so the fixture cannot silently
    /// diverge from the unit under test).
    fn missing_boundary_bbox(cell: CellIndex) -> (f64, f64, f64, f64) {
        let mut lat_min = f64::MAX;
        let mut lat_max = f64::MIN;
        let mut lon_min = f64::MAX;
        let mut lon_max = f64::MIN;
        for ll in cell.boundary().iter() {
            let (lat, lon) = (ll.lat_radians().to_degrees(), ll.lng_radians().to_degrees());
            lat_min = lat_min.min(lat);
            lat_max = lat_max.max(lat);
            lon_min = lon_min.min(lon);
            lon_max = lon_max.max(lon);
        }
        (lat_min, lat_max, lon_min, lon_max)
    }
}

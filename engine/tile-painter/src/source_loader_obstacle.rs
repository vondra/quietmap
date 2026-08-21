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
//! ALL-OR-RASTER (gg review 2026-07-28): a MISSING ring cell (strict
//! default) disables vector obstacles for the WHOLE region — a partial index
//! would silently delete raster buildings where coverage is absent;
//! `QM_OBSTACLES_ALLOW_PARTIAL=1` admits missing halo NEIGHBOURS at staging
//! frontiers for dev A/B, but never the region's own cell (popup's
//! query-cell rule). A shard READ/PARSE error — including a failed
//! directory listing — is a hard `Err` that fails the region build: a
//! pipeline must never silently paint with different physics than requested
//! (the popup, facing users, soft-falls to raster instead).
//! EXCEPTION (ingested-empty proof): a shard-less cell whose every
//! overlapped 1-degree tile is listed in the world ingest manifest
//! (`.ingested-tiles`, see `obstacle_ingest_coverage`) was provably swept
//! by the same Overture release our raster derives from and contributed
//! zero footprints — it is EMPTY, not missing, and vector mode proceeds
//! without it.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use arrow::array::{Array, BinaryArray, Float32Array, Float64Array, UInt8Array};
use arrow::ipc::reader::FileReader;
use h3o::{CellIndex, LatLng};
use noise_compute::envelope::EnvelopeClass;
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::propagation::obstacle_index::{
    vector_buildings_enabled, ObstacleIndex, ObstacleKind, ObstacleSet,
};
use noise_compute::wkb;

/// A region's vector obstacles: `None` ⇒ every scatter keeps the raster path.
pub struct ObstacleData {
    set: Option<Arc<ObstacleSet>>,
}

impl ObstacleData {
    /// Vector obstacles disabled (flag off / not ingested / policy fallback).
    pub fn off() -> Self {
        ObstacleData { set: None }
    }

    /// The region set, if vector mode is live.
    pub fn set(&self) -> Option<&ObstacleSet> {
        self.set.as_deref()
    }

    /// Load per-cell indexes for the region's ring when
    /// `QM_VECTOR_BUILDINGS=1`. Follows the all-or-raster policy above; a
    /// policy fallback logs and returns [`ObstacleData::off`], a shard ERROR
    /// is a hard `Err` (a pipeline region must not silently paint with
    /// different physics than requested).
    ///
    /// `region_r4` is the cell being PAINTED: even under
    /// `QM_OBSTACLES_ALLOW_PARTIAL=1` it must be ingested — partial mode
    /// admits a missing halo NEIGHBOUR at a staging frontier, never a
    /// missing centre (deleting the centre's raster buildings with no
    /// footprints to replace them). Same rule as the popup store's
    /// query-cell requirement.
    pub fn load_for_r4s(h3r4_dir: &Path, region_r4: u64, r4_hexes: &[u64]) -> Result<Self> {
        if !vector_buildings_enabled() {
            if renderer_evidence_requires_vector_mode() {
                bail!("renderer evidence requires vector obstacles, but vector mode is disabled");
            }
            return Ok(Self::off());
        }
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
                    continue;
                }
                if manifest.is_some_and(|m| m.covers_cell(cell)) {
                    // INGESTED-EMPTY, proven by the world ingest manifest:
                    // the sweep covered this cell and it contributed zero
                    // footprints, so an index for it would be empty anyway —
                    // vector mode proceeds WITHOUT dropping anything the
                    // raster fallback would have added (our building raster
                    // derives from the same Overture release).
                    continue;
                }
                eprintln!(
                    "[obstacles] {} cell {cell} not ingested — region stays on the raster \
                     path (QM_OBSTACLES_ALLOW_PARTIAL=1 admits missing halo neighbours \
                     for dev A/B)",
                    if r4 == region_r4 { "REGION" } else { "ring" },
                );
                if renderer_evidence_requires_vector_mode() {
                    bail!("renderer evidence requires complete vector obstacles; missing {cell}");
                }
                return Ok(Self::off());
            };
            let low_profile = load_low_profile(h3r4_dir, cell)?;
            indexes.push(Arc::new(build_cell_index(cell, &dir, &low_profile)?));
        }
        let set = ObstacleSet {
            indexes: indexes.clone(),
        };
        if set.edge_count() == 0 {
            if renderer_evidence_requires_vector_mode() {
                bail!("renderer evidence requires positive vector mode; obstacle ring is empty");
            }
            // Zero edges is vector-EMPTY only when NO shard-backed index was
            // loaded — i.e. every ring cell was proven ingested-empty by the
            // manifest. Shard-backed-but-zero-edges (degenerate WKB, zero-row
            // shard, rejected heights) falls back to raster exactly as before
            // this branch: a staged shard that builds no edges is a data
            // anomaly, not proof of emptiness (/gg Codex finding 3).
            if indexes.is_empty() {
                eprintln!(
                    "[obstacles] vector mode: 0 edges across {} cells (all ingested-empty)",
                    r4_hexes.len()
                );
                return Ok(ObstacleData {
                    set: Some(Arc::new(set)),
                });
            }
            return Ok(Self::off());
        }
        eprintln!(
            "[obstacles] vector mode: {} edges across {} cells",
            set.edge_count(),
            set.indexes.len()
        );
        Ok(ObstacleData {
            set: Some(Arc::new(set)),
        })
    }
}

fn renderer_evidence_requires_vector_mode() -> bool {
    std::env::var(crate::renderer_evidence::RENDERER_EVIDENCE_FLAG).as_deref() == Ok("1")
}

/// Overwrite one tile's pre-baked `rx_refl_db` with the VECTOR enclosure —
/// the SAME 150 × 150 m nine-probe footprint as the raster 3×3
/// (`noise_compute::…::enclosure_db`; SPEC §3.8). The single bake shared by
/// the CPU builder, the GPU runner, and e2-full (gg review 2026-07-28:
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

/// Classify the receiver lattice once per tile. The result is a class raster,
/// not a boolean mask: OUTDOOR footprints remain ordinary receivers and every
/// enclosed class carries the ΔL used by Pass B.
///
/// WHY classify at all: END / CNOSSOS strategic mapping puts receivers on
/// FACADES, never indoors. A receiver inside a footprint must therefore use a
/// façade donor and an explicit display estimate, not its self-screened value.
///
/// VECTOR-ONLY by construction: a raster-fallback region has no
/// [`ObstacleSet`] and keeps today's behavior. The 30 m building raster
/// cannot answer "inside THIS footprint" — only "some building in this
/// cell" — so masking off it would blank a 30 m block around every house.
///
/// A footprint smaller than one pixel (~12 m at the base zoom) rarely covers
/// a pixel centre and so rarely masks anything. Accepted, no special
/// handling: the mask is a display-semantics correction, not physics.
pub fn bake_tile_envelope_classes(
    tile: &raster_reader::fused_tile_z13::FusedTileZ13,
    set: &ObstacleSet,
) -> Vec<u8> {
    use raster_reader::fused_tile_z13::TILE_PX;
    let mut classes =
        vec![noise_compute::envelope::EnvelopeClass::Outdoor as u8; TILE_PX * TILE_PX];
    // One scratch vec for the whole tile — `contains_built` clears it per
    // probe (same reuse the 9-probe `enclosure_db` does).
    let mut seen: Vec<(u32, u32)> = Vec::new();
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
            if let Some((class, _, _, _)) = winner {
                classes[py * TILE_PX + px] = class as u8;
            }
        }
    }
    classes
}

/// Compatibility convenience for callers that have only one tile. Production
/// writers use [`apply_interior_estimate_with_donors`] with a real 3×3 halo;
/// this wrapper supplies outdoor neighbours so old unit fixtures retain the
/// local-tile behaviour without reintroducing the old quadratic search.
pub fn apply_interior_estimate(cells: &mut [u8], classes: &[u8]) {
    use raster_reader::fused_tile_z13::TILE_PX;

    debug_assert_eq!(cells.len(), TILE_PX * TILE_PX);
    debug_assert_eq!(classes.len(), cells.len());
    let outdoor = noise_compute::envelope::EnvelopeClass::Outdoor as u8;
    let halo_classes: [Vec<u8>; 9] = std::array::from_fn(|ordinal| {
        if ordinal == 4 {
            classes.to_vec()
        } else {
            vec![outdoor; TILE_PX * TILE_PX]
        }
    });
    let donors = bake_interior_donors(&halo_classes);
    let halo_cells: [Vec<u8>; 9] = std::array::from_fn(|ordinal| {
        if ordinal == 4 {
            cells.to_vec()
        } else {
            vec![crate::wire_hm3::NO_DATA; TILE_PX * TILE_PX]
        }
    });
    apply_interior_estimate_with_donors(cells, classes, &halo_cells, &donors);
}

/// A geometry-only exterior donor for one centre-tile receiver pixel. Tile
/// ordinal is row-major in the 3×3 halo (`0..9`); it is deliberately shared
/// by every layer so energy composition commutes with the envelope loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InteriorDonor {
    pub tile_ordinal: usize,
    pub pixel_ordinal: usize,
}

/// `INF` is larger than every squared distance in the 1536×1536 receiver
/// window. The EDT deliberately stays in signed integer space: no floating
/// rounding is allowed to decide which façade wins a tie.
const EDT_INF: i32 = i32::MAX / 4;

/// Exact Felzenszwalb–Huttenlocher nearest-site transform for a rectangular
/// query window. The only O(N) scratch is one squared-distance grid, one
/// nearest-site-y grid, and the two lower-envelope arrays. The closure is
/// queried directly instead of materialising an outdoor-point list.
fn nearest_site_offsets(
    side: usize,
    query_x: std::ops::Range<usize>,
    query_y: std::ops::Range<usize>,
    is_site: impl Fn(usize, usize) -> bool,
    is_query: impl Fn(usize, usize) -> bool,
) -> Vec<Option<(usize, usize)>> {
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
    let mut donors = vec![None; query_width * (query_y.end - query_y.start)];

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
            donors[output_index] = Some((site_x, site_y as usize));
        }
    }
    donors
}

/// Build donors for the centre member of a 3×3 class-raster halo. This never
/// inspects HM3 bytes: a silent exterior receiver is still the only valid
/// façade, and therefore remains silent indoors rather than jumping to a road.
pub fn bake_interior_donors(halo_classes: &[Vec<u8>; 9]) -> Vec<Option<InteriorDonor>> {
    use noise_compute::envelope::EnvelopeClass;
    use raster_reader::fused_tile_z13::TILE_PX;

    let expected_len = TILE_PX * TILE_PX;
    debug_assert!(halo_classes
        .iter()
        .all(|classes| classes.len() == expected_len));
    let offsets = nearest_site_offsets(
        TILE_PX * 3,
        TILE_PX..TILE_PX * 2,
        TILE_PX..TILE_PX * 2,
        |x, y| {
            let tile_ordinal = (y / TILE_PX) * 3 + (x / TILE_PX);
            let pixel_ordinal = (y % TILE_PX) * TILE_PX + (x % TILE_PX);
            EnvelopeClass::from_u8(halo_classes[tile_ordinal][pixel_ordinal])
                == EnvelopeClass::Outdoor
        },
        |x, y| {
            let pixel_ordinal = (y - TILE_PX) * TILE_PX + (x - TILE_PX);
            EnvelopeClass::from_u8(halo_classes[4][pixel_ordinal])
                .delta_db()
                .is_some()
        },
    );
    offsets
        .into_iter()
        .map(|offset| {
            offset.map(|(x, y)| InteriorDonor {
                tile_ordinal: (y / TILE_PX) * 3 + (x / TILE_PX),
                pixel_ordinal: (y % TILE_PX) * TILE_PX + (x % TILE_PX),
            })
        })
        .collect()
}

/// Pass B of the interior estimate. `halo_cells` are already collapsed and
/// area-filled Pass-A outputs; only the centre vector is mutated/written.
pub fn apply_interior_estimate_with_donors(
    cells: &mut [u8],
    centre_classes: &[u8],
    halo_cells: &[Vec<u8>; 9],
    donors: &[Option<InteriorDonor>],
) {
    use crate::wire_hm3::{dequantise_lden, quantise_lden, NO_DATA};
    use noise_compute::envelope::EnvelopeClass;
    debug_assert_eq!(cells.len(), donors.len());
    for (index, cell) in cells.iter_mut().enumerate() {
        let Some(delta) = EnvelopeClass::from_u8(centre_classes[index]).delta_db() else {
            continue;
        };
        *cell = donors[index]
            .and_then(|donor| {
                let facade = dequantise_lden(halo_cells[donor.tile_ordinal][donor.pixel_ordinal]);
                facade
                    .is_finite()
                    .then(|| quantise_lden((facade - delta).max(0.0)))
            })
            .unwrap_or(NO_DATA);
    }
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

/// The world-ingest manifest next to the staging tree, when present —
/// the proof that shard-less cells are ingested-empty (see
/// `noise_compute::propagation::obstacle_ingest_coverage`). Absent (e.g. a
/// Vast worker that staged only the h3r4 tree) ⇒ coverage unknown ⇒ the
/// all-or-raster fallback keeps today's behavior.
fn ingest_manifest(
    h3r4_dir: &Path,
) -> Option<&'static noise_compute::propagation::obstacle_ingest_coverage::IngestManifest> {
    noise_compute::propagation::obstacle_ingest_coverage::IngestManifest::load_cached(
        &staging_root(h3r4_dir)
            .parent()
            .map(|p| p.join(".ingested-tiles"))
            .unwrap_or_else(|| PathBuf::from(".ingested-tiles")),
    )
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
    /// full ring loads; a missing halo neighbour → raster-off strict but
    /// loads under partial; a missing REGION cell → raster-off EVEN under
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
    /// * Pass B attenuates exactly those cells from a finite façade donor.
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

        let classes = bake_tile_envelope_classes(&tile, &set);
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

        // ~12.26 m/px at this lat ⇒ (200² − 60²) + 20² m² ≈ 245 px.
        let masked = classes.iter().filter(|&&v| v != 0).count();
        assert!(
            (180..=320).contains(&masked),
            "masked area {masked} px is not the ~245 px the footprints cover"
        );

        let mut cells = vec![100u8; TILE_PX * TILE_PX];
        apply_interior_estimate(&mut cells, &classes);
        for (i, &class) in classes.iter().enumerate() {
            assert_eq!(
                cells[i],
                if class == noise_compute::envelope::EnvelopeClass::Outdoor as u8 {
                    100
                } else {
                    // All synthetic façade cells contain 50 dB, so the
                    // DEFAULT 28 dB product estimate quantises to 22 dB.
                    crate::wire_hm3::quantise_lden(22.0)
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
                        (dx * dx + dy * dy, site_x, site_y, (site_x, site_y))
                    })
                    .min()
                    .map(|(_, _, _, offset)| offset);
                assert_eq!(transformed[y * side + x], expected, "pixel ({x}, {y})");
            }
        }

        // (3, 3) is equidistant from the four cardinal sites; smaller x then
        // smaller y selects (0, 3) exactly.
        assert_eq!(transformed[3 * side + 3], Some((0, 3)));
        assert!(
            nearest_site_offsets(side, 0..side, 0..side, |_, _| false, |_, _| true)
                .into_iter()
                .all(|site| site.is_none())
        );
    }

    #[test]
    fn loading_policy_matrix() {
        let tmp = std::env::temp_dir().join(format!("qm-obst-pipe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("QM_VECTOR_BUILDINGS", "1");
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

        let strict = ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        assert!(
            strict.set().is_none(),
            "missing halo neighbour must stay raster (strict)"
        );

        std::env::set_var("QM_OBSTACLES_ALLOW_PARTIAL", "1");
        let partial = ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        let set = partial
            .set()
            .expect("partial mode admits a staging frontier");
        assert_eq!(set.indexes.len(), ring.len() - 1);
        assert!(set.edge_count() >= 4 * (ring.len() - 1));

        // Missing REGION cell: even partial mode must refuse.
        std::fs::remove_dir_all(tmp.join(region.to_string())).unwrap();
        let no_region = ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        assert!(
            no_region.set().is_none(),
            "missing REGION cell must stay raster even partial"
        );

        // Corrupt shard in an ingested cell: hard Err, not a silent fallback.
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
        // are ALL listed in the world ingest manifest keeps vector mode even
        // STRICT; remove the manifest and the same ring falls back to raster.
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
        let set = covered
            .set()
            .expect("manifest-proven empty halo cell keeps vector mode (strict)");
        assert_eq!(set.indexes.len(), ring.len() - 1);
        // Manifest gone → coverage unknown → strict fallback again.
        std::fs::remove_file(mroot.join(".ingested-tiles")).unwrap();
        let uncovered = ObstacleData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        assert!(
            uncovered.set().is_none(),
            "without the manifest the missing halo neighbour must fall back to raster"
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

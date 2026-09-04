//! Load the per-cell structure table for a region (`structures.arrow`,
//! contract `structures_v1`) — the pipeline twin of
//! `source-reader::structure_store`; keep loading policy in lockstep.
//!
//! ONE read of each H3R4 cell's `structures.arrow`
//! (`scripts/structures/build-structures.py`) produces everything the cell's
//! buildings and noise walls feed:
//!
//! * the cell's [`ObstacleIndex`] (origin = cell centre), shared across the
//!   region's tile batches as an [`ObstacleSet`]: `kind=0` rows with geometry
//!   become building polygons (envelope-classed, low-profile capped on the
//!   non-per-building height tiers 2/4); `kind=1` rows become
//!   [`ObstacleKind::Barrier`] polylines — never capped, never class-stamped.
//!   Walls ride the same index since the parallel `types::Barrier` slice
//!   channel was deleted (noise-compute e57941d3);
//! * the [`LowProfileLookup`] those caps consult, built from the SAME file's
//!   OSM-attributed building rows (kind=0 with an `osm_id`) at the OSM
//!   (emission) centroid — the rows and positions the old `buildings.arrow`
//!   pass fed it;
//! * the building layer's emission point stream: kind=0 rows with an
//!   `osm_id`, in file order, through [`prepare_building_points`] — the rows,
//!   order, and values the old `buildings.arrow` absorb produced. The
//!   screening polygon/centroid on a matched row are Overture's; emission
//!   reads the `emission_*` overrides where the merge stored them.
//!
//! ALL-OR-ERROR: a ring cell of the prepared world whose `structures.arrow`
//! is missing fails the whole region, because a partial index would silently
//! omit buildings where coverage is absent. A READ/PARSE error is the same
//! hard `Err`: a pipeline must never silently paint with different physics
//! than requested (the popup has its own visitor-facing loading policy).
//! Emptiness is not absence — a 0-row table is the finished sweep answering
//! "nothing stands here", and a cell with no directory at all is outside the
//! prepared world (`noise_compute::propagation::structure_cell_file`).

use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use arrow::array::{
    Array, BinaryArray, Float32Array, Float64Array, Int64Array, UInt32Array, UInt8Array,
};
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use h3o::{CellIndex, LatLng};
use memmap2::Mmap;
use noise_compute::envelope::{effective_envelope_class, EnvelopeClass};
use noise_compute::low_profile::LowProfileLookup;
use noise_compute::normalize::{prepare_building_points, RawBuildingInput};
use noise_compute::propagation::obstacle_index::{
    Builder as ObstacleIndexBuilder, ObstacleIndex, ObstacleKind, ObstacleSet,
};
use noise_compute::propagation::structure_cell_file::locate_cell_structures;
use noise_compute::wkb;

use crate::schema_check::STRUCTURES_CONTRACT_V1;
use crate::source_line::opt;
use crate::source_loader_industrial::{hex_encode, pos_f32};
use crate::source_loader_leisure::LeisureData;
use crate::source_point::PointRow;

/// `structures.arrow` kind codes — `ObstacleKind`'s stored codes, written by
/// `scripts/structures/build-structures.py` (`KIND_BUILDING`/`KIND_BARRIER`).
const KIND_BUILDING: u8 = 0;
const KIND_BARRIER: u8 = 1;

/// A region's structures: the screening world (buildings + walls in one
/// [`ObstacleSet`]) plus, when collected, the building layer's emission rows.
pub struct StructureData {
    set: Arc<ObstacleSet>,
    /// One entry per `r4_hexes` cell, each in the cell's file order; `None`
    /// when the load skipped emission prep ([`Self::load_screening_for_r4s`]
    /// — the aircraft runners never paint the building layer, so they never
    /// pay the per-row hex + discretise cost).
    building_rows_by_cell: Option<Vec<Vec<PointRow>>>,
}

impl StructureData {
    /// The region set. Always present: a region that could not load its
    /// structures never gets this far.
    pub fn set(&self) -> &ObstacleSet {
        &self.set
    }

    /// Retain the region geometry beyond this loader value's lifetime.
    pub fn shared_set(&self) -> Arc<ObstacleSet> {
        Arc::clone(&self.set)
    }

    /// The tile's interior estimate, baked from this region's structures.
    pub fn interior_estimate(
        &self,
        tile: &raster_reader::fused_tile_z13::FusedTileZ13,
    ) -> InteriorEstimate {
        InteriorEstimate::bake(tile, self.set())
    }

    /// Load per-cell indexes AND the building layer's emission rows for the
    /// region's ring.
    ///
    /// Every failure is an `Err`. There is no second building representation
    /// to fall back to, so "we could not read the structures" can only mean
    /// "do not paint this region" — painting it anyway would publish a quiet
    /// map of a loud place, and nothing downstream could tell the difference.
    ///
    /// `region_r4` is the cell being PAINTED: it is by definition part of the
    /// prepared world, so its own table must be there. Same rule as the
    /// popup store's query-cell requirement.
    pub fn load_for_r4s(h3r4_dir: &Path, region_r4: u64, r4_hexes: &[u64]) -> Result<Self> {
        Self::load(h3r4_dir, region_r4, r4_hexes, true)
    }

    /// The screening world only, for the aircraft painters: the emission
    /// point prep (hex-encode + discretise per OSM row) is wasted work where
    /// no building layer is painted.
    pub fn load_screening_for_r4s(
        h3r4_dir: &Path,
        region_r4: u64,
        r4_hexes: &[u64],
    ) -> Result<Self> {
        Self::load(h3r4_dir, region_r4, r4_hexes, false)
    }

    /// Take the building layer's point stream out of the region data: the
    /// emission rows merged with `leisure`, buildings before leisure WITHIN
    /// each ring cell — the order the old per-cell buildings.arrow +
    /// leisure.arrow reads produced (f32 accumulation order is part of the
    /// painted bytes).
    pub fn take_building_layer_rows(&mut self, leisure: LeisureData) -> Result<Vec<PointRow>> {
        let mut by_cell = self
            .building_rows_by_cell
            .take()
            .context("building emission rows were not collected (load_screening_for_r4s)")?;
        let leisure_by_cell = leisure.into_rows_by_cell();
        assert_eq!(
            by_cell.len(),
            leisure_by_cell.len(),
            "building and leisure rows disagree on the ring shape"
        );
        let mut rows = Vec::new();
        for (buildings, leisure) in by_cell.iter_mut().zip(leisure_by_cell) {
            rows.append(buildings);
            rows.extend(leisure);
        }
        Ok(rows)
    }

    fn load(h3r4_dir: &Path, region_r4: u64, r4_hexes: &[u64], collect_rows: bool) -> Result<Self> {
        let mut indexes = Vec::new();
        let mut rows_by_cell: Vec<Vec<PointRow>> = Vec::with_capacity(r4_hexes.len());
        for &r4 in r4_hexes {
            let cell = CellIndex::try_from(r4).context("invalid r4 hex")?;
            let located = locate_cell_structures(h3r4_dir, cell).map_err(|e| {
                anyhow::anyhow!(
                    "[structures] {e} — buildings are vector-only, so this region cannot be painted"
                )
            })?;
            let Some(structures_arrow) = located else {
                if r4 == region_r4 {
                    bail!(
                        "[structures] REGION cell {cell} is not in the prepared world \
                         ({}) — it cannot be painted",
                        h3r4_dir.display()
                    );
                }
                // Outside the prepared world: no cell directory at all, so it
                // holds no structures for the same reason it holds no roads.
                rows_by_cell.push(Vec::new());
                continue;
            };
            let cell_load = load_cell_structures(cell, &structures_arrow, collect_rows)?;
            indexes.push(Arc::new(cell_load.index));
            rows_by_cell.push(cell_load.building_rows);
        }
        let set = ObstacleSet { indexes };
        if set.edge_count() == 0 {
            if renderer_evidence_requires_vector_mode() {
                bail!("renderer evidence requires positive vector mode; structure ring is empty");
            }
            // Zero edges is a legitimate answer: ocean, desert, and any cell
            // whose table holds no structure. A table that exists HAS answered,
            // and calling its emptiness a fault would black out whole countries
            // the day an Overture release rejects their heights.
            eprintln!(
                "[structures] vector mode: 0 edges across {} cells",
                r4_hexes.len()
            );
        } else {
            eprintln!(
                "[structures] vector mode: {} edges across {} cells",
                set.edge_count(),
                set.indexes.len()
            );
        }
        Ok(StructureData {
            set: Arc::new(set),
            building_rows_by_cell: collect_rows.then_some(rows_by_cell),
        })
    }
}

fn renderer_evidence_requires_vector_mode() -> bool {
    std::env::var(crate::renderer_evidence::RENDERER_EVIDENCE_FLAG).as_deref() == Ok("1")
}

/// One cell's load: its index plus its building-layer emission rows (empty
/// when the caller collects only the screening world).
struct CellStructures {
    index: ObstacleIndex,
    building_rows: Vec<PointRow>,
}

/// Read one cell's `structures.arrow` (memory-mapped, one pass to stage the
/// batches, then the lookup pass and the index/emission pass over them) and
/// build everything the region takes from it.
fn load_cell_structures(
    cell: CellIndex,
    structures_arrow: &Path,
    collect_rows: bool,
) -> Result<CellStructures> {
    let file = File::open(structures_arrow)
        .with_context(|| format!("read {}", structures_arrow.display()))?;
    let mmap = unsafe { Mmap::map(&file)? };
    let reader = FileReader::try_new(Cursor::new(&mmap[..]), None)
        .with_context(|| format!("arrow open {}", structures_arrow.display()))?;
    let mut batches: Vec<RecordBatch> = Vec::new();
    for batch in reader {
        batches.push(batch.with_context(|| format!("arrow batch {}", structures_arrow.display()))?);
    }
    // Convention-B contract gate, the same per-batch loop
    // `schema_check::read_surface_arrow_for_r4_with_contract` runs (this
    // loader reads the file directly, so the check is inline). Single-file
    // IPC guarantees one schema per file, but a merged table is only
    // trustworthy if every batch carries the stamp.
    for (idx, batch) in batches.iter().enumerate() {
        let c = batch
            .schema_ref()
            .metadata()
            .get("structures_contract")
            .map(String::as_str);
        if c != Some(STRUCTURES_CONTRACT_V1) {
            bail!(
                "{}[batch {idx}] structures_contract mismatch (expected {STRUCTURES_CONTRACT_V1}, \
                 got {c:?}) — rebuild the cell with scripts/structures/build-structures.py",
                structures_arrow.display()
            );
        }
    }
    let low_profile = low_profile_lookup(&batches);

    let centre = LatLng::from(cell);
    let mut builder = ObstacleIndex::builder(centre.lat(), centre.lng());
    let mut building_rows = Vec::new();
    // The emission stream collects per batch in file order; the index rows
    // collect across ALL batches first, because the screening_ordinal is a
    // file-global sequence and the index's dense ids follow its sort.
    let mut index_rows: Vec<(u32, usize, usize)> = Vec::new(); // (ordinal, batch, row)
    for (batch_idx, batch) in batches.iter().enumerate() {
        absorb_cell_batch(
            batch,
            batch_idx,
            structures_arrow,
            collect_rows,
            &mut index_rows,
            &mut building_rows,
        )?;
    }
    index_rows.sort_unstable_by_key(|&(ordinal, _, _)| ordinal);
    let mut next_id: u32 = 0;
    let mut capped = 0usize;
    // Column resolution is per-batch, once — the insert pass reads them raw.
    let batch_columns: Vec<IndexColumns> = batches
        .iter()
        .map(|batch| {
            Ok::<_, anyhow::Error>(IndexColumns {
                kind: column::<UInt8Array>(batch, "kind", structures_arrow)?,
                wkb: column::<BinaryArray>(batch, "geometry_wkb", structures_arrow)?,
                heights: column::<Float32Array>(batch, "height_m", structures_arrow)?,
                tiers: column::<UInt8Array>(batch, "height_tier", structures_arrow)?,
                clats: column::<Float64Array>(batch, "centroid_lat", structures_arrow)?,
                clons: column::<Float64Array>(batch, "centroid_lon", structures_arrow)?,
                envelope: column::<UInt8Array>(batch, "envelope_class", structures_arrow)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for &(_, batch_idx, i) in &index_rows {
        insert_index_row(
            &batch_columns[batch_idx],
            i,
            &low_profile,
            &mut builder,
            &mut next_id,
            &mut capped,
        );
    }
    if capped > 0 {
        eprintln!("[structures] {cell}: {capped} defaulted heights capped to low-profile 3 m");
    }
    Ok(CellStructures {
        index: builder.build(),
        building_rows,
    })
}

/// The emission columns a batch carries, resolved only when the caller
/// collects the building layer's point stream. All are nullable in the v1
/// schema: only the OSM-attributed rows (kind=0 with an `osm_id`) carry them.
#[derive(Clone, Copy)]
struct EmissionColumns<'a> {
    osm_id: &'a Int64Array,
    building_type: &'a UInt8Array,
    height: &'a Float32Array,
    floors: &'a UInt8Array,
    area_m2: &'a Float32Array,
    polygon_wkb: &'a BinaryArray,
    centroid_lat: &'a Float64Array,
    centroid_lon: &'a Float64Array,
}

/// A v1 column by name and type, or a loud error — the contract stamp above
/// guarantees the shape, so a missing or mistyped column is a corrupt table,
/// never a reason to guess.
fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str, path: &Path) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<T>())
        .ok_or_else(|| anyhow::anyhow!("{}: missing/mistyped {name} column", path.display()))
}

/// One batch of one cell: collect the building layer's emission rows in file
/// order and the index-bound rows (ordinal, batch, row) for the global
/// ordinal-sorted insert pass. The dense edge id IS the ordinal's position in
/// the sorted order — the migration assigns ordinals reproducing the legacy
/// obstacles.arrow row order, and the engine's exact-δ tie resolution is
/// scan-order sensitive, so the order is load-bearing. The emission stream is
/// always file order (the buildings.arrow subsequence), independent of it.
fn absorb_cell_batch(
    batch: &RecordBatch,
    batch_idx: usize,
    structures_arrow: &Path,
    collect_rows: bool,
    index_rows: &mut Vec<(u32, usize, usize)>,
    building_rows: &mut Vec<PointRow>,
) -> Result<()> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(());
    }
    let kind = column::<UInt8Array>(batch, "kind", structures_arrow)?;
    let wkb = column::<BinaryArray>(batch, "geometry_wkb", structures_arrow)?;
    let clats = column::<Float64Array>(batch, "centroid_lat", structures_arrow)?;
    let clons = column::<Float64Array>(batch, "centroid_lon", structures_arrow)?;
    let ordinals = column::<UInt32Array>(batch, "screening_ordinal", structures_arrow)?;
    let emission = collect_rows
        .then(|| {
            Ok::<_, anyhow::Error>(EmissionColumns {
                osm_id: column::<Int64Array>(batch, "osm_id", structures_arrow)?,
                building_type: column::<UInt8Array>(batch, "building_type", structures_arrow)?,
                height: column::<Float32Array>(batch, "height", structures_arrow)?,
                floors: column::<UInt8Array>(batch, "floors", structures_arrow)?,
                area_m2: column::<Float32Array>(batch, "area_m2", structures_arrow)?,
                polygon_wkb: column::<BinaryArray>(
                    batch,
                    "emission_polygon_wkb",
                    structures_arrow,
                )?,
                centroid_lat: column::<Float64Array>(
                    batch,
                    "emission_centroid_lat",
                    structures_arrow,
                )?,
                centroid_lon: column::<Float64Array>(
                    batch,
                    "emission_centroid_lon",
                    structures_arrow,
                )?,
            })
        })
        .transpose()?;

    for i in 0..n {
        match kind.value(i) {
            KIND_BUILDING => {
                if let Some(emission) = emission {
                    absorb_emission_row(i, emission, clats, clons, wkb, building_rows);
                }
                if wkb.is_valid(i) {
                    if ordinals.is_null(i) {
                        bail!(
                            "{}: building row {i} with geometry but no screening_ordinal",
                            structures_arrow.display()
                        );
                    }
                    index_rows.push((ordinals.value(i), batch_idx, i));
                }
            }
            KIND_BARRIER => {
                if wkb.is_null(i) {
                    bail!(
                        "{}: barrier row {i} has no geometry",
                        structures_arrow.display()
                    );
                }
                if ordinals.is_null(i) {
                    bail!(
                        "{}: barrier row {i} has no screening_ordinal",
                        structures_arrow.display()
                    );
                }
                index_rows.push((ordinals.value(i), batch_idx, i));
            }
            other => bail!(
                "{}: unknown structure kind {other} at row {i}",
                structures_arrow.display()
            ),
        }
    }
    Ok(())
}

/// The columns the ordinal-sorted insert pass reads per row (resolved once per
/// batch, above).
struct IndexColumns<'a> {
    kind: &'a UInt8Array,
    wkb: &'a BinaryArray,
    heights: &'a Float32Array,
    tiers: &'a UInt8Array,
    clats: &'a Float64Array,
    clons: &'a Float64Array,
    envelope: &'a UInt8Array,
}

/// Insert one row into the cell's index under its dense (sorted) id: buildings
/// as capped, envelope-classed polygons; walls as uncapped, unclassed barrier
/// polylines.
fn insert_index_row(
    cols: &IndexColumns<'_>,
    i: usize,
    low_profile: &LowProfileLookup,
    builder: &mut ObstacleIndexBuilder,
    next_id: &mut u32,
    capped: &mut usize,
) {
    let id = *next_id;
    *next_id = next_id.wrapping_add(1);
    match cols.kind.value(i) {
        KIND_BUILDING => {
            let mut height = cols.heights.value(i);
            // The cap applies to buildings only, and only to the
            // non-per-building height tiers (2 default, 4 ANBH prior) —
            // `LowProfileLookup::capped_height` owns the tier gate.
            let capped_h = low_profile.capped_height(
                height,
                cols.tiers.value(i),
                cols.clats.value(i),
                cols.clons.value(i),
                wkb::outer_ring_area_m2(cols.wkb.value(i)),
            );
            if capped_h < height {
                *capped += 1;
                height = capped_h;
            }
            builder.add_polygon_wkb(
                cols.wkb.value(i),
                height,
                ObstacleKind::Building,
                id,
                EnvelopeClass::from_u8(cols.envelope.value(i)),
            );
        }
        KIND_BARRIER => {
            // Walls are never capped and never class-stamped: a wall height is
            // per-feature knowledge, and an open polyline encloses nothing.
            builder.add_polyline(
                &wkb::parse_wkb_linestring_bytes(cols.wkb.value(i)),
                cols.heights.value(i),
                ObstacleKind::Barrier,
                id,
            );
        }
        _ => unreachable!("kind was validated in the collection pass"),
    }
}

/// One OSM-attributed building row's emission points — the old
/// `source_loader_building::absorb_batch` row body, verbatim semantics: the
/// emission position is the OSM centroid (`emission_centroid_*` where the
/// merge stored it; a matched row's `centroid_*` is the Overture screening
/// centroid), the emission polygon is `emission_polygon_wkb ??
/// geometry_wkb`, and the raw OSM height/floors/type/area feed the emission
/// ladder.
fn absorb_emission_row(
    i: usize,
    emission: EmissionColumns<'_>,
    clats: &Float64Array,
    clons: &Float64Array,
    wkb: &BinaryArray,
    building_rows: &mut Vec<PointRow>,
) {
    if emission.osm_id.is_null(i) {
        return;
    }
    let centroid_lat = if emission.centroid_lat.is_valid(i) {
        emission.centroid_lat.value(i)
    } else {
        clats.value(i)
    };
    let centroid_lon = if emission.centroid_lon.is_valid(i) {
        emission.centroid_lon.value(i)
    } else {
        clons.value(i)
    };
    let polygon_wkb: &[u8] = if emission.polygon_wkb.is_valid(i) {
        emission.polygon_wkb.value(i)
    } else if wkb.is_valid(i) {
        wkb.value(i)
    } else {
        b""
    };
    let wkb_hex = hex_encode(polygon_wkb);
    let points = prepare_building_points(RawBuildingInput {
        centroid_lat,
        centroid_lon,
        height_m: if emission.height.is_valid(i) {
            emission.height.value(i)
        } else {
            0.0
        },
        floors: if emission.floors.is_valid(i) {
            emission.floors.value(i)
        } else {
            0
        },
        building_type: if emission.building_type.is_valid(i) {
            emission.building_type.value(i)
        } else {
            0
        },
        // Validity-blind read, exactly as the old buildings.arrow absorb:
        // null area slots carry a 0 in the values buffer, which pos_f32 maps
        // to "absent" either way.
        area_m2: pos_f32(emission.area_m2.value(i)).map(f64::from),
        polygon_wkb: &wkb_hex,
    });
    building_rows.extend(points.iter().map(PointRow::from_prepared));
}

/// Build the low-profile cap's lookup from the SAME file's OSM-attributed
/// building rows (kind=0 with an `osm_id`) — the old loader's separate
/// `buildings.arrow` read, folded into the one structure table. The lookup
/// position is the OSM (emission) centroid: on a matched row `centroid_*` is
/// the Overture screening centroid, while the cap must sit where the old
/// buildings.arrow centroid sat. A missing column means no capping — never a
/// hard error for a correction layer.
fn low_profile_lookup(batches: &[RecordBatch]) -> LowProfileLookup {
    let mut lookup = LowProfileLookup::default();
    for batch in batches {
        let (Some(kind), Some(osm_id), Some(types), Some(areas), Some(clats), Some(clons)) = (
            opt::<UInt8Array>(batch, "kind"),
            opt::<Int64Array>(batch, "osm_id"),
            opt::<UInt8Array>(batch, "building_type"),
            opt::<Float32Array>(batch, "area_m2"),
            opt::<Float64Array>(batch, "centroid_lat"),
            opt::<Float64Array>(batch, "centroid_lon"),
        ) else {
            return LowProfileLookup::default();
        };
        let eclats = opt::<Float64Array>(batch, "emission_centroid_lat");
        let eclons = opt::<Float64Array>(batch, "emission_centroid_lon");
        for i in 0..batch.num_rows() {
            // Overture-only rows and walls carry no OSM class — they can
            // never be the low-profile evidence (or its null-gated inputs).
            if kind.value(i) != KIND_BUILDING || osm_id.is_null(i) {
                continue;
            }
            if types.is_null(i) || areas.is_null(i) {
                continue;
            }
            let lat = match eclats {
                Some(a) if a.is_valid(i) => a.value(i),
                _ => clats.value(i),
            };
            let lon = match eclons {
                Some(a) if a.is_valid(i) => a.value(i),
                _ => clons.value(i),
            };
            lookup.insert_if_low(types.value(i), lat, lon, areas.value(i));
        }
    }
    lookup
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
    /// Classify the tile's lattice against the region's structures and bake
    /// the donor map (vector regions only — see
    /// [`StructureData::interior_estimate`]).
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int16Array, StringArray, UInt16Array, UInt32Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use h3o::Resolution;
    use noise_compute::constants::{m_per_deg_lon, M_PER_DEG_LAT};

    /// One row of the fixture's structures.arrow. The columns this loader
    /// never reads (building_use, name, addr_*, opening_hours_frac,
    /// source_id) are written null, matching the real table's OSM-less rows.
    #[derive(Clone, Default)]
    struct TestStructureRow {
        kind: u8,
        geometry_wkb: Option<Vec<u8>>,
        height_m: f32,
        height_tier: u8,
        envelope_class: u8,
        centroid: (f64, f64),
        osm_id: Option<i64>,
        building_type: Option<u8>,
        height: Option<f32>,
        floors: Option<u8>,
        area_m2: Option<f32>,
        emission_polygon_wkb: Option<Vec<u8>>,
        emission_centroid: Option<(f64, f64)>,
        segment_idx: Option<i16>,
        /// None falls back to the row index (dense), matching the builder's
        /// invariant; tests that shuffle file order set it explicitly.
        screening_ordinal: Option<u32>,
    }

    /// The v1 schema exactly as `scripts/structures/build-structures.py`
    /// writes it — all 22 columns, `structures_contract` in the metadata.
    fn write_cell_structures(h3r4_dir: &Path, cell: CellIndex, rows: &[TestStructureRow]) {
        write_cell_structures_with_contract(h3r4_dir, cell, rows, Some(STRUCTURES_CONTRACT_V1));
    }

    fn write_cell_structures_with_contract(
        h3r4_dir: &Path,
        cell: CellIndex,
        rows: &[TestStructureRow],
        contract: Option<&str>,
    ) {
        let dir = h3r4_dir.join(cell.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let mut metadata = std::collections::HashMap::new();
        if let Some(contract) = contract {
            metadata.insert("structures_contract".to_string(), contract.to_string());
        }
        let schema = Arc::new(
            Schema::new(vec![
                Field::new("kind", DataType::UInt8, false),
                Field::new("geometry_wkb", DataType::Binary, true),
                Field::new("height_m", DataType::Float32, false),
                Field::new("height_tier", DataType::UInt8, false),
                Field::new("envelope_class", DataType::UInt8, false),
                Field::new("centroid_lat", DataType::Float64, false),
                Field::new("centroid_lon", DataType::Float64, false),
                Field::new("osm_id", DataType::Int64, true),
                Field::new("building_type", DataType::UInt8, true),
                Field::new("building_use", DataType::UInt8, true),
                Field::new("height", DataType::Float32, true),
                Field::new("floors", DataType::UInt8, true),
                Field::new("name", DataType::Utf8, true),
                Field::new("addr_street", DataType::Utf8, true),
                Field::new("addr_housenumber", DataType::Utf8, true),
                Field::new("area_m2", DataType::Float32, true),
                Field::new("opening_hours_frac", DataType::UInt8, true),
                Field::new("source_id", DataType::UInt16, true),
                Field::new("emission_polygon_wkb", DataType::Binary, true),
                Field::new("emission_centroid_lat", DataType::Float64, true),
                Field::new("emission_centroid_lon", DataType::Float64, true),
                Field::new("segment_idx", DataType::Int16, true),
                Field::new("screening_ordinal", DataType::UInt32, true),
            ])
            .with_metadata(metadata),
        );
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(UInt8Array::from_iter_values(rows.iter().map(|r| r.kind))),
                Arc::new(BinaryArray::from_iter(
                    rows.iter().map(|r| r.geometry_wkb.as_deref()),
                )),
                Arc::new(Float32Array::from_iter_values(
                    rows.iter().map(|r| r.height_m),
                )),
                Arc::new(UInt8Array::from_iter_values(
                    rows.iter().map(|r| r.height_tier),
                )),
                Arc::new(UInt8Array::from_iter_values(
                    rows.iter().map(|r| r.envelope_class),
                )),
                Arc::new(Float64Array::from_iter_values(
                    rows.iter().map(|r| r.centroid.0),
                )),
                Arc::new(Float64Array::from_iter_values(
                    rows.iter().map(|r| r.centroid.1),
                )),
                Arc::new(Int64Array::from_iter(rows.iter().map(|r| r.osm_id))),
                Arc::new(UInt8Array::from_iter(rows.iter().map(|r| r.building_type))),
                Arc::new(UInt8Array::from_iter(rows.iter().map(|_| None::<u8>))),
                Arc::new(Float32Array::from_iter(rows.iter().map(|r| r.height))),
                Arc::new(UInt8Array::from_iter(rows.iter().map(|r| r.floors))),
                Arc::new(StringArray::from_iter(rows.iter().map(|_| None::<&str>))),
                Arc::new(StringArray::from_iter(rows.iter().map(|_| None::<&str>))),
                Arc::new(StringArray::from_iter(rows.iter().map(|_| None::<&str>))),
                Arc::new(Float32Array::from_iter(rows.iter().map(|r| r.area_m2))),
                Arc::new(UInt8Array::from_iter(rows.iter().map(|_| None::<u8>))),
                Arc::new(UInt16Array::from_iter(rows.iter().map(|_| None::<u16>))),
                Arc::new(BinaryArray::from_iter(
                    rows.iter().map(|r| r.emission_polygon_wkb.as_deref()),
                )),
                Arc::new(Float64Array::from_iter(
                    rows.iter().map(|r| r.emission_centroid.map(|c| c.0)),
                )),
                Arc::new(Float64Array::from_iter(
                    rows.iter().map(|r| r.emission_centroid.map(|c| c.1)),
                )),
                Arc::new(Int16Array::from_iter(rows.iter().map(|r| r.segment_idx))),
                Arc::new(UInt32Array::from_iter(
                    rows.iter()
                        .enumerate()
                        .map(|(i, r)| r.screening_ordinal.or(Some(i as u32))),
                )),
            ],
        )
        .unwrap();
        let file = std::fs::File::create(
            dir.join(noise_compute::propagation::structure_cell_file::CELL_STRUCTURE_FILENAME),
        )
        .unwrap();
        let mut w = arrow::ipc::writer::FileWriter::try_new(file, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    /// WKB little-endian Polygon, one square ring of half-size `half_m`
    /// centred at (lat, lon) — 1 ring × 5 points, the old fixture's shape.
    fn square_wkb(lat: f64, lon: f64, half_m: f64) -> Vec<u8> {
        let dlat = half_m / M_PER_DEG_LAT;
        let dlon = half_m / m_per_deg_lon(lat.to_radians());
        let mut wkb: Vec<u8> = vec![1, 3, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0];
        for (sx, sy) in [
            (-1.0, -1.0),
            (1.0, -1.0),
            (1.0, 1.0),
            (-1.0, 1.0),
            (-1.0, -1.0),
        ] {
            wkb.extend_from_slice(&f64::to_le_bytes(lon + sx * dlon));
            wkb.extend_from_slice(&f64::to_le_bytes(lat + sy * dlat));
        }
        wkb
    }

    /// WKB little-endian LineString of one 2-point segment — the wall row
    /// shape (`build-structures.py`'s `wall_wkb`).
    fn wall_wkb(a: (f64, f64), b: (f64, f64)) -> Vec<u8> {
        let mut wkb = vec![1u8, 2, 0, 0, 0, 2, 0, 0, 0];
        for (lat, lon) in [a, b] {
            wkb.extend_from_slice(&f64::to_le_bytes(lon));
            wkb.extend_from_slice(&f64::to_le_bytes(lat));
        }
        wkb
    }

    fn assert_point_rows_eq(actual: &[PointRow], expected: &[PointRow]) {
        assert_eq!(actual.len(), expected.len(), "emission stream point count");
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(a.lat.to_bits(), e.lat.to_bits(), "point {i} lat");
            assert_eq!(a.lon.to_bits(), e.lon.to_bits(), "point {i} lon");
            assert_eq!(
                a.source_height_m.to_bits(),
                e.source_height_m.to_bits(),
                "point {i} source_height_m"
            );
            assert_eq!(
                a.max_distance_m.to_bits(),
                e.max_distance_m.to_bits(),
                "point {i} max_distance_m"
            );
            assert_eq!(
                a.exclusion_radius_m.to_bits(),
                e.exclusion_radius_m.to_bits(),
                "point {i} exclusion_radius_m"
            );
            assert_eq!(
                a.max_day_emission_db.to_bits(),
                e.max_day_emission_db.to_bits(),
                "point {i} max_day_emission_db"
            );
            for (ap, ep) in a.emission_lin.iter().zip(e.emission_lin.iter()) {
                for (ab, eb) in ap.iter().zip(ep.iter()) {
                    assert_eq!(ab.to_bits(), eb.to_bits(), "point {i} emission_lin");
                }
            }
        }
    }

    /// The kind=0/osm_id subsequence IS the old buildings.arrow read: same
    /// rows, same order, same values through `prepare_building_points`. Walls
    /// (even with an osm_id) and Overture-only rows never enter it; the
    /// emission polygon/centroid overrides beat the screening columns exactly
    /// where the merge stored them; every geometry row screens.
    #[test]
    fn building_emission_stream_replays_the_old_buildings_arrow_read() {
        let tmp =
            std::env::temp_dir().join(format!("qm-structures-emit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let h3r4 = tmp.join("h3r4");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let centre = LatLng::from(cell);
        let (clat, clon) = (centre.lat(), centre.lng());
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(clat.to_radians());

        // Row 0: an OSM-only residential building — emission reads its own
        // geometry and centroid (no overrides stored).
        let row_a_polygon = square_wkb(clat, clon, 5.0);
        // Row 1: an Overture-only footprint (no osm_id) — screens, never emits.
        let row_b_polygon = square_wkb(clat + d_lat(300.0), clon, 8.0);
        // Row 2: a noise wall (osm_id present — walls carry one — kind routes).
        let wall = wall_wkb(
            (clat - d_lat(60.0), clon + d_lon(45.0)),
            (clat + d_lat(60.0), clon + d_lon(45.0)),
        );
        // Row 3: a matched row — screening polygon/centroid are Overture's
        // (300 m west), the emission overrides point at the OSM building
        // (40 m east) and its large polygon (which grid-splits, so the
        // polygon choice is observable in the point stream).
        let row_d_screening_polygon = square_wkb(clat, clon - d_lon(300.0), 10.0);
        let row_d_emission_polygon = square_wkb(clat, clon + d_lon(40.0), 40.0);
        let row_d_emission_centroid = (clat + d_lat(3.0), clon + d_lon(40.0));
        let rows = vec![
            TestStructureRow {
                kind: KIND_BUILDING,
                geometry_wkb: Some(row_a_polygon.clone()),
                height_m: 9.0,
                height_tier: 0,
                envelope_class: 1,
                centroid: (clat, clon),
                osm_id: Some(101),
                building_type: Some(0),
                height: Some(9.0),
                area_m2: Some(120.0),
                ..Default::default()
            },
            TestStructureRow {
                kind: KIND_BUILDING,
                geometry_wkb: Some(row_b_polygon.clone()),
                height_m: 8.0,
                height_tier: 2,
                envelope_class: 5,
                centroid: (clat + d_lat(300.0), clon),
                ..Default::default()
            },
            TestStructureRow {
                kind: KIND_BARRIER,
                geometry_wkb: Some(wall.clone()),
                height_m: 3.0,
                height_tier: 0,
                envelope_class: 0,
                centroid: (clat, clon + d_lon(45.0)),
                osm_id: Some(55),
                segment_idx: Some(0),
                ..Default::default()
            },
            TestStructureRow {
                kind: KIND_BUILDING,
                geometry_wkb: Some(row_d_screening_polygon.clone()),
                height_m: 21.0,
                height_tier: 0,
                envelope_class: 2,
                centroid: (clat, clon - d_lon(300.0)),
                osm_id: Some(202),
                building_type: Some(0),
                height: Some(21.0),
                area_m2: Some(5000.0),
                emission_polygon_wkb: Some(row_d_emission_polygon.clone()),
                emission_centroid: Some(row_d_emission_centroid),
                ..Default::default()
            },
        ];
        write_cell_structures(&h3r4, cell, &rows);
        let mut data =
            StructureData::load_for_r4s(&h3r4, u64::from(cell), &[u64::from(cell)]).unwrap();

        // Screening: the three polygons (4 edges each) plus the wall (1). The
        // emission override polygon never screens.
        assert_eq!(data.set().edge_count(), 13);

        // Emission: the replay the old buildings.arrow absorb would have
        // produced, row by row in file order.
        let hex_a = hex_encode(&row_a_polygon);
        let hex_d = hex_encode(&row_d_emission_polygon);
        let mut expected: Vec<PointRow> = prepare_building_points(RawBuildingInput {
            centroid_lat: clat,
            centroid_lon: clon,
            height_m: 9.0,
            floors: 0,
            building_type: 0,
            area_m2: Some(120.0),
            polygon_wkb: &hex_a,
        })
        .iter()
        .map(PointRow::from_prepared)
        .collect();
        expected.extend(
            prepare_building_points(RawBuildingInput {
                centroid_lat: row_d_emission_centroid.0,
                centroid_lon: row_d_emission_centroid.1,
                height_m: 21.0,
                floors: 0,
                building_type: 0,
                area_m2: Some(5000.0),
                polygon_wkb: &hex_d,
            })
            .iter()
            .map(PointRow::from_prepared),
        );
        assert!(
            expected.len() > 2,
            "the large emission polygon must grid-split, else the polygon \
             choice is unobservable"
        );

        // The leisure merge is the building layer's concatenation rule; with
        // no leisure.arrow the stream is the building rows unchanged.
        let merged = data
            .take_building_layer_rows(LeisureData::load_for_r4s(&h3r4, &[u64::from(cell)]).unwrap())
            .unwrap();
        assert_point_rows_eq(&merged, &expected);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A matched row UNDER the grid-split threshold stores no emission polygon,
    /// so the loader hands `prepare_building_points` the screening polygon —
    /// Overture's, several hundred metres from the OSM building. That is sound
    /// only because below the threshold the point stream is polygon-independent:
    /// one point at the emission centroid, whatever polygon comes with it. The
    /// whole sparse emission-polygon rule (1.79 % of polygon bytes) rests on
    /// this, so pin it against the polygon actually stored.
    #[test]
    fn a_small_matched_row_emits_one_point_at_the_osm_centroid() {
        let tmp =
            std::env::temp_dir().join(format!("qm-structures-small-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let h3r4 = tmp.join("h3r4");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let centre = LatLng::from(cell);
        let (clat, clon) = (centre.lat(), centre.lng());
        let d_lon = |m: f64| m / m_per_deg_lon(clat.to_radians());

        let screening_polygon = square_wkb(clat, clon - d_lon(300.0), 6.0);
        let rows = vec![TestStructureRow {
            kind: KIND_BUILDING,
            geometry_wkb: Some(screening_polygon),
            height_m: 12.0,
            height_tier: 4,
            envelope_class: 1,
            centroid: (clat, clon - d_lon(300.0)),
            osm_id: Some(303),
            building_type: Some(0),
            height: Some(9.0),
            area_m2: Some(120.0),
            emission_polygon_wkb: None,
            emission_centroid: Some((clat, clon)),
            ..Default::default()
        }];
        write_cell_structures(&h3r4, cell, &rows);
        let mut data =
            StructureData::load_for_r4s(&h3r4, u64::from(cell), &[u64::from(cell)]).unwrap();
        let merged = data
            .take_building_layer_rows(LeisureData::load_for_r4s(&h3r4, &[u64::from(cell)]).unwrap())
            .unwrap();

        // One point, at the OSM centroid — not at the Overture polygon 300 m west.
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].lat.to_bits(), clat.to_bits());
        assert_eq!(merged[0].lon.to_bits(), clon.to_bits());
        // …and the same stream an UNRELATED polygon produces: below the
        // threshold `discretize_area_source` never reads it.
        let elsewhere = hex_encode(&square_wkb(clat, clon + d_lon(900.0), 3.0));
        let expected: Vec<PointRow> = prepare_building_points(RawBuildingInput {
            centroid_lat: clat,
            centroid_lon: clon,
            height_m: 9.0,
            floors: 0,
            building_type: 0,
            area_m2: Some(120.0),
            polygon_wkb: &elsewhere,
        })
        .iter()
        .map(PointRow::from_prepared)
        .collect();
        assert_point_rows_eq(&merged, &expected);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The low-profile lookup is built from the SAME file's OSM rows (at the
    /// OSM emission centroid where the merge moved the screening centroid off
    /// it), caps only kind=0 rows on the non-per-building tiers, and never
    /// touches a wall — the old buildings.arrow-fed cap, reproduced from the
    /// one table.
    #[test]
    fn low_profile_cap_reads_the_same_files_osm_rows_and_never_walls() {
        let tmp =
            std::env::temp_dir().join(format!("qm-structures-cap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let h3r4 = tmp.join("h3r4");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let centre = LatLng::from(cell);
        let (clat, clon) = (centre.lat(), centre.lng());
        let d_lat = |m: f64| m / M_PER_DEG_LAT;
        let d_lon = |m: f64| m / m_per_deg_lon(clat.to_radians());

        // Row 0: a low OSM garage (class 7) at the cell centre — feeds the
        // lookup. Row 1: a defaulted (tier 2) 8 m Overture footprint at the
        // same spot with a comparable area — capped to 3 m. Row 2: an 8 m
        // wall on the same defaulted tier — walls are never capped.
        let rows = vec![
            TestStructureRow {
                kind: KIND_BUILDING,
                geometry_wkb: Some(square_wkb(clat, clon, 2.5)),
                height_m: 3.0,
                height_tier: 0,
                envelope_class: 1,
                centroid: (clat, clon),
                osm_id: Some(7),
                building_type: Some(7),
                area_m2: Some(22.0),
                ..Default::default()
            },
            TestStructureRow {
                kind: KIND_BUILDING,
                geometry_wkb: Some(square_wkb(clat, clon, 2.5)),
                height_m: 8.0,
                height_tier: 2,
                envelope_class: 5,
                centroid: (clat, clon),
                ..Default::default()
            },
            TestStructureRow {
                kind: KIND_BARRIER,
                geometry_wkb: Some(wall_wkb(
                    (clat - d_lat(60.0), clon + d_lon(50.0)),
                    (clat + d_lat(60.0), clon + d_lon(50.0)),
                )),
                height_m: 8.0,
                height_tier: 2,
                envelope_class: 0,
                centroid: (clat, clon + d_lon(50.0)),
                osm_id: Some(88),
                segment_idx: Some(0),
                ..Default::default()
            },
        ];
        write_cell_structures(&h3r4, cell, &rows);
        // The screening-only load runs the cap identically (the emission
        // stream is not what feeds the lookup).
        let data =
            StructureData::load_screening_for_r4s(&h3r4, u64::from(cell), &[u64::from(cell)])
                .unwrap();

        // A west→east ray through both squares and the wall. Candidate ids
        // are the dense file row ordinals.
        let mut cands = Vec::new();
        data.set().crossings(
            clat,
            clon - d_lon(100.0),
            clat,
            clon + d_lon(150.0),
            &mut cands,
        );
        let heights_of = |id: u32| -> Vec<f32> {
            cands
                .iter()
                .filter(|c| c.id == id)
                .map(|c| c.height_m)
                .collect()
        };
        let garage = heights_of(0);
        let footprint = heights_of(1);
        let wall = heights_of(2);
        assert!(
            !garage.is_empty() && garage.iter().all(|&h| h == 3.0),
            "the mapped 3 m garage keeps 3 m: {garage:?}"
        );
        assert!(
            !footprint.is_empty() && footprint.iter().all(|&h| h == 3.0),
            "the defaulted 8 m footprint next to the garage caps to 3 m: {footprint:?}"
        );
        assert!(
            !wall.is_empty() && wall.iter().all(|&h| h == 8.0),
            "the tier-2 wall is never capped: {wall:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The contract gate: a structures.arrow without the v1 stamp (a stale or
    /// foreign table) fails the region load loudly, before any row is read.
    #[test]
    fn structures_contract_gate_rejects_unstamped_files() {
        let tmp =
            std::env::temp_dir().join(format!("qm-structures-gate-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let h3r4 = tmp.join("h3r4");
        let cell = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);

        write_cell_structures_with_contract(&h3r4, cell, &[], Some("structures_v0"));
        let error = StructureData::load_for_r4s(&h3r4, u64::from(cell), &[u64::from(cell)])
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("structures_contract mismatch"),
            "stale stamp must be named: {error:#}"
        );

        write_cell_structures_with_contract(&h3r4, cell, &[], None);
        assert!(
            StructureData::load_for_r4s(&h3r4, u64::from(cell), &[u64::from(cell)]).is_err(),
            "a missing stamp is not the v1 contract"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The whole pipeline loading policy in ONE test (env vars are process
    /// globals; a single body keeps the assertions order-independent):
    /// full ring loads; a missing halo neighbour fails; a missing region cell
    /// fails even harder; a corrupt table is a hard Err. Conflating an EMPTY
    /// table with a MISSING one is the bug this design removes.
    #[test]
    fn loading_policy_matrix() {
        let tmp =
            std::env::temp_dir().join(format!("qm-structures-pipe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let h3r4 = tmp.join("h3r4");

        let region = LatLng::new(50.08, 14.43).unwrap().to_cell(Resolution::Four);
        let ring: Vec<u64> = region
            .grid_disk::<Vec<_>>(1)
            .into_iter()
            .map(u64::from)
            .collect();
        let last = *ring.last().unwrap(); // a halo neighbour (grid_disk puts the centre first)
        assert_ne!(last, u64::from(region));

        // Region with a structure, every halo neighbour swept and EMPTY.
        let centre = LatLng::from(region);
        write_cell_structures(
            &h3r4,
            region,
            &[TestStructureRow {
                kind: KIND_BUILDING,
                geometry_wkb: Some(square_wkb(centre.lat(), centre.lng(), 10.0)),
                height_m: 9.0,
                height_tier: 0,
                envelope_class: 1,
                centroid: (centre.lat(), centre.lng()),
                ..Default::default()
            }],
        );
        for &r4 in &ring {
            if r4 != u64::from(region) {
                write_cell_structures(&h3r4, CellIndex::try_from(r4).unwrap(), &[]);
            }
        }
        let loaded = StructureData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        assert_eq!(loaded.set().indexes.len(), ring.len());
        assert_eq!(
            loaded.set().edge_count(),
            4,
            "an empty table is an answer, not a gap"
        );

        // A prepared halo cell whose table was not delivered fails the region:
        // there is no second building representation to fall back to.
        let missing = CellIndex::try_from(last).unwrap();
        std::fs::remove_file(
            h3r4.join(missing.to_string())
                .join(noise_compute::propagation::structure_cell_file::CELL_STRUCTURE_FILENAME),
        )
        .unwrap();
        assert!(
            StructureData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "a halo cell without its structure table must fail the region"
        );

        // A cell the extract never produced has no directory at all: outside
        // the prepared world, contributing nothing, exactly as it contributes
        // no roads.
        std::fs::remove_dir_all(h3r4.join(missing.to_string())).unwrap();
        let outside = StructureData::load_for_r4s(&h3r4, u64::from(region), &ring).unwrap();
        assert_eq!(outside.set().indexes.len(), ring.len() - 1);

        // Missing REGION cell: never admissible.
        std::fs::remove_file(
            h3r4.join(region.to_string())
                .join(noise_compute::propagation::structure_cell_file::CELL_STRUCTURE_FILENAME),
        )
        .unwrap();
        assert!(
            StructureData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "a missing REGION table must fail the region"
        );

        // Corrupt table in a present cell: hard error.
        std::fs::write(
            h3r4.join(region.to_string())
                .join(noise_compute::propagation::structure_cell_file::CELL_STRUCTURE_FILENAME),
            b"garbage",
        )
        .unwrap();
        assert!(
            StructureData::load_for_r4s(&h3r4, u64::from(region), &ring).is_err(),
            "a corrupt table must fail the region build"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

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
}

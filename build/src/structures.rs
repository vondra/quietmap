//! dev4 `structures.arrow` (structures_v5, z30 grid) as screening outlines on the global step
//! lattice, read as dev4's obstacle index reads it (`structure_store.rs`).
//!
//! A row screens when it has a geometry and a positive height: a building (kind 0) with every
//! ring of every part, holes too, its height capped by the low-profile rule and clamped at 828 m;
//! a wall (kind 1) as its polyline, never capped ([`crate::screening`]). Rows without geometry
//! only emit; the rows of a square must number their geometries densely from 0. A footprint's
//! id is `square.x << 48 | square.y << 32 | screening_ordinal`, the square being the one whose
//! structures.arrow holds the row (dev4 gives each footprint to the square of its centroid), so
//! the id is global and every tile storing an outline stores the same one.

use crate::dev4::{Dev4, Square, require_stamp, z30_corner_degrees, z30_to_global};
use crate::low_profile::{LowProfileLookup, height_is_per_building, ring_area_m2};
use crate::screening::{RowGeometry, ScreeningOutline, row_outlines};
use arrow_array::{
    Array, BinaryArray, Float32Array, Int16Array, Int32Array, Int64Array, RecordBatch, UInt8Array,
    UInt32Array,
};
use tiles::geo::{GlobalSteps, STEPS_PER_TILE, TILES_PER_AXIS};
use tiles::obstacles::EnvelopeClass;

const KIND_BUILDING: u8 = 0;
const KIND_WALL: u8 = 1;
/// dev4 `height_source` of an unmapped noise wall, stored at its country's mean height.
const HEIGHT_SOURCE_WALL_DEFAULT: u8 = 8;
/// The one height of an unmapped wall (PLAN-z13 SIMPLIFY).
const UNMAPPED_WALL_HEIGHT_M: f32 = 4.0;
const WORLD_STEPS: i64 = STEPS_PER_TILE as i64 * TILES_PER_AXIS as i64;

/// A ring or wall as stored: z30 (x east, y north) pairs.
type Z30Ring = Vec<(i32, i32)>;

/// The global id of a row's footprint: its square and its ordinal there.
pub fn footprint_id(square: Square, screening_ordinal: u32) -> u64 {
    u64::from(square.x) << 48 | u64::from(square.y) << 32 | u64::from(screening_ordinal)
}

/// The copy of a global position whose x lies nearest `reference_x` (outlines run continuously
/// across the antimeridian).
fn nearest_copy(global: GlobalSteps, reference_x: i64) -> GlobalSteps {
    let copies = (reference_x - global.x + WORLD_STEPS / 2).div_euclid(WORLD_STEPS);
    GlobalSteps {
        x: global.x + copies * WORLD_STEPS,
        y: global.y,
    }
}

/// A footprint's area (m2): its parts' exterior rings less their holes; `None` on incomplete
/// topology.
pub(crate) fn footprint_area_m2(bytes: &[u8]) -> Option<f64> {
    let area = decode_parts(bytes)?
        .iter()
        .flat_map(|rings| {
            rings.iter().enumerate().map(|(index, ring)| {
                let area = crate::sources::cells::ring_area_m2(ring).unwrap_or(0.0);
                if index == 0 { area } else { -area }
            })
        })
        .sum::<f64>();
    (area > 0.0).then_some(area)
}

/// dev4 `decode_grid_polygons`: u32 parts; per part u32 rings; per ring u32 points and that many
/// (i32 x east, i32 y north) z30 pairs, exterior first. `None` on incomplete topology.
pub(crate) fn decode_parts(mut bytes: &[u8]) -> Option<Vec<Vec<Z30Ring>>> {
    fn count(bytes: &mut &[u8]) -> Option<usize> {
        let value = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        *bytes = &bytes[4..];
        Some(value)
    }
    let part_count = count(&mut bytes)?;
    if part_count == 0 || part_count > bytes.len() / 4 {
        return None;
    }
    let mut parts = Vec::with_capacity(part_count);
    for _ in 0..part_count {
        let ring_count = count(&mut bytes)?;
        if ring_count == 0 || ring_count > bytes.len() / 4 {
            return None;
        }
        let mut rings = Vec::with_capacity(ring_count);
        for _ in 0..ring_count {
            let point_count = count(&mut bytes)?;
            if point_count < 3 || point_count > bytes.len() / 8 {
                return None;
            }
            rings.push(pairs(&bytes[..8 * point_count]));
            bytes = &bytes[8 * point_count..];
        }
        parts.push(rings);
    }
    bytes.is_empty().then_some(parts)
}

/// dev4 `decode_grid_poly` for a wall: u32 points (two or more) and the z30 pairs, nothing else.
fn decode_wall(bytes: &[u8]) -> Option<Z30Ring> {
    let point_count = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    (point_count >= 2 && bytes.len() == 4 + 8 * point_count).then(|| pairs(&bytes[4..]))
}

fn pairs(bytes: &[u8]) -> Z30Ring {
    let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    (0..bytes.len() / 8)
        .map(|i| (word(8 * i), word(8 * i + 4)))
        .collect()
}

/// The columns of one structures.arrow batch this builder reads.
struct Rows<'b> {
    kind: &'b UInt8Array,
    geom: &'b BinaryArray,
    height: &'b Int16Array,
    height_source: &'b UInt8Array,
    envelope: &'b UInt8Array,
    ordinal: &'b UInt32Array,
    osm_id: &'b Int64Array,
    building_type: &'b UInt8Array,
    area: &'b Float32Array,
    centroid: [&'b Int32Array; 2],
    emission_centroid: [&'b Int32Array; 2],
}

impl<'b> Rows<'b> {
    fn of(batch: &'b RecordBatch) -> Result<Self, String> {
        fn column<'b, T: 'static>(batch: &'b RecordBatch, name: &str) -> Result<&'b T, String> {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<T>())
                .ok_or_else(|| format!("column {name} missing or of another type"))
        }
        let rows = Rows {
            kind: column(batch, "kind")?,
            geom: column(batch, "geom")?,
            height: column(batch, "height_m")?,
            height_source: column(batch, "height_source")?,
            envelope: column(batch, "envelope_class")?,
            ordinal: column(batch, "screening_ordinal")?,
            osm_id: column(batch, "osm_id")?,
            building_type: column(batch, "building_type")?,
            area: column(batch, "area_m2")?,
            centroid: [column(batch, "centroid_gx")?, column(batch, "centroid_gy")?],
            emission_centroid: [
                column(batch, "emission_centroid_gx")?,
                column(batch, "emission_centroid_gy")?,
            ],
        };
        if rows.height.null_count() != 0 || rows.height.values().iter().any(|&h| h < 0) {
            return Err("height_m must be non-null and non-negative".into());
        }
        Ok(rows)
    }

    fn position(columns: [&Int32Array; 2], row: usize) -> Option<(i32, i32)> {
        (!columns[0].is_null(row) && !columns[1].is_null(row))
            .then(|| (columns[0].value(row), columns[1].value(row)))
    }

    /// The low-profile seeds of this batch (dev4 `low_profile_from_structures`): OSM building
    /// rows that stand (a geometry), at their emission centroid where the merge kept one, else at
    /// the screening centroid.
    fn seed(&self, lookup: &mut LowProfileLookup) {
        for row in 0..self.kind.len() {
            if self.kind.value(row) != KIND_BUILDING
                || self.osm_id.is_null(row)
                || self.building_type.is_null(row)
                || self.area.is_null(row)
                || self.geom.is_null(row)
            {
                continue;
            }
            let position = Self::position(self.emission_centroid, row)
                .or_else(|| Self::position(self.centroid, row));
            if let Some((gx, gy)) = position {
                let (lat, lon) = z30_corner_degrees(gx, gy);
                lookup.insert_if_low(
                    self.building_type.value(row),
                    lat,
                    lon,
                    self.area.value(row),
                );
            }
        }
    }

    /// A building row's screening height: the low-profile cap applies where it has a centroid,
    /// with the largest exterior ring's area when the row has none.
    fn building_height(
        &self,
        row: usize,
        parts: &[Vec<Z30Ring>],
        lookup: &LowProfileLookup,
    ) -> Result<f32, String> {
        if self.height_source.is_null(row) {
            return Err(format!("row {row} has no height_source"));
        }
        let raw_height = f32::from(self.height.value(row));
        let Some((gx, gy)) = Self::position(self.centroid, row) else {
            return Ok(raw_height);
        };
        let (lat, lon) = z30_corner_degrees(gx, gy);
        let area_m2 = if self.area.is_null(row) {
            parts
                .iter()
                .map(|rings| ring_area_m2(&rings[0]))
                .fold(0.0, f64::max) as f32
        } else {
            self.area.value(row)
        };
        let per_building = height_is_per_building(self.height_source.value(row));
        Ok(lookup.capped_height(raw_height, per_building, lat, lon, area_m2))
    }

    /// One geometry row's screening height and geometry, vertices through `steps`.
    fn geometry(
        &self,
        row: usize,
        lookup: &LowProfileLookup,
        steps: impl Fn((i32, i32)) -> GlobalSteps,
    ) -> Result<(f32, RowGeometry), String> {
        let lattice = |ring: Z30Ring| ring.into_iter().map(&steps).collect();
        match self.kind.value(row) {
            KIND_BUILDING => {
                let parts = decode_parts(self.geom.value(row))
                    .ok_or_else(|| format!("row {row} has invalid building topology"))?;
                let height_m = self.building_height(row, &parts, lookup)?;
                let envelope = (!self.envelope.is_null(row))
                    .then(|| EnvelopeClass::from_code(self.envelope.value(row)))
                    .flatten()
                    .unwrap_or(EnvelopeClass::Default);
                let parts = parts
                    .into_iter()
                    .map(|rings| rings.into_iter().map(lattice).collect())
                    .collect();
                Ok((height_m, RowGeometry::Building { envelope, parts }))
            }
            KIND_WALL => {
                let points = decode_wall(self.geom.value(row))
                    .ok_or_else(|| format!("row {row} has invalid wall geometry"))?;
                let height_m = wall_height_m(self.height_source.value(row), self.height.value(row));
                Ok((height_m, RowGeometry::Wall(lattice(points))))
            }
            other => Err(format!("row {row} has unknown structure kind {other}")),
        }
    }
}

/// A wall's screening height: an unmapped wall stands 4 m everywhere (PLAN-z13 SIMPLIFY; dev4
/// stored its country's mean), any other its stored height.
fn wall_height_m(height_source: u8, stored_m: i16) -> f32 {
    if height_source == HEIGHT_SOURCE_WALL_DEFAULT {
        UNMAPPED_WALL_HEIGHT_M
    } else {
        f32::from(stored_m)
    }
}

/// The outlines of one square's structures.arrow for which `keep` holds, x taken on the copy of
/// the world nearest `reference_x`. A square without the file has no structures.
pub fn read_square(
    dev4: &Dev4,
    square: Square,
    reference_x: i64,
    keep: &dyn Fn(&[GlobalSteps]) -> bool,
) -> Result<Vec<ScreeningOutline>, String> {
    let path = dev4.prepared_file(square, "structures.arrow");
    let context = |error: String| format!("{}: {error}", path.display());
    let Some(table) = dev4.table(square, "structures.arrow")? else {
        return Ok(Vec::new());
    };
    require_stamp(&table, "structures_contract", "structures_v5").map_err(context)?;
    require_stamp(&table, "grid", "z30").map_err(context)?;
    let batches = table
        .batches
        .iter()
        .map(Rows::of)
        .collect::<Result<Vec<_>, _>>()
        .map_err(context)?;
    // The lookup is complete before any row is capped: the match is spatial.
    let mut lookup = LowProfileLookup::default();
    batches.iter().for_each(|rows| rows.seed(&mut lookup));
    let steps = |(gx, gy): (i32, i32)| nearest_copy(z30_to_global(gx, gy), reference_x);
    let (mut outlines, mut row_outlines_scratch, mut ordinals) =
        (Vec::new(), Vec::new(), Vec::new());
    for rows in &batches {
        for row in (0..rows.kind.len()).filter(|&row| !rows.geom.is_null(row)) {
            if rows.ordinal.is_null(row) {
                return Err(context(format!(
                    "row {row} has geometry but no screening_ordinal"
                )));
            }
            let ordinal = rows.ordinal.value(row);
            ordinals.push(ordinal);
            let (height_m, geometry) = rows.geometry(row, &lookup, steps).map_err(context)?;
            row_outlines(
                footprint_id(square, ordinal),
                f64::from(height_m),
                geometry,
                &mut row_outlines_scratch,
            );
            outlines.extend(
                row_outlines_scratch
                    .drain(..)
                    .filter(|outline| keep(&outline.vertices)),
            );
        }
    }
    ordinals.sort_unstable();
    if ordinals
        .iter()
        .enumerate()
        .any(|(index, &ordinal)| ordinal as usize != index)
    {
        return Err(context(
            "screening ordinals of geometry rows are not dense from 0".into(),
        ));
    }
    Ok(outlines)
}

#[cfg(test)]
#[path = "structures_tests.rs"]
mod tests;

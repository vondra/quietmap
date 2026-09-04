//! Read `leisure.arrow` for a set of H3 R4 hex cells into per-point
//! [`PointRow`]s — leisure AREA sources (sports pitch / playground / pool /
//! beer garden) that FOLD into the building layer (one HM3 source id, one
//! frontend toggle). Rows discretise via [`prepare_leisure_points`] into the
//! SAME [`PointRow`] stream as the building emission rows. The building layer's
//! concatenation rule — a cell's structures.arrow emission rows first, then its
//! leisure rows — lives in
//! [`crate::source_loader_structure::StructureData::take_building_layer_rows`];
//! keeping the rows grouped per ring cell here is what makes that order
//! reproducible (f32 accumulation order is part of the painted bytes).

use std::path::Path;

use anyhow::Result;
use arrow::array::{BinaryArray, Float32Array, Float64Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use noise_compute::normalize::{prepare_leisure_points, RawLeisureInput};

use crate::schema_check::{read_surface_arrow_for_r4_with_contract, LEISURE_CONTRACT_V1};
use crate::source_line::opt;
use crate::source_loader_industrial::{hex_encode, pos_f32};
use crate::source_point::PointRow;

pub struct LeisureData {
    /// One entry per ring cell, in the `r4_hexes` order the structure
    /// loader's emission rows share.
    rows_by_cell: Vec<Vec<PointRow>>,
}

impl LeisureData {
    /// Load + discretise every `leisure.arrow` row across `r4_hexes`. Missing
    /// files are skipped (R4s with no leisure).
    pub fn load_for_r4s(h3r4_dir: &Path, r4_hexes: &[u64]) -> Result<Self> {
        let mut rows_by_cell = Vec::with_capacity(r4_hexes.len());
        for &r4 in r4_hexes {
            let mut rows = Vec::new();
            read_surface_arrow_for_r4_with_contract(
                h3r4_dir,
                r4,
                "leisure.arrow",
                "leisure_contract",
                LEISURE_CONTRACT_V1,
                |batch| absorb_leisure_batch(batch, &mut rows),
            )?;
            rows_by_cell.push(rows);
        }
        Ok(Self { rows_by_cell })
    }

    pub fn into_rows_by_cell(self) -> Vec<Vec<PointRow>> {
        self.rows_by_cell
    }
}

/// Discretise one `leisure.arrow` batch into the building-layer point stream.
fn absorb_leisure_batch(batch: &RecordBatch, out: &mut Vec<PointRow>) -> Result<()> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(());
    }
    let (Some(clat), Some(clon)) = (
        opt::<Float64Array>(batch, "centroid_lat"),
        opt::<Float64Array>(batch, "centroid_lon"),
    ) else {
        return Ok(());
    };
    let sport = opt::<UInt8Array>(batch, "sport");
    let area = opt::<Float32Array>(batch, "area_m2");
    let wkb = opt::<BinaryArray>(batch, "polygon_wkb");

    for i in 0..n {
        // Null WKB (a leisure NODE without a footprint) reads as empty bytes
        // → centroid-only source in prepare_leisure_points (mirrors the building
        // path's lenient `value(i)` read).
        let wkb_hex = wkb.map(|a| hex_encode(a.value(i))).unwrap_or_default();
        let points = prepare_leisure_points(RawLeisureInput {
            centroid_lat: clat.value(i),
            centroid_lon: clon.value(i),
            sport: sport.map(|a| a.value(i)).unwrap_or(0),
            area_m2: area.and_then(|a| pos_f32(a.value(i)).map(f64::from)),
            polygon_wkb: &wkb_hex,
        });
        out.extend(points.iter().map(PointRow::from_prepared));
    }
    Ok(())
}

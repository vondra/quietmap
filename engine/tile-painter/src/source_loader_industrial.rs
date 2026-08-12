//! Read `industrial.arrow` for a set of H3 R4 hex cells into per-point
//! [`PointRow`]s with pre-computed per-period emission — the point-source
//! analogue of [`crate::source_loader_road`]. One on-disk row is one
//! industrial site (or wind turbine); [`prepare_industrial_points`] resolves
//! its emission profile (NACE → subtype → site-type), splits a large polygon
//! into weighted area cells (`Lw + 10·log10(cell_area / total_area)`), and sets
//! the self-screening exclusion radius `√(cell_area/π)` plus a per-cell reach
//! derived from that cell's loudest day band. This is the exact chain the popup
//! runs (`source-reader::lib`), so the scatter matches `compute_point_sources`
//! to quantisation noise.
//!
//! Column reads + the wind-turbine (`source_type == 10`) blank-WKB rule mirror
//! the popup reader; industrial has no admin-dependent defaults.

use std::path::Path;

use anyhow::Result;
use arrow::array::{BinaryArray, Float32Array, Float64Array, UInt16Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use noise_compute::normalize::{prepare_industrial_points, RawIndustrialInput};

use crate::source_line::opt;
use crate::source_point::PointRow;

pub struct IndustrialData {
    rows: Vec<PointRow>,
}

impl IndustrialData {
    /// Load + discretise every `industrial.arrow` row across `r4_hexes`.
    /// Missing files are skipped (R4s with no industry).
    pub fn load_for_r4s(h3r4_dir: &Path, r4_hexes: &[u64]) -> Result<Self> {
        let mut rows = Vec::new();
        for &r4 in r4_hexes {
            crate::schema_check::read_surface_arrow_for_r4(
                h3r4_dir,
                r4,
                "industrial.arrow",
                |batch| absorb_batch(batch, &mut rows),
            )?;
        }
        Ok(Self { rows })
    }

    pub fn into_rows(self) -> Vec<PointRow> {
        self.rows
    }
}

fn absorb_batch(batch: &RecordBatch, out: &mut Vec<PointRow>) -> Result<()> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(());
    }
    // Centroid is required; everything else defaults (popup-lenient reads).
    let (Some(clat), Some(clon)) = (
        opt::<Float64Array>(batch, "centroid_lat"),
        opt::<Float64Array>(batch, "centroid_lon"),
    ) else {
        return Ok(());
    };
    let stype = opt::<UInt8Array>(batch, "source_type");
    let subtype = opt::<UInt8Array>(batch, "site_subtype");
    let hub = opt::<Float32Array>(batch, "hub_height");
    let power = opt::<Float32Array>(batch, "rated_power_kw");
    let area = opt::<Float32Array>(batch, "area_m2");
    let nace = opt::<UInt16Array>(batch, "nace_4digit");
    let wkb = opt::<BinaryArray>(batch, "polygon_wkb");
    let suppressed = opt::<UInt8Array>(batch, "suppressed");

    for i in 0..n {
        // I-07 dedup: a `suppressed` row is a same-site duplicate the enricher
        // collapsed (E-PRTR kept, GPPD/GEM dropped) — skip so the site emits once.
        if suppressed.map(|a| a.value(i)).unwrap_or(0) != 0 {
            continue;
        }
        let st = stype.map(|a| a.value(i)).unwrap_or(0);
        // Wind turbines (source_type 10) carry no footprint polygon.
        let wkb_hex = if st == 10 {
            String::new()
        } else {
            wkb.map(|a| hex_encode(a.value(i))).unwrap_or_default()
        };
        let points = prepare_industrial_points(RawIndustrialInput {
            centroid_lat: clat.value(i),
            centroid_lon: clon.value(i),
            source_type: st,
            site_subtype: subtype.map(|a| a.value(i)).unwrap_or(0),
            hub_height_m: hub.and_then(|a| pos_f32(a.value(i))),
            rated_power_kw: power.and_then(|a| pos_f32(a.value(i))),
            area_m2: area.and_then(|a| pos_f32(a.value(i)).map(f64::from)),
            polygon_wkb: &wkb_hex,
            nace_4digit: nace.map(|a| a.value(i)).filter(|&v| v > 0),
        });
        out.extend(points.iter().map(PointRow::from_prepared));
    }
    Ok(())
}

/// `Some(v)` iff positive — matches the popup's "0 → absent → default" reads.
/// `pub` so the parity validator shares one canonical helper (no drift).
#[inline]
pub fn pos_f32(v: f32) -> Option<f32> {
    (v > 0.0).then_some(v)
}

/// Lowercase hex of a WKB byte slice; matches `source-reader::hex_encode` so
/// `prepare_industrial_points`' polygon grid points are byte-identical. `pub`
/// so the parity validator (a separate bin crate) builds `PointSource`s the
/// same way — one shared encoder, no drift.
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

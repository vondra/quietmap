//! Read v6 `airborne.arrow` files for a set of H3 R4 hex cells and
//! expose them as [`AirborneRowView`] slices for the heatmap kernel.
//! Mirrors the loader pattern in `source_loader.rs` / `_traffic.rs`
//! and the column shape from
//! `source-reader/src/aircraft_v6/airborne_view.rs`.

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use arrow::array::{
    Array, FixedSizeBinaryArray, Float32Array, Int16Array, ListArray, StringArray, StructArray,
    UInt64Array, UInt8Array,
};
use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{AirborneRowView, BBox, SubSegmentSlice};

pub struct AirborneData {
    rows: Vec<OwnedRow>,
}

struct OwnedRow {
    flight_id: u64,
    callsign: String,
    aircraft_type: [u8; 4],
    profile_idx: u8,
    source_id: u8,
    origin: u8,
    bbox: BBox,
    sub_start_lat: Vec<f32>,
    sub_start_lon: Vec<f32>,
    sub_start_alt_m: Vec<f32>,
    sub_end_lat: Vec<f32>,
    sub_end_lon: Vec<f32>,
    sub_end_alt_m: Vec<f32>,
    sub_speed_kt: Vec<f32>,
    sub_length_m: Vec<f32>,
    sub_period: Vec<u8>,
    sub_date_id: Vec<i16>,
    sub_flags: Vec<u8>,
    /// Only start/end terrain elevations are stored. Heatmap and popup feed
    /// `SegmentTerrain` with zeroed q1/mid/q3 and retain the endpoint-based
    /// checks; the removed chord check remains the SPEC §5 known gap.
    sub_terrain_start_elev_m: Vec<f32>,
    sub_terrain_end_elev_m: Vec<f32>,
}

impl AirborneData {
    pub fn empty() -> Self {
        Self { rows: Vec::new() }
    }

    pub fn load_for_r4s(h3r4_dir: &Path, r4_hexes: &[u64]) -> Result<Self> {
        let mut rows = Vec::new();
        for &r4 in r4_hexes {
            crate::schema_check::read_arrow_for_r4(
                h3r4_dir,
                r4,
                "airborne.arrow",
                crate::schema_check::check_airborne_contract,
                |batch| absorb_batch(batch, &mut rows),
            )?;
        }
        Ok(Self { rows })
    }

    pub fn views(&self) -> Vec<AirborneRowView<'_>> {
        self.rows
            .iter()
            .map(|r| AirborneRowView {
                flight_id: r.flight_id,
                callsign: &r.callsign,
                aircraft_type: r.aircraft_type,
                profile_idx: r.profile_idx,
                source_id: r.source_id,
                origin: r.origin,
                sub_segments: SubSegmentSlice {
                    start_lat: &r.sub_start_lat,
                    start_lon: &r.sub_start_lon,
                    start_alt_m: &r.sub_start_alt_m,
                    end_lat: &r.sub_end_lat,
                    end_lon: &r.sub_end_lon,
                    end_alt_m: &r.sub_end_alt_m,
                    speed_kt: &r.sub_speed_kt,
                    length_m: &r.sub_length_m,
                    period: &r.sub_period,
                    date_id: &r.sub_date_id,
                    flags: &r.sub_flags,
                    terrain_start_elev_m: &r.sub_terrain_start_elev_m,
                    terrain_end_elev_m: &r.sub_terrain_end_elev_m,
                },
                bbox: r.bbox,
            })
            .collect()
    }

    pub fn n_rows(&self) -> usize {
        self.rows.len()
    }
}

fn absorb_batch(batch: &RecordBatch, rows: &mut Vec<OwnedRow>) -> Result<()> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(());
    }
    let fid = col_u64(batch, "flight_id")?;
    let callsign = col_str(batch, "callsign")?;
    let typecode = batch
        .column_by_name("aircraft_type")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeBinaryArray>())
        .ok_or_else(|| anyhow!("missing or wrong-typed column aircraft_type"))?;
    if typecode.value_length() != 4 {
        bail!(
            "aircraft_type expected FixedSizeBinary(4), got FixedSizeBinary({}) — \
             copy_from_slice would panic on per-row read",
            typecode.value_length()
        );
    }
    let profile_idx = col_u8(batch, "profile_idx")?;
    let source_id = col_u8(batch, "source_id")?;
    let origin = col_u8(batch, "origin")?;
    let bbox_min_lat = col_f32(batch, "bbox_min_lat")?;
    let bbox_max_lat = col_f32(batch, "bbox_max_lat")?;
    let bbox_min_lon = col_f32(batch, "bbox_min_lon")?;
    let bbox_max_lon = col_f32(batch, "bbox_max_lon")?;
    let sub_list = col_list(batch, "sub_segments")?;

    for i in 0..n {
        let mut typebuf = [0u8; 4];
        typebuf.copy_from_slice(typecode.value(i));
        let bbox = BBox {
            min_lat: bbox_min_lat.value(i),
            max_lat: bbox_max_lat.value(i),
            min_lon: bbox_min_lon.value(i),
            max_lon: bbox_max_lon.value(i),
        };
        let sub_arr = sub_list.value(i);
        let sub_struct = sub_arr
            .as_any()
            .downcast_ref::<StructArray>()
            .ok_or_else(|| anyhow!("sub_segments[{i}] not a StructArray"))?;
        let sub = decode_sub_segments(sub_struct)?;
        rows.push(OwnedRow {
            flight_id: fid.value(i),
            callsign: callsign.value(i).to_string(),
            aircraft_type: typebuf,
            profile_idx: profile_idx.value(i),
            source_id: source_id.value(i),
            origin: origin.value(i),
            bbox,
            sub_start_lat: sub.start_lat,
            sub_start_lon: sub.start_lon,
            sub_start_alt_m: sub.start_alt_m,
            sub_end_lat: sub.end_lat,
            sub_end_lon: sub.end_lon,
            sub_end_alt_m: sub.end_alt_m,
            sub_speed_kt: sub.speed_kt,
            sub_length_m: sub.length_m,
            sub_period: sub.period,
            sub_date_id: sub.date_id,
            sub_flags: sub.flags,
            sub_terrain_start_elev_m: sub.terrain_start_elev_m,
            sub_terrain_end_elev_m: sub.terrain_end_elev_m,
        });
    }
    Ok(())
}

struct OwnedSubColumns {
    start_lat: Vec<f32>,
    start_lon: Vec<f32>,
    start_alt_m: Vec<f32>,
    end_lat: Vec<f32>,
    end_lon: Vec<f32>,
    end_alt_m: Vec<f32>,
    speed_kt: Vec<f32>,
    length_m: Vec<f32>,
    period: Vec<u8>,
    date_id: Vec<i16>,
    flags: Vec<u8>,
    terrain_start_elev_m: Vec<f32>,
    terrain_end_elev_m: Vec<f32>,
}

fn decode_sub_segments(s: &StructArray) -> Result<OwnedSubColumns> {
    let take_f32 = |name: &str| -> Result<Vec<f32>> {
        let col = s
            .column_by_name(name)
            .ok_or_else(|| anyhow!("missing sub-seg column {name}"))?;
        let arr = col
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow!("sub-seg column {name} not Float32"))?;
        Ok(arr.values().to_vec())
    };
    let take_u8 = |name: &str| -> Result<Vec<u8>> {
        let col = s
            .column_by_name(name)
            .ok_or_else(|| anyhow!("missing sub-seg column {name}"))?;
        let arr = col
            .as_any()
            .downcast_ref::<UInt8Array>()
            .ok_or_else(|| anyhow!("sub-seg column {name} not UInt8"))?;
        Ok(arr.values().to_vec())
    };
    let take_i16 = |name: &str| -> Result<Vec<i16>> {
        let col = s
            .column_by_name(name)
            .ok_or_else(|| anyhow!("missing sub-seg column {name}"))?;
        let arr = col
            .as_any()
            .downcast_ref::<Int16Array>()
            .ok_or_else(|| anyhow!("sub-seg column {name} not Int16"))?;
        Ok(arr.values().to_vec())
    };
    Ok(OwnedSubColumns {
        start_lat: take_f32("start_lat")?,
        start_lon: take_f32("start_lon")?,
        start_alt_m: take_f32("start_alt_m")?,
        end_lat: take_f32("end_lat")?,
        end_lon: take_f32("end_lon")?,
        end_alt_m: take_f32("end_alt_m")?,
        speed_kt: take_f32("speed_kt")?,
        length_m: take_f32("length_m")?,
        period: take_u8("period")?,
        date_id: take_i16("date_id")?,
        flags: take_u8("flags")?,
        terrain_start_elev_m: take_f32("terrain_start_elev_m")?,
        terrain_end_elev_m: take_f32("terrain_end_elev_m")?,
    })
}

fn col_u64<'a>(b: &'a RecordBatch, n: &str) -> Result<&'a UInt64Array> {
    b.column_by_name(n)
        .and_then(|c| c.as_any().downcast_ref())
        .ok_or_else(|| anyhow!("missing or wrong-typed column {n}"))
}
fn col_u8<'a>(b: &'a RecordBatch, n: &str) -> Result<&'a UInt8Array> {
    b.column_by_name(n)
        .and_then(|c| c.as_any().downcast_ref())
        .ok_or_else(|| anyhow!("missing or wrong-typed column {n}"))
}
fn col_f32<'a>(b: &'a RecordBatch, n: &str) -> Result<&'a Float32Array> {
    b.column_by_name(n)
        .and_then(|c| c.as_any().downcast_ref())
        .ok_or_else(|| anyhow!("missing or wrong-typed column {n}"))
}
fn col_str<'a>(b: &'a RecordBatch, n: &str) -> Result<&'a StringArray> {
    b.column_by_name(n)
        .and_then(|c| c.as_any().downcast_ref())
        .ok_or_else(|| anyhow!("missing or wrong-typed column {n}"))
}
fn col_list<'a>(b: &'a RecordBatch, n: &str) -> Result<&'a ListArray> {
    b.column_by_name(n)
        .and_then(|c| c.as_any().downcast_ref())
        .ok_or_else(|| anyhow!("missing or wrong-typed column {n}"))
}

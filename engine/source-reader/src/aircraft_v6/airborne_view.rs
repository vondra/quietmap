//! Zero-copy airborne row views. `AirborneRowAccum<'a>` borrows
//! directly into the `Arc<RecordBatch>` arrow buffers passed in — no
//! per-row `Vec<f32>` clones, no per-row `String::from(callsign)`. The
//! lifetime `'a` is tied to the caller's `&[RecordBatch]`, which the
//! popup path keeps alive across `add_v6_aircraft_to_result` via the
//! `PointQueryData` struct (lib.rs:444). Arrow buffers behind
//! `RecordBatch` are Arc-backed, so cloning batches is a refcount bump
//! and the underlying f32/u8 slices stay resident for the whole call.
//!
//! At LKPR, roughly 21 k rows × 11 columns × avg 200
//! sub-segs/row = ~46 M f32 copies eliminated per popup. `aircraft_type`
//! is now `[u8; 4]` by value on `AirborneRowView` (4 bytes vs an `&[u8; 4]`
//! borrow that would otherwise need to alias into a self-owned Vec).

use arrow::array::*;
use noise_compute::compute::aircraft_v6::{AirborneRowView, BBox, SubSegmentSlice};

use super::columns::{col_f32, col_fixed_size_binary, col_list, col_str, col_u64, col_u8};

pub struct AirborneRowAccum<'a> {
    rows: Vec<AirborneRowView<'a>>,
}

struct SubSegmentColumns<'a> {
    start_lat: &'a [f32],
    start_lon: &'a [f32],
    start_alt_m: &'a [f32],
    end_lat: &'a [f32],
    end_lon: &'a [f32],
    end_alt_m: &'a [f32],
    speed_kt: &'a [f32],
    length_m: &'a [f32],
    period: &'a [u8],
    date_id: &'a [i16],
    flags: &'a [u8],
    terrain_start_elev_m: &'a [f32],
    terrain_end_elev_m: &'a [f32],
}

impl<'a> SubSegmentColumns<'a> {
    fn from_struct(s: &'a StructArray) -> Option<Self> {
        let f32_col = |name: &str| -> Option<&'a [f32]> {
            s.column_by_name(name)?
                .as_any()
                .downcast_ref::<Float32Array>()
                .map(|a| a.values().as_ref())
        };
        let u8_col = |name: &str| -> Option<&'a [u8]> {
            s.column_by_name(name)?
                .as_any()
                .downcast_ref::<UInt8Array>()
                .map(|a| a.values().as_ref())
        };
        let i16_col = |name: &str| -> Option<&'a [i16]> {
            s.column_by_name(name)?
                .as_any()
                .downcast_ref::<Int16Array>()
                .map(|a| a.values().as_ref())
        };
        Some(SubSegmentColumns {
            start_lat: f32_col("start_lat")?,
            start_lon: f32_col("start_lon")?,
            start_alt_m: f32_col("start_alt_m")?,
            end_lat: f32_col("end_lat")?,
            end_lon: f32_col("end_lon")?,
            end_alt_m: f32_col("end_alt_m")?,
            speed_kt: f32_col("speed_kt")?,
            length_m: f32_col("length_m")?,
            period: u8_col("period")?,
            date_id: i16_col("date_id")?,
            flags: u8_col("flags")?,
            terrain_start_elev_m: f32_col("terrain_start_elev_m")?,
            terrain_end_elev_m: f32_col("terrain_end_elev_m")?,
        })
    }
}

impl<'a> AirborneRowAccum<'a> {
    pub fn new(batches: &'a [arrow::record_batch::RecordBatch]) -> Self {
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        let mut rows: Vec<AirborneRowView<'a>> = Vec::with_capacity(total_rows);
        for batch in batches {
            let n = batch.num_rows();
            if n == 0 {
                continue;
            }
            let Some(flight_id) = col_u64(batch, "flight_id") else {
                continue;
            };
            let Some(callsign) = col_str(batch, "callsign") else {
                continue;
            };
            let Some(aircraft_type) = col_fixed_size_binary(batch, "aircraft_type", 4) else {
                continue;
            };
            let Some(profile_idx) = col_u8(batch, "profile_idx") else {
                continue;
            };
            let Some(source_id) = col_u8(batch, "source_id") else {
                continue;
            };
            let origin = col_u8(batch, "origin");
            let Some(bb_min_lat) = col_f32(batch, "bbox_min_lat") else {
                continue;
            };
            let Some(bb_max_lat) = col_f32(batch, "bbox_max_lat") else {
                continue;
            };
            let Some(bb_min_lon) = col_f32(batch, "bbox_min_lon") else {
                continue;
            };
            let Some(bb_max_lon) = col_f32(batch, "bbox_max_lon") else {
                continue;
            };
            let Some(sub_list) = col_list(batch, "sub_segments") else {
                continue;
            };
            let Some(sub_struct) = sub_list.values().as_any().downcast_ref::<StructArray>() else {
                continue;
            };
            // Fail-loud on schema mismatch: `assert_schema_version` has
            // already gated v15, so any missing column here is a
            // corrupt batch (post-deploy file truncation, dev override,
            // …). Print the batch's column names + drop the batch
            // rather than silently zero-out the popup. Mirrors the
            // heatmap loader's expect-style panic on malformed batches.
            let Some(s) = SubSegmentColumns::from_struct(sub_struct) else {
                eprintln!(
                    "WARN: airborne.arrow sub_segments struct missing v15 \
                     terrain columns at batch with {} rows; dropping batch. \
                     Re-run `./scripts/run-aircraft-extract.sh` to refresh.",
                    n
                );
                continue;
            };
            let offsets = sub_list.value_offsets();
            for i in 0..n {
                let lo = offsets[i] as usize;
                let hi = offsets[i + 1] as usize;
                let mut typecode = [0u8; 4];
                typecode.copy_from_slice(aircraft_type.value(i));
                rows.push(AirborneRowView {
                    flight_id: flight_id.value(i),
                    callsign: callsign.value(i),
                    aircraft_type: typecode,
                    profile_idx: profile_idx.value(i),
                    source_id: source_id.value(i),
                    origin: origin.map(|a| a.value(i)).unwrap_or(0),
                    bbox: BBox {
                        min_lat: bb_min_lat.value(i),
                        max_lat: bb_max_lat.value(i),
                        min_lon: bb_min_lon.value(i),
                        max_lon: bb_max_lon.value(i),
                    },
                    sub_segments: SubSegmentSlice {
                        start_lat: &s.start_lat[lo..hi],
                        start_lon: &s.start_lon[lo..hi],
                        start_alt_m: &s.start_alt_m[lo..hi],
                        end_lat: &s.end_lat[lo..hi],
                        end_lon: &s.end_lon[lo..hi],
                        end_alt_m: &s.end_alt_m[lo..hi],
                        speed_kt: &s.speed_kt[lo..hi],
                        length_m: &s.length_m[lo..hi],
                        period: &s.period[lo..hi],
                        date_id: &s.date_id[lo..hi],
                        flags: &s.flags[lo..hi],
                        terrain_start_elev_m: &s.terrain_start_elev_m[lo..hi],
                        terrain_end_elev_m: &s.terrain_end_elev_m[lo..hi],
                    },
                });
            }
        }
        Self { rows }
    }

    pub fn views(&self) -> &[AirborneRowView<'a>] {
        &self.rows
    }
}

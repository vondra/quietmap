//! Borrow prepared integer geometry and flight metadata without copying segment columns.

use super::columns::required_array;
use arrow::{array::*, record_batch::RecordBatch};
use noise_compute::compute::aircraft_v6::{AirborneRowView, SubSegmentSlice};

pub struct AirborneRowAccum<'a> {
    batches: Vec<DecodedBatch<'a>>,
}

struct DecodedBatch<'a> {
    flight_id: &'a UInt64Array,
    callsign: &'a StringArray,
    aircraft_type: &'a FixedSizeBinaryArray,
    profile: &'a UInt8Array,
    source: &'a UInt8Array,
    origin: &'a UInt8Array,
    offsets: &'a [i32],
    start_gy: &'a [i32],
    start_gx: &'a [i32],
    end_gy: &'a [i32],
    end_gx: &'a [i32],
    start_alt: &'a [i16],
    end_alt: &'a [i16],
    start_terrain: &'a [i16],
    end_terrain: &'a [i16],
    speed: &'a [f32],
    length: &'a [f32],
    period: &'a [u8],
    date: &'a [i16],
    flags: &'a [u8],
}

impl<'a> AirborneRowAccum<'a> {
    pub fn new(batches: &'a [RecordBatch]) -> Result<Self, String> {
        let mut decoded = Vec::with_capacity(batches.len());
        for batch in batches {
            let list =
                required_array::<ListArray>(batch.column_by_name("sub_segments"), "sub_segments")?;
            if list
                .value_offsets()
                .windows(2)
                .any(|pair| pair[0] == pair[1])
            {
                return Err("airborne event has no sub-segment geometry".into());
            }
            let values = required_array::<StructArray>(Some(list.values()), "sub_segments.item")?;
            let flight_id =
                required_array::<UInt64Array>(batch.column_by_name("flight_id"), "flight_id")?;
            let callsign =
                required_array::<StringArray>(batch.column_by_name("callsign"), "callsign")?;
            let aircraft_type = required_array::<FixedSizeBinaryArray>(
                batch.column_by_name("aircraft_type"),
                "aircraft_type",
            )?;
            if aircraft_type.value_length() != 4 {
                return Err("airborne aircraft_type must be FixedSizeBinary(4)".into());
            }
            let profile =
                required_array::<UInt8Array>(batch.column_by_name("profile_idx"), "profile_idx")?;
            let source =
                required_array::<UInt8Array>(batch.column_by_name("source_id"), "source_id")?;
            let origin = required_array::<UInt8Array>(batch.column_by_name("origin"), "origin")?;
            decoded.push(DecodedBatch {
                flight_id,
                callsign,
                aircraft_type,
                profile,
                source,
                origin,
                offsets: list.value_offsets(),
                start_gy: required_array::<Int32Array>(
                    values.column_by_name("start_gy"),
                    "start_gy",
                )?
                .values(),
                start_gx: required_array::<Int32Array>(
                    values.column_by_name("start_gx"),
                    "start_gx",
                )?
                .values(),
                end_gy: required_array::<Int32Array>(values.column_by_name("end_gy"), "end_gy")?
                    .values(),
                end_gx: required_array::<Int32Array>(values.column_by_name("end_gx"), "end_gx")?
                    .values(),
                start_alt: required_array::<Int16Array>(
                    values.column_by_name("start_alt_m"),
                    "start_alt_m",
                )?
                .values(),
                end_alt: required_array::<Int16Array>(
                    values.column_by_name("end_alt_m"),
                    "end_alt_m",
                )?
                .values(),
                start_terrain: required_array::<Int16Array>(
                    values.column_by_name("terrain_start_elev_m"),
                    "terrain_start_elev_m",
                )?
                .values(),
                end_terrain: required_array::<Int16Array>(
                    values.column_by_name("terrain_end_elev_m"),
                    "terrain_end_elev_m",
                )?
                .values(),
                speed: required_array::<Float32Array>(
                    values.column_by_name("speed_kt"),
                    "speed_kt",
                )?
                .values(),
                length: required_array::<Float32Array>(
                    values.column_by_name("length_m"),
                    "length_m",
                )?
                .values(),
                period: required_array::<UInt8Array>(values.column_by_name("period"), "period")?
                    .values(),
                date: required_array::<Int16Array>(values.column_by_name("date_id"), "date_id")?
                    .values(),
                flags: required_array::<UInt8Array>(values.column_by_name("flags"), "flags")?
                    .values(),
            });
        }
        Ok(Self { batches: decoded })
    }

    pub fn views(&self) -> Vec<AirborneRowView<'_>> {
        self.batches
            .iter()
            .flat_map(|batch| (0..batch.flight_id.len()).map(|row| batch.view(row)))
            .collect()
    }
}

impl DecodedBatch<'_> {
    fn view(&self, row: usize) -> AirborneRowView<'_> {
        let range = self.offsets[row] as usize..self.offsets[row + 1] as usize;
        let sub_segments = SubSegmentSlice {
            start_gy: &self.start_gy[range.clone()],
            start_gx: &self.start_gx[range.clone()],
            end_gy: &self.end_gy[range.clone()],
            end_gx: &self.end_gx[range.clone()],
            start_alt_m: &self.start_alt[range.clone()],
            end_alt_m: &self.end_alt[range.clone()],
            terrain_start_elev_m: &self.start_terrain[range.clone()],
            terrain_end_elev_m: &self.end_terrain[range.clone()],
            speed_kt: &self.speed[range.clone()],
            length_m: &self.length[range.clone()],
            period: &self.period[range.clone()],
            date_id: &self.date[range.clone()],
            flags: &self.flags[range],
        };
        AirborneRowView {
            flight_id: self.flight_id.value(row),
            callsign: self.callsign.value(row),
            aircraft_type: self
                .aircraft_type
                .value(row)
                .try_into()
                .expect("checked width"),
            profile_idx: self.profile.value(row),
            source_id: self.source.value(row),
            origin: self.origin.value(row),
            bbox: sub_segments.bbox(),
            sub_segments,
        }
    }
}

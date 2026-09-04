//! Decode integer aircraft geometry once per Arrow batch and borrow flight metadata.

use super::columns::required_array;
use arrow::{array::*, record_batch::RecordBatch};
use noise_compute::compute::aircraft_v6::{AirborneRowView, BBox, SubSegmentSlice};

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
    start_lat: Vec<f32>,
    start_lon: Vec<f32>,
    end_lat: Vec<f32>,
    end_lon: Vec<f32>,
    start_alt: Vec<f32>,
    end_alt: Vec<f32>,
    start_terrain: Vec<f32>,
    end_terrain: Vec<f32>,
    speed: &'a [f32],
    length: &'a [f32],
    period: &'a [u8],
    date: &'a [i16],
    flags: &'a [u8],
}

fn coordinates(values: &StructArray, prefix: &str) -> Result<(Vec<f32>, Vec<f32>), String> {
    let x_name = format!("{prefix}_gx");
    let y_name = format!("{prefix}_gy");
    let xs = required_array::<Int32Array>(values.column_by_name(&x_name), &x_name)?;
    let ys = required_array::<Int32Array>(values.column_by_name(&y_name), &y_name)?;
    Ok(xs
        .values()
        .iter()
        .zip(ys.values())
        .map(|(&gx, &gy)| {
            let (lon, lat) = square_store::grid_cols::grid_cell_lonlat(gx, gy);
            (lat as f32, lon as f32)
        })
        .unzip())
}

fn heights(values: &StructArray, name: &str) -> Result<Vec<f32>, String> {
    let values = required_array::<Int16Array>(values.column_by_name(name), name)?;
    Ok(values
        .values()
        .iter()
        .map(|&height| f32::from(height))
        .collect())
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
            let (start_lat, start_lon) = coordinates(values, "start")?;
            let (end_lat, end_lon) = coordinates(values, "end")?;
            decoded.push(DecodedBatch {
                flight_id,
                callsign,
                aircraft_type,
                profile,
                source,
                origin,
                offsets: list.value_offsets(),
                start_lat,
                start_lon,
                end_lat,
                end_lon,
                start_alt: heights(values, "start_alt_m")?,
                end_alt: heights(values, "end_alt_m")?,
                start_terrain: heights(values, "terrain_start_elev_m")?,
                end_terrain: heights(values, "terrain_end_elev_m")?,
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
        let mut bbox = BBox {
            min_lat: f32::INFINITY,
            max_lat: f32::NEG_INFINITY,
            min_lon: f32::INFINITY,
            max_lon: f32::NEG_INFINITY,
        };
        for index in range.clone() {
            bbox.min_lat = bbox
                .min_lat
                .min(self.start_lat[index])
                .min(self.end_lat[index]);
            bbox.max_lat = bbox
                .max_lat
                .max(self.start_lat[index])
                .max(self.end_lat[index]);
            bbox.min_lon = bbox
                .min_lon
                .min(self.start_lon[index])
                .min(self.end_lon[index]);
            bbox.max_lon = bbox
                .max_lon
                .max(self.start_lon[index])
                .max(self.end_lon[index]);
        }
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
            bbox,
            sub_segments: SubSegmentSlice {
                start_lat: &self.start_lat[range.clone()],
                start_lon: &self.start_lon[range.clone()],
                end_lat: &self.end_lat[range.clone()],
                end_lon: &self.end_lon[range.clone()],
                start_alt_m: &self.start_alt[range.clone()],
                end_alt_m: &self.end_alt[range.clone()],
                terrain_start_elev_m: &self.start_terrain[range.clone()],
                terrain_end_elev_m: &self.end_terrain[range.clone()],
                speed_kt: &self.speed[range.clone()],
                length_m: &self.length[range.clone()],
                period: &self.period[range.clone()],
                date_id: &self.date[range.clone()],
                flags: &self.flags[range],
            },
        }
    }
}

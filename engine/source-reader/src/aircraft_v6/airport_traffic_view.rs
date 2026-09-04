//! Strict z30 airport traffic decoding for popup physics.

use arrow::array::*;
use arrow::record_batch::RecordBatch;
use noise_compute::compute::aircraft_v6::{AirportTrafficRowView, NUM_GSE_CLASSES};

use super::columns::required_array;

const NUM_BANDS: usize = 8;

/// Owned column buffers for one airport_traffic.arrow v5 slice. Each
/// `Vec` parallels the others — index `i` describes one row.
pub struct AirportTrafficRowAccum {
    airport_key: Vec<String>,
    osm_id: Vec<u64>,
    segment_idx: Vec<u16>,
    geometry_kind: Vec<u8>,
    start_lat: Vec<f32>,
    start_lon: Vec<f32>,
    end_lat: Vec<f32>,
    end_lon: Vec<f32>,
    length_m: Vec<f32>,
    ops_kind: Vec<u8>,
    is_departure: Vec<u8>,
    veh_kind: Vec<u8>,
    class_idx: Vec<u8>,
    period: Vec<u8>,
    band_energy_lin: Vec<[f32; NUM_BANDS]>,
    unique_movement_count: Vec<u32>,
    unique_arr_count: Vec<u32>,
    unique_dep_count: Vec<u32>,
    unique_gse_count_per_class: Vec<[u32; NUM_GSE_CLASSES]>,
    microseg_unique_count: Vec<u32>,
    microseg_unique_arr_count: Vec<u32>,
    microseg_unique_dep_count: Vec<u32>,
    microseg_unique_gse_count_per_class: Vec<[u32; NUM_GSE_CLASSES]>,
    microseg_unique_ga_count: Vec<u32>,
    microseg_unique_ga_arr_count: Vec<u32>,
    microseg_unique_ga_dep_count: Vec<u32>,
}

impl AirportTrafficRowAccum {
    pub fn new(batches: &[RecordBatch]) -> Result<Self, String> {
        let mut out = AirportTrafficRowAccum {
            airport_key: Vec::new(),
            osm_id: Vec::new(),
            segment_idx: Vec::new(),
            geometry_kind: Vec::new(),
            start_lat: Vec::new(),
            start_lon: Vec::new(),
            end_lat: Vec::new(),
            end_lon: Vec::new(),
            length_m: Vec::new(),
            ops_kind: Vec::new(),
            is_departure: Vec::new(),
            veh_kind: Vec::new(),
            class_idx: Vec::new(),
            period: Vec::new(),
            band_energy_lin: Vec::new(),
            unique_movement_count: Vec::new(),
            unique_arr_count: Vec::new(),
            unique_dep_count: Vec::new(),
            unique_gse_count_per_class: Vec::new(),
            microseg_unique_count: Vec::new(),
            microseg_unique_arr_count: Vec::new(),
            microseg_unique_dep_count: Vec::new(),
            microseg_unique_gse_count_per_class: Vec::new(),
            microseg_unique_ga_count: Vec::new(),
            microseg_unique_ga_arr_count: Vec::new(),
            microseg_unique_ga_dep_count: Vec::new(),
        };
        for batch in batches {
            out.absorb(batch)?;
        }
        Ok(out)
    }

    fn absorb(&mut self, batch: &RecordBatch) -> Result<(), String> {
        let n = batch.num_rows();
        let airport_key =
            required_array::<StringArray>(batch.column_by_name("airport_key"), "airport_key")?;
        let osm_id = required_array::<UInt64Array>(batch.column_by_name("osm_id"), "osm_id")?;
        let seg_idx =
            required_array::<UInt16Array>(batch.column_by_name("segment_idx"), "segment_idx")?;
        let geom_kind =
            required_array::<UInt8Array>(batch.column_by_name("geometry_kind"), "geometry_kind")?;
        let sgx = required_array::<Int32Array>(batch.column_by_name("start_gx"), "start_gx")?;
        let sgy = required_array::<Int32Array>(batch.column_by_name("start_gy"), "start_gy")?;
        let egx = required_array::<Int32Array>(batch.column_by_name("end_gx"), "end_gx")?;
        let egy = required_array::<Int32Array>(batch.column_by_name("end_gy"), "end_gy")?;
        let len_m = required_array::<Float32Array>(batch.column_by_name("length_m"), "length_m")?;
        let ops_kind = required_array::<UInt8Array>(batch.column_by_name("ops_kind"), "ops_kind")?;
        let is_dep =
            required_array::<UInt8Array>(batch.column_by_name("is_departure"), "is_departure")?;
        let veh_kind = required_array::<UInt8Array>(batch.column_by_name("veh_kind"), "veh_kind")?;
        let class_idx =
            required_array::<UInt8Array>(batch.column_by_name("class_idx"), "class_idx")?;
        let period = required_array::<UInt8Array>(batch.column_by_name("period"), "period")?;
        let bands = required_array::<FixedSizeListArray>(
            batch.column_by_name("band_energy_lin"),
            "band_energy_lin",
        )?;
        let unique_mov = required_array::<UInt32Array>(
            batch.column_by_name("unique_movement_count"),
            "unique_movement_count",
        )?;
        let unique_arr = required_array::<UInt32Array>(
            batch.column_by_name("unique_arr_count"),
            "unique_arr_count",
        )?;
        let unique_dep = required_array::<UInt32Array>(
            batch.column_by_name("unique_dep_count"),
            "unique_dep_count",
        )?;
        let gse_list = required_array::<FixedSizeListArray>(
            batch.column_by_name("unique_gse_count_per_class"),
            "unique_gse_count_per_class",
        )?;
        let microseg_unique = required_array::<UInt32Array>(
            batch.column_by_name("microseg_unique_count"),
            "microseg_unique_count",
        )?;
        let microseg_unique_arr = required_array::<UInt32Array>(
            batch.column_by_name("microseg_unique_arr_count"),
            "microseg_unique_arr_count",
        )?;
        let microseg_unique_dep = required_array::<UInt32Array>(
            batch.column_by_name("microseg_unique_dep_count"),
            "microseg_unique_dep_count",
        )?;
        let microseg_gse_list = required_array::<FixedSizeListArray>(
            batch.column_by_name("microseg_unique_gse_count_per_class"),
            "microseg_unique_gse_count_per_class",
        )?;
        let microseg_unique_ga = required_array::<UInt32Array>(
            batch.column_by_name("microseg_unique_ga_count"),
            "microseg_unique_ga_count",
        )?;
        let microseg_unique_ga_arr = required_array::<UInt32Array>(
            batch.column_by_name("microseg_unique_ga_arr_count"),
            "microseg_unique_ga_arr_count",
        )?;
        let microseg_unique_ga_dep = required_array::<UInt32Array>(
            batch.column_by_name("microseg_unique_ga_dep_count"),
            "microseg_unique_ga_dep_count",
        )?;
        if bands.value_length() != NUM_BANDS as i32
            || gse_list.value_length() != NUM_GSE_CLASSES as i32
            || microseg_gse_list.value_length() != NUM_GSE_CLASSES as i32
        {
            return Err("airport_traffic fixed-size list width mismatch".into());
        }
        let band_buf =
            required_array::<Float32Array>(Some(bands.values()), "band_energy_lin.item")?.values();
        let gse_buf = required_array::<UInt32Array>(
            Some(gse_list.values()),
            "unique_gse_count_per_class.item",
        )?
        .values();
        let microseg_gse_buf = required_array::<UInt32Array>(
            Some(microseg_gse_list.values()),
            "microseg_unique_gse_count_per_class.item",
        )?
        .values();
        self.airport_key.reserve(n);
        self.band_energy_lin.reserve(n);
        for i in 0..n {
            self.airport_key.push(airport_key.value(i).to_string());
            self.osm_id.push(osm_id.value(i));
            self.segment_idx.push(seg_idx.value(i));
            self.geometry_kind.push(geom_kind.value(i));
            let (start_lon, start_lat) =
                square_store::grid_cols::grid_cell_lonlat(sgx.value(i), sgy.value(i));
            let (end_lon, end_lat) =
                square_store::grid_cols::grid_cell_lonlat(egx.value(i), egy.value(i));
            self.start_lat.push(start_lat as f32);
            self.start_lon.push(start_lon as f32);
            self.end_lat.push(end_lat as f32);
            self.end_lon.push(end_lon as f32);
            self.length_m.push(len_m.value(i));
            self.ops_kind.push(ops_kind.value(i));
            self.is_departure.push(is_dep.value(i));
            self.veh_kind.push(veh_kind.value(i));
            self.class_idx.push(class_idx.value(i));
            self.period.push(period.value(i));
            let lo_b = i * NUM_BANDS;
            let mut row_bands = [0.0f32; NUM_BANDS];
            row_bands.copy_from_slice(&band_buf[lo_b..lo_b + NUM_BANDS]);
            self.band_energy_lin.push(row_bands);
            self.unique_movement_count.push(unique_mov.value(i));
            self.unique_arr_count.push(unique_arr.value(i));
            self.unique_dep_count.push(unique_dep.value(i));
            let lo_g = i * NUM_GSE_CLASSES;
            let mut row_gse = [0u32; NUM_GSE_CLASSES];
            row_gse.copy_from_slice(&gse_buf[lo_g..lo_g + NUM_GSE_CLASSES]);
            self.unique_gse_count_per_class.push(row_gse);
            self.microseg_unique_count.push(microseg_unique.value(i));
            self.microseg_unique_arr_count
                .push(microseg_unique_arr.value(i));
            self.microseg_unique_dep_count
                .push(microseg_unique_dep.value(i));
            let mut row_microseg_gse = [0u32; NUM_GSE_CLASSES];
            row_microseg_gse.copy_from_slice(&microseg_gse_buf[lo_g..lo_g + NUM_GSE_CLASSES]);
            self.microseg_unique_gse_count_per_class
                .push(row_microseg_gse);
            self.microseg_unique_ga_count
                .push(microseg_unique_ga.value(i));
            self.microseg_unique_ga_arr_count
                .push(microseg_unique_ga_arr.value(i));
            self.microseg_unique_ga_dep_count
                .push(microseg_unique_ga_dep.value(i));
        }
        Ok(())
    }

    pub fn views(&self) -> Vec<AirportTrafficRowView<'_>> {
        (0..self.airport_key.len())
            .map(|i| AirportTrafficRowView {
                airport_key: &self.airport_key[i],
                osm_id: self.osm_id[i],
                segment_idx: self.segment_idx[i],
                geometry_kind: self.geometry_kind[i],
                start_lat: self.start_lat[i],
                start_lon: self.start_lon[i],
                end_lat: self.end_lat[i],
                end_lon: self.end_lon[i],
                length_m: self.length_m[i],
                ops_kind: self.ops_kind[i],
                is_departure: self.is_departure[i],
                veh_kind: self.veh_kind[i],
                class_idx: self.class_idx[i],
                period: self.period[i],
                band_energy_lin: &self.band_energy_lin[i],
                unique_movement_count: self.unique_movement_count[i],
                unique_arr_count: self.unique_arr_count[i],
                unique_dep_count: self.unique_dep_count[i],
                unique_gse_count_per_class: &self.unique_gse_count_per_class[i],
                microseg_unique_count: self.microseg_unique_count[i],
                microseg_unique_arr_count: self.microseg_unique_arr_count[i],
                microseg_unique_dep_count: self.microseg_unique_dep_count[i],
                microseg_unique_gse_count_per_class: &self.microseg_unique_gse_count_per_class[i],
                microseg_unique_ga_count: self.microseg_unique_ga_count[i],
                microseg_unique_ga_arr_count: self.microseg_unique_ga_arr_count[i],
                microseg_unique_ga_dep_count: self.microseg_unique_ga_dep_count[i],
            })
            .collect()
    }
}

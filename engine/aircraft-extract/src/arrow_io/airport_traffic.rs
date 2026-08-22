//! `airport_traffic.arrow` writer + reader (v5).
//!
//! Rev 2: drops the per-row `flight_ids: List<UInt64>` payload. Each
//! row now carries scalar `unique_*_count` counters plus row-replicated
//! `microseg_unique_*` UNIONs. Airport-level UNION across R4s lives
//! in the global `airport_summary.arrow` sidecar.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use arrow::array::{
    ArrayRef, FixedSizeListArray, Float32Array, Float32Builder, StringArray, StringBuilder,
    UInt16Array, UInt16Builder, UInt32Array, UInt32Builder, UInt64Array, UInt64Builder, UInt8Array,
    UInt8Builder,
};
use arrow::datatypes::{DataType, Field};
use arrow::record_batch::RecordBatch;

use noise_compute::emission::gse::NUM_GSE_CLASSES;
use noise_compute::types::NUM_BANDS;

use crate::arrow_schemas;

use super::{read_all_batches, write_record_batches};

/// One traffic counter — sparse on the (segment × class × period) grid.
/// Per-row scalar unique counts replace the v4 `flight_ids: List<UInt64>`.
/// Per-microsegment UNION counts are row-replicated so the popup loader
/// can fold rows of the same `(osm_id, segment_idx)` into ONE microseg
/// trace without needing to UNION HashSets across rows.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct AirportTrafficRow {
    pub airport_key: String,
    pub osm_id: u64,
    pub segment_idx: u16,
    /// 0 = line (runway/taxi/stopway), 1 = area_grid_point (apron),
    /// 2 = synthetic (DBSCAN auto-discovery).
    pub geometry_kind: u8,
    pub start_lat: f32,
    pub start_lon: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    pub length_m: f32,
    /// 1 = runway, 2 = taxi, 3 = apron.
    pub ops_kind: u8,
    pub is_departure: u8,
    /// 0 = aircraft, 1 = GSE.
    pub veh_kind: u8,
    /// Indexes `noise_compute::emission::profiles_generated::CLASS_OF_PROFILE`
    /// when `veh_kind=0` (range 0..NUM_CLASSES), or
    /// `noise_compute::emission::gse::GSE_LW_BANDS_DB` when `veh_kind=1`
    /// (range 0..NUM_GSE_CLASSES=3).
    pub class_idx: u8,
    /// 0 = day, 1 = evening, 2 = night.
    pub period: u8,
    /// Per-band **raw Σ** of linear Z-weighted energy contribution
    /// from this microsegment for this period, summed over the n_days
    /// extraction window. Units depend on `veh_kind`:
    ///  - `veh_kind = 0` (aircraft): per-metre `LW'` × `(hit_length /
    ///    line.length_m)` density factor. Consumer applies CNOSSOS-EU
    ///    §2.5.5 `+ 10·log10(θ / d_perp)` over the full microsegment
    ///    geometry at receiver — refinement-invariant by Chasles.
    ///  - `veh_kind = 1` (GSE): per-event SEL@25m from the kinematic
    ///    moving-point integral; consumer applies point-source
    ///    `+ 10·log10(25 / d_endpoint)` divergence.
    ///
    /// Either way the consumer divides by `n_days × period_seconds`
    /// via `period_leq` to recover Leq.
    pub band_energy_lin: [f32; NUM_BANDS],
    /// Distinct fids that crossed this microsegment-row, regardless
    /// of ops_kind / is_departure / veh_kind. Display count.
    pub unique_movement_count: u32,
    /// Distinct fids that crossed this row with `ops_kind=RUNWAY_ROLL`,
    /// `is_departure=0`, `veh_kind=0`. Zero when the row's key
    /// doesn't match arrival semantics.
    pub unique_arr_count: u32,
    /// Analogous for departures.
    pub unique_dep_count: u32,
    /// Per GSE-class distinct fids that crossed this row, populated
    /// only when `veh_kind=1`.
    pub unique_gse_count_per_class: [u32; NUM_GSE_CLASSES],
    /// UNION across ALL rows of this microsegment `(osm_id,
    /// segment_idx)` regardless of period / class / ops_kind. v9: these
    /// three count NON-GA-class fids only — the GA-class union lives in
    /// `microseg_unique_ga_*`. Replicated on every row of the same
    /// microsegment so the popup loader can populate per-microseg
    /// observed_movements without a HashSet UNION join, and divide each
    /// window by its own day count.
    pub microseg_unique_count: u32,
    pub microseg_unique_arr_count: u32,
    pub microseg_unique_dep_count: u32,
    pub microseg_unique_gse_count_per_class: [u32; NUM_GSE_CLASSES],
    /// v9 GA-class (PROP_C172 + HELICOPTER) microsegment UNION — the
    /// full-year-window split of the three counts above. Zero on a
    /// non-hybrid extract. (GSE has no GA split — airline-pass only.)
    pub microseg_unique_ga_count: u32,
    pub microseg_unique_ga_arr_count: u32,
    pub microseg_unique_ga_dep_count: u32,
}

/// Write one R4 hex's traffic counters. `n_days` (airline window) +
/// `ga_n_days` (GA-class window, 0 = single-window extract) stamp the GA
/// hybrid metadata (`n_days`, `ga_n_days`, `sample_days_by_class`) so the
/// popup consumer weights GA energy at `1/ga_n_days` and divides the GA-split
/// movement counts by their own window.
pub fn write_airport_traffic(
    path: &Path,
    rows: &[AirportTrafficRow],
    n_days: u16,
    ga_n_days: u16,
) -> Result<()> {
    let schema = arrow_schemas::with_n_days_and_windows(
        arrow_schemas::airport_traffic_schema(),
        n_days,
        ga_n_days,
    );
    let n = rows.len();
    let mut airport_key = StringBuilder::with_capacity(n, 8 * n);
    let mut osm_id = UInt64Builder::with_capacity(n);
    let mut segment_idx = UInt16Builder::with_capacity(n);
    let mut geometry_kind = UInt8Builder::with_capacity(n);
    let mut start_lat = Float32Builder::with_capacity(n);
    let mut start_lon = Float32Builder::with_capacity(n);
    let mut end_lat = Float32Builder::with_capacity(n);
    let mut end_lon = Float32Builder::with_capacity(n);
    let mut length_m = Float32Builder::with_capacity(n);
    let mut ops_kind = UInt8Builder::with_capacity(n);
    let mut is_departure = UInt8Builder::with_capacity(n);
    let mut veh_kind = UInt8Builder::with_capacity(n);
    let mut class_idx = UInt8Builder::with_capacity(n);
    let mut period = UInt8Builder::with_capacity(n);
    let mut unique_mov = UInt32Builder::with_capacity(n);
    let mut unique_arr = UInt32Builder::with_capacity(n);
    let mut unique_dep = UInt32Builder::with_capacity(n);
    let mut microseg_unique = UInt32Builder::with_capacity(n);
    let mut microseg_unique_arr = UInt32Builder::with_capacity(n);
    let mut microseg_unique_dep = UInt32Builder::with_capacity(n);
    let mut microseg_unique_ga = UInt32Builder::with_capacity(n);
    let mut microseg_unique_ga_arr = UInt32Builder::with_capacity(n);
    let mut microseg_unique_ga_dep = UInt32Builder::with_capacity(n);

    let mut band_values: Vec<f32> = Vec::with_capacity(n * NUM_BANDS);
    let mut gse_values: Vec<u32> = Vec::with_capacity(n * NUM_GSE_CLASSES);
    let mut microseg_gse_values: Vec<u32> = Vec::with_capacity(n * NUM_GSE_CLASSES);
    // Popup batch pruning (docs/dev/popup-batch-pruning.md): microsegment
    // endpoint box per row; f32→f64 is exact.
    let mut row_bboxes = Vec::with_capacity(n);

    for r in rows {
        row_bboxes.push([
            r.start_lat.min(r.end_lat) as f64,
            r.start_lon.min(r.end_lon) as f64,
            r.start_lat.max(r.end_lat) as f64,
            r.start_lon.max(r.end_lon) as f64,
        ]);
        airport_key.append_value(&r.airport_key);
        osm_id.append_value(r.osm_id);
        segment_idx.append_value(r.segment_idx);
        geometry_kind.append_value(r.geometry_kind);
        start_lat.append_value(r.start_lat);
        start_lon.append_value(r.start_lon);
        end_lat.append_value(r.end_lat);
        end_lon.append_value(r.end_lon);
        length_m.append_value(r.length_m);
        ops_kind.append_value(r.ops_kind);
        is_departure.append_value(r.is_departure);
        veh_kind.append_value(r.veh_kind);
        class_idx.append_value(r.class_idx);
        period.append_value(r.period);
        band_values.extend_from_slice(&r.band_energy_lin);
        unique_mov.append_value(r.unique_movement_count);
        unique_arr.append_value(r.unique_arr_count);
        unique_dep.append_value(r.unique_dep_count);
        gse_values.extend_from_slice(&r.unique_gse_count_per_class);
        microseg_unique.append_value(r.microseg_unique_count);
        microseg_unique_arr.append_value(r.microseg_unique_arr_count);
        microseg_unique_dep.append_value(r.microseg_unique_dep_count);
        microseg_gse_values.extend_from_slice(&r.microseg_unique_gse_count_per_class);
        microseg_unique_ga.append_value(r.microseg_unique_ga_count);
        microseg_unique_ga_arr.append_value(r.microseg_unique_ga_arr_count);
        microseg_unique_ga_dep.append_value(r.microseg_unique_ga_dep_count);
    }

    let band_list = FixedSizeListArray::new(
        Arc::new(Field::new("item", DataType::Float32, false)),
        NUM_BANDS as i32,
        Arc::new(Float32Array::from(band_values)),
        None,
    );
    let gse_field = Arc::new(Field::new("item", DataType::UInt32, false));
    let gse_list = FixedSizeListArray::new(
        gse_field.clone(),
        NUM_GSE_CLASSES as i32,
        Arc::new(UInt32Array::from(gse_values)),
        None,
    );
    let microseg_gse_list = FixedSizeListArray::new(
        gse_field,
        NUM_GSE_CLASSES as i32,
        Arc::new(UInt32Array::from(microseg_gse_values)),
        None,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(airport_key.finish()),
        Arc::new(osm_id.finish()),
        Arc::new(segment_idx.finish()),
        Arc::new(geometry_kind.finish()),
        Arc::new(start_lat.finish()),
        Arc::new(start_lon.finish()),
        Arc::new(end_lat.finish()),
        Arc::new(end_lon.finish()),
        Arc::new(length_m.finish()),
        Arc::new(ops_kind.finish()),
        Arc::new(is_departure.finish()),
        Arc::new(veh_kind.finish()),
        Arc::new(class_idx.finish()),
        Arc::new(period.finish()),
        Arc::new(band_list),
        Arc::new(unique_mov.finish()),
        Arc::new(unique_arr.finish()),
        Arc::new(unique_dep.finish()),
        Arc::new(gse_list),
        Arc::new(microseg_unique.finish()),
        Arc::new(microseg_unique_arr.finish()),
        Arc::new(microseg_unique_dep.finish()),
        Arc::new(microseg_gse_list),
        Arc::new(microseg_unique_ga.finish()),
        Arc::new(microseg_unique_ga_arr.finish()),
        Arc::new(microseg_unique_ga_dep.finish()),
    ];
    let (schema, batches) =
        arrow_batching::spatially_batched(schema.as_ref().clone(), columns, &row_bboxes)?;
    write_record_batches(path, &schema, &batches)
}

pub fn read_airport_traffic(path: &Path) -> Result<Vec<AirportTrafficRow>> {
    let (schema, batches) = read_all_batches(path)?;
    arrow_schemas::assert_airport_traffic_contract_v9(schema.metadata())?;
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    let mut out = Vec::with_capacity(total_rows);
    for b in batches {
        let airport_key = column::<StringArray>(&b, "airport_key")?;
        let osm_id = column::<UInt64Array>(&b, "osm_id")?;
        let segment_idx = column::<UInt16Array>(&b, "segment_idx")?;
        let geometry_kind = column::<UInt8Array>(&b, "geometry_kind")?;
        let start_lat = column::<Float32Array>(&b, "start_lat")?;
        let start_lon = column::<Float32Array>(&b, "start_lon")?;
        let end_lat = column::<Float32Array>(&b, "end_lat")?;
        let end_lon = column::<Float32Array>(&b, "end_lon")?;
        let length_m = column::<Float32Array>(&b, "length_m")?;
        let ops_kind = column::<UInt8Array>(&b, "ops_kind")?;
        let is_departure = column::<UInt8Array>(&b, "is_departure")?;
        let veh_kind = column::<UInt8Array>(&b, "veh_kind")?;
        let class_idx = column::<UInt8Array>(&b, "class_idx")?;
        let period = column::<UInt8Array>(&b, "period")?;
        let band_list = column::<FixedSizeListArray>(&b, "band_energy_lin")?;
        let band_buf = band_list
            .values()
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("band_energy_lin inner type"))?
            .values();
        let unique_mov = column::<UInt32Array>(&b, "unique_movement_count")?;
        let unique_arr = column::<UInt32Array>(&b, "unique_arr_count")?;
        let unique_dep = column::<UInt32Array>(&b, "unique_dep_count")?;
        let unique_gse_list = column::<FixedSizeListArray>(&b, "unique_gse_count_per_class")?;
        let gse_buf = unique_gse_list
            .values()
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or_else(|| anyhow::anyhow!("unique_gse_count_per_class inner type"))?
            .values();
        let microseg_unique = column::<UInt32Array>(&b, "microseg_unique_count")?;
        let microseg_unique_arr = column::<UInt32Array>(&b, "microseg_unique_arr_count")?;
        let microseg_unique_dep = column::<UInt32Array>(&b, "microseg_unique_dep_count")?;
        let microseg_gse_list =
            column::<FixedSizeListArray>(&b, "microseg_unique_gse_count_per_class")?;
        let microseg_gse_buf = microseg_gse_list
            .values()
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or_else(|| anyhow::anyhow!("microseg_unique_gse_count_per_class inner type"))?
            .values();
        let microseg_unique_ga = column::<UInt32Array>(&b, "microseg_unique_ga_count")?;
        let microseg_unique_ga_arr = column::<UInt32Array>(&b, "microseg_unique_ga_arr_count")?;
        let microseg_unique_ga_dep = column::<UInt32Array>(&b, "microseg_unique_ga_dep_count")?;

        for i in 0..b.num_rows() {
            let lo_b = i * NUM_BANDS;
            let mut bands = [0.0f32; NUM_BANDS];
            bands.copy_from_slice(&band_buf[lo_b..lo_b + NUM_BANDS]);
            let lo_g = i * NUM_GSE_CLASSES;
            let mut gse = [0u32; NUM_GSE_CLASSES];
            gse.copy_from_slice(&gse_buf[lo_g..lo_g + NUM_GSE_CLASSES]);
            let mut microseg_gse = [0u32; NUM_GSE_CLASSES];
            microseg_gse.copy_from_slice(&microseg_gse_buf[lo_g..lo_g + NUM_GSE_CLASSES]);
            out.push(AirportTrafficRow {
                airport_key: airport_key.value(i).to_string(),
                osm_id: osm_id.value(i),
                segment_idx: segment_idx.value(i),
                geometry_kind: geometry_kind.value(i),
                start_lat: start_lat.value(i),
                start_lon: start_lon.value(i),
                end_lat: end_lat.value(i),
                end_lon: end_lon.value(i),
                length_m: length_m.value(i),
                ops_kind: ops_kind.value(i),
                is_departure: is_departure.value(i),
                veh_kind: veh_kind.value(i),
                class_idx: class_idx.value(i),
                period: period.value(i),
                band_energy_lin: bands,
                unique_movement_count: unique_mov.value(i),
                unique_arr_count: unique_arr.value(i),
                unique_dep_count: unique_dep.value(i),
                unique_gse_count_per_class: gse,
                microseg_unique_count: microseg_unique.value(i),
                microseg_unique_arr_count: microseg_unique_arr.value(i),
                microseg_unique_dep_count: microseg_unique_dep.value(i),
                microseg_unique_gse_count_per_class: microseg_gse,
                microseg_unique_ga_count: microseg_unique_ga.value(i),
                microseg_unique_ga_arr_count: microseg_unique_ga_arr.value(i),
                microseg_unique_ga_dep_count: microseg_unique_ga_dep.value(i),
            });
        }
    }
    Ok(out)
}

fn column<'a, T: arrow::array::Array + 'static>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("missing column {name}"))?
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "column {name} type mismatch (expected {})",
                std::any::type_name::<T>()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_row() -> AirportTrafficRow {
        AirportTrafficRow {
            airport_key: "LKPR".into(),
            osm_id: 42,
            segment_idx: 7,
            geometry_kind: arrow_schemas::GEOMETRY_KIND_LINE,
            start_lat: 50.105,
            start_lon: 14.260,
            end_lat: 50.106,
            end_lon: 14.262,
            length_m: 250.0,
            ops_kind: 1, // runway
            is_departure: 1,
            veh_kind: 0,
            class_idx: 2, // WING_B738
            period: 0,    // day
            // 8 strictly distinct values — a transposition of any two
            // positions changes the read-back.
            band_energy_lin: [1.0e6, 2.0e6, 3.0e6, 4.0e6, 5.0e6, 6.0e6, 7.0e6, 8.0e6],
            unique_movement_count: 25,
            unique_arr_count: 0,
            unique_dep_count: 25,
            unique_gse_count_per_class: [0, 0, 0],
            microseg_unique_count: 50,
            microseg_unique_arr_count: 25,
            microseg_unique_dep_count: 25,
            microseg_unique_gse_count_per_class: [0, 0, 0],
            microseg_unique_ga_count: 3,
            microseg_unique_ga_arr_count: 1,
            microseg_unique_ga_dep_count: 2,
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("airport_traffic.arrow");
        let rows = vec![sample_row()];
        write_airport_traffic(&path, &rows, 14, 365).unwrap();
        let read = read_airport_traffic(&path).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], rows[0], "every field must round-trip exactly");
    }

    #[test]
    fn round_trip_two_rows_distinguishable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("airport_traffic.arrow");
        let mut row_gse = sample_row();
        row_gse.veh_kind = 1;
        row_gse.class_idx = 2; // HEAVY
        row_gse.airport_key = "strip:871e3558effffff".into();
        row_gse.unique_movement_count = 6;
        row_gse.unique_arr_count = 0;
        row_gse.unique_dep_count = 0;
        row_gse.unique_gse_count_per_class = [0, 0, 6];
        row_gse.microseg_unique_count = 9;
        row_gse.microseg_unique_arr_count = 0;
        row_gse.microseg_unique_dep_count = 0;
        row_gse.microseg_unique_gse_count_per_class = [1, 2, 6];
        // GSE row: no GA aircraft split (airline-pass only).
        row_gse.microseg_unique_ga_count = 0;
        row_gse.microseg_unique_ga_arr_count = 0;
        row_gse.microseg_unique_ga_dep_count = 0;
        // Distinct band values so a row offset bug surfaces.
        row_gse.band_energy_lin = [
            10.0e6, 20.0e6, 30.0e6, 40.0e6, 50.0e6, 60.0e6, 70.0e6, 80.0e6,
        ];
        let rows = vec![sample_row(), row_gse.clone()];
        write_airport_traffic(&path, &rows, 14, 365).unwrap();
        let read = read_airport_traffic(&path).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0], rows[0], "row 0 round-trip");
        assert_eq!(read[1], rows[1], "row 1 round-trip");
    }

    #[test]
    fn empty_rows_writes_valid_arrow_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("airport_traffic.arrow");
        write_airport_traffic(&path, &[], 14, 0).unwrap();
        let read = read_airport_traffic(&path).unwrap();
        assert!(read.is_empty());
    }

    #[test]
    fn hybrid_window_metadata_stamped() {
        // Hybrid extract stamps n_days, ga_n_days, and the per-class vector
        // the consumer's ClassWeights parses.
        let dir = tempdir().unwrap();
        let path = dir.path().join("airport_traffic.arrow");
        write_airport_traffic(&path, &[sample_row()], 12, 365).unwrap();
        let (schema, _) = crate::arrow_io::read_record_batches(&path).unwrap();
        let md = schema.metadata();
        assert_eq!(md.get("n_days").map(String::as_str), Some("12"));
        assert_eq!(md.get("ga_n_days").map(String::as_str), Some("365"));
        let vec = md
            .get("sample_days_by_class")
            .expect("sample_days_by_class stamped");
        assert_eq!(vec.split(',').count(), 15, "15-class vector");
        assert!(vec.contains("365"), "GA classes carry 365: {vec}");
        assert!(vec.contains("12"), "airline classes carry 12: {vec}");

        // Single-window extract: no ga_n_days, uniform vector.
        let p2 = dir.path().join("single.arrow");
        write_airport_traffic(&p2, &[sample_row()], 14, 0).unwrap();
        let (s2, _) = crate::arrow_io::read_record_batches(&p2).unwrap();
        assert!(s2.metadata().get("ga_n_days").is_none());
        assert!(s2
            .metadata()
            .get("sample_days_by_class")
            .unwrap()
            .split(',')
            .all(|d| d == "14"));
    }

    #[test]
    fn reader_rejects_wrong_contract() {
        // Synthetic file with bogus contract metadata must be rejected
        // by `assert_airport_traffic_contract_v9`. Older versions had
        // different column shapes or energy normalization; silent
        // decoding would produce wrong popup numbers.
        for stale_contract in [
            "bogus_v9",
            "airport_traffic_v1",
            "airport_traffic_v2",
            "airport_traffic_v3",
            "airport_traffic_v4",
            "airport_traffic_v8",
        ] {
            use crate::arrow_io::write_record_batches;
            use std::sync::Arc;
            let dir = tempdir().unwrap();
            let path = dir.path().join("bogus.arrow");
            let schema = arrow_schemas::airport_traffic_schema();
            let mut md = schema.metadata().clone();
            md.insert("airport_traffic_contract".into(), stale_contract.into());
            let bogus = Arc::new((*schema).clone().with_metadata(md));
            let empty_batch = RecordBatch::new_empty(bogus.clone());
            write_record_batches(&path, &bogus, &[empty_batch]).unwrap();
            let err = read_airport_traffic(&path).unwrap_err();
            assert!(
                err.to_string().contains("airport_traffic_contract"),
                "stale_contract={stale_contract}: expected contract-mismatch error, got: {err}"
            );
        }
    }
}

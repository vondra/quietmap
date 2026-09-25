//! Registry helpers and one-square IPC rewrite.

use crate::finalize_square;
use arrow::array::{
    Array, ArrayRef, Float32Array, Float64Array, Int16Array, Int32Array, Int32Builder, Int64Array,
    ListBuilder, StringArray, UInt16Array, UInt8Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::reader::FileReader;
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use grid::Square;
use std::collections::HashMap;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::sources::should_overwrite;

#[test]
fn empty_and_self_overwrite() {
    assert!(should_overwrite(0, 100));
    assert!(should_overwrite(100, 100));
}

const SQUARE: Square = Square { x: 276, y: 173 };
static NEXT: AtomicU64 = AtomicU64::new(0);

fn year_dir() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "qm-rf-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("year/z9/276/173")).unwrap();
    root.join("year")
}

fn write_parent_arrow(path: &Path, contract: Option<&str>) {
    let (gx, gy) = grid::lonlat_to_grid(14.0, 50.0);
    let (ex, ey) = grid::lonlat_to_grid(14.001, 50.0);
    let mut metadata = HashMap::from([(
        "osm_railways_contract".to_owned(),
        square_store::osm_contract::RAILWAYS_CONTRACT.to_owned(),
    )]);
    metadata.insert("grid".to_owned(), "z30".to_owned());
    metadata.insert(
        "railways_contract".to_owned(),
        "country_baked_v1".to_owned(),
    );
    if let Some(contract) = contract {
        metadata.insert("rail_traffic_contract".to_owned(), contract.to_owned());
    }
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("segment_idx", DataType::Int16, false),
            Field::new("start_gx", DataType::Int32, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
            Field::new("length_m", DataType::Float32, false),
            Field::new("rail_type", DataType::UInt8, false),
            Field::new("usage", DataType::UInt8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("ref", DataType::Utf8, false),
            Field::new("service", DataType::UInt8, false),
            Field::new("source_id", DataType::UInt16, false),
            Field::new("country_iso", DataType::UInt16, false),
            Field::new("city_id", DataType::UInt16, false),
            Field::new("continent", DataType::UInt8, false),
        ],
        metadata,
    ));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![7i64])),
            Arc::new(Int16Array::from(vec![0i16])),
            Arc::new(Int32Array::from(vec![gx])),
            Arc::new(Int32Array::from(vec![gy])),
            Arc::new(Int32Array::from(vec![ex])),
            Arc::new(Int32Array::from(vec![ey])),
            Arc::new(Float32Array::from(vec![100.0f32])),
            Arc::new(UInt8Array::from(vec![0u8])),
            Arc::new(UInt8Array::from(vec![0u8])),
            Arc::new(StringArray::from(vec![""])),
            Arc::new(StringArray::from(vec![""])),
            Arc::new(UInt8Array::from(vec![0u8])),
            Arc::new(UInt16Array::from(vec![0u16])),
            Arc::new(UInt16Array::from(vec![u16::from_le_bytes(*b"CZ")])),
            Arc::new(UInt16Array::from(vec![0u16])),
            Arc::new(UInt8Array::from(vec![1u8])),
        ],
    )
    .unwrap();
    let file = File::create(path).unwrap();
    let mut writer = FileWriter::try_new(file, batch.schema().as_ref()).unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
}

fn write_evidence_arrow(path: &Path, contract_key: &str, columns: Vec<(&str, ArrayRef)>) {
    let batch = RecordBatch::try_from_iter(columns).unwrap();
    let schema = Schema::new(batch.schema().fields().clone())
        .with_metadata(HashMap::from([(contract_key.to_owned(), "1".to_owned())]));
    let mut writer = FileWriter::try_new(File::create(path).unwrap(), &schema).unwrap();
    writer
        .write(&batch.with_schema(Arc::new(schema)).unwrap())
        .unwrap();
    writer.finish().unwrap();
}

fn read_arrow(path: &Path) -> (HashMap<String, String>, RecordBatch) {
    let bytes = std::fs::read(path).unwrap();
    let reader = FileReader::try_new(Cursor::new(bytes), None).unwrap();
    let metadata = reader.schema().metadata().clone();
    let batches: Vec<_> = reader.map(|batch| batch.unwrap()).collect();
    let schema = batches[0].schema();
    (
        metadata,
        arrow::compute::concat_batches(&schema, &batches).unwrap(),
    )
}

fn f64_col(batch: &RecordBatch, name: &str) -> Vec<f64> {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap()
        .iter()
        .map(|value| value.unwrap())
        .collect()
}

fn u8_col(batch: &RecordBatch, name: &str) -> Vec<u8> {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<UInt8Array>()
        .unwrap()
        .values()
        .to_vec()
}

#[test]
fn class_defaults_stamp_contract_and_skip_on_retry() {
    let year = year_dir();
    let arrow = year.join("z9/276/173/railways.arrow");
    write_parent_arrow(&arrow, None);
    let first = finalize_square(&year, SQUARE).unwrap().unwrap();
    assert!(first.rewritten);
    assert_eq!(first.rows_in, 1);
    let (metadata, batch) = read_arrow(&arrow);
    assert_eq!(
        metadata.get("rail_traffic_contract").map(String::as_str),
        Some("1")
    );
    assert!(batch.column_by_name("source_id").is_none());
    let passenger: f64 = (0..3)
        .map(|period| {
            f64_col(
                &batch,
                [
                    "trains_passenger_day",
                    "trains_passenger_evening",
                    "trains_passenger_night",
                ][period],
            )[0]
        })
        .sum();
    assert!((passenger - 80.0).abs() < 1e-6);
    assert_eq!(u8_col(&batch, "passenger_status"), vec![2]);
    let second = finalize_square(&year, SQUARE).unwrap().unwrap();
    assert!(!second.rewritten);
    let mut corrupt = std::fs::read(&arrow).unwrap();
    let footer_size = i32::from_le_bytes(
        corrupt[corrupt.len() - 10..corrupt.len() - 6]
            .try_into()
            .unwrap(),
    ) as usize;
    let footer =
        arrow::ipc::root_as_footer(&corrupt[corrupt.len() - 10 - footer_size..corrupt.len() - 10])
            .unwrap();
    let offset = footer.recordBatches().unwrap().get(0).offset() as usize;
    corrupt[offset + 8..offset + 12].copy_from_slice(&i32::MAX.to_le_bytes());
    std::fs::write(&arrow, &corrupt).unwrap();
    assert!(finalize_square(&year, SQUARE).is_err());
    assert_eq!(std::fs::read(&arrow).unwrap(), corrupt);
    write_parent_arrow(&arrow, Some("1"));
    assert!(finalize_square(&year, SQUARE).is_err());
    let _ = std::fs::remove_dir_all(year.parent().unwrap());
}

#[test]
fn partial_evidence_preserves_middle_counts_and_zero_with_class_priors_on_uncovered_ends() {
    let year = year_dir();
    let arrow = year.join("z9/276/173/railways.arrow");
    // Way 7: three nodes, two pieces; the null-metre piece of an incomplete way is left out.
    let lon = [14.0, 14.001, 14.002].map(|lon| (lon * 1e7_f64).round() as i32);
    let metres = grid::geo::cumulative_flat_metres(0.0, &lon.map(|lon| [50.0, lon as f64 / 1e7]));
    let mut chain_lat = ListBuilder::new(Int32Builder::new());
    let mut chain_lon = ListBuilder::new(Int32Builder::new());
    for piece in 0..3 {
        chain_lat.append_value([Some(500_000_000); 2]);
        chain_lon.append_value([Some(lon[piece % 2]), Some(lon[piece % 2 + 1])]);
    }
    let piece_metres = |offset: usize| {
        Arc::new(Float64Array::from(vec![
            Some(metres[offset.min(1)]),
            Some(metres[1 + offset.min(1)]),
            None,
        ])) as ArrayRef
    };
    write_evidence_arrow(
        &arrow.with_file_name(crate::topology::PIECES_FILE),
        "transport_pieces_contract",
        vec![
            (
                "way_id",
                Arc::new(Int64Array::from(vec![7, 7, 9])) as ArrayRef,
            ),
            ("segment_idx", Arc::new(Int16Array::from(vec![0, 1, 0]))),
            ("first_vertex_m", piece_metres(0)),
            ("from_m", piece_metres(0)),
            ("to_m", piece_metres(1)),
            ("chain_lat_e7", Arc::new(chain_lat.finish())),
            ("chain_lon_e7", Arc::new(chain_lon.finish())),
        ],
    );
    let pieces = crate::topology::load_square_pieces(arrow.parent().unwrap()).unwrap();
    assert_eq!(pieces.len(), 2);
    assert_eq!(
        pieces[&(7, 1)].from_m.to_bits(),
        pieces[&(7, 0)].to_m.to_bits()
    );
    for (daily_passenger, evidence_status) in [(4.0, 2), (0.0, 1)] {
        write_parent_arrow(&arrow, None);
        let status = |known: bool| Arc::new(UInt8Array::from(vec![u8::from(known)])) as ArrayRef;
        write_evidence_arrow(
            &arrow.with_file_name("rail-intervals.CZ.arrow"),
            "rail_intervals_contract",
            vec![
                ("osm_id", Arc::new(Int64Array::from(vec![7])) as ArrayRef),
                ("segment_idx", Arc::new(Int16Array::from(vec![0]))),
                ("from_m", Arc::new(Float64Array::from(vec![20.0]))),
                ("to_m", Arc::new(Float64Array::from(vec![60.0]))),
                ("occurrence", Arc::new(Int64Array::from(vec![0]))),
                ("source_id", Arc::new(UInt16Array::from(vec![100]))),
                (
                    "passenger",
                    Arc::new(Float64Array::from(vec![daily_passenger])),
                ),
                ("freight", Arc::new(Float64Array::from(vec![0.0]))),
                (
                    "passenger_status",
                    Arc::new(UInt8Array::from(vec![evidence_status])),
                ),
                ("freight_status", status(evidence_status == 1)),
                ("matching", Arc::new(UInt8Array::from(vec![2]))),
            ],
        );
        let receipt = finalize_square(&year, SQUARE).unwrap().unwrap();
        assert_eq!((receipt.rows_in, receipt.rows_out), (1, 3));
        let (_metadata, batch) = read_arrow(&arrow);
        let idx = batch
            .column_by_name("segment_idx")
            .unwrap()
            .as_any()
            .downcast_ref::<Int16Array>()
            .unwrap();
        assert!(idx.iter().all(|value| value == Some(0)));
        let lengths = batch
            .column_by_name("length_m")
            .unwrap()
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        let mut actual_lengths = lengths.values().to_vec();
        let mut expected_lengths = [20.0, 40.0, (pieces[&(7, 0)].to_m - 60.0) as f32];
        actual_lengths.sort_by(f32::total_cmp);
        expected_lengths.sort_by(f32::total_cmp);
        assert_eq!(actual_lengths, expected_lengths);
        let columns = crate::rail_traffic::RailTrafficColumns::read(&batch).unwrap();
        let mut observed = 0;
        let mut defaults = 0;
        for row in 0..batch.num_rows() {
            let result = columns.row(row);
            let passenger = result.passenger.periods.iter().sum::<f64>();
            let freight = result.freight.periods.iter().sum::<f64>();
            if result.passenger.matching == 2 {
                observed += 1;
                assert!((passenger - daily_passenger).abs() < 1e-9);
                assert!((freight - if evidence_status == 1 { 0.0 } else { 13.5 }).abs() < 1e-9);
                assert_eq!(result.passenger.source_id, 100);
                assert_eq!(result.passenger.status, 2); // Daily evidence uses estimated period shares.
                assert_eq!(result.freight.status, 2);
                assert_eq!(result.is_silent(), daily_passenger == 0.0);
            } else {
                defaults += 1;
                assert!((passenger - 80.0).abs() < 1e-9);
                assert!((freight - 13.5).abs() < 1e-9);
                for category in [result.passenger, result.freight] {
                    assert_eq!(
                        (category.status, category.source_id, category.matching),
                        (2, 0, 0)
                    );
                }
                assert!(!result.is_silent());
            }
        }
        assert_eq!((observed, defaults), (1, 2));
    }
    let _ = std::fs::remove_dir_all(year.parent().unwrap());
}

#[test]
fn heritage_unknown_traffic_survives_finalization_and_retry() {
    let year = year_dir();
    let path = year.join("z9/276/173/railways.arrow");
    write_parent_arrow(&path, None);
    let (_, parent) = read_arrow(&path);
    let mut columns = parent.columns().to_vec();
    columns[parent.schema().index_of("rail_type").unwrap()] = Arc::new(UInt8Array::from(vec![5u8]));
    let batch = RecordBatch::try_new(parent.schema(), columns).unwrap();
    let mut writer =
        FileWriter::try_new(File::create(&path).unwrap(), batch.schema().as_ref()).unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
    drop(writer);
    assert!(finalize_square(&year, SQUARE).unwrap().unwrap().rewritten);
    let (_, batch) = read_arrow(&path);
    let traffic = crate::rail_traffic::RailTrafficColumns::read(&batch)
        .unwrap()
        .row(0);
    assert_eq!(traffic, crate::merge::RowTraffic::default());
    assert_eq!(u8_col(&batch, "rail_type"), vec![5]);
    assert!(!finalize_square(&year, SQUARE).unwrap().unwrap().rewritten);
    std::fs::remove_dir_all(year.parent().unwrap()).unwrap();
}

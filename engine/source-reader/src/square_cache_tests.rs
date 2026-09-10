//! Broken optional Arrow files fail both the pure query and native popup boundary.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Int32Array, UInt16Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::{reader::FileReader, writer::FileWriter};
use arrow::record_batch::RecordBatch;
use square_store::store::load_square;

use super::{acquire_squares_parallel, STORE};
use crate::structure_test_fixture as fx;

#[path = "native_receiver_tests.rs"]
mod native_receiver_tests;

fn parallel_square_load_error_leaves_cache_unchanged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let valid_empty_name = "z9/0/0".to_string();
    let invalid_name = "z9/0/1".to_string();
    let invalid_dir = tmp.path().join(&invalid_name);
    std::fs::create_dir_all(&invalid_dir).unwrap();
    fx::write_structure_file(
        &invalid_dir.join("structures.arrow"),
        &[fx::StructureRow::default()],
        false,
    );
    {
        let mut store = STORE.write().expect("square store poisoned");
        store.prepared_dir = tmp.path().display().to_string();
        store.squares.clear();
    }

    let result = acquire_squares_parallel(&[valid_empty_name, invalid_name]);
    let cached_count = STORE.read().expect("square store poisoned").squares.len();
    {
        let mut store = STORE.write().expect("square store poisoned");
        store.prepared_dir.clear();
        store.squares.clear();
    }

    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("invalid square must fail the whole load"),
    };
    let message = error.to_string();
    assert!(
        message.contains("failed to load square z9/0/1"),
        "{message}"
    );
    assert!(
        message.contains("structures_contract mismatch"),
        "{message}"
    );
    assert_eq!(cached_count, 0, "a failed load must not populate the cache");
}

// Valid schema/footer and first batch, but the second IPC message is corrupt.
fn two_batches_with_broken_second_message(path: &Path) {
    let base = fx::structure_batch(&[fx::StructureRow::default()]);
    let mut fields: Vec<_> = base.schema().fields().iter().cloned().collect();
    fields.push(Arc::new(Field::new("start_gx", DataType::Int32, false)));
    fields.push(Arc::new(Field::new("maxspeed", DataType::UInt16, false)));
    let mut metadata = base.schema().metadata().clone();
    metadata.insert(
        "leisure_contract".into(),
        square_store::store::LEISURE_CONTRACT_V2.into(),
    );
    metadata.insert("n_days".into(), "12".into());
    metadata.insert(
        arrow_batching::QM_BLOCKS_KEY.into(),
        arrow_batching::encode_blocks(&[
            arrow_batching::Block {
                cell_x: 8_840,
                cell_y: 5_556,
                bbox: [50.0, 14.25, 50.0, 14.25],
            },
            arrow_batching::Block {
                cell_x: 9_102,
                cell_y: 4_757,
                bbox: [60.0, 20.0, 60.0, 20.0],
            },
        ]),
    );
    let schema = Arc::new(Schema::new_with_metadata(fields, metadata));
    let mut columns = base.columns().to_vec();
    columns.push(Arc::new(Int32Array::from(vec![0])));
    columns.push(Arc::new(UInt16Array::from(vec![80])));
    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
    let mut bytes = Vec::new();
    {
        let mut writer = FileWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    let footer_size =
        i32::from_le_bytes(bytes[bytes.len() - 10..bytes.len() - 6].try_into().unwrap()) as usize;
    let footer =
        arrow::ipc::root_as_footer(&bytes[bytes.len() - 10 - footer_size..bytes.len() - 10])
            .unwrap();
    let offset = footer.recordBatches().unwrap().get(1).offset() as usize;
    assert_eq!(&bytes[offset..offset + 4], &[255; 4]);
    bytes[offset + 8..offset + 12].fill(255);
    let mut reader = FileReader::try_new(Cursor::new(&bytes), None).unwrap();
    assert_eq!(reader.num_batches(), 2);
    assert!(reader.next().unwrap().is_ok());
    assert!(reader.next().unwrap().is_err());
    std::fs::write(path, bytes).unwrap();
}

fn reset_store(root: &Path) {
    let mut store = STORE.write().unwrap();
    store.squares.clear();
    store.prepared_dir = root.display().to_string();
}

fn scan_to_insert_interleaving_preserves_hits_and_active_pins() {
    let tmp = tempfile::tempdir().unwrap();
    reset_store(tmp.path());
    let a = "z9/0/0".to_string();
    let b = "z9/1/0".to_string();
    let c = "z9/2/0".to_string();
    let d = "z9/3/0".to_string();

    let warmed_a = acquire_squares_parallel(std::slice::from_ref(&a)).unwrap();
    drop(warmed_a);
    let (scan_pins, missing) = STORE.read().unwrap().pin_cached(&[a.clone(), b.clone()]);
    assert_eq!(missing.as_slice(), std::slice::from_ref(&b));

    // Deterministically interleave a disjoint acquisition between the first
    // request's cache scan and its final insertion. Its scan pin must keep A
    // discoverable while B is being loaded outside the lock.
    let pinned_c = acquire_squares_parallel(std::slice::from_ref(&c)).unwrap();
    {
        let store = STORE.read().unwrap();
        assert!(store.squares.contains_key(&a));
        assert!(store.squares.contains_key(&c));
    }
    let mixed = acquire_squares_parallel(&[a.clone(), b.clone()]).unwrap();
    assert!(Arc::ptr_eq(&scan_pins[0], &mixed[0]));
    {
        let store = STORE.read().unwrap();
        assert!(store.squares.contains_key(&a));
        assert!(store.squares.contains_key(&b));
        assert!(store.squares.contains_key(&c));
    }

    drop(scan_pins);
    drop(mixed);
    drop(pinned_c);
    let pinned_d = acquire_squares_parallel(std::slice::from_ref(&d)).unwrap();
    let store = STORE.read().unwrap();
    assert_eq!(store.squares.len(), 1);
    assert!(Arc::ptr_eq(&pinned_d[0], store.squares.get(&d).unwrap()));
    drop(store);
    drop(pinned_d);
    reset_store(Path::new(""));
}

#[test]
fn native_queries_preserve_receiver_sources_and_reject_broken_arrow() {
    // One test owns STORE; no competing process-wide cache fixture.
    parallel_square_load_error_leaves_cache_unchanged();
    scan_to_insert_interleaving_preserves_hits_and_active_pins();
    let tmp = tempfile::tempdir().unwrap();
    super::YEAR_DIR.set(tmp.path().to_path_buf()).unwrap();
    let (lat, lon) = (60.0, 20.0);
    let dir = fx::square_dir(tmp.path(), grid::square_of(lat, lon));
    std::fs::create_dir_all(&dir).unwrap();
    reset_store(tmp.path());
    assert_eq!(super::query_roads(lat, lon, 1000.0).unwrap(), "[]");
    let empty = super::collect_sources_at_point(tmp.path(), lat, lon).unwrap();
    assert!(empty.roads.is_empty() && empty.aircraft_airborne_batches.is_empty());

    for name in [
        "roads",
        "railways",
        "structures",
        "industrial",
        "leisure",
        "airborne",
        "cruise",
        "airport_traffic",
        "airport_lines",
    ] {
        if name != "structures" {
            fx::write_square_structures(tmp.path(), grid::square_of(lat, lon), &[]);
        }
        let path = dir.join(format!("{name}.arrow"));
        std::fs::write(&path, b"not Arrow").unwrap();
        reset_store(tmp.path());
        let pure = super::collect_sources_at_point(tmp.path(), lat, lon).unwrap_err();
        let native = super::query_roads(lat, lon, 1000.0).unwrap_err();
        assert!(pure.contains(&path.display().to_string()), "{pure}");
        assert!(
            native.reason.contains(&path.display().to_string()),
            "{native}"
        );
        assert!(STORE.read().unwrap().squares.is_empty());
        std::fs::remove_file(&path).unwrap();

        two_batches_with_broken_second_message(&path);
        if name == "airport_lines" {
            let mut reader =
                FileReader::try_new(std::fs::File::open(&path).unwrap(), None).unwrap();
            let batch = reader.next().unwrap().unwrap();
            let mut writer = FileWriter::try_new(
                std::fs::File::create(dir.join("airport_traffic.arrow")).unwrap(),
                &batch.schema(),
            )
            .unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }
        let data = load_square(&dir).expect("opening must remain footer-only");
        if name == "roads" {
            let first = data.roads.batches_within(50.0, 14.25, 1000.0).unwrap();
            let again = data.roads.batches_where(|bbox| bbox[0] == 50.0).unwrap();
            assert_eq!(
                first.len(),
                1,
                "the distant corrupt body must remain pruned"
            );
            assert!(
                Arc::ptr_eq(first[0].column(0), again[0].column(0)),
                "healthy batches stay cached"
            );
            let error = data.roads.batches_all().unwrap_err();
            assert_eq!(data.roads.batches_all().unwrap_err(), error);
        }
        let pure = super::collect_from_square_data(&[(grid::square_of(lat, lon), &data)], lat, lon)
            .unwrap_err();
        assert!(pure.contains(&path.display().to_string()), "{pure}");
        assert!(pure.contains("batch 1"), "{pure}");
        reset_store(tmp.path());
        // A rewritten structures table no longer pairs with the pipeline's
        // index, so the popup is refused before it decodes one batch.
        let (native_file, native_error) = if name == "structures" {
            (dir.join("structures.qoix"), "structures-finalize")
        } else {
            (path.clone(), "batch 1")
        };
        for _ in 0..2 {
            let native = super::query_noise_at_point(lat, lon).unwrap_err();
            assert!(
                native.reason.contains(&native_file.display().to_string()),
                "{native}"
            );
            assert!(native.reason.contains(native_error), "{native}");
            let listing = match name {
                "roads" => Some(super::query_roads(lat, lon, 1000.0)),
                "structures" => Some(super::query_buildings(lat, lon, 1000.0)),
                _ => None,
            };
            if let Some(result) = listing {
                assert!(result.unwrap_err().reason.contains("batch 1"));
            }
            if name == "structures" {
                assert!(super::query_barriers(lat, lon, 1000.0)
                    .unwrap_err()
                    .reason
                    .contains("batch 1"));
            }
        }
        reset_store(tmp.path());
        drop(data);
        std::fs::remove_file(&path).unwrap();
        if name == "airport_lines" {
            std::fs::remove_file(dir.join("airport_traffic.arrow")).unwrap();
        }
    }
    std::fs::remove_file(dir.join("structures.arrow")).unwrap();
    let path = dir.join("industrial.arrow");
    std::fs::write(&path, []).unwrap();
    assert!(
        load_square(&dir).is_err(),
        "zero bytes are not a valid empty Arrow"
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(
        load_square(&dir).is_err(),
        "a directory is not an absent optional file"
    );
    std::fs::remove_dir(&path).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("missing.arrow", &path).unwrap();
        assert!(
            load_square(&dir).is_err(),
            "a dangling input link is broken, not absent"
        );
        std::fs::remove_file(&path).unwrap();
    }
    fx::write_roads_file(&dir.join("roads.arrow"), &[]);
    reset_store(tmp.path());
    assert_eq!(super::query_roads(lat, lon, 1000.0).unwrap(), "[]");
    native_receiver_tests::native_listings_honor_requested_radius(tmp.path());
    native_receiver_tests::facade_popup_preserves_aircraft_and_observation_multiplicity(tmp.path());
    let initialized = tmp.path().display().to_string();
    let cached = STORE.read().unwrap().squares.len();
    assert!(super::source_init(initialized.clone())
        .unwrap()
        .contains("already initialized"));
    assert!(super::source_init(initialized.clone())
        .unwrap()
        .contains("already initialized"));
    assert!(
        super::source_init(tmp.path().join("different").display().to_string())
            .unwrap_err()
            .reason
            .contains("cannot change within one native process")
    );
    assert!(super::source_init(String::new())
        .unwrap_err()
        .reason
        .contains("cannot be empty"));
    assert!(super::source_init(initialized.clone())
        .unwrap()
        .contains("already initialized"));
    assert_eq!(STORE.read().unwrap().prepared_dir, initialized);
    assert_eq!(STORE.read().unwrap().squares.len(), cached);
    assert_eq!(super::year_dir().unwrap(), tmp.path());
    reset_store(Path::new(""));
}

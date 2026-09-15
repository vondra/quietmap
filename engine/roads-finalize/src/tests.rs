//! Regression classes: count basis, longitudinal identity, priors and cross-owner finalization.

use crate::{allocation, input::Road};
use noise_compute::sources::{provenance_of, Provenance};
use noise_compute::square_country_city::SquareCountryCity;

fn road(id: i64, basis: u8, reverse: bool) -> Road {
    Road { way_id: id, segment_idx: 0, start: (if reverse { 10.0 } else { 0.0 }, 0.0),
        end: (if reverse { 10.0 } else { 0.0 }, 100.0), direction: if reverse { 2 } else { 1 },
        class: 2, lanes: 2, access: 0, tunnel: false, country: SquareCountryCity::UNKNOWN,
        source_id: 10, observation_source_id: 10, provenance: provenance_of(10), counts: [10_000.0, 0.0, 0.0, 0.0],
        basis, estimated: 0, observation: "counter:A".to_owned(), corridor: "R1".to_owned() }
}

#[test]
fn directional_counts_never_change_with_osm_direction_or_other_carriageways() {
    let other = road(2, 1, true);
    for direction in 0..=2 {
        let mut observed = road(1, 1, false);
        observed.direction = direction;
        assert_eq!(allocation::resolve(&observed, [&other]), ([10_000.0, 0.0, 0.0, 0.0], 0));
    }
}

#[test]
fn section_total_is_shared_once_while_longitudinal_subdivisions_keep_through_flow() {
    let forward = road(1, 2, false);
    let reverse = road(2, 2, true);
    let whole_forward = allocation::resolve(&forward, [&reverse]);
    let whole_reverse = allocation::resolve(&reverse, [&forward]);
    assert_eq!(whole_forward, ([5000.0, 0.0, 0.0, 0.0], 15));
    assert_eq!(whole_forward.0[0] + whole_reverse.0[0], 10_000.0);
    let mut first = forward.clone();
    let mut second = forward.clone();
    first.end.1 = 50.0;
    second.start.1 = 50.0;
    second.segment_idx = 1;
    assert_eq!(allocation::resolve(&reverse, [&first, &second]), whole_reverse);
    assert_eq!(allocation::resolve(&first, [&reverse]), whole_forward);
    assert_eq!(allocation::resolve(&second, [&reverse]), whole_forward);
    let already_allocated = Road { basis: 3, counts: whole_forward.0, estimated: 15, ..forward };
    assert_eq!(allocation::resolve(&already_allocated, [&reverse]), whole_forward);
}

#[test]
fn staggered_carriageway_ends_split_the_longer_source_and_conserve_each_cross_section() {
    let long = road(1, 2, false);
    let short = Road { end: (10.0, 40.0), ..road(2, 2, true) };
    let long_intervals = allocation::intervals(&long, &[&short]);
    let short_intervals = allocation::intervals(&short, &[&long]);
    assert_eq!(long_intervals.len(), 2);
    assert_eq!((long_intervals[0].0, long_intervals[0].1), (0.0, 0.4));
    assert_eq!((long_intervals[1].0, long_intervals[1].1), (0.4, 1.0));
    assert_eq!(long_intervals[0].2[0] + short_intervals[0].2[0], 10_000.0);
    assert_eq!(long_intervals[1].2[0], 10_000.0);
    let first = Road { end: (0.0, 30.0), ..long.clone() };
    let second = Road { start: (0.0, 30.0), segment_idx: 1, ..long };
    assert_eq!(allocation::intervals(&first, &[&short])[0].2[0], 5000.0);
    let second_intervals = allocation::intervals(&second, &[&short]);
    assert_eq!(second_intervals[0].2[0], 5000.0);
    assert_eq!(second_intervals[1].2[0], 10_000.0);
}

#[test]
fn more_than_two_alternatives_form_one_cross_section_instead_of_pairwise_divisors() {
    let a = road(1, 2, false);
    let b = Road { start: (40.0, 0.0), end: (40.0, 100.0), ..road(2, 2, true) };
    let c = Road { start: (80.0, 0.0), end: (80.0, 100.0), ..road(3, 2, false) };
    let index = crate::spatial::RoadIndex::new(vec![a.clone(), b.clone(), c.clone()]);
    let totals = [&a, &b, &c].map(|r| allocation::resolve(r, index.alternatives(r)).0[0]);
    assert!((totals.iter().sum::<f64>() - 10_000.0).abs() < 1e-9);
    assert!(totals.iter().all(|total| (*total - 10_000.0 / 3.0).abs() < 1e-9));
}

#[test]
fn unrelated_parallel_street_does_not_steal_a_section_count() {
    let observed = road(1, 2, false);
    let mut other = road(2, 2, true);
    other.corridor = "unrelated".to_owned();
    other.observation = "different counter".to_owned();
    assert_eq!(allocation::resolve(&observed, [&other]).0[0], 10_000.0);
}

#[test]
fn standalone_directional_prior_keeps_legacy_level_and_divided_roads_share_one_prior() {
    let a = Road { source_id: 0, provenance: Provenance::None, counts: [0.0; 4], basis: 0,
        estimated: 15, observation: String::new(), class: 0, lanes: 3, ..road(1, 0, false) };
    let b = Road { way_id: 2, lanes: 2, direction: 2, start: (10.0, 0.0), end: (10.0, 100.0), ..a.clone() };
    let standalone = allocation::resolve(&a, []);
    let split_a = allocation::resolve(&a, [&b]);
    let split_b = allocation::resolve(&b, [&a]);
    assert_eq!(standalone, split_a);
    assert_eq!(split_a.1, 15);
    assert!((split_a.0.iter().sum::<f64>() + split_b.0.iter().sum::<f64>() - 30_000.0 * 1.42).abs() < 1e-8);
    let local = Road { class: 8, lanes: 0, direction: 0, ..a };
    let count = allocation::resolve(&local, []).0;
    assert!(count.iter().any(|v| *v > 0.0 && *v < 1.0), "fractional quiet-road priors survive");
}

#[test]
fn measured_heavy_only_keeps_positive_traffic_at_restricted_access() {
    let input = Road { counts: [0.0, 0.0, 500.0, 0.0], access: 2, ..road(1, 1, false) };
    assert_eq!(allocation::resolve(&input, []).0, [0.0, 0.0, 500.0, 0.0]);
}

#[test]
fn cross_owner_ipc_allocates_before_promoting_and_rerun_is_byte_identical() {
    cross_owner_ipc(false);
}

#[test]
fn dateline_crossing_ipc_splits_along_the_short_wrapped_segment() {
    cross_owner_ipc(true);
    let half = grid::EARTH_CIRCUMFERENCE_M / 2.0;
    let a = Road { start: (half - 5.0, 0.0), end: (half - 5.0, 100.0), ..road(1, 2, false) };
    let b = Road { start: (-half + 5.0, 0.0), end: (-half + 5.0, 100.0), ..road(2, 2, true) };
    let index = crate::spatial::RoadIndex::new(vec![a.clone(), b.clone()]);
    for road in [&a, &b] {
        assert_eq!(allocation::resolve(road, index.alternatives(road)).0[0], 5000.0);
    }
}

fn cross_owner_ipc(dateline: bool) {
    use arrow::array::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::ipc::writer::FileWriter;
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("road-finalize-test-{}-{dateline}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let year = root.join("2026");
    let mut paths = Vec::new();
    let mut original_bytes = Vec::new();
    for (way, x, square, direction) in [(1_i64, -5.0, "z9/255/255", 1_u8), (2, 5.0, "z9/256/255", 2)] {
        let directory = year.join(square);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("roads.arrow");
        let coordinate = |x, y| {
            if dateline {
                let (gx, gy) = grid::meters_to_grid(grid::EARTH_CIRCUMFERENCE_M / 2.0 - 20.0 + y, x);
                (gx.rem_euclid(1 << 30), gy)
            } else { grid::meters_to_grid(x, y) }
        };
        let (sx, sy) = coordinate(x, 0.0);
        let length = if way == 1 { 100.0_f32 } else { 40.0 };
        let (ex, ey) = coordinate(x, length as f64);
        let mut columns: Vec<(&str, ArrayRef)> = vec![
            ("osm_id", Arc::new(Int64Array::from(vec![way]))),
            ("segment_idx", Arc::new(Int16Array::from(vec![0]))),
            ("length_m", Arc::new(Float32Array::from(vec![length]))),
            ("start_gx", Arc::new(Int32Array::from(vec![sx]))),
            ("start_gy", Arc::new(Int32Array::from(vec![sy]))),
            ("end_gx", Arc::new(Int32Array::from(vec![ex]))),
            ("end_gy", Arc::new(Int32Array::from(vec![ey]))),
            ("oneway", Arc::new(UInt8Array::from(vec![direction]))),
            ("road_class", Arc::new(UInt8Array::from(vec![2]))),
            ("lanes", Arc::new(UInt8Array::from(vec![2]))),
            ("access", Arc::new(UInt8Array::from(vec![0]))),
            ("tunnel", Arc::new(BooleanArray::from(vec![false]))),
            ("country_iso", Arc::new(UInt16Array::from(vec![0]))),
            ("city_id", Arc::new(UInt16Array::from(vec![0]))),
            ("continent", Arc::new(UInt8Array::from(vec![0]))),
            ("source_id", Arc::new(UInt16Array::from(vec![10]))),
            ("traffic_observation_source", Arc::new(UInt16Array::from(vec![10]))),
            ("ref", Arc::new(StringArray::from(vec!["R1"]))),
            ("traffic_count_basis", Arc::new(UInt8Array::from(vec![2]))),
            ("traffic_estimated", Arc::new(UInt8Array::from(vec![0]))),
            ("traffic_observation_id", Arc::new(StringArray::from(vec!["counter:A"]))),
        ];
        for (i, name) in crate::input::COUNTS.iter().enumerate() {
            columns.push((name, Arc::new(Float64Array::from(vec![if i == 0 { 10_000.0 } else { 0.0 }]))));
        }
        let schema = Arc::new(Schema::new(columns.iter().map(|(name, array)| Field::new(*name, array.data_type().clone(), false)).collect::<Vec<_>>()));
        let batch = RecordBatch::try_new(schema.clone(), columns.into_iter().map(|(_, array)| array).collect()).unwrap();
        let mut writer = FileWriter::try_new(std::fs::File::create(&path).unwrap(), &schema).unwrap();
        writer.write(&batch).unwrap(); writer.finish().unwrap();
        original_bytes.push(std::fs::read(&path).unwrap());
        paths.push(path);
    }
    let serial = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let corrupt = year.join("z9/300/255/roads.arrow");
    std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    std::fs::write(&corrupt, b"invalid Arrow").unwrap();
    assert!(serial.install(|| crate::finalize_year(&year)).is_err());
    assert_eq!(paths.iter().map(std::fs::read).collect::<Result<Vec<_>, _>>().unwrap(), original_bytes,
        "a later owner failure must not promote already staged output");
    assert!(year.join(".roads-finalize/z9/255/255/roads.arrow").is_file());
    assert!(!year.join(".roads-finalize/ready").exists());
    assert_eq!(std::fs::read(&corrupt).unwrap(), b"invalid Arrow");
    std::fs::remove_file(corrupt).unwrap();
    assert_eq!(serial.install(|| crate::finalize_year(&year)).unwrap(), 2);
    let serial_output = paths.iter().map(|path| crate::input::load(path).unwrap()).collect::<Vec<_>>();
    for (path, bytes) in paths.iter().zip(&original_bytes) { std::fs::write(path, bytes).unwrap(); }
    let parallel = rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap();
    assert_eq!(parallel.install(|| crate::finalize_year(&year)).unwrap(), 2);
    for (path, expected) in paths.iter().zip(serial_output) {
        assert_eq!(crate::input::load(path).unwrap(), expected, "parallel allocation must match serial batches and values");
    }
    for (index, path) in paths.iter().enumerate() {
        let batches = crate::input::load(path).unwrap();
        let batch = &batches[0];
        assert_eq!(batch.schema().metadata().get(crate::input::CONTRACT).map(String::as_str), Some("1"));
        assert!(batch.column_by_name("traffic_observation_id").is_none());
        assert!(batch.column_by_name("traffic_count_basis").is_none());
        assert_eq!(batch.column_by_name("aadt_light").unwrap().data_type(), &DataType::Float64);
        let mut rows = batches.iter().flat_map(|batch| crate::input::roads(batch).unwrap()).collect::<Vec<_>>();
        if dateline {
            for row in &mut rows {
                let shift = ((grid::EARTH_CIRCUMFERENCE_M / 2.0 - row.midpoint().0) / grid::EARTH_CIRCUMFERENCE_M).round()
                    * grid::EARTH_CIRCUMFERENCE_M;
                row.start.0 += shift;
                row.end.0 += shift;
                assert!((row.end.0 - row.start.0).abs() < 101.0);
            }
            rows.sort_by(|a, b| a.start.0.total_cmp(&b.start.0));
        } else { rows.sort_by(|a, b| a.start.1.total_cmp(&b.start.1)); }
        if index == 0 {
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].counts, [5000.0, 0.0, 0.0, 0.0]);
            assert_eq!(rows[1].counts, [10_000.0, 0.0, 0.0, 0.0]);
            assert!((rows[0].end.0 - rows[1].start.0).hypot(rows[0].end.1 - rows[1].start.1) < 1e-6);
        } else { assert_eq!(rows[0].counts, [5000.0, 0.0, 0.0, 0.0]); }
    }
    let before = paths.iter().map(std::fs::read).collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(crate::finalize_year(&year).unwrap(), 0);
    assert_eq!(paths.iter().map(std::fs::read).collect::<Result<Vec<_>, _>>().unwrap(), before);
    let pending = year.join(".roads-finalize/z9/256/255/roads.arrow");
    std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
    std::fs::write(&pending, &before[1]).unwrap();
    std::fs::write(year.join(".roads-finalize/ready"), []).unwrap();
    std::fs::write(&paths[1], &original_bytes[1]).unwrap();
    assert_eq!(crate::finalize_year(&year).unwrap(), 1, "resume partially promoted generation");
    assert_eq!(paths.iter().map(std::fs::read).collect::<Result<Vec<_>, _>>().unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dense_owner_uses_its_own_batch_without_serializing_sparse_owners() {
    let allowances = [2, 2, 8, 2, 2, 2];
    let mut start = 0;
    let mut ranges = Vec::new();
    while start < allowances.len() {
        let end = crate::scheduling::batch_end(&allowances, start, 3, 8);
        assert!(end - start <= 3);
        assert!(allowances[start..end].iter().sum::<u64>() <= 8);
        ranges.push(start..end);
        start = end;
    }
    assert_eq!(ranges, [0..2, 2..3, 3..6]);
}

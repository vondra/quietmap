//! Reject normalized streams that silently lose the estimate for unobserved calendar dates.
use super::{Movement, Record, STREAM_VERSION, read, write_record};

#[test]
fn a_normalized_stream_requires_the_current_version_and_its_calendar_component() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("movements.jsonl");
    let mut movement: Movement = serde_json::from_value(serde_json::json!({
        "id":"partial", "source_id":1, "country":"NO", "passenger":2.0, "freight":null,
        "passenger_status":2, "freight_status":0, "matching":0, "rail_types":[0],
        "coverage":{"window_days":4, "covered":[1,0,0,0,0,0]},
        "location":{"geometry":"unlocated", "reason":"test has no mapped station"}
    }))
    .unwrap();
    let write = |version, row: &Movement| {
        let mut file = std::fs::File::create(&path).unwrap();
        write_record(
            &mut file,
            &Record::Window {
                version,
                start: "2024-01-01".into(),
                end: "2024-01-05".into(),
            },
        )
        .unwrap();
        write_record(&mut file, &Record::Movement(Box::new(row.clone()))).unwrap();
    };
    let read_stream = || read(&path, "2024-01-01", "2024-01-05", |_| Ok(()));
    write(STREAM_VERSION - 1, &movement);
    assert!(read_stream().is_err());
    write(STREAM_VERSION, &movement);
    assert!(read_stream().unwrap_err().contains("calendar estimate"));
    movement.estimate_missing_calendar();
    assert_eq!(movement.missing_calendar_estimate, [6.0, 0.0]);
    write(STREAM_VERSION, &movement);
    read_stream().unwrap();
}

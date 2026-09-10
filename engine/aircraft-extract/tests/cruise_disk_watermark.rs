//! Real Arrow writes respect a scoped spill watermark before replacing retained output.

use aircraft_extract::arrow_io::{write_flights, FlightRow, SpillDiskReservation};

#[test]
fn oversized_write_is_refused_and_prior_output_survives_then_guard_releases() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("flights.arrow");
    std::fs::write(&output, b"retained original").unwrap();
    let reservation = SpillDiskReservation::new(temp.path(), 8 * 1024 * 1024, 1).unwrap();
    let callsign = "X".repeat(16 * 1024 * 1024);
    let row = FlightRow {
        flight_id: 1,
        callsign: &callsign,
        aircraft_type: b"A320",
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        base_timestamp: 0.0,
        points: &[],
    };
    let error = write_flights(&output, &[row]).unwrap_err();
    assert!(format!("{error:#}").contains("disk admission"), "{error:#}");
    assert_eq!(std::fs::read(&output).unwrap(), b"retained original");
    assert!(
        temp.path()
            .join("flights.arrow.tmp")
            .metadata()
            .unwrap()
            .len()
            < 8 * 1024 * 1024
    );
    drop(reservation);
    write_flights(&output, &[]).unwrap();
    assert_ne!(std::fs::read(output).unwrap(), b"retained original");
}

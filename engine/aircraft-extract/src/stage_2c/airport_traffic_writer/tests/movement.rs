//! Every touched microsegment carries the observed rotation.
use super::*;
#[test]
fn flight_ids_touch_every_intersected_microseg() {
    use crate::arrow_io::{read_airport_traffic, write_segments};
    use crate::flight::{FlightSegment, Phase};
    use std::collections::HashSet;
    let tmp = tempfile::tempdir().unwrap();
    let by_square_dir = tmp.path().join("segments_by_square");
    let prepared_year_dir = tmp.path().join("prepared_year");
    let lat = 50.0_f64;
    let dlon = 0.001397_f64;
    let lon0 = 14.0;
    let lon1 = lon0 + dlon;
    let lon2 = lon0 + 2.0 * dlon;
    let lon3 = lon0 + 3.0 * dlon;
    let mid_lat = lat;
    let mid_lon = (lon0 + lon3) * 0.5;
    let square = crate::spatial::square_id(mid_lat, mid_lon).expect("valid square");
    let square_prepared_year_dir = prepared_year_dir.join(square_path(square));
    let square_input_dir = by_square_dir.join(square_path(square));
    std::fs::create_dir_all(&square_prepared_year_dir).unwrap();
    std::fs::create_dir_all(&square_input_dir).unwrap();
    write_real_airport_lines_arrow(
        &square_prepared_year_dir.join("airport_lines.arrow"),
        &[
            FakeRealLine {
                osm_id: 42,
                segment_idx: 0,
                start_lat: lat,
                start_lon: lon0,
                end_lat: lat,
                end_lon: lon1,
                length_m: 100.0,
                aeroway_type: 0,
            },
            FakeRealLine {
                osm_id: 42,
                segment_idx: 1,
                start_lat: lat,
                start_lon: lon1,
                end_lat: lat,
                end_lon: lon2,
                length_m: 100.0,
                aeroway_type: 0,
            },
            FakeRealLine {
                osm_id: 42,
                segment_idx: 2,
                start_lat: lat,
                start_lon: lon2,
                end_lat: lat,
                end_lon: lon3,
                length_m: 100.0,
                aeroway_type: 0,
            },
        ],
    );

    let leg = FlightSegment {
        flight_id: 0xDEAD_BEEF_u64,
        callsign: "TEST123".to_string(),
        aircraft_type: *b"B738",
        profile_idx: 23, // narrowbody jet
        source_id: 0,
        origin: 0,
        veh_kind: 0, // aircraft
        gse_class: 0,
        period: 0, // day
        date_id: 0,
        phase: Phase::Ground,
        flags: 0,
        start_lat: lat as f32,
        start_lon: lon0 as f32,
        start_alt_m: 0.0,
        end_lat: lat as f32,
        end_lon: lon3 as f32,
        end_alt_m: 0.0,
        speed_kt: 90.0,
        length_m: 300.0,
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    };

    let aerodrome = AirportArea::new(
        1,
        AERODROME_AEROWAY_TYPE,
        "Test Aerodrome".to_string(),
        "LKTEST".to_string(),
        lat,
        lon0 + 1.5 * dlon,
        Vec::new(),
        100_000_000.0,
    );

    write_segments(&square_input_dir.join("ground.arrow"), &[leg]).unwrap();

    let n = run_airport_traffic(
        &by_square_dir,
        std::slice::from_ref(&aerodrome),
        &prepared_year_dir,
        1,
        365,
        None,
    )
    .unwrap();
    assert!(n > 0, "writer must populate at least one z9");

    let traffic_path = square_prepared_year_dir.join("airport_traffic.arrow");
    assert!(traffic_path.exists(), "airport_traffic.arrow must exist");
    let rows = read_airport_traffic(&traffic_path).unwrap();

    let our_rows: Vec<_> = rows.iter().filter(|r| r.osm_id == 42).collect();
    let seg_idxs: HashSet<u16> = our_rows.iter().map(|r| r.segment_idx).collect();
    assert_eq!(
        seg_idxs,
        HashSet::from([0u16, 1, 2]),
        "all three microsegments must have a row under v5 touch semantics; got {seg_idxs:?}"
    );

    for r in &our_rows {
        assert_eq!(
            r.unique_movement_count, 1,
            "microsegment {} must show one unique movement (v5 scalar); got {}",
            r.segment_idx, r.unique_movement_count,
        );
        assert_eq!(
            r.microseg_unique_count, 1,
            "microsegment {} row-replicated UNION must show one unique movement; got {}",
            r.segment_idx, r.microseg_unique_count,
        );
        assert_eq!(
            r.microseg_unique_ga_count, 0,
            "B738 (non-GA jet) must NOT land in the GA microseg split"
        );
        assert!(
            r.band_energy_lin.iter().any(|b| *b > 0.0),
            "microsegment {} must have positive band_energy_lin",
            r.segment_idx,
        );
    }
}

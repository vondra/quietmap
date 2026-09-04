//! Observed aircraft data processing on the canonical square grid.

use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    Float32Builder, Int16Builder, Int32Builder, Int64Builder, StringBuilder, UInt8Builder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;

use aircraft_extract::airport_index::AerodromeIndex;
use aircraft_extract::arrow_io::read_airport_traffic;
use aircraft_extract::flight::{FlightSegment, Phase};
use aircraft_extract::stage_2c::airport_traffic_writer::run_airport_traffic;
use aircraft_extract::stage_airport_discover_runner::run_stage_airport_discover;

const TEST_LAT: f32 = 30.0;
const TEST_LON: f32 = -40.0;

fn square_path_for(lat: f64, lon: f64) -> String {
    aircraft_extract::spatial::square_path(aircraft_extract::spatial::square_id(lat, lon).unwrap())
}

fn ground_segment(start_lat: f32, start_lon: f32, end_lat: f32, end_lon: f32) -> FlightSegment {
    FlightSegment {
        flight_id: 1,
        callsign: String::new(),
        aircraft_type: [0u8; 4],
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        veh_kind: 0,
        gse_class: 0,
        period: 0,
        date_id: 0,
        phase: Phase::Ground,
        flags: 0,
        start_lat,
        start_lon,
        start_alt_m: 0.0,
        end_lat,
        end_lon,
        end_alt_m: 0.0,
        speed_kt: 30.0,
        length_m: 100.0,
        agl_avg_m: 0.0,
        start_elev_m: 0.0,
        end_elev_m: 0.0,
    }
}

/// Write a stub `airport_lines.arrow` matching the real
/// osm-extract schema. Far enough from the test cluster that
/// Stage 1.5's miss-snap detection treats every test vertex as
/// a candidate.
fn write_stub_airport_lines(path: &Path) {
    let schema = Arc::new(
        Schema::new(vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("segment_idx", DataType::Int16, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("start_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("length_m", DataType::Float32, false),
            Field::new("heading_deg", DataType::Float32, false),
            Field::new("aeroway_type", DataType::UInt8, false),
            Field::new("ref", DataType::Utf8, true),
            Field::new("surface", DataType::Utf8, true),
            Field::new("width_m", DataType::Float32, true),
        ])
        .with_metadata({
            let mut md = std::collections::HashMap::new();
            md.insert("schema_version".to_string(), "v13".to_string());
            md
        }),
    );

    let mut osm_id = Int64Builder::new();
    let mut seg_idx = Int16Builder::new();
    let mut sla = Int32Builder::new();
    let mut slo = Int32Builder::new();
    let mut ela = Int32Builder::new();
    let mut elo = Int32Builder::new();
    let mut len = Float32Builder::new();
    let mut heading = Float32Builder::new();
    let mut atype = UInt8Builder::new();
    let mut ref_col = StringBuilder::new();
    let mut surface = StringBuilder::new();
    let mut width = Float32Builder::new();

    // One real "decoy" runway segment 10 km north of the test
    // cluster. Far enough that the 50 m miss-snap buffer won't
    // catch any test vertex but close enough to share an z9 cell
    osm_id.append_value(1);
    seg_idx.append_value(0);
    let (start_gx, start_gy) = grid::lonlat_to_grid(TEST_LON as f64, TEST_LAT as f64 + 0.1);
    let (end_gx, end_gy) = grid::lonlat_to_grid(TEST_LON as f64 + 0.001, TEST_LAT as f64 + 0.1);
    sla.append_value(start_gy);
    slo.append_value(start_gx);
    ela.append_value(end_gy);
    elo.append_value(end_gx);
    len.append_value(96.5);
    heading.append_value(90.0);
    atype.append_value(0); // runway
    ref_col.append_null();
    surface.append_null();
    width.append_null();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(seg_idx.finish()),
            Arc::new(sla.finish()),
            Arc::new(slo.finish()),
            Arc::new(ela.finish()),
            Arc::new(elo.finish()),
            Arc::new(len.finish()),
            Arc::new(heading.finish()),
            Arc::new(atype.finish()),
            Arc::new(ref_col.finish()),
            Arc::new(surface.finish()),
            Arc::new(width.finish()),
        ],
    )
    .unwrap();
    let f = std::fs::File::create(path).unwrap();
    let mut writer = FileWriter::try_new(f, &schema).unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
}

fn build_ground_strip() -> Vec<FlightSegment> {
    // 60 ground segments arranged head-to-tail along a 1 km east-west
    // line at TEST_LAT/TEST_LON. Density (~17 m per segment) sits
    // well above the DBSCAN min-samples floor.
    let dlon_for_1km = (1000.0 / 111_320.0) as f32 / (TEST_LAT.to_radians()).cos();
    let mut segs = Vec::with_capacity(60);
    for i in 0..60 {
        let lon_a = TEST_LON + dlon_for_1km * (i as f32) / 60.0;
        let lon_b = TEST_LON + dlon_for_1km * ((i + 1) as f32) / 60.0;
        segs.push(ground_segment(TEST_LAT, lon_a, TEST_LAT, lon_b));
    }
    segs
}

#[test]
fn stage_1_5_then_stage_2c_round_trips_synth_airport_key() {
    let tmp = tempfile::tempdir().unwrap();
    let prepared_year_dir = tmp.path();
    let square_name = square_path_for(TEST_LAT as f64, TEST_LON as f64);
    let square_dir = prepared_year_dir.join(&square_name);
    std::fs::create_dir_all(&square_dir).unwrap();

    // Stub a real airport_lines.arrow that does NOT cover the test
    // strip's geometry — Stage 1.5 must produce miss-snap candidates
    // for every test vertex.
    write_stub_airport_lines(&square_dir.join("airport_lines.arrow"));

    let segs = build_ground_strip();

    // Materialise per-z9 ground shard (what shuffle would produce in
    // a full run) so Stage 1.5 / 2C consume the post-shuffle layout.
    let by_square_dir = tmp.path().join("segments_by_square");
    let by_square_square_dir = by_square_dir.join(&square_name);
    std::fs::create_dir_all(&by_square_square_dir).unwrap();
    aircraft_extract::arrow_io::write_segments(&by_square_square_dir.join("ground.arrow"), &segs)
        .unwrap();

    // No `airport_areas.arrow` file → empty global aerodromes →
    // cluster gets classified as SynthAirport (not Reattribute).
    let areas: Vec<noise_compute::types::AirportArea> = Vec::new();

    let r1_5 = run_stage_airport_discover(
        &by_square_dir,
        &AerodromeIndex::build(&areas),
        &[],
        prepared_year_dir,
        None,
    )
    .unwrap();
    assert_eq!(r1_5, 1, "Stage 1.5 should populate exactly one z9");

    let synth_lines_path = square_dir.join("synth_airport_lines.arrow");
    assert!(
        synth_lines_path.exists(),
        "Stage 1.5 must have written synth_airport_lines.arrow"
    );

    // Stage 2C reads both the real airport_lines.arrow (no relevant
    // coverage) and the synth file (covers the test strip), and
    // emits airport_traffic.arrow with rows under the synth key.
    let n_days = 1u16;
    let r2c =
        run_airport_traffic(&by_square_dir, &areas, prepared_year_dir, n_days, 0, None).unwrap();
    assert_eq!(
        r2c, 1,
        "Stage 2C should write airport_traffic.arrow for one z9"
    );

    let traffic_path = square_dir.join("airport_traffic.arrow");
    let traffic_rows = read_airport_traffic(&traffic_path).unwrap();
    assert!(
        !traffic_rows.is_empty(),
        "airport_traffic.arrow must have rows from snapped synth lines"
    );

    // Every row must carry the content-addressed synth key.
    let auto_keys: std::collections::HashSet<&str> = traffic_rows
        .iter()
        .map(|r| r.airport_key.as_str())
        .filter(|k| k.starts_with("auto-"))
        .collect();
    assert_eq!(
        auto_keys.len(),
        1,
        "exactly one synth airport_key expected (rows: {})",
        traffic_rows
            .iter()
            .map(|r| r.airport_key.as_str())
            .collect::<Vec<_>>()
            .join(",")
    );
    let synth_key = *auto_keys.iter().next().unwrap();

    // Without Stage 1.5, those vertices would have fallen through
    // to `strip:<cruise_cell_id>` (the orphan fallback in `resolve_airport_key`)
    // because the stub airport_lines.arrow doesn't cover them. The
    // synth key proves the discovery → emission → consumption chain
    // worked.
    assert!(
        !traffic_rows.iter().any(|r| r.airport_key.starts_with("strip:")),
        "no row should fall through to the strip:z15 orphan bucket — synth file should absorb the leg projection",
    );
    assert!(
        synth_key.len() > "auto-".len(),
        "synth key must include the canonical coordinate payload"
    );
}

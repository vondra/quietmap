//! Airport auto-discovery and stale-output regressions.
use super::*;

/// Build an AerodromeIndex from a slice — the gates take the index now.
fn idx(areas: &[AirportArea]) -> AerodromeIndex<'_> {
    AerodromeIndex::build(areas)
}
use crate::airport_io::AERODROME_AEROWAY_TYPE;
use crate::flight::Phase;
use crate::synth_airport_io::{read_synth_airport_areas, read_synth_airport_lines};

fn ground_segment_at(start_lat: f32, start_lon: f32, end_lat: f32, end_lon: f32) -> FlightSegment {
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

fn strip(length_m: f32, vertex_count: u32, is_line: bool) -> DiscoveredStrip {
    DiscoveredStrip {
        center_lat: 50.0,
        center_lon: 14.0,
        length_m,
        heading_deg: 90.0,
        width_m: 30.0,
        vertex_count,
        is_line,
    }
}

#[test]
fn microsegment_count_matches_length_over_step() {
    // 1000 m strip / 250 m step = 4 microsegments.
    let s = strip(1000.0, 100, true);
    let ms = microsegment_strip(&s);
    assert_eq!(ms.len(), 4);
    assert!((ms[0].length_m - 250.0).abs() < 1e-3);
    let total: f32 = ms.iter().map(|m| m.length_m).sum();
    assert!((total - 1000.0).abs() < 1e-3);
}

#[test]
fn microsegment_orientation_is_east_for_90_heading() {
    let s = strip(1000.0, 100, true);
    let ms = microsegment_strip(&s);
    let first = &ms[0];
    let last = ms.last().unwrap();
    assert!((first.start_lat - 50.0).abs() < 1e-4);
    assert!((last.end_lat - 50.0).abs() < 1e-4);
    assert!(first.start_lon < 14.0);
    assert!(last.end_lon > 14.0);
}

#[test]
fn offset_latlon_wraps_across_antimeridian() {
    // Anchor at 179.99°E, push 5000m east → should wrap into
    // negative longitude rather than producing 180.05°.
    let (_, lon) = offset_latlon(0.0, 179.99, 1.0, 0.0, 1.0, 5000.0);
    assert!(lon < 180.0, "longitude must wrap to <= 180, got {lon}");
    assert!(lon < -179.0, "expected wrap into negative half-circle");
}

/// One AirportLineRow with midpoint at `(lat, lon)` — used by
/// the new near-real-line gate tests. Length 49 m matches the OSM
/// extract step so the midpoint check is the dominant signal.
fn line_at(lat: f64, lon: f64) -> AirportLineRow {
    AirportLineRow {
        grid: (
            grid::lonlat_to_grid(lon, lat),
            grid::lonlat_to_grid(lon, lat),
        ),
        osm_id: 1,
        segment_idx: 0,
        start_lat: lat as f32,
        start_lon: lon as f32,
        end_lat: lat as f32,
        end_lon: lon as f32,
        length_m: 49.0,
        aeroway_type: 0,
    }
}

#[test]
fn classify_cluster_rejects_non_line_only() {
    // `is_line=false` clusters are still rejected — apron-equivalent
    // blobs need a geometry_kind extension that's out of scope.
    // The previous CLUSTER_MIN_LENGTH / CLUSTER_MIN_VERTICES gates
    // were dropped (zero floors); tiny line clusters now flow to
    // SynthAirport so they stay visible in the popup.
    assert!(matches!(
        classify_cluster(&strip(800.0, 100, false), &idx(&[]), &[]),
        ClusterDisposition::Reject,
    ));
}

#[test]
fn classify_cluster_admits_tiny_line() {
    // Length 100 m / 40 vertices — short clusters pass as
    // SynthAirport so they stay visible in the popup with
    // low-confidence labeling.
    assert!(matches!(
        classify_cluster(&strip(100.0, 40, true), &idx(&[]), &[]),
        ClusterDisposition::SynthAirport,
    ));
}

#[test]
fn classify_cluster_passes_long_sparse() {
    let index = idx(&[]);
    let result = classify_cluster(&strip(500.0, 40, true), &index, &[]);
    assert!(matches!(result, ClusterDisposition::SynthAirport));
}

#[test]
fn classify_cluster_passes_short_dense() {
    let index = idx(&[]);
    let result = classify_cluster(&strip(150.0, 200, true), &index, &[]);
    assert!(matches!(result, ClusterDisposition::SynthAirport));
}

#[test]
fn classify_cluster_reattributes_to_nearby_real_aerodrome() {
    let areas = vec![AirportArea::new(
        1,
        AERODROME_AEROWAY_TYPE,
        "Test Aerodrome".to_string(),
        "LKTEST".to_string(),
        50.0,
        14.0,
        Vec::new(),
        10_000_000.0, // ~1.8km equivalent radius → snap window
    )];
    // Strip centroid is (50.0, 14.0); add a real OSM line right
    // there so the new line-buffer gate accepts re-attribution.
    let lines = vec![line_at(50.0, 14.0)];
    let s = strip(800.0, 100, true);
    let index = idx(&areas);
    let result = classify_cluster(&s, &index, &lines);
    match result {
        ClusterDisposition::Reattribute(a) => assert_eq!(a.airport_key, "LKTEST"),
        _ => panic!("expected Reattribute, got Reject/SynthAirport"),
    }
}

#[test]
fn classify_cluster_relabels_when_in_polygon_buffer_but_not_near_line() {
    let areas = vec![AirportArea::new(
        1,
        AERODROME_AEROWAY_TYPE,
        "Test Aerodrome".to_string(),
        "LKTEST".to_string(),
        50.0,
        14.0,
        Vec::new(),
        10_000_000.0,
    )];
    // No lines anywhere — cluster passes polygon gate but fails
    // line-buffer gate → SynthAirport (relabeled, not rejected).
    let s = strip(800.0, 100, true);
    let index = idx(&areas);
    let result = classify_cluster(&s, &index, &[]);
    assert!(matches!(result, ClusterDisposition::SynthAirport));
}

/// A cluster far from every real aerodrome (no polygon match)
/// still flows to `SynthAirport` regardless of the line-buffer
/// gate — auto-discovery of genuinely new airfields must keep
/// working with no OSM coverage in the area.
#[test]
fn classify_cluster_synth_airport_unaffected_by_line_gate() {
    // No areas at all → no polygon match → SynthAirport.
    let s = strip(800.0, 100, true);
    assert!(matches!(
        classify_cluster(&s, &idx(&[]), &[]),
        ClusterDisposition::SynthAirport,
    ));
}

/// Same path with NON-empty lines: the gate must stay scoped
/// to the polygon-match arm, never leak into the no-polygon
/// branch. Without this test a future refactor that moved the
/// line check outside the match could silently turn auto-
/// discovered fields into Rejects.
#[test]
fn classify_cluster_no_polygon_with_lines_still_synth_airport() {
    let s = strip(800.0, 100, true);
    let lines = vec![line_at(50.0, 14.0)];
    assert!(matches!(
        classify_cluster(&s, &idx(&[]), &lines),
        ClusterDisposition::SynthAirport,
    ));
}

#[test]
fn classify_cluster_ignores_apron_polygons_for_reattribution() {
    // An apron / taxiway polygon in airport_areas should NOT
    // trigger re-attribution — only `aeroway_type == AERODROME`
    // counts. The shared `nearest_aerodrome_within` enforces this.
    let apron = AirportArea::new(
        1,
        2, // 2 = apron, not 5
        "apron".to_string(),
        "APRONKEY".to_string(),
        50.0,
        14.0,
        Vec::new(),
        10_000_000.0,
    );
    let s = strip(800.0, 100, true);
    assert!(matches!(
        classify_cluster(&s, &idx(&[apron]), &[]),
        ClusterDisposition::SynthAirport,
    ));
}

/// Regression for the LKPR-365d long tail. Stage 1.5 used to feed
/// ~27 M ground segments inside LKPR into a per-segment O(M_lines)
/// snap kernel before dropping them at cluster classification —
/// 30+ min per hub z9. With the polygon gate, both endpoints of a
/// fully-inside leg are filtered before the snap kernel runs.
#[test]
fn collect_miss_snap_drops_segment_when_both_endpoints_in_known_aerodrome() {
    // 1 km² aerodrome centred on (50.0, 14.0) — flat_dist radius
    // ≈ 564 m, lifted to the 6 km NEAREST_AERODROME_FLOOR_M.
    let aerodrome = AirportArea::new(
        1,
        5, // 5 = AERODROME_AEROWAY_TYPE
        "Test".to_string(),
        "LKTEST".to_string(),
        50.0,
        14.0,
        Vec::new(),
        1_000_000.0,
    );
    let inside = ground_segment_at(50.001, 14.001, 50.002, 14.002);
    let outside = ground_segment_at(50.5, 14.5, 50.501, 14.501);
    let candidates = collect_miss_snap_vertices(
        &[inside, outside],
        &[],
        &idx(std::slice::from_ref(&aerodrome)),
    );
    // `inside` → 0 vertices (both endpoints filtered by polygon gate).
    // `outside` → 2 vertices.
    assert_eq!(candidates.len(), 2);
    for (lat, _) in &candidates {
        assert!(*lat > 50.4, "inside-aerodrome vertex leaked: {lat}");
    }
}

/// Boundary-crossing leg (one endpoint inside a known aerodrome,
/// the other outside): under the EITHER-inside drop rule the
/// whole segment is filtered. The exterior endpoint of a leg
/// rooted at a known airport is a takeoff-climb / final-approach
/// point, never a synth-strip seed — keeping it bloated DBSCAN
/// input at hub z9s without aiding discovery.
#[test]
fn collect_miss_snap_drops_boundary_crossing_leg() {
    let aerodrome = AirportArea::new(
        1,
        5,
        "Test".to_string(),
        "LKTEST".to_string(),
        50.0,
        14.0,
        Vec::new(),
        1_000_000.0,
    );
    let crossing = ground_segment_at(50.001, 14.001, 50.5, 14.5);
    let candidates =
        collect_miss_snap_vertices(&[crossing], &[], &idx(std::slice::from_ref(&aerodrome)));
    assert_eq!(
        candidates.len(),
        0,
        "boundary-crossing leg must be filtered entirely",
    );
}

#[test]
fn collect_miss_snap_skips_segments_that_project_onto_osm() {
    let lines = vec![AirportLineSegment {
        grid: (
            grid::lonlat_to_grid(14.0, 50.0),
            grid::lonlat_to_grid(14.001, 50.0),
        ),
        osm_id: 1,
        segment_idx: 0,
        start_lat: 50.0,
        start_lon: 14.0,
        end_lat: 50.0,
        end_lon: 14.001,
        length_m: 71.5,
        aeroway_type: 0,
    }];
    let near = ground_segment_at(50.00001, 14.0001, 50.00001, 14.00099);
    let far = ground_segment_at(50.01, 14.0, 50.01, 14.001);
    let segs = vec![near, far];

    // No known aerodromes → polygon gate is a no-op; only the
    // line-snap gate decides.
    let candidates = collect_miss_snap_vertices(&segs, &lines, &idx(&[]));
    // `near` snaps → 0 vertices. `far` doesn't snap → 2 vertices.
    assert_eq!(candidates.len(), 2);
    // Both `far` endpoints share the same latitude.
    assert!((candidates[0].0 - 50.01).abs() < 1e-4);
    assert!((candidates[1].0 - 50.01).abs() < 1e-4);
    // The two endpoints' longitudes are distinct (start vs end).
    assert!(candidates[0].1 != candidates[1].1);
    // None of the `near` segment's endpoints leaked into the
    // candidate set.
    for (lat, _) in &candidates {
        assert!((*lat - 50.00001).abs() > 1e-5, "near segment leaked");
    }
}

/// Materialise a per-z9 ground shard the way `shuffle_per_square`
/// would for tests. Returns `(tmp_root, segments_by_square_dir,
/// prepared_year_dir)` so callers can pass `prepared_year_dir` through and inspect
/// the same dir later.
fn materialise_per_square(
    tmp: &tempfile::TempDir,
    segs: &[FlightSegment],
) -> (std::path::PathBuf, std::path::PathBuf) {
    use crate::arrow_io::write_segments;
    let by_square = tmp.path().join("segments_by_square");
    let prepared_year = tmp.path().join("prepared_year");
    // Group segs by midpoint z9.
    let mut by_square_map: std::collections::HashMap<u64, Vec<FlightSegment>> =
        std::collections::HashMap::new();
    for seg in segs {
        let mid_lat = (seg.start_lat + seg.end_lat) as f64 * 0.5;
        let mid_lon = (seg.start_lon + seg.end_lon) as f64 * 0.5;
        let square = crate::spatial::square_id(mid_lat, mid_lon).unwrap();
        by_square_map.entry(square).or_default().push(seg.clone());
    }
    for (square, segs) in by_square_map {
        let dir = by_square.join(square_path(square));
        std::fs::create_dir_all(&dir).unwrap();
        write_segments(&dir.join("ground.arrow"), &segs).unwrap();
    }
    (by_square, prepared_year)
}

#[test]
fn empty_input_returns_zero_populated_squares() {
    let tmp = tempfile::tempdir().unwrap();
    let by_square = tmp.path().join("segments_by_square");
    std::fs::create_dir_all(&by_square).unwrap();
    let n = run_stage_airport_discover(
        &by_square,
        &AerodromeIndex::build(&[]),
        &[],
        tmp.path(),
        None,
    )
    .unwrap();
    assert_eq!(n, 0);
}

/// 60 ADS-B "ground" segments head-to-tail along a 1 km east-west
/// line at a fixed mid-Atlantic anchor (30°N 40°W). Shared by the
/// end-to-end + rerun tests so a tweak to the cluster shape stays
/// in one place.
const ANCHOR_LAT: f32 = 30.0;
const ANCHOR_LON: f32 = -40.0;
fn unmapped_strip_segments() -> Vec<FlightSegment> {
    let dlon = (1000.0 / 111_320.0) as f32 / ANCHOR_LAT.to_radians().cos();
    (0..60)
        .map(|i| {
            let lon_a = ANCHOR_LON + dlon * (i as f32) / 60.0;
            let lon_b = ANCHOR_LON + dlon * ((i + 1) as f32) / 60.0;
            ground_segment_at(ANCHOR_LAT, lon_a, ANCHOR_LAT, lon_b)
        })
        .collect()
}

fn only_square_dir(root: &Path) -> std::path::PathBuf {
    let dirs = crate::spatial::square_directories(root).unwrap();
    assert_eq!(dirs.len(), 1, "expected exactly one z9 directory");
    dirs[0].1.clone()
}

fn assert_sidecars_empty(dir: &Path) {
    let lines = read_synth_airport_lines(&dir.join(SYNTH_LINES_FILE)).unwrap();
    let areas = read_synth_airport_areas(&dir.join(SYNTH_AREAS_FILE)).unwrap();
    assert!(
        lines.is_empty(),
        "stale synth_airport_lines must clear, got {} rows",
        lines.len()
    );
    assert!(areas.is_empty(), "stale synth_airport_areas must clear");
}

/// Drive Stage 1.5 on a remote synthetic strip and assert both
/// sidecars receive the expected row content + key shape.
#[test]
fn end_to_end_emits_synth_arrows_for_an_unmapped_strip() {
    let tmp = tempfile::tempdir().unwrap();
    let (by_square, prepared_year) = materialise_per_square(&tmp, &unmapped_strip_segments());
    let n = run_stage_airport_discover(
        &by_square,
        &AerodromeIndex::build(&[]),
        &[],
        &prepared_year,
        None,
    )
    .unwrap();
    assert_eq!(n, 1, "expected exactly one z9 to receive synth rows");
    let dir = only_square_dir(&prepared_year);
    let lines = read_synth_airport_lines(&dir.join(SYNTH_LINES_FILE)).unwrap();
    let areas = read_synth_airport_areas(&dir.join(SYNTH_AREAS_FILE)).unwrap();
    assert!(!lines.is_empty(), "synth_airport_lines must have rows");
    assert_eq!(areas.len(), 1, "one synth area per cluster");
    let key = &areas[0].airport_key;
    assert!(key.starts_with("auto-"), "synth key, not re-attribution");
    for r in &lines {
        assert_eq!(&r.airport_key, key);
    }
}

/// z9 drops out of the current run's ground-segment set entirely.
/// The on-disk scan must rediscover it and clear the stale sidecars
/// — otherwise Stage 2C consumes zombie airport areas.
#[test]
fn rerun_clears_stale_synth_files_when_square_drops_out() {
    let tmp = tempfile::tempdir().unwrap();
    let (by_square, prepared_year) = materialise_per_square(&tmp, &unmapped_strip_segments());
    run_stage_airport_discover(
        &by_square,
        &AerodromeIndex::build(&[]),
        &[],
        &prepared_year,
        None,
    )
    .unwrap();
    let dir = only_square_dir(&prepared_year);
    let lines_first = read_synth_airport_lines(&dir.join(SYNTH_LINES_FILE)).unwrap();
    assert!(!lines_first.is_empty(), "first run must populate sidecars");

    // Empty second run — wipe the segments_by_square dir so the z9 has
    // no shard, exercise the stale-sidecar path.
    std::fs::remove_dir_all(&by_square).unwrap();
    std::fs::create_dir_all(&by_square).unwrap();
    run_stage_airport_discover(
        &by_square,
        &AerodromeIndex::build(&[]),
        &[],
        &prepared_year,
        None,
    )
    .unwrap();
    assert_sidecars_empty(&dir);
}

/// z9 stays in scope (one distant non-clustering segment) but no
/// cluster forms. Existing idempotency rewrites empty sidecars.
#[test]
fn rerun_clears_stale_synth_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (by_square, prepared_year) = materialise_per_square(&tmp, &unmapped_strip_segments());
    run_stage_airport_discover(
        &by_square,
        &AerodromeIndex::build(&[]),
        &[],
        &prepared_year,
        None,
    )
    .unwrap();
    let dir = only_square_dir(&prepared_year);

    // Second run with one distant segment in the SAME z9 (so the
    // shard isn't empty but doesn't cluster).
    let dlon = (1000.0 / 111_320.0) as f32 / ANCHOR_LAT.to_radians().cos();
    let kept = ground_segment_at(ANCHOR_LAT, ANCHOR_LON, ANCHOR_LAT, ANCHOR_LON + dlon / 60.0);
    let (by_square_b, _) = materialise_per_square(&tmp, std::slice::from_ref(&kept));
    run_stage_airport_discover(
        &by_square_b,
        &AerodromeIndex::build(&[]),
        &[],
        &prepared_year,
        None,
    )
    .unwrap();
    assert_sidecars_empty(&dir);
}

#[test]
fn known_runway_across_partition_edge_does_not_seed_a_false_airfield() {
    let lat = 50.0;
    let segment = ground_segment_at(lat, -0.0001, lat, 0.0001);
    let owner = crate::spatial::square_id(lat as f64, -0.0001).unwrap();
    let mut line = line_at(lat as f64, 0.0);
    line.start_lon = -0.001;
    line.end_lon = 0.001;
    line.length_m = 140.0;
    line.grid = (
        grid::lonlat_to_grid(-0.001, lat as f64),
        grid::lonlat_to_grid(0.001, lat as f64),
    );
    let candidates = nearby_airport_lines(owner, std::slice::from_ref(&segment), &[line]);
    assert_eq!(candidates.len(), 1);
    assert!(collect_miss_snap_vertices(&[segment], &candidates, &idx(&[])).is_empty());
}

#[test]
fn corrupt_ground_input_preserves_every_existing_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let (by_square, prepared_year) = materialise_per_square(&tmp, &unmapped_strip_segments());
    let active = crate::spatial::square_directories(&by_square).unwrap()[0].0;
    let corrupt = crate::spatial::square_id(50.0, 14.0).unwrap();
    let corrupt_dir = by_square.join(square_path(corrupt));
    std::fs::create_dir_all(&corrupt_dir).unwrap();
    std::fs::write(corrupt_dir.join("ground.arrow"), b"corrupt").unwrap();
    let mut snapshots = Vec::new();
    for square in [active, corrupt] {
        let dir = prepared_year.join(square_path(square));
        write_synth_airport_lines(&dir.join(SYNTH_LINES_FILE), &[]).unwrap();
        write_synth_airport_areas(&dir.join(SYNTH_AREAS_FILE), &[]).unwrap();
        for name in [SYNTH_LINES_FILE, SYNTH_AREAS_FILE] {
            let path = dir.join(name);
            snapshots.push((path.clone(), std::fs::read(path).unwrap()));
        }
    }
    assert!(run_stage_airport_discover(&by_square, &idx(&[]), &[], &prepared_year, None).is_err());
    for (path, bytes) in snapshots {
        assert!(
            std::fs::read(&path).unwrap() == bytes,
            "changed {} after failed input validation",
            path.display()
        );
    }
}

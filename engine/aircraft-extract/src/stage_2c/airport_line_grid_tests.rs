//! Grid broadphase agrees with the brute-force kernel on every leg.
use super::*;
use crate::stage_2c::airport_traffic::project_leg_onto_airport_lines;

fn line(osm_id: u64, segment_idx: u16, lat1: f32, lon1: f32, lat2: f32, lon2: f32) -> AirportLineSegment {
    AirportLineSegment {
        osm_id,
        segment_idx,
        start_lat: lat1,
        start_lon: lon1,
        end_lat: lat2,
        end_lon: lon2,
        grid: ((0, 0), (0, 0)),
        length_m: 100.0,
        aeroway_type: 0,
    }
}

/// Deterministic xorshift64: the exactness fuzz needs no RNG dependency.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: f32) -> f32 {
        (self.next() % 1_000_000) as f32 / 1_000_000.0 * n
    }
}

fn check_matches_brute_force(lines: &[AirportLineSegment], legs: &[[f32; 4]], buffer: f32) {
    let allowance = AirportLineGrid::allocation_allowance(lines.len()).unwrap();
    let mut grid = AirportLineGrid::new(lines, buffer, allowance).unwrap();
    let mut hits = Vec::new();
    for leg in legs {
        grid.project(*leg, &mut hits);
        let expected =
            project_leg_onto_airport_lines(leg[0], leg[1], leg[2], leg[3], lines, buffer);
        assert_eq!(hits, expected, "leg {leg:?}");
    }
}

#[test]
fn grid_matches_brute_force_on_dense_airport_layout() {
    let mut rng = XorShift(0x9E3779B97F4A7C15);
    // 400 lines over a 3×3 km field with runways, taxiways and a diagonal.
    let mut lines = Vec::new();
    for i in 0..300u16 {
        let lat = 50.0 + rng.below(0.03);
        let lon = 14.0 + rng.below(0.04);
        let east = rng.next().is_multiple_of(2);
        let len = 0.0005 + rng.below(0.004);
        lines.push(if east {
            line(u64::from(i), 0, lat, lon, lat, lon + len)
        } else {
            line(u64::from(i), 0, lat, lon, lat + len, lon)
        });
    }
    for i in 300..400u16 {
        let lat = 50.0 + rng.below(0.03);
        let lon = 14.0 + rng.below(0.04);
        lines.push(line(
            u64::from(i),
            0,
            lat,
            lon,
            lat + 0.001 + rng.below(0.003),
            lon + 0.001 + rng.below(0.003),
        ));
    }
    // Legs: half on the field (hits likely), half scattered around it.
    let mut legs = Vec::new();
    for _ in 0..2000 {
        let lat = 50.0 - 0.005 + rng.below(0.04);
        let lon = 14.0 - 0.005 + rng.below(0.05);
        legs.push([lat, lon, lat + rng.below(0.002) - 0.001, lon + rng.below(0.002) - 0.001]);
    }
    check_matches_brute_force(&lines, &legs, 50.0);
}

#[test]
fn grid_matches_brute_force_across_the_dateline() {
    let lines = vec![
        line(1, 0, 51.0, 179.999, 51.0, -179.999),
        line(2, 0, 51.001, 179.998, 51.002, 179.9995),
        line(3, 0, 50.999, -179.9995, 51.0, -179.998),
        line(4, 0, 51.005, 179.99, 51.006, 179.992),
    ];
    let legs = vec![
        [51.0, 179.9992, 51.0002, -179.9997],
        [51.0012, 179.9985, 51.0018, 179.9992],
        [50.9992, -179.9992, 51.0005, -179.9985],
        [51.0052, 179.9905, 51.0058, 179.9915],
        [51.05, 179.9, 51.051, 179.901],
        [51.05, -179.9, 51.051, -179.901],
        [51.0, 179.5, 51.001, 179.501],
    ];
    check_matches_brute_force(&lines, &legs, 50.0);
}

#[test]
fn grid_matches_brute_force_near_the_pole() {
    // 78°N (Svalbard): longitude padding is wide but finite.
    let lines = vec![
        line(1, 0, 78.22, 15.63, 78.24, 15.65),
        line(2, 0, 78.23, 15.62, 78.231, 15.64),
    ];
    let legs = vec![
        [78.225, 15.631, 78.226, 15.632],
        [78.235, 15.645, 78.236, 15.646],
        [78.3, 15.7, 78.301, 15.701],
    ];
    check_matches_brute_force(&lines, &legs, 50.0);
}

#[test]
fn degenerate_lines_are_skipped_everywhere() {
    let lines = vec![
        line(1, 0, 50.0, 14.0, 50.0, 14.0),
        line(2, 0, 50.0, 14.0, 50.0, 14.00001),
        line(3, 0, 50.0, 14.0, 50.001, 14.001),
    ];
    let legs = vec![[50.0, 14.0, 50.0005, 14.0005], [51.0, 15.0, 51.001, 15.001]];
    check_matches_brute_force(&lines, &legs, 50.0);
    // All degenerate: empty grid, empty hits, no panic.
    let allowance = AirportLineGrid::allocation_allowance(1).unwrap();
    let mut grid = AirportLineGrid::new(&lines[..1], 50.0, allowance).unwrap();
    let mut hits = Vec::new();
    grid.project(legs[0], &mut hits);
    assert!(hits.is_empty());
}

#[test]
fn absurd_extent_falls_back_to_the_flat_list() {
    // Lines on opposite sides of the planet: the grid refuses the frame and
    // still matches brute force through the global list.
    let lines = vec![
        line(1, 0, 50.0, 14.0, 50.001, 14.001),
        line(2, 0, -33.9, 151.17, -33.899, 151.171),
    ];
    let legs = vec![
        [50.0002, 14.0002, 50.0005, 14.0005],
        [-33.8995, 151.1705, -33.8992, 151.1708],
        [0.0, 0.0, 0.001, 0.001],
    ];
    check_matches_brute_force(&lines, &legs, 50.0);
}

#[test]
fn buffer_boundary_legs_agree_with_brute_force() {
    // Legs grazing the 50 m buffer edge: the padded box must never drop a leg
    // the scalar clip keeps. ~55 m per 0.0005° latitude here.
    let lines = vec![line(1, 0, 50.0, 14.0, 50.002, 14.0)];
    let mut legs = Vec::new();
    for i in 0..40 {
        let dlon = 0.00045 + (i as f32) * 0.00001;
        legs.push([50.001, 14.0 + dlon, 50.0012, 14.0 + dlon]);
    }
    check_matches_brute_force(&lines, &legs, 50.0);
}

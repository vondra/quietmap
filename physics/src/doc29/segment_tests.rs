//! One segment at a receiver: dev4's closest-point, helicopter, partition and screening cases,
//! and the extension dev4's Filter D dropped, in the receiver's frame.

use std::cell::RefCell;

use super::*;
use crate::doc29::screening::Unscreened;

/// dev4's receiver-local projection, to keep its test coordinates (lat, lon, altitude).
fn local(receiver: [f64; 3], point: [f64; 3]) -> [f64; 3] {
    const M_PER_DEG_LAT: f64 = 111_132.92;
    let m_per_deg_lon = M_PER_DEG_LAT * receiver[0].to_radians().cos();
    [
        (point[1] - receiver[1]) * m_per_deg_lon,
        (point[0] - receiver[0]) * M_PER_DEG_LAT,
        point[2] - receiver[2],
    ]
}

fn norm(point: [f64; 3]) -> f64 {
    point[0].hypot(point[1]).hypot(point[2])
}

fn elevation_deg(point: [f64; 3]) -> f64 {
    point[2].atan2(point[0].hypot(point[1])).to_degrees()
}

/// Flat ground 4 m below the receiver.
fn in_the_open(start_m: [f64; 3], end_m: [f64; 3]) -> SegmentGeometry {
    SegmentGeometry {
        start_m,
        end_m,
        ground_under_start_m: -4.0,
        ground_under_end_m: -4.0,
    }
}

fn flight(departure: bool, speed_kt: f64, climb_sine: f64) -> SegmentFlight {
    SegmentFlight {
        departure,
        on_ground: false,
        speed_kt,
        pressure_altitude_m: 1_000.0,
        climb_sine,
        height_above_field_m: 1_000.0,
    }
}

fn emission(designator: &str, flight: &SegmentFlight, descent: bool) -> SegmentEmission {
    let aircraft = AircraftType::from_designator(designator);
    SegmentEmission::new(&aircraft, flight, descent).expect("inside the thrust domain")
}

#[test]
fn closest_point_alongside_inside_behind_and_below() {
    let receiver = [50.005, 14.01, 300.0];
    let (start, end) = ([50.0, 14.0, 1_000.0], [50.01, 14.0, 1_000.0]);
    let alongside = closest_points(local(receiver, start), local(receiver, end));
    assert!(
        alongside.along > 0.0 && alongside.along < 1.0,
        "{alongside:?}"
    );
    assert_eq!(alongside.on_line_m, alongside.on_segment_m);
    let slant_m = norm(alongside.on_line_m);
    assert!(slant_m > 500.0 && slant_m < 2_000.0, "{slant_m}");
    let beta = elevation_deg(alongside.on_line_m);
    assert!(beta > 20.0 && beta < 70.0, "{beta}");
    let behind_receiver = [49.99, 14.01, 300.0];
    let behind = closest_points(local(behind_receiver, start), local(behind_receiver, end));
    assert!(behind.along < 0.0, "{behind:?}");
    let under = [50.005, 14.0, 300.0];
    let (high_start, high_end) = ([50.0, 14.0, 3_000.0], [50.01, 14.0, 3_000.0]);
    let overhead = closest_points(local(under, high_start), local(under, high_end));
    assert!(overhead.on_line_m[0].hypot(overhead.on_line_m[1]) < 50.0);
    assert!(elevation_deg(overhead.on_line_m) > 80.0);
}

/// A segment in a valley below a ridge-top receiver keeps its signed height.
#[test]
fn a_segment_below_the_receiver_keeps_a_signed_height() {
    let receiver = [49.7846, 14.0306, 684.0];
    let (start, end) = ([49.7813, 14.0350, 0.0], [49.7863, 14.0283, 0.0]);
    let closest = closest_points(local(receiver, start), local(receiver, end));
    assert!(closest.on_line_m[0].hypot(closest.on_line_m[1]) < 50.0);
    assert!(closest.on_line_m[2] < -600.0 && norm(closest.on_line_m) > 600.0);
    assert!(elevation_deg(closest.on_line_m) < 0.0);
}

/// A curving departure's short, high leg 7 km away (flight 39d311 over LKPR): its line extended
/// back passes 40 m from the receiver. The kernel keeps that foot (Delta_F needs it); the point on
/// the segment is the aircraft's real, far closest position.
#[test]
fn a_curving_departure_phantom_keeps_its_foot_for_the_sel_only() {
    let receiver = [50.1110, 14.3819, 279.0];
    let start = local(receiver, [50.1700, 14.4197, 1_509.0]);
    let end = local(receiver, [50.1711, 14.4204, 1_532.0]);
    let geometry = SegmentGeometry {
        ground_under_start_m: 170.0 - 279.0,
        ground_under_end_m: 170.0 - 279.0,
        ..in_the_open(start, end)
    };
    let departure = flight(true, 100.0, 23.0 / 131.0);
    let sel = segment_sel_at_receiver(&emission("B738", &departure, false), &geometry, &Unscreened);
    assert!(
        sel.closest.along < 0.0 && norm(sel.closest.on_line_m) < 100.0,
        "{sel:?}"
    );
    assert!(norm(sel.closest.on_segment_m) > 5_000.0 && sel.closest.on_segment_m[2] > 1_000.0);
    assert!(sel.sel_db.is_finite());
    // Past the end the clamp takes the end point: 800 m above a receiver north of a climb.
    let receiver = [50.02, 14.0, 300.0];
    let (start, end) = ([50.0, 14.0, 1_000.0], [50.01, 14.0, 1_100.0]);
    let past_end = closest_points(local(receiver, start), local(receiver, end));
    assert!(past_end.along > 1.0 && (past_end.on_segment_m[2] - 800.0).abs() < 1e-9);
    assert!(norm(past_end.on_segment_m) > norm(past_end.on_line_m));
}

/// Three symmetric EC135 rows over one receiver differ by the certification uplifts: climb +3.4
/// and descent (blade-vortex interaction) +8.7 dB over level flight; the +-11 m altitudes keep
/// the slant and Delta_F spread inside 0.5 dB.
#[test]
fn helicopter_states_differ_by_the_certification_uplifts() {
    let sel = |departure: bool, end_height_m: f64, descent: bool| {
        let geometry = in_the_open([0.0, -250.0, 150.0], [0.0, 250.0, end_height_m]);
        let climb_sine = (end_height_m - 150.0) / 500f64.hypot(end_height_m - 150.0);
        let emission = emission("EC35", &flight(departure, 100.0, climb_sine), descent);
        segment_sel_at_receiver(&emission, &geometry, &Unscreened).sel_db
    };
    let level = sel(false, 150.0, false);
    let descent = sel(false, 139.0, true);
    let climb = sel(true, 161.0, false);
    assert!(
        (descent - level - 8.7).abs() < 0.5,
        "descent {descent} level {level}"
    );
    assert!(
        (climb - level - 3.4).abs() < 0.5,
        "climb {climb} level {level}"
    );
}

#[test]
fn a_nearby_b738_approach_reads_a_plausible_sel() {
    let receiver = [50.005, 14.005, 300.0];
    let geometry = in_the_open(
        local(receiver, [50.0, 14.0, 1_000.0]),
        local(receiver, [50.01, 14.0, 900.0]),
    );
    let approach = flight(false, 150.0, -100.0 / 1_100.0);
    let sel = segment_sel_at_receiver(&emission("B738", &approach, false), &geometry, &Unscreened);
    assert!(sel.sel_db > 50.0 && sel.sel_db < 110.0, "{}", sel.sel_db);
    let slant_m = norm(sel.closest.on_line_m);
    assert!(slant_m > 100.0 && slant_m < 2_000.0, "{slant_m}");
}

/// A final approach whose line, extended beyond touchdown, passes 46 m under the ground at the
/// receiver's closest point is heard through its own extent (Doc 29; dev4's Filter D dropped it):
/// finite, and quieter than with the receiver abeam its nearer end.
#[test]
fn an_approach_is_heard_beyond_its_touchdown() {
    let approach = emission("B738", &flight(false, 140.0, -0.05), false);
    let geometry = SegmentGeometry {
        start_m: [-4_000.0, 0.0, 150.0],
        end_m: [-2_000.0, 0.0, 50.0],
        ground_under_start_m: -4.0,
        ground_under_end_m: -4.0,
    };
    let beyond = segment_sel_at_receiver(&approach, &geometry, &Unscreened).sel_db;
    let abeam = SegmentGeometry {
        start_m: [-2_000.0, 2_000.0, 150.0],
        end_m: [0.0, 2_000.0, 50.0],
        ..geometry
    };
    let beside = segment_sel_at_receiver(&approach, &abeam, &Unscreened).sel_db;
    assert!(beyond.is_finite() && beyond > 30.0, "{beyond}");
    assert!(beyond < beside, "{beyond} vs {beside}");
}

/// Collinear pieces share the line's closest point and d_lambda, and Eq. 4-20 telescopes: they
/// carry exactly the energy of the whole segment (dev4 allowed 30 % with its fast arctangent).
#[test]
fn collinear_pieces_carry_the_energy_of_their_segment() {
    let fallback = emission("XXXX", &flight(true, 160.0, 0.0), false);
    let energy = |from_m: f64, to_m: f64| {
        let geometry = in_the_open([from_m, 8_000.0, 8_000.0], [to_m, 8_000.0, 8_000.0]);
        let sel = segment_sel_at_receiver(&fallback, &geometry, &Unscreened);
        10f64.powf(sel.sel_db / 10.0)
    };
    let whole = energy(-10_000.0, 10_000.0);
    let pieces: f64 = (0..10)
        .map(|piece| {
            let from_m = -10_000.0 + 2_000.0 * f64::from(piece);
            energy(from_m, from_m + 2_000.0)
        })
        .sum();
    assert!((pieces / whole - 1.0).abs() < 1e-9, "{}", pieces / whole);
}

/// Overhead a long level segment (Lambda, Delta_I and Delta_F vanish) the kernel reads the
/// levels a box sums at each NPD distance.
#[test]
fn the_npd_distance_levels_are_the_kernel_under_a_long_segment() {
    for (designator, departure) in [("B738", false), ("A320", true), ("EC35", false)] {
        let emission = emission(designator, &flight(departure, 130.0, 0.0), false);
        let levels = emission.npd_distance_levels();
        for (k, distance_ft) in NPD_DISTANCES_FT.iter().enumerate() {
            let height_m = distance_ft * METRES_PER_FOOT;
            let geometry = in_the_open([-5e5, 0.0, height_m], [5e5, 0.0, height_m]);
            let sel = segment_sel_at_receiver(&emission, &geometry, &Unscreened);
            assert!(
                (sel.free_sel_db - levels.sel_db[k]).abs() < 1e-3,
                "{designator} {k}"
            );
            assert_eq!(sel.npd.scaled_distance_m, levels.scaled_distance_m[k]);
        }
    }
}

/// Stub horizons with fixed losses that record where they are asked.
struct FixedHorizons {
    terrain_db: f64,
    building_db: f64,
    asked: RefCell<Vec<[f64; 3]>>,
}

impl FixedHorizons {
    fn new(terrain_db: f64, building_db: f64) -> Self {
        FixedHorizons {
            terrain_db,
            building_db,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl ReceiverHorizons for FixedHorizons {
    fn terrain_loss_db(&self, point_m: [f64; 3]) -> f64 {
        self.asked.borrow_mut().push(point_m);
        self.terrain_db
    }

    fn building_loss_db(&self, point_m: [f64; 3]) -> f64 {
        self.asked.borrow_mut().push(point_m);
        self.building_db
    }
}

/// Terrain diffraction competes with lateral attenuation at every slant; terrain and building
/// losses take the maximum; the terrain horizon is asked at the line's closest point, the building
/// horizon at the segment's.
#[test]
fn screening_takes_the_larger_loss_net_of_lateral_attenuation() {
    let jet = emission("A320", &flight(false, 140.0, 0.0), false);
    for (lateral_m, height_m) in [(3_000.0, 400.0), (8_000.0, 600.0)] {
        let geometry = in_the_open(
            [lateral_m, -1_000.0, height_m],
            [lateral_m, 1_000.0, height_m],
        );
        let both = FixedHorizons::new(18.0, 18.0);
        let sel = segment_sel_at_receiver(&jet, &geometry, &both);
        assert!(sel.lateral_attenuation_db > 1.0);
        let expected = sel.free_sel_db - (18.0 - sel.lateral_attenuation_db);
        assert!((sel.sel_db - expected).abs() < 1e-9, "{sel:?}");
    }
    let geometry = in_the_open([-4_000.0, 300.0, 500.0], [-3_000.0, 300.0, 520.0]);
    let recording = FixedHorizons::new(2.0, 1.0);
    let sel = segment_sel_at_receiver(&jet, &geometry, &recording);
    let asked = recording.asked.borrow();
    assert_eq!(
        *asked,
        vec![sel.closest.on_line_m, sel.closest.on_segment_m]
    );
    assert_ne!(sel.closest.on_line_m, sel.closest.on_segment_m);
}

#[test]
fn sources_above_the_screening_ceiling_are_not_screened() {
    let jet = emission("A320", &flight(false, 250.0, 0.0), false);
    let high = SegmentGeometry {
        ground_under_start_m: -300.0,
        ground_under_end_m: -300.0,
        ..in_the_open([8_000.0, -1_000.0, 7_400.0], [8_000.0, 1_000.0, 7_400.0])
    };
    let wall = FixedHorizons::new(18.0, 18.0);
    let sel = segment_sel_at_receiver(&jet, &high, &wall);
    assert_eq!(sel.sel_db, sel.free_sel_db);
    assert!(wall.asked.borrow().is_empty());
    let lower = SegmentGeometry {
        ground_under_start_m: -100.0,
        ground_under_end_m: -100.0,
        ..high
    };
    assert!(segment_sel_at_receiver(&jet, &lower, &wall).sel_db < sel.sel_db);
}

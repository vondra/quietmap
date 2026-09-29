//! A real two-roof profile exposed a log-domain failure outside the ISO accuracy fixtures.
//! This tests finite output under the documented numerical policy, not measured accuracy.
use super::*;

struct Fixture {
    source: PlanePoint,
    receiver: PlanePoint,
    source_ground_factor: f64,
    distance_m: Vec<f64>,
    altitude_m: Vec<f64>,
    ground_factor: Vec<f64>,
    terrain_candidates: Vec<PlanePoint>,
    obstacle_tops: Vec<PlanePoint>,
}

fn fixture() -> Fixture {
    let value: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cnossos_ground_split_regression.json"
    )))
    .unwrap();
    let numbers = |key: &str| -> Vec<f64> {
        value[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    };
    let point = |v: &serde_json::Value| (v[0].as_f64().unwrap(), v[1].as_f64().unwrap());
    let points = |key: &str| -> Vec<PlanePoint> {
        value[key].as_array().unwrap().iter().map(point).collect()
    };
    Fixture {
        source: point(&value["source"]),
        receiver: point(&value["receiver"]),
        source_ground_factor: value["source_ground_factor"].as_f64().unwrap(),
        distance_m: numbers("distance_m"),
        altitude_m: numbers("altitude_m"),
        ground_factor: numbers("ground_factor"),
        terrain_candidates: points("terrain_candidates"),
        obstacle_tops: points("obstacle_tops"),
    }
}

#[test]
fn source_below_mean_plane_with_two_roofs_keeps_every_band_finite() {
    let fixture = fixture();
    let path = VerticalPath {
        profile: VerticalProfile {
            distance_m: &fixture.distance_m,
            altitude_m: &fixture.altitude_m,
            ground_factor: &fixture.ground_factor,
        },
        source: fixture.source,
        receiver: fixture.receiver,
        source_ground_factor: fixture.source_ground_factor,
        terrain_candidates: &fixture.terrain_candidates,
        obstacle_tops: &fixture.obstacle_tops,
    };
    for state in [
        MeteorologicalState::Homogeneous,
        MeteorologicalState::Favourable,
    ] {
        let mut scratch = VerticalPathScratch::default();
        let boundary = state_boundary(&path, state, &mut scratch);
        assert_eq!(
            scratch.path.points.len(),
            match state {
                MeteorologicalState::Homogeneous => 2,
                MeteorologicalState::Favourable => 1,
            },
            "{state:?}"
        );
        assert!(
            boundary.attenuation_db.iter().all(|v| v.is_finite()),
            "{state:?}: {boundary:?}"
        );
    }
}

/// The same two-roof profile mirrored end for end: the receiver now sits below its side's
/// mean plane, exercising the symmetric fallback. Finite in both states, same point counts.
#[test]
fn mirrored_receiver_below_mean_plane_keeps_every_band_finite() {
    let fixture = fixture();
    let length = fixture.receiver.0;
    let n = fixture.distance_m.len();
    let distance_m: Vec<f64> = (0..n)
        .map(|i| length - fixture.distance_m[n - 1 - i])
        .collect();
    let altitude_m: Vec<f64> = (0..n).map(|i| fixture.altitude_m[n - 1 - i]).collect();
    let ground_factor: Vec<f64> = (0..n).map(|i| fixture.ground_factor[n - 1 - i]).collect();
    let mirror = |points: &[PlanePoint]| {
        let mut mirrored: Vec<PlanePoint> = points.iter().map(|&(x, z)| (length - x, z)).collect();
        mirrored.sort_by(|a, b| a.0.total_cmp(&b.0));
        mirrored
    };
    let terrain = mirror(&fixture.terrain_candidates);
    let tops = mirror(&fixture.obstacle_tops);
    let path = VerticalPath {
        profile: VerticalProfile {
            distance_m: &distance_m,
            altitude_m: &altitude_m,
            ground_factor: &ground_factor,
        },
        source: (0.0, fixture.receiver.1),
        receiver: (length, fixture.source.1),
        source_ground_factor: fixture.source_ground_factor,
        terrain_candidates: &terrain,
        obstacle_tops: &tops,
    };
    for state in [
        MeteorologicalState::Homogeneous,
        MeteorologicalState::Favourable,
    ] {
        let mut scratch = VerticalPathScratch::default();
        let boundary = state_boundary(&path, state, &mut scratch);
        assert_eq!(
            scratch.path.points.len(),
            match state {
                MeteorologicalState::Homogeneous => 2,
                MeteorologicalState::Favourable => 1,
            },
            "{state:?}"
        );
        assert!(
            boundary.attenuation_db.iter().all(|v| v.is_finite()),
            "{state:?}: {boundary:?}"
        );
    }
}

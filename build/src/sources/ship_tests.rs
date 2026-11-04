//! Water-supported ship placement conserves cell power and keeps its sampled exclusion radii.
use super::*;

const NORTH_ALIGNED_KM: [[f64; 2]; 2] = [[1_000.0, 0.0], [0.0, 1_000.0]];

/// Over open sea a 1 km cell is sixty-four 250 m sub-cells over 2 km, their weighted areas adding
/// up to its 1 km2, most at its centre; the tents of a block of cells add up to the same water
/// everywhere inside it (no edges); over a coast running north-south through its middle only
/// the eastern half, on the water.
#[test]
fn a_cell_fades_into_its_neighbours_on_its_water_alone() {
    let centre = (54.0, 10.0);
    let sea = water_points(centre, NORTH_ALIGNED_KM, |_, _| Ok(1.0)).unwrap();
    assert_eq!(sea.len(), 64);
    let total: f64 = sea.iter().map(|(point, _)| point.area_m2).sum();
    assert!((total - 1e6).abs() < 1e-3, "{total}");
    let (low, high) = sea
        .iter()
        .fold((f64::INFINITY, 0.0f64), |(low, high), (point, _)| {
            (low.min(point.area_m2), high.max(point.area_m2))
        });
    assert!((high - 62_500.0 * 0.875f64.powi(2)).abs() < 1.0, "{high}");
    assert!((low - 62_500.0 * 0.125f64.powi(2)).abs() < 1.0, "{low}");
    assert!(
        sea.iter()
            .all(|(_, extent_m2)| (extent_m2 - 62_500.0).abs() < 1e-6)
    );
    // Nine cells 1 km apart: each 250 m of the middle cell holds 62,500 m2 of their water.
    let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos();
    let mut summed = std::collections::HashMap::new();
    for east in -1..=1 {
        for north in -1..=1 {
            let cell = (
                centre.0 + f64::from(north) * 1_000.0 / M_PER_DEG_LAT,
                centre.1 + f64::from(east) * 1_000.0 / metres_per_lon,
            );
            for (point, _) in water_points(cell, NORTH_ALIGNED_KM, |_, _| Ok(1.0)).unwrap() {
                // Its 250 m sub-cell of the middle cell (a point leans to its own cell's centre).
                let north_m = ((point.lat - centre.0) * M_PER_DEG_LAT).round() as i64;
                let east_m = ((point.lon - centre.1) * metres_per_lon).round() as i64;
                let sub_cell = (north_m.div_euclid(250), east_m.div_euclid(250));
                *summed.entry(sub_cell).or_insert(0.0) += point.area_m2;
            }
        }
    }
    let inside: Vec<f64> = (summed.iter())
        .filter(|(sub_cell, _)| (-2..2).contains(&sub_cell.0) && (-2..2).contains(&sub_cell.1))
        .map(|(_, area)| *area)
        .collect();
    assert_eq!(inside.len(), 16);
    assert!(
        inside.iter().all(|area| (area - 62_500.0).abs() < 1.0),
        "{inside:?}"
    );
    let east_of = |lon: f64| move |_: f64, at: f64| Ok(if at > lon { 1.0 } else { 0.0 });
    let coast = water_points(centre, NORTH_ALIGNED_KM, east_of(centre.1)).unwrap();
    assert_eq!(coast.len(), 32);
    assert!(coast.iter().all(|(point, _)| point.lon > centre.1));
    let total: f64 = coast.iter().map(|(point, _)| point.area_m2).sum();
    assert!((total - 500_000.0).abs() < 1e-3, "{total}");
    assert!(
        water_points(centre, NORTH_ALIGNED_KM, |_, _| Ok(0.0))
            .unwrap()
            .is_empty()
    );
    // Water only in the first and last sample column of every sub-cell: its water's centre is
    // its middle column, dry; the point goes to a wet sample. Ten of its 25 samples are wet.
    let step_lon = 50.0 / metres_per_lon;
    let west_edge = centre.1 - 1_000.0 / metres_per_lon;
    let piers = |_: f64, lon: f64| {
        let column = ((lon - west_edge) / step_lon).floor() as i64 % 5;
        Ok(if column == 0 || column == 4 { 1.0 } else { 0.0 })
    };
    for (point, extent_m2) in water_points(centre, NORTH_ALIGNED_KM, piers).unwrap() {
        assert_eq!(piers(point.lat, point.lon).unwrap(), 1.0, "{point:?}");
        assert!((extent_m2 - 25_000.0).abs() < 1e-6 && point.area_m2 < 25_000.0);
    }
    // One wet sample at 1 % (the sample north-east of the centre): 25 m2 of water at its tent,
    // heard as near as its 50 m sample, not as a 25 m2 disk.
    let speck = water_points(centre, NORTH_ALIGNED_KM, |lat, lon| {
        let near =
            lat > centre.0 && lat - centre.0 < 3e-4 && lon > centre.1 && lon - centre.1 < 6e-4;
        Ok(if near { 0.01 } else { 0.0 })
    })
    .unwrap();
    assert_eq!(speck.len(), 1);
    assert!((speck[0].0.area_m2 - 25.0 * 0.975f64.powi(2)).abs() < 1e-6);
    assert!((speck[0].1 - 2_500.0).abs() < 1e-6);
    // Two wet samples of the outer north-east sub-cell, half wet at 775 m north and east and all
    // at 975 m: its point stands where the tent puts its ships, at the inner one (97.6 % of them).
    let two = water_points(centre, NORTH_ALIGNED_KM, |lat, lon| {
        let north_m = (lat - centre.0) * M_PER_DEG_LAT;
        let east_m = (lon - centre.1) * metres_per_lon;
        let at = |metres: f64| (north_m - metres).abs() < 1.0 && (east_m - metres).abs() < 1.0;
        Ok(if at(775.0) {
            0.5
        } else if at(975.0) {
            1.0
        } else {
            0.0
        })
    })
    .unwrap();
    assert_eq!(two.len(), 1);
    assert!(((two[0].0.lat - centre.0) * M_PER_DEG_LAT - 775.0).abs() < 1.0);
}

/// A cell gives off its ships whole however its water lies: over open sea, and when its only
/// water is 1 % of the tent's outer corner sample (0.016 m2, which `push_site_points` alone
/// would have counted as 1 m2: 64 times the cell). Power normalization must not change the
/// near-field radius of the 250 m sub-cell or its lone 50 m water sample.
#[test]
fn a_cell_gives_off_its_ships_whole() {
    let attribute = Attribute {
        layer: Layer::Ship,
        height_m: 5.0,
        ground_percent: GROUND_FROM_TERRAIN,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: 1,
        emission: [[90.0; tiles::sources::BANDS]; tiles::sources::PERIODS],
        display: Vec::new(),
    };
    let centre: (f64, f64) = (54.0, 10.0);
    let metres_per_lon = M_PER_DEG_LON_EQ * centre.0.to_radians().cos();
    let corner = |lat: f64, lon: f64| {
        let far =
            (lat - centre.0) * M_PER_DEG_LAT > 950.0 && (lon - centre.1) * metres_per_lon > 950.0;
        Ok(if far { 0.01 } else { 0.0 })
    };
    let sea = |_: f64, _: f64| Ok(1.0);
    for (points, count, wet_side_m) in [
        (
            water_points(centre, NORTH_ALIGNED_KM, sea).unwrap(),
            64,
            250.0,
        ),
        (
            water_points(centre, NORTH_ALIGNED_KM, corner).unwrap(),
            1,
            50.0,
        ),
    ] {
        let mut out = Vec::new();
        assert!(push_cell(points, 1e6, &attribute, &mut out));
        let energy: f64 = (out.iter())
            .map(|item| 10f64.powf(item.attribute.emission[0][0] / 10.0))
            .sum();
        assert_eq!(out.len(), count);
        assert!((10.0 * energy.log10() - 90.0).abs() < 1e-6, "{energy}");
        let radius_m = wet_side_m / std::f64::consts::PI.sqrt();
        assert!(
            out.iter()
                .all(|item| (item.attribute.exclusion_radius_m - radius_m).abs() < 1e-6)
        );
    }
}

//! Settlement tests.

use super::*;

fn building_row(osm_id: i64) -> fx::StructureRow {
    fx::StructureRow {
        kind: square_store::store::STRUCTURE_KIND_BUILDING,
        ring_lonlat: Some(fx::square_ring_lonlat(LAT, LON)),
        height_m: 12,
        height_tier: 0,
        envelope_class: 1,
        centroid_lonlat: Some((LON + 0.0001, LAT + 0.0001)),
        osm_id: Some(osm_id),
        building_type: Some(1),
        area_m2: Some(450.0),
        ..Default::default()
    }
}

#[test]
fn building_rows_feed_emission_and_walls_do_not() {
    let tmp = tempfile::TempDir::new().unwrap();
    fx::write_square_structures(
        tmp.path(),
        prague(),
        &[
            building_row(55),
            fx::StructureRow {
                kind: square_store::store::STRUCTURE_KIND_BARRIER,
                ring_lonlat: Some(vec![(LON, LAT), (LON + 0.001, LAT + 0.001)]),
                height_m: 3,
                height_tier: 0,
                envelope_class: 0,
                centroid_lonlat: Some((LON + 0.0005, LAT + 0.0005)),
                osm_id: Some(66),
                segment_idx: Some(0),
                ..Default::default()
            },
            fx::StructureRow {
                kind: square_store::store::STRUCTURE_KIND_BUILDING,
                ring_lonlat: Some(fx::square_ring_lonlat(LAT + 0.001, LON + 0.001)),
                height_m: 8,
                height_tier: 2,
                envelope_class: 5,
                centroid_lonlat: Some((LON + 0.001, LAT + 0.001)),
                ..Default::default()
            },
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.buildings.len(), 1);
    assert_eq!(data.buildings[0].osm_id, 55);
}

#[test]
fn far_building_is_outside_the_building_reach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut far = building_row(57);
    far.centroid_lonlat = Some((LON, LAT + 0.03));
    far.ring_lonlat = Some(fx::square_ring_lonlat(LAT + 0.03, LON));
    fx::write_square_structures(tmp.path(), prague(), &[building_row(55), far]);
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(
        data.buildings.iter().map(|b| b.osm_id).collect::<Vec<_>>(),
        vec![55]
    );
}

#[test]
fn emission_overrides_win_over_screening_geometry() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut row = building_row(56);
    row.emission_centroid_lonlat = Some((LON + 0.0002, LAT + 0.0002));
    row.emission_ring_lonlat = Some(fx::square_ring_lonlat(LAT + 0.0002, LON + 0.0002));
    fx::write_square_structures(tmp.path(), prague(), &[row]);
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.buildings.len(), 1);
    let pt = &data.buildings[0];
    assert!((pt.lon - (LON + 0.0002)).abs() < 0.0002, "lon={}", pt.lon);
}

#[test]
fn leisure_folds_into_buildings_with_sport_tag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_leisure_file(
        &dir.join("leisure.arrow"),
        &[fx::FixtureLeisure {
            osm_id: 88,
            centroid: (LON, LAT),
            sport: 3,
            name: "Court".to_string(),
            chain_lonlat: None,
            tags: &[],
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.buildings.len(), 1);
    assert_eq!(
        data.buildings[0].source_type,
        noise_compute::types::LEISURE_TYPE_BASE + 3
    );
}

#[test]
fn leisure_formula_rows_reach_past_2km_and_indoor_stays_out() {
    // A speedway point 3 km off (past the area-class horizon) collects with
    // industrial reach; a pitch at the same distance does not; and a roofed
    // (`building=*`) kart hall at the receiver stays silent.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let m_per_deg_lon = grid::geo::m_per_deg_lon(LAT.to_radians());
    let row = |osm_id: i64,
               east_m: f64,
               sport: u8,
               tags: &'static [(&'static str, &'static str)]|
     -> fx::FixtureLeisure {
        fx::FixtureLeisure {
            osm_id,
            centroid: (LON + east_m / m_per_deg_lon, LAT),
            sport,
            name: String::new(),
            chain_lonlat: None,
            tags,
        }
    };
    fx::write_leisure_file(
        &dir.join("leisure.arrow"),
        &[
            row(200, 3000.0, noise_compute::emission::leisure::MOTORSPORT, &[("sport", "speedway")]),
            row(201, 3000.0, noise_compute::emission::leisure::PITCH, &[]),
            row(
                202,
                0.0,
                noise_compute::emission::leisure::MOTORSPORT,
                &[("sport", "karting"), ("building", "yes")],
            ),
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let ids: Vec<i64> = data.buildings.iter().map(|b| b.osm_id).collect();
    assert!(ids.contains(&200), "3 km speedway collects: {ids:?}");
    assert!(!ids.contains(&201), "3 km pitch stays out: {ids:?}");
    assert!(!ids.contains(&202), "indoor kart hall stays out: {ids:?}");
    let speedway = data.buildings.iter().find(|b| b.osm_id == 200).unwrap();
    assert_eq!(
        speedway.source_type,
        noise_compute::types::LEISURE_TYPE_BASE + noise_compute::emission::leisure::MOTORSPORT
    );
    assert!(speedway.max_radius_m > 2_000.0, "reach {}", speedway.max_radius_m);
}

#[test]
fn leisure_enclosing_polygon_goes_silent_for_its_lines() {
    // A motorsport polygon enclosing a raceway line goes silent; the line
    // carries the circuit emission. A lone motocross area still emits.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let ring = fx::square_ring_lonlat(LAT, LON);
    let line = vec![(LON, LAT), (LON + 0.0002, LAT + 0.0001)];
    fx::write_leisure_file(
        &dir.join("leisure.arrow"),
        &[
            fx::FixtureLeisure {
                osm_id: 210,
                centroid: (LON + 0.0001, LAT + 0.0001),
                sport: noise_compute::emission::leisure::MOTORSPORT,
                name: "Circuit".to_string(),
                chain_lonlat: Some(ring),
                tags: &[("sport", "car_racing")],
            },
            fx::FixtureLeisure {
                osm_id: 211,
                centroid: (LON + 0.00015, LAT + 0.0001),
                sport: noise_compute::emission::leisure::MOTORSPORT,
                name: "Raceway".to_string(),
                chain_lonlat: Some(line),
                tags: &[("sport", "car_racing")],
            },
            fx::FixtureLeisure {
                osm_id: 212,
                centroid: (LON + 0.02, LAT + 0.02),
                sport: noise_compute::emission::leisure::MOTORSPORT,
                name: "MX".to_string(),
                chain_lonlat: None,
                tags: &[("sport", "motocross")],
            },
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let mut ids: Vec<i64> = data.buildings.iter().map(|b| b.osm_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids, vec![211, 212]);
    // The line carries the circuit total (119.1 day LwA), split over pieces.
    let energy: f64 = data
        .buildings
        .iter()
        .filter(|b| b.osm_id == 211)
        .map(|b| 10f64.powf(f64::from(b.lw_day[4]) / 10.0))
        .sum();
    let emission = noise_compute::emission::leisure::motorsport_emission(
        noise_compute::emission::leisure::MotorsportSubtype::Circuit,
    )
    .unwrap();
    let total = 10f64.powf(
        noise_compute::emission::leisure::leisure_formula_bands(&emission)[4] as f64 / 10.0,
    );
    assert!((10.0 * (energy / total).log10()).abs() < 0.5, "line total");
}

#[test]
fn leisure_shooting_subtypes_resolve_per_row() {
    // A clay ground and an untyped range emit (shotgun below the rifle
    // default); a paintball site stays out.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let m_per_deg_lon = grid::geo::m_per_deg_lon(LAT.to_radians());
    let row = |osm_id: i64,
               east_m: f64,
               tags: &'static [(&'static str, &'static str)],
               name: &str|
     -> fx::FixtureLeisure {
        fx::FixtureLeisure {
            osm_id,
            centroid: (LON + east_m / m_per_deg_lon, LAT),
            sport: noise_compute::emission::leisure::SHOOTING,
            name: name.to_string(),
            chain_lonlat: None,
            tags,
        }
    };
    fx::write_leisure_file(
        &dir.join("leisure.arrow"),
        &[
            row(220, 300.0, &[("shooting", "clay_pigeon")], "Clays"),
            row(221, 500.0, &[], "Střelnice"),
            row(222, 300.0, &[("shooting", "paintball")], "Paintball"),
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let mut ids: Vec<i64> = data.buildings.iter().map(|b| b.osm_id).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![220, 221]);
    let lw = |id: i64| {
        data.buildings
            .iter()
            .find(|b| b.osm_id == id)
            .unwrap()
            .lw_day[4]
    };
    assert!(lw(220) < lw(221), "clay {} vs rifle {}", lw(220), lw(221));
}

#[test]
fn industrial_row_collects() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_industrial_file(
        &dir.join("industrial.arrow"),
        &[fx::FixtureIndustrial {
            osm_id: 99,
            centroid: (LON, LAT),
            source_type: 0,
            name: "Plant".to_string(),
            ring_lonlat: None,
            suppressed: false,
            tags: &[],
            rated_power_kw: None,
        }],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    assert_eq!(data.industrial.len(), 1);
    assert_eq!(data.industrial[0].osm_id, 99);
}

#[test]
fn industrial_gate_is_the_polygon_edge_not_its_centroid() {
    // Garzweiler east end: the mine's centroid is 5.6 km away (past the old
    // 5 km centroid gate) but its boundary is 250 m off — the popup must see
    // it, as the painter always has. A near-centroid point row past 4 km is
    // correctly gone (the painter's per-point cap drops it too).
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let m_per_deg_lon = grid::geo::m_per_deg_lon(LAT.to_radians());
    let mine_centroid = (LON + 5600.0 / m_per_deg_lon, LAT);
    let mine_west_edge = LON + 250.0 / m_per_deg_lon;
    let mine_east_edge = mine_centroid.0 + (mine_centroid.0 - mine_west_edge);
    let half_height = 0.004;
    fx::write_industrial_file(
        &dir.join("industrial.arrow"),
        &[
            fx::FixtureIndustrial {
                osm_id: 100,
                centroid: mine_centroid,
                source_type: 0,
                name: "Mine".to_string(),
                suppressed: false,
                tags: &[],
                rated_power_kw: None,
                ring_lonlat: Some(vec![
                    (mine_west_edge, LAT - half_height),
                    (mine_east_edge, LAT - half_height),
                    (mine_east_edge, LAT + half_height),
                    (mine_west_edge, LAT + half_height),
                    (mine_west_edge, LAT - half_height),
                ]),
            },
            fx::FixtureIndustrial {
                osm_id: 101,
                centroid: (LON + 4500.0 / m_per_deg_lon, LAT),
                source_type: 0,
                name: "Far shed".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[],
                rated_power_kw: None,
            },
            // A lifecycle-retired quarry at the receiver: silent either way.
            fx::FixtureIndustrial {
                osm_id: 102,
                centroid: (LON, LAT),
                source_type: 1,
                name: "Disused quarry".to_string(),
                ring_lonlat: None,
                suppressed: true,
                tags: &[],
                rated_power_kw: None,
            },
        ],
    );
    let centroid_dist =
        grid::geo::flat_dist(LAT, LON, mine_centroid.1, mine_centroid.0);
    assert!(centroid_dist > 5000.0, "centroid {centroid_dist:.0} m");
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let ids: Vec<i64> = data.industrial.iter().map(|p| p.osm_id).collect();
    assert!(ids.contains(&100), "the mine edge reaches: {ids:?}");
    assert!(!ids.contains(&101), "a 4.5 km point is past reach: {ids:?}");
    assert!(!ids.contains(&102), "a suppressed quarry stays silent: {ids:?}");
}

#[test]
fn power_classes_read_tags_and_join_transformers() {
    // Solar MW from `plant:output:electricity`; substation MVA from the
    // joined transformer ratings inside its polygon; a second substation
    // with no transformers falls back to its voltage class (main 25 MVA);
    // the outline, the inactive row and the transformer itself emit nothing.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    let m_per_deg_lon = grid::geo::m_per_deg_lon(LAT.to_radians());
    let east = |east_m: f64| LON + east_m / m_per_deg_lon;
    // A 400 × 400 m substation yard holding two transformers (25 + 40 MVA).
    let yard = |cx: f64| {
        let (dx, dy) = (200.0 / m_per_deg_lon, 200.0 / 111_320.0);
        vec![
            (cx - dx, LAT - dy),
            (cx + dx, LAT - dy),
            (cx + dx, LAT + dy),
            (cx - dx, LAT + dy),
            (cx - dx, LAT - dy),
        ]
    };
    fx::write_industrial_file(
        &dir.join("industrial.arrow"),
        &[
            fx::FixtureIndustrial {
                osm_id: 300,
                centroid: (east(500.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_SOLAR_FARM,
                name: "Solar".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[("plant:output:electricity", "24 MW")],
                rated_power_kw: None,
            },
            fx::FixtureIndustrial {
                osm_id: 301,
                centroid: (east(700.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_SUBSTATION,
                name: "Yard".to_string(),
                ring_lonlat: Some(yard(east(700.0))),
                suppressed: false,
                tags: &[("voltage", "400000;110000")],
                rated_power_kw: None,
            },
            fx::FixtureIndustrial {
                osm_id: 302,
                centroid: (east(650.0), LAT + 0.0005),
                source_type: noise_compute::emission::industrial::SOURCE_TRANSFORMER,
                name: "T1".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[("rating", "25 MVA")],
                rated_power_kw: None,
            },
            fx::FixtureIndustrial {
                osm_id: 303,
                centroid: (east(750.0), LAT - 0.0005),
                source_type: noise_compute::emission::industrial::SOURCE_TRANSFORMER,
                name: "T2".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[("rating", "40 MVA")],
                rated_power_kw: None,
            },
            // A transformer far outside the yard: never joined.
            fx::FixtureIndustrial {
                osm_id: 304,
                centroid: (east(3000.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_TRANSFORMER,
                name: "T3".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[("rating", "160 MVA")],
                rated_power_kw: None,
            },
            // An untagged 220 kV node substation: main-median 25 MVA.
            fx::FixtureIndustrial {
                osm_id: 305,
                centroid: (east(1000.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_SUBSTATION,
                name: "Node sub".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[("voltage", "220000")],
                rated_power_kw: None,
            },
            fx::FixtureIndustrial {
                osm_id: 306,
                centroid: (east(200.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_WIND_OUTLINE,
                name: "Wind farm".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[],
                rated_power_kw: None,
            },
            fx::FixtureIndustrial {
                osm_id: 307,
                centroid: (east(300.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_INACTIVE,
                name: "Disused".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[],
                rated_power_kw: None,
            },
            // A gas valve station at the receiver: no transformer hum.
            fx::FixtureIndustrial {
                osm_id: 308,
                centroid: (east(100.0), LAT),
                source_type: noise_compute::emission::industrial::SOURCE_SUBSTATION,
                name: "Gas valves".to_string(),
                ring_lonlat: None,
                suppressed: false,
                tags: &[("substation", "valve_group")],
                rated_power_kw: None,
            },
        ],
    );
    let data = collect_sources_at_point(tmp.path(), LAT, LON).unwrap();
    let mut ids: Vec<i64> = data.industrial.iter().map(|p| p.osm_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids, vec![300, 301, 305]);
    // Per-row totals (the yard grids into cells; energy sums back).
    let total_aw = |id: i64| {
        let energy: f64 = data
            .industrial
            .iter()
            .filter(|p| p.osm_id == id)
            .map(|p| {
                let day: [f64; 8] = std::array::from_fn(|i| f64::from(p.lw_day[i]));
                10f64.powf(noise_compute::propagation::iso9613::a_weighted_total(&day) / 10.0)
            })
            .sum();
        10.0 * energy.log10()
    };
    // Solar: the tag beats the missing footprint (24 MW → 96.8 day-only);
    // substation: the joined 65 MVA wins; the node sub takes main-median 25.
    assert!((total_aw(300) - 96.8).abs() < 0.5, "solar {}", total_aw(300));
    let joined = noise_compute::emission::industrial::substation_lw(65.0);
    assert!((total_aw(301) - joined).abs() < 0.5, "joined {}", total_aw(301));
    let main = noise_compute::emission::industrial::substation_lw(25.0);
    assert!((total_aw(305) - main).abs() < 0.5, "median {}", total_aw(305));
}

#[test]
fn unstamped_structures_fail_loud() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = fx::square_dir(tmp.path(), prague());
    std::fs::create_dir_all(&dir).unwrap();
    fx::write_structure_file(&dir.join("structures.arrow"), &[building_row(1)], false);
    let err = collect_sources_at_point(tmp.path(), LAT, LON).unwrap_err();
    assert!(err.contains("structures_contract mismatch"), "got: {err}");
}

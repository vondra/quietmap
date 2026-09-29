//! Industrial rows: power physics, silent classes, facility shares and turbine inputs.

use super::*;

fn row(source_type: u8) -> IndustrialRow {
    IndustrialRow {
        osm_kind: "way".into(),
        osm_id: 1,
        centroid: (1 << 29, 1 << 29),
        source_type,
        site_subtype: 0,
        name: String::new(),
        hub_height_m: None,
        rated_power_kw: None,
        ring: Vec::new(),
        area_m2: Some(46_710.0),
        nace: None,
        tags: Tags::new(),
        suppressed: false,
        source_id: 0,
    }
}

fn tagged(source_type: u8, tags: &str) -> IndustrialRow {
    IndustrialRow {
        tags: parse_tags(tags),
        ..row(source_type)
    }
}

fn emission(row: &IndustrialRow) -> Option<RowEmission> {
    row_emission(row, &FacilityJoins::default())
}

fn day_dba(row: &IndustrialRow) -> Option<f64> {
    Some(emission(row)?.sound.day_dba)
}

/// Registry solar (NACE 3599) at 24 MW is the Vienna pilot 96.8, untagged solar the area density;
/// substations take their rating, else their class median; the rest of the power classes are silent.
#[test]
fn power_classes_use_their_own_physics_and_outlines_stay_silent() {
    let registry = IndustrialRow {
        nace: Some(SOLAR_NACE),
        ..tagged(0, r#"{}"#)
    };
    let registry = IndustrialRow {
        rated_power_kw: Some(24_000.0),
        ..registry
    };
    assert!((day_dba(&registry).unwrap() - 96.8).abs() < 0.1);
    let untagged = day_dba(&row(SOURCE_SOLAR_FARM)).unwrap();
    assert!((untagged - (88.0 + 10.0 * (4.671 * 0.55f64).log10() - 5.0)).abs() < 1e-9);
    let rated = tagged(SOURCE_SUBSTATION, r#"{"rating":"50 MVA"}"#);
    assert!((day_dba(&rated).unwrap() - substation_sound_power(50.0).day_dba).abs() < 1e-9);
    let auto = tagged(SOURCE_SUBSTATION, r#"{"transformer":"auto"}"#);
    assert!((day_dba(&auto).unwrap() - substation_sound_power(160.0).day_dba).abs() < 1e-9);
    let substation = emission(&rated).unwrap();
    assert_eq!(
        (substation.sound.night_offset_db, substation.height_m),
        (0.0, 5.0)
    );
    for silent in [
        SOURCE_RAIL_YARD,
        SOURCE_WIND_OUTLINE,
        SOURCE_INACTIVE,
        SOURCE_TRANSFORMER,
        16,
        u8::MAX,
    ] {
        assert_eq!(day_dba(&row(silent)), None, "type {silent}");
    }
    assert_eq!(
        day_dba(&IndustrialRow {
            suppressed: true,
            ..row(0)
        }),
        None
    );
    assert_eq!(
        day_dba(&tagged(SOURCE_SUBSTATION, r#"{"substation":"gas"}"#)),
        None
    );
}

/// Ten equal parts of a 100 MVA (or 0.4 MVA) facility radiate what the whole does; a solar plant
/// shares its nameplate, while unit outputs and area densities are already per part.
#[test]
fn facility_parts_share_one_acoustic_power_by_area() {
    for rating in ["100 MVA", "0.4 MVA"] {
        let part = |share: &str| {
            let tags = format!(r#"{{"rating":"{rating}","qm:facility_share":"{share}"}}"#);
            10f64.powf(day_dba(&tagged(SOURCE_SUBSTATION, &tags)).unwrap() / 10.0)
        };
        let error_db = 10.0 * (10.0 * part("0.1") / part("1")).log10();
        assert!(error_db.abs() < 0.15, "{rating}: {error_db:+.2} dB");
    }
    let solar = |tags: &str, kw: Option<f64>| {
        let row = IndustrialRow {
            rated_power_kw: kw,
            ..tagged(SOURCE_SOLAR_FARM, tags)
        };
        10f64.powf(day_dba(&row).unwrap() / 10.0)
    };
    let nameplate = r#"{"plant:output:electricity":"24 MW","qm:facility_share":"0.5"}"#;
    let whole = r#"{"plant:output:electricity":"24 MW"}"#;
    assert!((10.0 * (2.0 * solar(nameplate, None) / solar(whole, None)).log10()).abs() < 0.15);
    let half = r#"{"qm:facility_share":"0.5"}"#;
    assert_eq!(solar(half, None), solar("{}", None));
    assert_eq!(solar(half, Some(5.0)), solar("{}", Some(5.0)));
}

#[test]
fn solar_points_need_an_output_or_a_footprint_and_generators_defer_to_their_plant() {
    let bare = IndustrialRow {
        area_m2: None,
        ..row(SOURCE_SOLAR_FARM)
    };
    assert_eq!(day_dba(&bare), None);
    let unit = IndustrialRow {
        rated_power_kw: Some(5.0),
        ..bare
    };
    assert!((day_dba(&unit).unwrap() - (88.0 + 10.0 * 0.005f64.log10() - 5.0)).abs() < 1e-9);
    let (x, y) = (1 << 29, 1 << 29);
    let plant_ring = vec![
        (x - 500, y - 500),
        (x + 500, y - 500),
        (x + 500, y + 500),
        (x - 500, y + 500),
    ];
    let mut joins = FacilityJoins::default();
    joins.add_row(
        SOURCE_SOLAR_FARM,
        ("way", 2),
        (x, y),
        &plant_ring,
        &parse_tags(r#"{"power":"plant"}"#),
    );
    let generator = tagged(SOURCE_SOLAR_FARM, r#"{"power":"generator"}"#);
    assert_eq!(row_emission(&generator, &joins), None);
    let plant = IndustrialRow {
        ring: plant_ring,
        ..tagged(SOURCE_SOLAR_FARM, r#"{"power":"plant"}"#)
    };
    assert!(row_emission(&plant, &joins).is_some());
}

/// Untagged hubs stand at 105 m, tag errors clamp to 175 m, and ratings over 8 MW are unknown.
#[test]
fn turbines_take_default_and_clamped_inputs() {
    let turbine = |hub: Option<f64>, kw: Option<f64>| {
        let row = IndustrialRow {
            hub_height_m: hub,
            rated_power_kw: kw,
            ..row(SOURCE_WIND_TURBINE)
        };
        emission(&row).unwrap()
    };
    let unknown = turbine(None, Some(20_000.0));
    assert_eq!(
        (unknown.height_m, unknown.rated_power_kw, unknown.area_m2),
        (105.0, None, None)
    );
    assert!((unknown.sound.day_dba - (105.0 - 2.14)).abs() < 1e-9);
    assert_eq!(turbine(Some(250.0), None).height_m, 175.0);
    assert_eq!(turbine(Some(120.0), Some(3_000.0)).height_m, 120.0);
    assert_eq!(turbine(None, Some(3_000.0)).rated_power_kw, Some(3_000.0));
}

#[test]
fn profiles_resolve_nace_then_subtype_then_type_with_their_heights() {
    let steel = IndustrialRow {
        nace: Some(2410),
        area_m2: Some(3_000_000.0),
        ..row(0)
    };
    let expected = area_law_sound_power(
        &nace_profile(2410).unwrap(),
        3e6,
        INDUSTRIAL_AREA_CAP_HEAVY_M2,
    );
    assert_eq!(
        row_emission(&steel, &FacilityJoins::default()),
        Some(RowEmission {
            sound: expected,
            height_m: 10.0,
            label: "industrial_area",
            area_m2: Some(3e6),
            rated_power_kw: None,
        })
    );
    let warehouse = IndustrialRow {
        site_subtype: 1,
        area_m2: Some(10_000.0),
        ..row(3)
    };
    assert!(
        (day_dba(&warehouse).unwrap() - 86.0 - a_weighted_sum(&nace_profile(5210).unwrap())).abs()
            < 1e-9
    );
    let quarry = emission(&row(SOURCE_QUARRY)).unwrap();
    assert_eq!((quarry.height_m, quarry.label), (8.0, "quarry"));
}

fn a_weighted_sum(profile: &IndustrialProfile) -> f64 {
    physics::emission::spectrum::a_weighted_level_db(&profile.spectrum_db)
}

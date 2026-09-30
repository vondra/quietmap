//! Helicopter levels: dev4's corrections kept, the EASA levels reproduced, and the FIX.

use super::*;
use crate::doc29::npd::{class_anchor, curve_level_db, tail_absorption_db_per_m};
use crate::doc29::profiles_generated::profile_idx;
use crate::doc29::segment::AircraftType;

/// dev4's generated corrections (level, climb, descent; dB), thrust_generated.rs at c815f215.
const DEV4_CORRECTIONS: [(&str, [f64; 3]); 20] = [
    (
        "EC35",
        [-14.22694323436491, -8.826943234364919, -5.526943234364907],
    ),
    (
        "EC45",
        [-11.606867651416081, -7.806867651416084, -6.006867651416087],
    ),
    (
        "EC55",
        [-8.554914059698547, -3.2549140596985495, -1.7549140596985495],
    ),
    (
        "EC30",
        [-13.704914059698552, -10.404914059698555, -7.404914059698555],
    ),
    (
        "EC20",
        [-16.104914059698544, -12.804914059698547, -12.00491405969855],
    ),
    (
        "AS50",
        [-10.23358594371868, -5.933585943718668, -6.333585943718674],
    ),
    (
        "AS55",
        [-10.234508605529243, -6.234508605529243, -4.134508605529248],
    ),
    (
        "AS65",
        [-6.661242608739656, -2.1612426087396557, -1.0612426087396614],
    ),
    (
        "H500",
        [-11.098737091705033, -7.748737091705038, -8.148737091705044],
    ),
    (
        "MD52",
        [-17.017026756784617, -9.617026756784611, -9.317026756784614],
    ),
    (
        "B06",
        [-10.804402375648749, -5.60440237564876, -5.60440237564876],
    ),
    (
        "B407",
        [-9.73164198385183, -6.431641983851833, -5.631641983851836],
    ),
    (
        "B412",
        [-4.087614312319289, -2.6876143123192975, -1.8876143123193003],
    ),
    (
        "R22",
        [-17.40491405969854, -14.104914059698544, -13.304914059698547],
    ),
    (
        "R44",
        [-14.599166573620565, -11.299166573620568, -10.49916657362057],
    ),
    (
        "R66",
        [-12.924547549192766, -8.174547549192766, -9.37454754919277],
    ),
    (
        "S76",
        [-5.836675976963221, -1.6366759769632182, -1.4366759769632154],
    ),
    (
        "A109",
        [-7.166536514589808, -4.766536514589802, -5.166536514589808],
    ),
    (
        "BK17",
        [-6.639452684005704, -5.0394526840056955, -1.639452684005704],
    ),
    (
        "B505",
        [-13.154914059698541, -8.554914059698532, -7.754914059698535],
    ),
];

fn corrections(levels: HelicopterLevels) -> [f64; 3] {
    [
        levels.correction_db(false, false),
        levels.correction_db(true, false),
        levels.correction_db(false, true),
    ]
}

#[test]
fn dev4_designators_keep_their_corrections() {
    for (designator, expected) in DEV4_CORRECTIONS {
        let actual = corrections(helicopter_levels(designator));
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "{designator}: {actual} vs {expected}"
            );
        }
    }
}

/// Level, climb and descent SEL at 150 m of the certified records (EASA Issue 52) through the
/// helicopter class curve.
#[test]
fn corrections_reproduce_the_easa_sel_at_150_m() {
    let class = AircraftType::from_designator("EC35").class;
    let anchor = class_anchor(class);
    let at_150_m = |curve| curve_level_db(curve, tail_absorption_db_per_m(curve), 150.0);
    let (approach, departure) = (
        at_150_m(&anchor.approach_sel),
        at_150_m(&anchor.departure_sel),
    );
    for (designator, level, climb_uplift, descent_uplift) in [
        ("EC35", 80.6, 3.4, 8.7),
        ("AS50", 84.6, 2.3, 3.9),
        ("B412", 90.7, -0.6, 2.2),
        ("A139", 87.8, -0.4, 3.4),
        ("S92", 94.55, -2.6, 0.3),
    ] {
        let [level_db, climb_db, descent_db] = corrections(helicopter_levels(designator));
        assert!(
            (approach + level_db - level).abs() < 0.1,
            "{designator} level"
        );
        let climb = departure + climb_db - (level + climb_uplift);
        assert!(climb.abs() < 0.1, "{designator} climb");
        let descent = approach + descent_db - (level + descent_uplift);
        assert!(descent.abs() < 0.1, "{designator} descent");
    }
}

/// PLAN-z13 FIX: 22.8 % of helicopter points read the EC135 level, 5-14 dB too quiet. Each
/// designator with an EASA record now reads its own: its level over EC135's (dB).
#[test]
fn a139_and_s92_no_longer_read_as_ec135() {
    let ec135 = helicopter_levels("EC35");
    for (designator, over_ec135_db) in [
        ("A139", 7.19),
        ("A169", 5.37),
        ("A119", 5.92),
        ("B429", 6.37),
        ("EC75", 7.77),
        ("EC25", 10.27),
        ("AS32", 10.25),
        ("S92", 13.97),
        ("H160", 5.37),
    ] {
        assert_eq!(profile_idx(designator), profile_idx("EC35"), "{designator}");
        let levels = AircraftType::from_designator(designator)
            .helicopter
            .unwrap();
        let over = levels.correction_db(false, false) - ec135.correction_db(false, false);
        assert!((over - over_ec135_db).abs() < 0.01, "{designator}: {over}");
    }
}

#[test]
fn designators_without_easa_levels_read_their_mass_class() {
    use HelicopterMassClass::{Heavy, Light, Medium};
    for (designator, class) in [
        ("H60", Heavy),
        ("B212", Heavy),
        ("GYRO", Light),
        ("UHEL", Light),
    ] {
        assert_eq!(
            helicopter_levels(designator),
            class.levels(),
            "{designator}"
        );
    }
    // Any other rotorcraft the mapping routes to the helicopter class reads the light class.
    let unlisted = AircraftType::from_designator("H199").helicopter;
    assert_eq!(unlisted, Some(Light.levels()));
    assert_eq!(HelicopterMassClass::from_mass_kg(5_080.0), Heavy);
    assert_eq!(HelicopterMassClass::from_mass_kg(5_000.0), Medium);
    assert_eq!(HelicopterMassClass::from_mass_kg(3_200.0), Light);
    let spec = [(Light, 83.1), (Medium, 84.4), (Heavy, 89.7)];
    for (class, level) in spec {
        assert!(
            (class.levels().level_sel_150m_db - level).abs() < 0.05,
            "{class:?}"
        );
    }
}

#[test]
fn h_series_names_read_their_icao_designator() {
    for (name, designator) in [
        ("H125", "AS50"),
        ("H135", "EC35"),
        ("H145", "EC45"),
        ("H225", "EC25"),
    ] {
        assert_eq!(
            helicopter_levels(name),
            helicopter_levels(designator),
            "{name}"
        );
    }
}

/// No row of the table is unreachable: the designator mapping routes every listed designator to
/// the helicopter class, and no fixed-wing type carries helicopter levels.
#[test]
fn every_listed_designator_is_a_helicopter_and_no_airliner_is() {
    let listed = CERTIFIED
        .iter()
        .flat_map(|(designators, _)| designators.iter().copied())
        .chain(WITHOUT_LEVELS.iter().map(|(designator, _)| *designator));
    for designator in listed {
        assert!(
            AircraftType::from_designator(designator)
                .helicopter
                .is_some(),
            "{designator}"
        );
    }
    for designator in ["B738", "A320", "C172", "DH8D", "XXXX"] {
        assert_eq!(
            AircraftType::from_designator(designator).helicopter,
            None,
            "{designator}"
        );
    }
}

//! Leisure areas and shooting ranges against their published anchors.

use super::*;

fn day(class: u8, area_m2: f64) -> f64 {
    leisure_sound_power(class, area_m2).unwrap().day_dba
}

fn lden(sound: &SoundPower) -> f64 {
    let (day, evening, night) = (
        sound.day_dba,
        sound.day_dba + sound.evening_offset_db + 5.0,
        sound.day_dba + sound.night_offset_db + 10.0,
    );
    10.0 * ((12.0 * 10f64.powf(day / 10.0)
        + 4.0 * 10f64.powf(evening / 10.0)
        + 8.0 * 10f64.powf(night / 10.0))
        / 24.0)
        .log10()
}

/// The Parkplatzlaermstudie arithmetic: 63 dB(A) per space and movement, 0.40 movements per
/// space and hour, K_D above ten spaces, and the study's 06-22 / 22-06 blocks re-averaged.
#[test]
fn car_parks_follow_the_parking_study() {
    let study = |spaces: f64| {
        let searching = if spaces > 10.0 {
            2.5 * (spaces - 9.0).log10()
        } else {
            0.0
        };
        63.0 + searching + 10.0 * (spaces * 0.40).log10()
    };
    for (class, m2_per_space, area_m2) in [
        (CAR_PARK, 23.8, 5_000.0),
        (CAR_PARK, 23.8, 1_000.0),
        (CAR_PARK_STREET, 13.3, 200.0),
        (CAR_PARK_STREET, 13.3, 120.0),
    ] {
        let expected = study(area_m2 / m2_per_space);
        assert!(
            (day(class, area_m2) - expected).abs() < 0.1,
            "class {class} at {area_m2} m2"
        );
    }
    let lot = leisure_profile(CAR_PARK).unwrap();
    let evening = 10.0 * ((3.0 * 0.40 + 0.05) / 4.0 / 0.40f64).log10();
    let night = 10.0 * ((0.40 + 7.0 * 0.05) / 8.0 / 0.40f64).log10();
    assert!((lot.evening_offset_db - evening).abs() < 0.05);
    assert!((lot.night_offset_db - night).abs() < 0.05);
    assert_eq!(leisure_profile(PITCH).unwrap().m2_per_parking_space, None);
}

/// Pitch duty from published use: 59.8 dB(A)/m2 active; grass 4 day and 1 evening hours a week
/// for 40 weeks, a booked artificial pitch 40 h a week all year; Lden 83.5 and 93.8 at 7,000 m2.
#[test]
fn pitch_duty_follows_published_use() {
    for (class, day_hours, evening_hours, weeks) in [
        (PITCH, 4.0f64, 1.0, 40.0),
        (
            ARTIFICIAL_TURF_PITCH,
            26.0 / 34.0 * 40.0,
            8.0 / 34.0 * 40.0,
            52.0,
        ),
    ] {
        let profile = leisure_profile(class).unwrap();
        let day_duty = 10.0 * (day_hours * weeks / (12.0 * 365.0)).log10();
        let evening_duty = 10.0 * (evening_hours * weeks / (4.0 * 365.0)).log10();
        assert!((profile.lw_per_m2_dba - (59.8 + day_duty)).abs() < 0.1);
        assert!((profile.evening_offset_db - (evening_duty - day_duty)).abs() < 0.1);
        assert_eq!(profile.night_offset_db, -25.0);
    }
    let grass = leisure_sound_power(PITCH, 7_000.0).unwrap();
    let booked = leisure_sound_power(ARTIFICIAL_TURF_PITCH, 7_000.0).unwrap();
    assert!((lden(&grass) - 83.5).abs() < 0.2 && (lden(&booked) - 93.8).abs() < 0.2);
}

/// The PLAN-z13 fix: a pitch counts at most 10 ha, other classes keep growing.
#[test]
fn pitches_stop_growing_at_ten_hectares() {
    for class in [PITCH, ARTIFICIAL_TURF_PITCH] {
        assert_eq!(day(class, 1_000_000.0), day(class, PITCH_AREA_CAP_M2));
        assert!(day(class, PITCH_AREA_CAP_M2) > day(class, 50_000.0));
    }
    assert!(day(STADIUM, 1_000_000.0) > day(STADIUM, PITCH_AREA_CAP_M2));
}

#[test]
fn areas_scale_three_decibels_per_doubling_and_padel_is_loudest() {
    let terrace = leisure_profile(OUTDOOR_SEATING).unwrap().reference_area_m2;
    assert!((day(OUTDOOR_SEATING, terrace) - 65.8).abs() < 0.2);
    let doubled = day(OUTDOOR_SEATING, 2.0 * terrace) - day(OUTDOOR_SEATING, terrace);
    assert!((doubled - 3.0103).abs() < 0.05);
    let at_reference = |class: u8| day(class, leisure_profile(class).unwrap().reference_area_m2);
    assert!(at_reference(PADEL) > at_reference(TENNIS));
    assert!(at_reference(TENNIS) > at_reference(BASKETBALL));
    assert!((at_reference(PADEL) - 81.0).abs() < 0.2);
}

#[test]
fn motorsport_shooting_and_unknown_classes_have_no_area_law() {
    for class in [MOTORSPORT, SHOOTING, ARTIFICIAL_TURF_PITCH + 1, u8::MAX] {
        assert_eq!(leisure_sound_power(class, 10_000.0), None, "class {class}");
    }
}

/// The w7-sources pilots at 20,000 shots a year: rifle 110.0, shotgun 105.8, pistol 104.6 dB(A),
/// by day only.
#[test]
fn shooting_ranges_reproduce_the_pilots() {
    let shots = |subtype| shooting_sound_power(subtype).unwrap();
    assert!((shots(ShootingSubtype::Rifle).day_dba - 110.0).abs() < 0.05);
    assert!((shots(ShootingSubtype::Shotgun).day_dba - 105.8).abs() < 0.05);
    assert!((shots(ShootingSubtype::Pistol).day_dba - 104.6).abs() < 0.05);
    assert_eq!(shots(ShootingSubtype::Rifle).night_offset_db, -50.0);
    assert_eq!(shooting_sound_power(ShootingSubtype::Silent), None);
}

#[test]
fn shooting_subtype_reads_tags_then_the_name() {
    use ShootingSubtype::*;
    let tag = |value: &str| shooting_subtype(Some(value), &[], "");
    assert_eq!(
        (
            tag("rifle"),
            tag("pistol"),
            tag("clay-pigeon"),
            tag("skeet")
        ),
        (Rifle, Pistol, Shotgun, Shotgun)
    );
    assert_eq!(
        (tag("pistol;rifle"), tag("pistol;clay_pigeon")),
        (Rifle, Shotgun)
    );
    assert_eq!(
        (tag("archery"), tag("paintball"), tag("indoor_range")),
        (Silent, Silent, Silent)
    );
    assert_eq!(shooting_subtype(None, &["pistol"], ""), Pistol);
    let named = |name: &str| shooting_subtype(None, &[], name);
    assert_eq!(
        (named("Tatra clay"), named("Hodonice IPSC")),
        (Shotgun, Pistol)
    );
    assert_eq!(
        (named("Rifle and pistol club"), named("City archery club")),
        (Rifle, Silent)
    );
    assert_eq!((named("Strelnice"), named("")), (Rifle, Rifle));
}

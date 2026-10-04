use super::*;

fn hours(from: usize, to: usize) -> WeekHours {
    let mut week = [[false; 24]; 7];
    for day in 0..7 {
        for hour in from..to {
            week[(day + hour / 24) % 7][hour % 24] = true;
        }
    }
    week
}

/// Ten seats taken from 19 to 23 h: 0.75 of them on Friday and Saturday evenings and 2.5 dB fewer
/// on the other five, each person at 70.7 dB(A): 70.7 + 10 lg(10 x 0.75 x (2 + 5 x 10^-0.25) / 7)
/// = 77.8 dB(A) in the evening; day and night silent.
#[test]
fn seats_sound_by_their_occupancy_and_the_weekday() {
    let terrace = hours(19, 23);
    let venue = Venue {
        seats: 10.0,
        season_share: 1.0,
        open: &terrace,
        terrace: &terrace,
        door_from_hour: None,
        evening_dba: LIVELY_DBA,
        night_dba: STANDING_DBA,
        drinking: false,
        crowd_dba: None,
    };
    let sound = venue_sound_power(&venue).unwrap();
    let evening = sound.day_dba + sound.evening_offset_db;
    let expected = LIVELY_DBA + 10.0 * (7.5 * (2.0 + 5.0 * 10f64.powf(-0.25)) / 7.0).log10();
    assert!((evening - expected).abs() < 1e-9, "{evening} {expected}");
    assert!((expected - 77.82).abs() < 0.01);
    assert!(sound.evening_offset_db > 49.0 && sound.night_offset_db < -49.0);
    // Half the year: 3 dB down.
    let half = Venue {
        season_share: 0.5,
        ..venue
    };
    let half = venue_sound_power(&half).unwrap();
    assert!((half.day_dba + half.evening_offset_db - (expected - 3.01)).abs() < 0.01);
}

/// A bar open 18-02 h keeps 2.4 people at its door from 19 h (73 dB(A) each, 6 dB fewer on weekday
/// nights) and empties into the street from 02 to 03 h at 5 dB under its last hour.
#[test]
fn a_bar_has_people_at_its_door_and_empties_when_it_closes() {
    let open = hours(18, 26);
    let closed = [[false; 24]; 7];
    let bar = Venue {
        seats: 0.0,
        season_share: 1.0,
        open: &open,
        terrace: &closed,
        door_from_hour: Some(19),
        evening_dba: LIVELY_DBA,
        night_dba: STANDING_DBA,
        drinking: true,
        crowd_dba: None,
    };
    let sound = venue_sound_power(&bar).unwrap();
    // Night: 3 open hours (23, 00, 01) and the dispersal hour (02) of 8, weekend nights at full
    // strength, the others 6 dB down.
    let week = (2.0 + 5.0 * 10f64.powf(-0.6)) / 7.0;
    let night = 10.0 * (DOOR_PEOPLE * week * (3.0 + 10f64.powf(-0.5)) / 8.0).log10() + STANDING_DBA;
    let modelled = sound.day_dba + sound.night_offset_db;
    assert!((modelled - night).abs() < 1e-9, "{modelled} {night}");
    // By day the door crowd stands only from 19 h: the day's 18 h is open but empty.
    assert!(sound.evening_offset_db > 49.0);
}

/// Ballesteros's five places along 40 m gather 0.20 people a m2 at the weekend's peak: 91.3 dB(A)
/// over the segment (the research's 89.6-92.8); three places 86.8 (84.5-88.7); two none.
#[test]
fn street_crowds_grow_with_the_places_along_the_street() {
    assert!((street_crowd_dba(5.0).unwrap() - 91.3).abs() < 0.05);
    assert!((street_crowd_dba(3.0).unwrap() - 86.8).abs() < 0.05);
    assert!(street_crowd_dba(2.9).is_none());
}

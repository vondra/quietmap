//! Rolls: a flight seen low along a runway rolls where the ground receivers missed it, once per
//! movement; a roll runs where Doc 29 puts it, and its tile holds it at its start.

use super::*;
use crate::airport::low_ends::LowEnd;
use crate::airport::rolls::{RollMovement, movements};

/// The first or last low segment of flight `flight`: 100 m along the runway line direction
/// (eastwards) ending at `east_m`.
fn low_end(flight: u64, first: bool, departure: bool, class: u8, east_m: f64) -> LowEnd {
    LowEnd {
        flight_id: flight,
        episode: u16::from(!first),
        class,
        period: 0,
        first,
        departure,
        secondary_only: false,
        start: at(0.0, east_m - 100.0),
        end: at(0.0, east_m),
    }
}

/// On a 2,500 m runway (ten 250 m pieces, west to east): a departure first seen 500 m past its
/// east end takes off eastwards, a landing last seen 1 km before the west threshold lands
/// eastwards, a Cessna first seen 200 m from the west end and an A320 400 m from it (an
/// intersection's take-off, or one whose first airborne sample came early) take off. A flight
/// seen rolling, a helicopter and a climb across the runway get none.
#[test]
fn flights_seen_low_along_a_runway_get_the_rolls_they_miss() {
    let aeroways = airport(10);
    let runways = Runways::new(&aeroways);
    let (a320, helicopter, c172) = (class("A320-232"), class("HELICOPTER"), class("CNA172"));
    let ends = [
        low_end(1, true, true, a320, 3_000.0),
        low_end(2, false, false, a320, -1_000.0),
        low_end(3, true, true, a320, 3_000.0),
        low_end(4, true, true, helicopter, 3_000.0),
        LowEnd {
            end: at(100.0, 2_900.0),
            ..low_end(5, true, true, a320, 2_900.0)
        },
        low_end(6, true, true, a320, 500.0),
        low_end(7, true, true, c172, 300.0),
    ];
    let key = |landing| RollKey {
        runway: 0,
        forward: true,
        landing,
        class: a320,
    };
    let seen_rolling = RollMovement {
        key: key(false),
        flight_id: 3,
        episode: 0,
        period: 0,
        secondary_only: false,
    };
    let found: Vec<(u64, RollKey)> = movements(vec![seen_rolling], &ends, &runways)
        .into_iter()
        .map(|roll| (roll.flight_id, roll.key))
        .collect();
    let cessna = RollKey {
        class: c172,
        ..key(false)
    };
    assert_eq!(
        found,
        vec![
            (3, key(false)),
            (1, key(false)),
            (2, key(true)),
            (6, key(false)),
            (7, cessna)
        ]
    );
}

/// An A320 takes off from the end it leaves over its 1,800 m to 150 kt, either way; it lands from
/// 291 m past the threshold over its 1,400 m from 130 kt. On a 1,000 m runway both stop at the
/// runway's end, the landing touching down a quarter in; a helicopter does not roll.
#[test]
fn a_roll_runs_from_the_runway_end_or_the_touchdown_point() {
    let runways = Runways::new(&airport(10));
    let a320 = class("A320-232");
    let key = |forward, landing, class| RollKey {
        runway: 0,
        forward,
        landing,
        class,
    };
    let east_of = |point: [f64; 2]| length_m(at(0.0, 0.0), point);
    let eastwards = runways.path(key(true, false, a320)).unwrap();
    assert!(east_of(eastwards.start) < 1.0);
    assert!((east_of(eastwards.end) - 1_800.0).abs() < 1.0);
    assert_eq!(eastwards.length_m, 1_800.0);
    assert!((eastwards.speed_ms - 150.0 * 1_852.0 / 3_600.0).abs() < 1e-9);
    let westwards = runways.path(key(false, false, a320)).unwrap();
    assert!((east_of(westwards.start) - 2_500.0).abs() < 1.0);
    assert!((east_of(westwards.end) - 700.0).abs() < 1.0);
    let landing = runways.path(key(true, true, a320)).unwrap();
    assert!((east_of(landing.start) - 291.0).abs() < 1.0);
    assert!((east_of(landing.end) - 1_691.0).abs() < 1.0);
    let short = Runways::new(&airport(4));
    assert!((short.path(key(true, false, a320)).unwrap().length_m - 1_000.0).abs() < 0.5);
    assert!((short.path(key(true, true, a320)).unwrap().length_m - 750.0).abs() < 0.5);
    assert_eq!(runways.path(key(true, false, class("HELICOPTER"))), None);
}

/// One count per movement: an increment day on which the primary saw a departure low at the
/// runway and the secondary saw its roll counts the primary's (a roll from its low end), not the
/// secondary's; a landing only the secondary saw 50 km away, at no runway, rolls nowhere. A landing
/// the primary followed low on final and the secondary completed rolls once, from the primary's
/// end.
#[test]
fn where_the_primary_saw_a_movement_its_roll_counts_once() {
    let aeroways = airport(10);
    let a320 = class("A320-232");
    let roll = |departure: bool, east_m: f64| GroundLeg {
        secondary_only: true,
        departure,
        speed_kt: 120.0,
        ..leg(
            7,
            Mover::Aircraft { class: a320 },
            at(0.0, east_m),
            at(0.0, east_m + 250.0),
        )
    };
    let landing = GroundLeg {
        episode: 1,
        ..roll(false, 50_000.0)
    };
    let legs = [roll(true, 0.0), landing];
    let ends = [
        low_end(7, true, true, a320, 3_000.0),
        LowEnd {
            secondary_only: true,
            ..low_end(7, false, false, a320, 49_000.0)
        },
    ];
    let traffic = traffic_of(&aeroways, (&legs, &ends), (0.25, 1.0));
    assert_eq!(traffic.movements[0], [0.0, 0.25, 0.0]);
    assert_eq!(traffic.rolls.len(), 1);
    let legs = [GroundLeg {
        flight_id: 8,
        episode: 1,
        ..roll(false, 0.0)
    }];
    let ends = [
        low_end(8, false, false, a320, -1_200.0),
        LowEnd {
            secondary_only: true,
            ..low_end(8, false, false, a320, -300.0)
        },
    ];
    let traffic = traffic_of(&aeroways, (&legs, &ends), (0.25, 1.0));
    assert_eq!(traffic.movements[0], [0.25, 0.0, 0.0]);
}

/// A day of rolls in a fresh tree without terrain (the sea, 0 m): each roll in the tile of its
/// start, along its runway's heading, with its length, speed, class and movements.
#[test]
fn rolls_are_written_in_the_tile_of_their_start() {
    let out = std::env::temp_dir().join(format!("qm-roll-tiles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let runways = Runways::new(&airport(10));
    let a320 = class("A320-232");
    let key = |forward| RollKey {
        runway: 0,
        forward,
        landing: false,
        class: a320,
    };
    let rolls = BTreeMap::from([(key(true), [2.0, 1.0, 0.5]), (key(false), [1.0, 0.0, 0.0])]);
    assert_eq!(roll_tiles::write(&rolls, &runways, &out, &out).unwrap(), 2);
    for (forward, east_m, heading_rad) in [(true, 0.0, 0.0), (false, 2_500.0, std::f64::consts::PI)]
    {
        let [lat, lon] = at(0.0, east_m);
        let tile = TileId::containing(Mercator::from_degrees(lat, lon));
        let bytes = tiles::square::read_tile(&out, tile, tiles::Kind::AircraftRolls)
            .unwrap()
            .expect("the tile of the start");
        let read = tiles::aircraft_rolls::parse(&bytes).unwrap();
        let roll = read
            .iter()
            .find(|roll| (roll.heading_rad - heading_rad).abs() < 0.01)
            .expect("the roll");
        let start = tile.to_mercator(roll.start).to_degrees();
        assert!(length_m([start.0, start.1], [lat, lon]) < 1.0, "{start:?}");
        assert_eq!(roll.length_m, 1_800.0);
        assert_eq!((roll.class, roll.landing, roll.field_m), (a320, false, 0.0));
        assert_eq!(roll.movements_per_day, rolls[&key(forward)]);
    }
    std::fs::remove_dir_all(&out).unwrap();
}

/// Every class of a runway direction rolls on one heading, the runway's, whatever its own length:
/// on a diagonal runway at 60 deg N a 300 m piston roll, an A320's 1,800 m and a 777's 2,600 m
/// keep the same stored heading (each heading toward its own end had split one runway's take-offs
/// into several streams of passes, `popup::aircraft::rolls`).
#[test]
fn every_class_of_a_runway_direction_rolls_on_one_heading() {
    let out = std::env::temp_dir().join(format!("qm-roll-headings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let mut aeroways = Aeroways::default();
    let airport = aeroways.intern(Airport {
        key: "DIAGONAL".into(),
        name: String::new(),
    });
    let (first, last) = ([60.0, 10.0], [60.03125, 10.0625]);
    let point = |share: f64| [0, 1].map(|k| first[k] + share * (last[k] - first[k]));
    for piece in 0..10u16 {
        let share = f64::from(piece) / 10.0;
        aeroways.lines.push(AerowayLine {
            osm_id: 1,
            segment: piece,
            ends: [point(share), point(share + 0.1)],
            aeroway: Aeroway::Runway,
            airport,
            square: Square { x: 270, y: 148 },
        });
    }
    let runways = Runways::new(&aeroways);
    let key = |class| RollKey {
        runway: 0,
        forward: true,
        landing: false,
        class,
    };
    let classes = [class("CNA172"), class("A320-232"), class("777300")];
    let rolls: BTreeMap<RollKey, [f64; 3]> = (classes.iter())
        .map(|&class| (key(class), [1.0, 0.0, 0.0]))
        .collect();
    assert_eq!(roll_tiles::write(&rolls, &runways, &out, &out).unwrap(), 3);
    let tile = TileId::containing(Mercator::from_degrees(first[0], first[1]));
    let bytes = tiles::square::read_tile(&out, tile, tiles::Kind::AircraftRolls)
        .unwrap()
        .expect("the tile of the start");
    let read = tiles::aircraft_rolls::parse(&bytes).unwrap();
    let lengths: Vec<f64> = read.iter().map(|roll| roll.length_m).collect();
    assert_eq!(lengths.len(), 3, "{lengths:?}");
    assert!(
        read.iter()
            .all(|roll| roll.heading_rad == read[0].heading_rad),
        "{read:?}"
    );
    std::fs::remove_dir_all(&out).unwrap();
}

/// A runway drawn as two ways meeting end to end is one runway: a take-off starts at its far end
/// and rolls on past the join; a way meeting it across is another runway, and a stopway past its
/// end is not its pavement to roll from.
#[test]
fn a_runway_drawn_as_two_ways_is_one() {
    let mut aeroways = airport(10);
    for line in aeroways.lines.iter_mut().filter(|line| line.osm_id == 7) {
        if line.segment >= 4 {
            (line.osm_id, line.segment) = (8, line.segment - 4);
        }
    }
    aeroways.lines.push(AerowayLine {
        osm_id: 9,
        segment: 0,
        ends: [at(0.0, 1_000.0), at(500.0, 1_000.0)],
        aeroway: Aeroway::Runway,
        airport: 0,
        square: Square { x: 276, y: 173 },
    });
    aeroways.lines.push(AerowayLine {
        osm_id: 10,
        segment: 0,
        ends: [at(0.0, -200.0), at(0.0, 0.0)],
        aeroway: Aeroway::Stopway,
        airport: 0,
        square: Square { x: 276, y: 173 },
    });
    let runways = Runways::new(&aeroways);
    let a320 = class("A320-232");
    let b738 = Mover::Aircraft { class: a320 };
    let on_the_second_way = leg(1, b738, at(0.0, 1_500.0), at(0.0, 1_750.0));
    let (runway, forward) = runways
        .rolling(8, &on_the_second_way, a320)
        .expect("a roll");
    assert_eq!(
        runways.rolling(7, &on_the_second_way, a320),
        Some((runway, forward))
    );
    let key = RollKey {
        runway,
        forward,
        landing: false,
        class: a320,
    };
    let path = runways.path(key).expect("a path");
    assert!(length_m(path.start, at(0.0, 0.0)) < 1.0, "{path:?}");
    assert!((path.length_m - 1_800.0).abs() < 1e-9);
    assert_ne!(
        runways.rolling(9, &leg(2, b738, at(0.0, 1_000.0), at(400.0, 1_000.0)), a320),
        Some((runway, forward))
    );
}

/// A circuit seen rolling on its first touch-and-go and then low on its final approach: the final
/// landing, a later stretch on the ground, still gets its roll.
#[test]
fn a_circuits_final_landing_is_not_its_touch_and_go() {
    let aeroways = airport(10);
    let a320 = class("A320-232");
    let touch_and_go = GroundLeg {
        episode: 1,
        departure: false,
        ..leg(
            9,
            Mover::Aircraft { class: a320 },
            at(0.0, 400.0),
            at(0.0, 650.0),
        )
    };
    let final_approach = LowEnd {
        episode: 3,
        ..low_end(9, false, false, a320, -1_000.0)
    };
    let traffic = traffic_of(&aeroways, (&[touch_and_go], &[final_approach]), (1.0, 0.0));
    assert_eq!(traffic.movements[0], [2.0, 0.0, 0.0]);
}

/// A helicopter's fast leg on a runway taxis: it has no roll to take its sound.
#[test]
fn a_helicopter_on_a_runway_taxis() {
    let aeroways = airport(2);
    let helicopter = class("HELICOPTER");
    let runways = Runways::new(&aeroways);
    let hover = leg(
        4,
        Mover::Aircraft { class: helicopter },
        at(0.0, 0.0),
        at(0.0, 250.0),
    );
    assert_eq!(runways.rolling(7, &hover, helicopter), None);
    let traffic = traffic_of(&aeroways, (&[hover], &[]), (1.0, 0.0));
    assert!(traffic.rolls.is_empty());
    assert!(traffic.energy[0][0].iter().sum::<f64>() > 0.0);
}

/// At 78 deg N a cell of longitude is 2.3 km: a landing last seen 2.9 km before the threshold of a
/// 2,500 m runway lies two cells from the runway's middle, and still lands on it.
#[test]
fn a_low_end_far_from_the_middle_fits_at_high_latitude() {
    let lat = 78.2f64;
    let degrees_per_metre = 1.0 / (111_320.0 * lat.to_radians().cos());
    let point = |east_m: f64| [lat, 15.405 + east_m * degrees_per_metre];
    let mut aeroways = Aeroways::default();
    let airport = aeroways.intern(Airport {
        key: "ENSB".into(),
        name: String::new(),
    });
    for piece in 0..10 {
        let east = 250.0 * f64::from(piece);
        aeroways.lines.push(AerowayLine {
            osm_id: 1,
            segment: piece,
            ends: [point(east), point(east + 250.0)],
            aeroway: Aeroway::Runway,
            airport,
            square: Square { x: 277, y: 60 },
        });
    }
    let runways = Runways::new(&aeroways);
    let a320 = class("A320-232");
    let landing = LowEnd {
        start: point(-3_000.0),
        end: point(-2_900.0),
        ..low_end(1, false, false, a320, 0.0)
    };
    let found = movements(Vec::new(), &[landing], &runways);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].key.landing && found[0].key.forward);
}

/// A primary departure lost while still low witnesses no landing: the secondary's landing of that
/// flight at its destination stays.
#[test]
fn a_departure_lost_low_leaves_the_destinations_landing() {
    let a320 = class("A320-232");
    let lost_climbing = low_end(3, false, true, a320, 3_000.0);
    let legs = [GroundLeg {
        episode: 1,
        secondary_only: true,
        departure: false,
        ..leg(
            3,
            Mover::Aircraft { class: a320 },
            at(0.0, 400.0),
            at(0.0, 650.0),
        )
    }];
    let traffic = traffic_of(&airport(10), (&legs, &[lost_climbing]), (0.25, 1.0));
    assert_eq!(traffic.movements[0], [1.0, 0.0, 0.0]);
}

/// A stop that lands westwards and takes off eastwards in one stretch on the ground keeps each
/// roll's own direction: the landing's legs slow to the turn, the take-off's speed up from it.
#[test]
fn a_touch_and_go_keeps_each_rolls_direction() {
    let aeroways = airport(10);
    let a320 = class("A320-232");
    let mover = Mover::Aircraft { class: a320 };
    let leg = |start_m: f64, end_m: f64| GroundLeg {
        episode: 1,
        departure: false,
        ..leg(4, mover, at(0.0, start_m), at(0.0, end_m))
    };
    let landing = GroundLeg {
        speed_kt: 120.0,
        ..leg(2_000.0, 1_750.0)
    };
    let slowed = GroundLeg {
        speed_kt: 60.0,
        ..leg(1_000.0, 750.0)
    };
    let take_off = GroundLeg {
        departure: true,
        speed_kt: 100.0,
        ..leg(500.0, 750.0)
    };
    let traffic = traffic_with(
        &aeroways,
        (&[landing, slowed, take_off], &[], &[(4, 1)]),
        (1.0, 0.0),
    );
    let rolls: Vec<(bool, bool)> = traffic
        .rolls
        .keys()
        .map(|k| (k.landing, k.forward))
        .collect();
    assert_eq!(rolls, vec![(true, false), (false, true)]);
}

/// An operation counts at the weight of the provider that saw its own legs: a stop's take-off with
/// no leg of its own borrows nothing from the landing the primary saw, and the climb only the
/// secondary found places it at the increment's weight; a sole landing whose decelerating legs
/// only the secondary saw is the primary's where the primary saw it accelerate to vacate.
#[test]
fn an_operation_counts_at_the_weight_of_the_provider_of_its_own_legs() {
    let aeroways = airport(10);
    let a320 = class("A320-232");
    let mover = Mover::Aircraft { class: a320 };
    let rollout = GroundLeg {
        episode: 1,
        departure: false,
        speed_kt: 120.0,
        ..leg(5, mover, at(0.0, 500.0), at(0.0, 750.0))
    };
    let climb = LowEnd {
        episode: 1,
        secondary_only: true,
        ..low_end(5, true, true, a320, 3_000.0)
    };
    let traffic = traffic_with(
        &aeroways,
        (std::slice::from_ref(&rollout), &[climb], &[(5, 1)]),
        (0.25, 1.0),
    );
    assert_eq!(traffic.movements[0], [0.25, 1.0, 0.0]);
    let vacate = GroundLeg {
        departure: true,
        speed_kt: 45.0,
        ..rollout.clone()
    };
    let unseen_rollout = GroundLeg {
        secondary_only: true,
        ..rollout
    };
    let traffic = traffic_of(&aeroways, (&[unseen_rollout, vacate], &[]), (0.25, 1.0));
    assert_eq!(traffic.movements[0], [0.25, 0.0, 0.0]);
}

/// A stop's operations take the legs flown before and after its slowest: the primary's slower
/// take-off leg keeps the take-off the primary's though the secondary saw a faster one, and the
/// primary's vacate, accelerating before the stop, makes the landing the primary's, not the
/// take-off the secondary rolled after it.
#[test]
fn a_stops_legs_split_at_the_slowest() {
    let aeroways = airport(10);
    let mover = Mover::Aircraft {
        class: class("A320-232"),
    };
    let rolling = |speed_kt: f64, departure: bool, secondary_only: bool| GroundLeg {
        episode: 1,
        departure,
        secondary_only,
        speed_kt,
        ..leg(6, mover, at(0.0, 500.0), at(0.0, 750.0))
    };
    let legs = [
        rolling(120.0, false, true),
        rolling(45.0, false, true),
        rolling(80.0, true, false),
        rolling(120.0, true, true),
    ];
    let traffic = traffic_with(&aeroways, (&legs, &[], &[(6, 1)]), (0.25, 1.0));
    assert_eq!(traffic.movements[0], [1.0, 0.25, 0.0]);
    let legs = [
        rolling(120.0, false, true),
        rolling(30.0, true, false),
        rolling(50.0, true, true),
        rolling(140.0, true, true),
    ];
    let traffic = traffic_with(&aeroways, (&legs, &[], &[(6, 1)]), (0.25, 1.0));
    assert_eq!(traffic.movements[0], [0.25, 1.0, 0.0]);
}

/// A runway across the antimeridian is listed in the cells on both sides: a landing last seen
/// west of 180 deg on its approach from the west lands on it.
#[test]
fn a_runway_across_the_antimeridian_is_found_from_both_sides() {
    let degrees_per_metre = 1.0 / (111_320.0 * 10.0f64.to_radians().cos());
    let point = |east_m: f64| {
        let lon = 179.99 + east_m * degrees_per_metre;
        [10.0, if lon >= 180.0 { lon - 360.0 } else { lon }]
    };
    let mut aeroways = Aeroways::default();
    let airport = aeroways.intern(Airport {
        key: "TEST".into(),
        name: String::new(),
    });
    for piece in 0..10 {
        let east = 250.0 * f64::from(piece);
        aeroways.lines.push(AerowayLine {
            osm_id: 1,
            segment: piece,
            ends: [point(east), point(east + 250.0)],
            aeroway: Aeroway::Runway,
            airport,
            square: Square { x: 511, y: 241 },
        });
    }
    let runways = Runways::new(&aeroways);
    let a320 = class("A320-232");
    let landing = LowEnd {
        start: point(-1_100.0),
        end: point(-1_000.0),
        ..low_end(1, false, false, a320, 0.0)
    };
    let found = movements(Vec::new(), &[landing], &runways);
    assert_eq!(found.len(), 1, "{found:?}");
}

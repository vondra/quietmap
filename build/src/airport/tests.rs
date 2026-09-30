//! Ground legs onto synthetic aeroway lines: a leg's length is conserved when buffers overlap, a
//! day of passes becomes sound power per metre, a flight is one movement per airport and day, and
//! strips missing from the lines are found in the legs off them.

use super::lines::{AerowayLine, Airport};
use super::*;
use crate::aircraft::flat::{M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR, flat_distance_m};
use physics::emission::airport::GroundVehicle;

const LAT: f64 = 50.105;

/// Degrees of latitude and longitude of `north_m`, `east_m` metres at [`LAT`].
fn offset(north_m: f64, east_m: f64) -> [f64; 2] {
    let east_per_degree = f64::from(M_PER_DEG_LON_EQUATOR) * LAT.to_radians().cos();
    [north_m / f64::from(M_PER_DEG_LAT), east_m / east_per_degree]
}

/// The point `north_m`, `east_m` metres from (LAT, 14.25).
fn at(north_m: f64, east_m: f64) -> [f64; 2] {
    let [lat, lon] = offset(north_m, east_m);
    [LAT + lat, 14.25 + lon]
}

fn length_m(start: [f64; 2], end: [f64; 2]) -> f64 {
    f64::from(flat_distance_m(
        start[0] as f32,
        start[1] as f32,
        end[0] as f32,
        end[1] as f32,
    ))
}

fn projected(lines: &[[[f64; 2]; 2]], start: [f64; 2], end: [f64; 2]) -> Vec<(u32, f64)> {
    let mut hits = Vec::new();
    LineIndex::new(lines).project(start, end, &mut hits);
    hits
}

/// An east-west line of 700 m: a leg 30 m beside it runs its whole length on it, 60 m beside it
/// runs off it; a leg crossing it at right angles runs the buffer's 100 m on it; a leg reaching
/// 100 m past its end runs only its inner part.
#[test]
fn a_leg_runs_on_a_line_within_the_buffer_rectangle() {
    let line = [[at(0.0, 0.0), at(0.0, 700.0)]];
    let (start, end) = (at(30.0, 100.0), at(30.0, 500.0));
    let hits = projected(&line, start, end);
    assert_eq!(hits.len(), 1);
    assert!((hits[0].1 - length_m(start, end)).abs() < 0.5, "{hits:?}");
    assert!(projected(&line, at(60.0, 100.0), at(60.0, 500.0)).is_empty());
    let across = projected(&line, at(-100.0, 350.0), at(100.0, 350.0));
    assert!((across[0].1 - 100.0).abs() < 0.5, "{across:?}");
    let past_end = projected(&line, at(0.0, -100.0), at(0.0, 100.0));
    assert!((past_end[0].1 - 100.0).abs() < 0.5, "{past_end:?}");
}

/// dev4's conservation rule: between two parallel taxiways 80 m apart a leg is in both buffers and
/// counts half on each; along three abutting lines it counts once in all; so the lengths always
/// add up to the leg's.
#[test]
fn overlapping_buffers_share_a_leg_and_keep_its_length() {
    let parallel = [
        [at(0.0, 0.0), at(0.0, 700.0)],
        [at(80.0, 0.0), at(80.0, 700.0)],
    ];
    let (start, end) = (at(40.0, 100.0), at(40.0, 500.0));
    let hits = projected(&parallel, start, end);
    let leg = length_m(start, end);
    assert_eq!(hits.len(), 2);
    let total: f64 = hits.iter().map(|hit| hit.1).sum();
    assert!((total - leg).abs() < 1e-6, "{total} {leg}");
    assert!((hits[0].1 - hits[1].1).abs() < 0.5, "{hits:?}");
    let abutting = [
        [at(0.0, 0.0), at(0.0, 200.0)],
        [at(0.0, 200.0), at(0.0, 400.0)],
        [at(0.0, 400.0), at(0.0, 600.0)],
    ];
    let (start, end) = (at(0.0, 0.0), at(0.0, 600.0));
    let hits = projected(&abutting, start, end);
    assert_eq!(hits.len(), 3);
    let total: f64 = hits.iter().map(|hit| hit.1).sum();
    assert!((total - length_m(start, end)).abs() < 1.0, "{total}");
}

#[test]
fn a_line_across_the_antimeridian_is_taken_the_short_way() {
    let line = [[[10.0, 179.999], [10.0, -179.999]]];
    let hits = projected(&line, [10.0, 179.9995], [10.0, -179.9995]);
    assert_eq!(hits.len(), 1);
    assert!((hits[0].1 - 109.6).abs() < 1.0, "{hits:?}");
}

/// A runway of `count` abutting 250 m lines along east from (LAT, 14.25), then a parallel
/// taxiway line 200 m north (the next index), all of airport TEST.
fn airport(count: usize) -> Aeroways {
    let mut aeroways = Aeroways::default();
    let airport = aeroways.intern(Airport {
        key: "TEST".into(),
        name: "Test Airport".into(),
    });
    let line = |ends, operation, segment| AerowayLine {
        osm_id: 7,
        segment,
        ends,
        operation,
        airport,
        square: Square { x: 276, y: 173 },
    };
    for index in 0..count {
        let east = 250.0 * index as f64;
        let ends = [at(0.0, east), at(0.0, east + 250.0)];
        aeroways
            .lines
            .push(line(ends, GroundOperation::RunwayRoll, index as u16));
    }
    let taxiway = [at(200.0, 0.0), at(200.0, 250.0)];
    aeroways
        .lines
        .push(line(taxiway, GroundOperation::Taxi, count as u16));
    aeroways
}

fn leg(flight_id: u64, mover: Mover, start: [f64; 2], end: [f64; 2]) -> GroundLeg {
    GroundLeg {
        flight_id,
        mover,
        period: 0,
        departure: true,
        secondary_only: false,
        start,
        end,
        speed_kt: 70.0,
    }
}

fn index_of(aeroways: &Aeroways) -> LineIndex {
    LineIndex::new(&aeroways.lines.iter().map(|l| l.ends).collect::<Vec<_>>())
}

/// Ten departures along a whole runway line on one of two baseline days: 5 a day, each leaving its
/// pass energy on every metre, which the day spreads over its 12 hours.
#[test]
fn a_day_of_passes_becomes_sound_power_per_metre() {
    let aeroways = airport(1);
    let index = index_of(&aeroways);
    let b738 = Mover::Aircraft { class: 2 };
    let legs: Vec<GroundLeg> = (0..10)
        .map(|flight| leg(flight, b738, at(0.0, 0.0), at(0.0, 250.0)))
        .collect();
    let mut traffic = Traffic::new(&aeroways);
    traffic.add_day(&aeroways, &index, &legs, (0.5, 0.0));
    let pass = aircraft_pass_energy_db(2, GroundOperation::RunwayRoll, true, 70.0).unwrap();
    let power = traffic.power_db(0);
    for band in 0..BANDS {
        let expected = pass[band] + 10.0 * (5.0f64 / (12.0 * 3_600.0)).log10();
        assert!((power[0][band] - expected).abs() < 0.01, "{band}");
        assert_eq!(power[1][band], f64::NEG_INFINITY);
    }
    assert_eq!(traffic.movements[0], [0.0, 5.0, 0.0]);
    assert!(
        traffic
            .power_db(1)
            .iter()
            .flatten()
            .all(|l| *l == f64::NEG_INFINITY)
    );
}

/// One flight rolling over two runway lines and taxiing is one departure; a flight only the
/// secondary provider saw counts at the increment weight; a vehicle's pass on the taxiway is its
/// W / v and a vehicle movement; an arrival rolls without the departure bonus.
#[test]
fn movements_count_once_per_flight_and_day_at_their_weight() {
    let aeroways = airport(2);
    let index = index_of(&aeroways);
    let b738 = Mover::Aircraft { class: 2 };
    let heavy = Mover::Vehicle(GroundVehicle::Heavy);
    let mut secondary = leg(2, b738, at(0.0, 0.0), at(0.0, 250.0));
    secondary.secondary_only = true;
    let mut arrival = leg(3, b738, at(0.0, 250.0), at(0.0, 500.0));
    arrival.departure = false;
    let legs = [
        leg(1, b738, at(0.0, 0.0), at(0.0, 250.0)),
        leg(1, b738, at(0.0, 250.0), at(0.0, 500.0)),
        leg(1, b738, at(200.0, 0.0), at(200.0, 250.0)),
        secondary,
        arrival,
        leg(4, heavy, at(200.0, 0.0), at(200.0, 250.0)),
    ];
    let mut traffic = Traffic::new(&aeroways);
    traffic.add_day(&aeroways, &index, &legs, (0.25, 0.5));
    assert_eq!(traffic.movements[0], [0.25, 0.25 + 0.5, 0.25]);
    let day_energy = |line: usize| {
        let bands = traffic.energy[line][0];
        bands.iter().sum::<f64>()
    };
    let total = |bands: [f64; BANDS]| bands.iter().map(|level| energy(*level)).sum::<f64>();
    let departure =
        total(aircraft_pass_energy_db(2, GroundOperation::RunwayRoll, true, 70.0).unwrap());
    let landing =
        total(aircraft_pass_energy_db(2, GroundOperation::RunwayRoll, false, 70.0).unwrap());
    let taxi = total(aircraft_pass_energy_db(2, GroundOperation::Taxi, true, 70.0).unwrap());
    let vehicle = total(vehicle_pass_energy_db(GroundVehicle::Heavy, 70.0).unwrap());
    let close = |got: f64, expected: f64| (got / expected - 1.0).abs() < 1e-3;
    assert!(close(day_energy(0), 0.25 * departure + 0.5 * departure));
    assert!(close(day_energy(1), 0.25 * departure + 0.25 * landing));
    assert!(close(day_energy(2), 0.25 * taxi + 0.25 * vehicle));
}

/// On a runway, a flight crossing at 15 kt taxis and is no movement; the start of a take-off roll
/// at 15 kt rolls but counts only with a leg of the roll above 40 kt; a landing at 70 kt arrives.
#[test]
fn slow_legs_on_a_runway_taxi_unless_they_roll_off() {
    let aeroways = airport(1);
    let index = index_of(&aeroways);
    let b738 = Mover::Aircraft { class: 2 };
    let slow = |flight: u64, departure: bool| GroundLeg {
        departure,
        speed_kt: 15.0,
        ..leg(flight, b738, at(0.0, 0.0), at(0.0, 250.0))
    };
    let mut landing = leg(3, b738, at(0.0, 0.0), at(0.0, 250.0));
    landing.departure = false;
    let legs = [slow(1, false), slow(2, true), landing];
    let mut traffic = Traffic::new(&aeroways);
    traffic.add_day(&aeroways, &index, &legs, (1.0, 0.0));
    assert_eq!(traffic.movements[0], [1.0, 0.0, 0.0]);
    let total = |bands: [f64; BANDS]| bands.iter().map(|level| energy(*level)).sum::<f64>();
    let pass = |operation, departure, speed| {
        total(aircraft_pass_energy_db(2, operation, departure, speed).unwrap())
    };
    let expected = pass(GroundOperation::Taxi, false, 15.0)
        + pass(GroundOperation::RunwayRoll, true, 15.0)
        + pass(GroundOperation::RunwayRoll, false, 70.0);
    let got: f64 = traffic.energy[0][0].iter().sum();
    assert!((got / expected - 1.0).abs() < 1e-3, "{got} vs {expected}");
}

#[test]
fn a_day_counts_on_its_lists() {
    let window = Window {
        baseline_days: vec!["2025-09-01".into(), "2025-09-02".into()],
        increment_days: vec!["2025-09-01".into()],
    };
    assert_eq!(day_weights(&window, "2025-09-01"), (0.5, 1.0));
    assert_eq!(day_weights(&window, "2025-09-02"), (0.5, 0.0));
    assert_eq!(day_weights(&window, "2025-09-03"), (0.0, 0.0));
}

#[test]
fn a_square_file_reads_back_as_written() {
    let dir = std::env::temp_dir().join(format!("qm-airport-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let square = Square { x: 276, y: 173 };
    let mut power_db = [[70.25; BANDS]; PERIODS];
    power_db[2] = [f64::NEG_INFINITY; BANDS];
    let lines: Vec<LineTraffic> = [GroundOperation::RunwayRoll, GroundOperation::Taxi]
        .into_iter()
        .enumerate()
        .map(|(index, operation)| LineTraffic {
            osm_id: 123 + index as i64,
            segment: index as u16,
            ends: [at(0.0, 0.0), at(0.0, 250.0)],
            operation,
            airport: Airport {
                key: "LKPR".into(),
                name: "Letiště Václava Havla Praha".into(),
            },
            movements_per_day: [101.5, 99.25, 12.0],
            power_db,
        })
        .collect();
    let metadata = HashMap::from([("baseline_days".to_string(), "2025-09-01".to_string())]);
    file::write(&dir, square, &lines, &metadata).unwrap();
    assert_eq!(file::read(&dir, square).unwrap(), lines);
    assert!(file::read(&dir, Square { x: 1, y: 1 }).unwrap().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Legs of `flights` flights on each of `days` days along an 800 m strip 2 km north of the test
/// airport (off all its lines), in eight 100 m legs at 60 kt, landing westwards.
fn strip_days(flights: u64, days: usize) -> Vec<Vec<GroundLeg>> {
    let c172 = Mover::Aircraft { class: 3 };
    (0..days)
        .map(|day| {
            (0..flights)
                .flat_map(|flight| {
                    (0..8).map(move |step| {
                        let east = 800.0 - 100.0 * step as f64;
                        let mut leg = leg(
                            100 * day as u64 + flight,
                            c172,
                            at(2_000.0, east),
                            at(2_000.0, east - 100.0),
                        );
                        (leg.departure, leg.speed_kt) = (false, 60.0);
                        leg
                    })
                })
                .collect()
        })
        .collect()
}

fn discovered(days: &[Vec<GroundLeg>]) -> Vec<[[f64; 2]; 2]> {
    let aeroways = airport(2);
    let index = index_of(&aeroways);
    let mut discovery = strips::Discovery::default();
    for legs in days {
        discovery.add_day(legs, &index);
    }
    discovery.strips()
}

/// Ten flights on three days find the strip: a line along it spanning its 800 m (the end cells
/// taken whole: within one z19 cell, 49 m here), cut into four runway pieces of an airstrip
/// airport, on which the landings then count as arrivals.
#[test]
fn a_strip_off_every_line_is_found_and_carries_its_landings() {
    let days = strip_days(10, 3);
    let found = discovered(&days);
    assert_eq!(found.len(), 1, "{found:?}");
    let [west, east] = if found[0][0][1] < found[0][1][1] {
        found[0]
    } else {
        [found[0][1], found[0][0]]
    };
    assert!(length_m(west, at(2_000.0, 0.0)) < 50.0, "{west:?}");
    assert!(length_m(east, at(2_000.0, 800.0)) < 50.0, "{east:?}");
    let mut aeroways = airport(2);
    aeroways.add_strips(&found, square_of);
    let pieces: Vec<&AerowayLine> = aeroways.lines.iter().filter(|l| l.osm_id < 0).collect();
    assert_eq!(pieces.len(), 4);
    assert!(
        pieces
            .iter()
            .all(|piece| length_m(piece.ends[0], piece.ends[1]) <= 250.0)
    );
    let strip_airport = &aeroways.airports[pieces[0].airport as usize];
    assert!(
        strip_airport.key.starts_with("airstrip "),
        "{strip_airport:?}"
    );
    let index = index_of(&aeroways);
    let mut traffic = Traffic::new(&aeroways);
    traffic.add_day(&aeroways, &index, &days[0], (1.0, 0.0));
    assert_eq!(
        traffic.movements[pieces[0].airport as usize],
        [10.0, 0.0, 0.0]
    );
}

/// One flight's legs on three days, or ten flights on two days, find nothing; nor do legs on a
/// mapped line.
#[test]
fn a_strip_needs_ten_flights_on_three_days_off_every_line() {
    assert!(discovered(&strip_days(1, 3)).is_empty());
    assert!(discovered(&strip_days(10, 2)).is_empty());
    let b738 = Mover::Aircraft { class: 2 };
    let on_the_runway: Vec<Vec<GroundLeg>> = (0..3)
        .map(|day| {
            (0..10)
                .map(|flight| leg(100 * day + flight, b738, at(0.0, 0.0), at(0.0, 250.0)))
                .collect()
        })
        .collect();
    assert!(discovered(&on_the_runway).is_empty());
}

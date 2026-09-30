//! Take-off and landing rolls the ground receivers missed. ADS-B sees an aircraft on the ground
//! only near a receiver: at Sao Paulo Congonhas about 40 % of the flights seen low over the
//! airport had no ground leg, hence no roll noise, where Prague misses almost none. A flight whose
//! first airborne segment is low and climbing beyond a runway, along it, and which has no take-off
//! roll of its own, gets one from the runway's end behind it; a flight whose last airborne segment
//! is low before or over a runway, along it, and which has no landing roll of its own, gets one
//! from the touchdown point on. A roll has its class's typical length and speed, in four legs at
//! the speeds of an even acceleration or deceleration, and the traffic pass takes its legs as any
//! other (the slow end of a landing roll taxis there).

use super::legs::{GroundLeg, LowEnd, Mover};
use super::lines::Aeroways;
use crate::aircraft::flat::{M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR, signed_longitude_delta};
use physics::doc29::npd::Family;
use physics::doc29::profiles_generated::CLASS_FAMILY;
use physics::emission::airport::GroundOperation;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A low end lies within this distance of a runway's axis (m) and heads within this angle of it.
const MAX_LATERAL_M: f64 = 150.0;
const MAX_ANGLE_DEG: f64 = 20.0;
/// A take-off's first low segment starts at most this far past the runway's end, a landing's
/// last low segment ends at most this far before its threshold (m): 150 m of height at a 3 deg
/// approach is 2.9 km out.
const MAX_OUTSIDE_M: f64 = 3_000.0;
/// Touchdown lies this far past the threshold at least (m): the aiming point.
const TOUCHDOWN_M: f64 = 300.0;
/// Legs per roll, and the speed a landing roll ends at (kt).
pub const LEGS: usize = 4;
const ROLL_END_KT: f64 = 30.0;
/// Runways are listed in cells of this many degrees by their middle.
const CELL_DEG: f64 = 0.1;

/// A class's typical rolls: take-off length (m) and lift-off speed (kt), landing length (m) and
/// touchdown speed (kt); helicopters have none.
fn rolls_of(class: u8) -> Option<[f64; 4]> {
    Some(match *CLASS_FAMILY.get(usize::from(class))? {
        Family::Helicopter => return None,
        Family::Piston => [300.0, 55.0, 250.0, 55.0],
        Family::Turboprop => [1_100.0, 115.0, 900.0, 100.0],
        Family::Business => [1_100.0, 115.0, 900.0, 105.0],
        Family::Regional => [1_700.0, 145.0, 1_300.0, 125.0],
        Family::Widebody => [2_600.0, 165.0, 1_800.0, 140.0],
        Family::Narrowbody => [1_800.0, 150.0, 1_400.0, 130.0],
    })
}

/// One runway: its axis from end to end in a flat frame at the first end.
#[derive(Debug, Clone)]
struct Runway {
    first: [f64; 2],
    metres_per_degree_lon: f64,
    axis: [f64; 2],
    length_m: f64,
}

impl Runway {
    fn new(ends: [[f64; 2]; 2]) -> Option<Self> {
        let metres_per_degree_lon =
            f64::from(M_PER_DEG_LON_EQUATOR) * (0.5 * (ends[0][0] + ends[1][0])).to_radians().cos();
        let mut runway = Runway {
            first: ends[0],
            metres_per_degree_lon,
            axis: [1.0, 0.0],
            length_m: 0.0,
        };
        let [east, north] = runway.local(ends[1]);
        runway.length_m = east.hypot(north);
        runway.axis = [east / runway.length_m, north / runway.length_m];
        (runway.length_m >= 100.0).then_some(runway)
    }

    /// Metres east and north of the first end.
    fn local(&self, point: [f64; 2]) -> [f64; 2] {
        let east = f64::from(signed_longitude_delta(
            self.first[1] as f32,
            point[1] as f32,
        )) * self.metres_per_degree_lon;
        [east, (point[0] - self.first[0]) * f64::from(M_PER_DEG_LAT)]
    }

    /// The point `along_m` from the first end on the axis (latitude, longitude).
    fn at(&self, along_m: f64) -> [f64; 2] {
        [
            self.first[0] + along_m * self.axis[1] / f64::from(M_PER_DEG_LAT),
            self.first[1] + along_m * self.axis[0] / self.metres_per_degree_lon,
        ]
    }

    /// Where a segment lies on the runway: the along position of `point` measured from the end
    /// the segment heads away from, and whether it heads from the first end to the second;
    /// `None` when it is off the axis or across it.
    fn place(&self, point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> Option<(f64, bool)> {
        let [x, y] = self.local(point);
        let along = x * self.axis[0] + y * self.axis[1];
        let lateral = (x * self.axis[1] - y * self.axis[0]).abs();
        let ([a, b], [c, d]) = (self.local(start), self.local(end));
        let (east, north) = (c - a, d - b);
        let length = east.hypot(north);
        if lateral > MAX_LATERAL_M || length <= 0.0 {
            return None;
        }
        let cos = (east * self.axis[0] + north * self.axis[1]) / length;
        if cos.abs() < MAX_ANGLE_DEG.to_radians().cos() {
            return None;
        }
        let forward = cos > 0.0;
        Some((
            if forward {
                along
            } else {
                self.length_m - along
            },
            forward,
        ))
    }
}

/// A runway piece: its segment number and ends.
type Piece = (u16, [[f64; 2]; 2]);

/// The runways of the aeroways (their roll lines by OSM way, pieces joined end to end), listed
/// by cell.
pub struct Runways {
    runways: Vec<Runway>,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

fn cell_of(point: [f64; 2]) -> (i64, i64) {
    (
        (point[0] / CELL_DEG).floor() as i64,
        (point[1] / CELL_DEG).floor() as i64,
    )
}

impl Runways {
    pub fn new(aeroways: &Aeroways) -> Self {
        let mut ways: BTreeMap<i64, Vec<Piece>> = BTreeMap::new();
        for line in &aeroways.lines {
            if line.operation == GroundOperation::RunwayRoll {
                ways.entry(line.osm_id)
                    .or_default()
                    .push((line.segment, line.ends));
            }
        }
        let mut runways = Runways {
            runways: Vec::new(),
            cells: HashMap::new(),
        };
        for pieces in ways.values_mut() {
            pieces.sort_by_key(|(segment, _)| *segment);
            let ends = [pieces[0].1[0], pieces[pieces.len() - 1].1[1]];
            if let Some(runway) = Runway::new(ends) {
                let middle = runway.at(0.5 * runway.length_m);
                runways
                    .cells
                    .entry(cell_of(middle))
                    .or_default()
                    .push(runways.runways.len());
                runways.runways.push(runway);
            }
        }
        runways
    }

    /// The runway a low end fits best (nearest the axis): its place there.
    fn fitting(&self, end: &LowEnd, take_off: bool) -> Option<(&Runway, f64, bool)> {
        let point = if take_off { end.start } else { end.end };
        let (row, column) = cell_of(point);
        let mut best: Option<(&Runway, f64, bool, f64)> = None;
        for dy in -1..=1 {
            for dx in -1..=1 {
                for &index in self
                    .cells
                    .get(&(row + dy, column + dx))
                    .into_iter()
                    .flatten()
                {
                    let runway = &self.runways[index];
                    let Some((along, forward)) = runway.place(point, end.start, end.end) else {
                        continue;
                    };
                    let fits = if take_off {
                        along > 0.3 * runway.length_m && along < runway.length_m + MAX_OUTSIDE_M
                    } else {
                        along > -MAX_OUTSIDE_M && along < runway.length_m - TOUCHDOWN_M
                    };
                    let [x, y] = runway.local(point);
                    let lateral = (x * runway.axis[1] - y * runway.axis[0]).abs();
                    if fits && best.is_none_or(|(_, _, _, nearest)| lateral < nearest) {
                        best = Some((runway, along, forward, lateral));
                    }
                }
            }
        }
        best.map(|(runway, along, forward, _)| (runway, along, forward))
    }
}

/// The legs of a roll on `runway` from `from_m` to `to_m` along it (measured from the end the
/// motion leaves), at the speeds of an even change from `first_kt` to `last_kt`.
fn roll_legs(
    runway: &Runway,
    forward: bool,
    (from_m, to_m): (f64, f64),
    (first_kt, last_kt): (f64, f64),
    template: &GroundLeg,
) -> Vec<GroundLeg> {
    let position = |along: f64| {
        runway.at(if forward {
            along
        } else {
            runway.length_m - along
        })
    };
    (0..LEGS)
        .map(|leg| {
            let (a, b) = (leg as f64 / LEGS as f64, (leg + 1) as f64 / LEGS as f64);
            let middle = 0.5 * (a + b);
            let speed_kt = (first_kt * first_kt
                + (last_kt * last_kt - first_kt * first_kt) * middle)
                .max(0.0)
                .sqrt();
            GroundLeg {
                start: position(from_m + a * (to_m - from_m)),
                end: position(from_m + b * (to_m - from_m)),
                speed_kt,
                ..template.clone()
            }
        })
        .collect()
}

/// The rolls the day's flights miss: for every low end on a runway whose flight has no roll of
/// that kind (a leg of 40 kt or more, take-off or not) among `legs`.
pub fn missing_rolls(legs: &[GroundLeg], low_ends: &[LowEnd], runways: &Runways) -> Vec<GroundLeg> {
    let rolled: HashSet<(u64, bool)> = legs
        .iter()
        .filter(|leg| {
            matches!(leg.mover, Mover::Aircraft { .. }) && leg.speed_kt >= super::ROLL_MIN_KT
        })
        .map(|leg| (leg.flight_id, leg.departure))
        .collect();
    let mut rolls = Vec::new();
    for end in low_ends {
        let take_off = end.first && end.departure;
        let landing = !end.first && !end.departure;
        if !(take_off || landing) || rolled.contains(&(end.flight_id, take_off)) {
            continue;
        }
        let Some([take_off_m, lift_off_kt, landing_m, touchdown_kt]) = rolls_of(end.class) else {
            continue;
        };
        let Some((runway, along, forward)) = runways.fitting(end, take_off) else {
            continue;
        };
        let template = GroundLeg {
            flight_id: end.flight_id,
            mover: Mover::Aircraft { class: end.class },
            period: end.period,
            departure: take_off,
            secondary_only: end.secondary_only,
            start: end.start,
            end: end.end,
            speed_kt: 0.0,
        };
        rolls.extend(if take_off {
            let length = take_off_m.min(along).min(runway.length_m);
            roll_legs(
                runway,
                forward,
                (0.0, length),
                (0.0, lift_off_kt),
                &template,
            )
        } else {
            let touchdown = along.max(TOUCHDOWN_M);
            let stop = (touchdown + landing_m).min(runway.length_m);
            roll_legs(
                runway,
                forward,
                (touchdown, stop),
                (touchdown_kt, ROLL_END_KT),
                &template,
            )
        });
    }
    rolls
}

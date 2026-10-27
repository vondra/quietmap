//! The runways rolls run on ([`super::rolls`]): the aeroway lines' runways joined end to end (a
//! runway drawn as several ways is one), the runway and direction a rolling leg runs along, the
//! runway a flight's low end fits, and where a roll of a noise class runs on its runway (Doc 29's
//! touchdown point, the class's typical lengths and speeds).

use super::legs::GroundLeg;
use super::lines::{Aeroway, Aeroways};
use super::low_ends::LowEnd;
use crate::aircraft::flat::{
    M_PER_DEG_LAT, M_PER_DEG_LON_EQUATOR, flat_distance_m, signed_longitude_delta,
};
use physics::doc29::npd::Family;
use physics::doc29::profiles_generated::CLASS_FAMILY;
use std::collections::{BTreeMap, HashMap};

/// A low end lies within this distance of a runway's axis (m) and heads within this angle of it.
const MAX_LATERAL_M: f64 = 150.0;
const MAX_ANGLE_DEG: f64 = 20.0;
/// A take-off's first low segment starts at most this far past the runway's end, a landing's
/// last low segment ends at most this far before its threshold (m): 150 m of height at a 3 deg
/// approach is 2.9 km out.
const MAX_OUTSIDE_M: f64 = 3_000.0;
/// Touchdown lies this far past the threshold (m): Doc 29 3.6.3, a 3 deg glideslope crossing the
/// threshold at 50 ft; on a strip shorter than four times that, a quarter of the strip in (its
/// light aircraft fly steeper and land short, outside Doc 29's airports).
const TOUCHDOWN_M: f64 = 291.0;

/// Where a landing touches down on a runway of `length_m`.
fn touchdown_m(length_m: f64) -> f64 {
    TOUCHDOWN_M.min(0.25 * length_m)
}
/// Runways are listed in every cell of this many degrees that a low end fitting them can lie in.
const CELL_DEG: f64 = 0.1;
const METRES_PER_SECOND_PER_KNOT: f64 = 1_852.0 / 3_600.0;

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

/// One runway: its axis from end to end in a flat frame at the first end, and its airport.
#[derive(Debug, Clone)]
struct Runway {
    first: [f64; 2],
    metres_per_degree_lon: f64,
    axis: [f64; 2],
    length_m: f64,
    airport: u32,
}

impl Runway {
    fn new(ends: [[f64; 2]; 2], airport: u32) -> Option<Self> {
        let metres_per_degree_lon =
            f64::from(M_PER_DEG_LON_EQUATOR) * (0.5 * (ends[0][0] + ends[1][0])).to_radians().cos();
        let mut runway = Runway {
            first: ends[0],
            metres_per_degree_lon,
            axis: [1.0, 0.0],
            length_m: 0.0,
            airport,
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

    /// The cells a low end fitting the runway can lie in: its ends' box widened by the farthest a
    /// low end may lie from them (a cell of longitude is 3.8 km at 70 deg N).
    fn cells(&self) -> Vec<(i64, i64)> {
        let reach = MAX_OUTSIDE_M + MAX_LATERAL_M;
        let ends = [self.first, self.at(self.length_m)];
        let (lat_reach, lon_reach) = (
            reach / f64::from(M_PER_DEG_LAT),
            reach / self.metres_per_degree_lon,
        );
        let low = cell_of([
            ends[0][0].min(ends[1][0]) - lat_reach,
            ends[0][1].min(ends[1][1]) - lon_reach,
        ]);
        let high = cell_of([
            ends[0][0].max(ends[1][0]) + lat_reach,
            ends[0][1].max(ends[1][1]) + lon_reach,
        ]);
        // An east end past 180 deg is the low columns: the box's columns run unwrapped, then wrap.
        let high_column = low.1 + (high.1 - low.1).rem_euclid((360.0 / CELL_DEG) as i64);
        (low.0..=high.0)
            .flat_map(|row| (low.1..=high_column).map(move |column| (row, wrapped_column(column))))
            .collect()
    }

    /// The cosine of the motion from `start` to `end` against the axis from the first end to the
    /// second; `None` without motion.
    fn heading_cos(&self, start: [f64; 2], end: [f64; 2]) -> Option<f64> {
        let ([a, b], [c, d]) = (self.local(start), self.local(end));
        let length = (c - a).hypot(d - b);
        (length > 0.0).then(|| ((c - a) * self.axis[0] + (d - b) * self.axis[1]) / length)
    }

    /// Where a segment lies on the runway: the along position of `point` measured from the end
    /// the segment heads away from, and whether it heads from the first end to the second;
    /// `None` when it is off the axis or across it.
    fn place(&self, point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> Option<(f64, bool)> {
        let [x, y] = self.local(point);
        let along = x * self.axis[0] + y * self.axis[1];
        let lateral = (x * self.axis[1] - y * self.axis[0]).abs();
        let cos = self.heading_cos(start, end)?;
        if lateral > MAX_LATERAL_M || cos.abs() < MAX_ANGLE_DEG.to_radians().cos() {
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

/// The runways of the aeroways (their runway lines by OSM way, pieces joined end to end), listed
/// by cell and by way.
pub struct Runways {
    runways: Vec<Runway>,
    cells: HashMap<(i64, i64), Vec<u32>>,
    by_way: HashMap<i64, u32>,
}

/// The unit vector east and north from a line's first end to its second; `None` under a metre.
fn flat_axis(ends: [[f64; 2]; 2]) -> Option<[f64; 2]> {
    let metres_per_degree_lon =
        f64::from(M_PER_DEG_LON_EQUATOR) * (0.5 * (ends[0][0] + ends[1][0])).to_radians().cos();
    let east = f64::from(signed_longitude_delta(ends[0][1] as f32, ends[1][1] as f32))
        * metres_per_degree_lon;
    let north = (ends[1][0] - ends[0][0]) * f64::from(M_PER_DEG_LAT);
    let length = east.hypot(north);
    (length >= 1.0).then(|| [east / length, north / length])
}

fn cell_of(point: [f64; 2]) -> (i64, i64) {
    (
        (point[0] / CELL_DEG).floor() as i64,
        wrapped_column((point[1] / CELL_DEG).floor() as i64),
    )
}

/// A cell's column, longitudes past the antimeridian wrapped.
fn wrapped_column(column: i64) -> i64 {
    let columns = (360.0 / CELL_DEG) as i64;
    (column + columns / 2).rem_euclid(columns) - columns / 2
}

/// Which roll: a runway (an index of [`Runways`]), its direction (from its first end to its
/// second), a landing (else a take-off) and the noise class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RollKey {
    pub runway: u32,
    pub forward: bool,
    pub landing: bool,
    pub class: u8,
}

/// Where a roll runs: from its start (the start of roll, the touchdown point) to its end
/// (latitude, longitude), its length and its speed at lift-off or touchdown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollPath {
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub length_m: f64,
    pub speed_ms: f64,
}

impl Runways {
    pub fn new(aeroways: &Aeroways) -> Self {
        let mut pieces_of: BTreeMap<i64, (u32, Vec<Piece>)> = BTreeMap::new();
        for line in &aeroways.lines {
            if line.aeroway == Aeroway::Runway {
                let (_, pieces) = pieces_of
                    .entry(line.osm_id)
                    .or_insert((line.airport, Vec::new()));
                pieces.push((line.segment, line.ends));
            }
        }
        let ways: Vec<(i64, u32, [[f64; 2]; 2])> = (pieces_of.into_iter())
            .map(|(osm_id, (airport, mut pieces))| {
                pieces.sort_by_key(|(segment, _)| *segment);
                (
                    osm_id,
                    airport,
                    [pieces[0].1[0], pieces[pieces.len() - 1].1[1]],
                )
            })
            .collect();
        // A runway drawn as several ways (Prague's 06/24 is two): ways
        // meeting end to end at one node along one line are one runway.
        let mut group: Vec<usize> = (0..ways.len()).collect();
        fn root(group: &mut [usize], mut way: usize) -> usize {
            while group[way] != way {
                group[way] = group[group[way]];
                way = group[way];
            }
            way
        }
        let mut at_node: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (index, (_, _, ends)) in ways.iter().enumerate() {
            for end in ends {
                at_node
                    .entry(end.map(f64::to_bits))
                    .or_default()
                    .push(index);
            }
        }
        for meeting in at_node.values() {
            for (k, &a) in meeting.iter().enumerate() {
                for &b in &meeting[k + 1..] {
                    let [axis_a, axis_b] = [a, b].map(|way| flat_axis(ways[way].2));
                    let along = axis_a.zip(axis_b).is_some_and(|(p, q)| {
                        (p[0] * q[0] + p[1] * q[1]).abs() >= MAX_ANGLE_DEG.to_radians().cos()
                    });
                    if along {
                        let (a, b) = (root(&mut group, a), root(&mut group, b));
                        group[b] = a;
                    }
                }
            }
        }
        let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for way in 0..ways.len() {
            members.entry(root(&mut group, way)).or_default().push(way);
        }
        let mut runways = Runways {
            runways: Vec::new(),
            cells: HashMap::new(),
            by_way: HashMap::new(),
        };
        for (first, joined) in members {
            // The joined ways' two ends farthest apart, in the first way's direction.
            let ends: Vec<[f64; 2]> = joined.iter().flat_map(|&way| ways[way].2).collect();
            let apart = |a: [f64; 2], b: [f64; 2]| {
                flat_distance_m(a[0] as f32, a[1] as f32, b[0] as f32, b[1] as f32)
            };
            let [a, b] = (ends.iter())
                .flat_map(|&a| ends.iter().map(move |&b| [a, b]))
                .max_by(|x, y| apart(x[0], x[1]).total_cmp(&apart(y[0], y[1])))
                .expect("a way has ends");
            let [start, end] = ways[first].2;
            let reversed = Runway::new([a, b], ways[first].1)
                .and_then(|runway| runway.heading_cos(start, end))
                .is_some_and(|cos| cos < 0.0);
            let farthest = if reversed { [b, a] } else { [a, b] };
            if let Some(runway) = Runway::new(farthest, ways[first].1) {
                let index = runways.runways.len() as u32;
                for cell in runway.cells() {
                    runways.cells.entry(cell).or_default().push(index);
                }
                for &way in &joined {
                    runways.by_way.insert(ways[way].0, index);
                }
                runways.runways.push(runway);
            }
        }
        runways
    }

    /// The airport of runway `runway` (an index into [`Aeroways::airports`]).
    pub fn airport(&self, runway: u32) -> u32 {
        self.runways[runway as usize].airport
    }

    /// The runway of OSM way `osm_id` a flight of class `class` rolls along on `leg`, and whether it
    /// heads from the runway's first end; `None` when the leg crosses the runway rather than runs
    /// along it, or for a class that does not roll (a helicopter's leg taxis).
    pub fn rolling(&self, osm_id: i64, leg: &GroundLeg, class: u8) -> Option<(u32, bool)> {
        rolls_of(class)?;
        let runway = *self.by_way.get(&osm_id)?;
        let cos = self.runways[runway as usize].heading_cos(leg.start, leg.end)?;
        (cos.abs() >= MAX_ANGLE_DEG.to_radians().cos()).then_some((runway, cos > 0.0))
    }

    /// The runway a low end fits best (nearest the axis) and its direction there: a take-off first
    /// seen airborne over the runway past the end it left or beyond it, a landing last seen on the
    /// approach or over the runway with room to touch down ahead; a class that rolls.
    pub(super) fn fitting(&self, end: &LowEnd, take_off: bool) -> Option<(u32, bool)> {
        let point = end.runway_side();
        rolls_of(end.class)?;
        let mut best: Option<(u32, bool, f64)> = None;
        for &index in self.cells.get(&cell_of(point)).into_iter().flatten() {
            let runway = &self.runways[index as usize];
            let Some((along, forward)) = runway.place(point, end.start, end.end) else {
                continue;
            };
            let fits = if take_off {
                along > 0.0 && along < runway.length_m + MAX_OUTSIDE_M
            } else {
                along > -MAX_OUTSIDE_M && along < runway.length_m - touchdown_m(runway.length_m)
            };
            let [x, y] = runway.local(point);
            let lateral = (x * runway.axis[1] - y * runway.axis[0]).abs();
            if fits && best.is_none_or(|(_, _, nearest)| lateral < nearest) {
                best = Some((index, forward, lateral));
            }
        }
        best.map(|(index, forward, _)| (index, forward))
    }

    /// Where the rolls of `key` run: a take-off from the end it leaves over its class's take-off
    /// length, a landing from touchdown over its class's landing length, both within the runway;
    /// `None` for a helicopter or a runway too short to land on.
    pub fn path(&self, key: RollKey) -> Option<RollPath> {
        let runway = &self.runways[key.runway as usize];
        let [take_off_m, lift_off_kt, landing_m, touchdown_kt] = rolls_of(key.class)?;
        let (from, to, speed_kt) = if key.landing {
            let touchdown = touchdown_m(runway.length_m);
            let stop = (touchdown + landing_m).min(runway.length_m);
            (touchdown, stop, touchdown_kt)
        } else {
            (0.0, take_off_m.min(runway.length_m), lift_off_kt)
        };
        let position = |along: f64| {
            runway.at(if key.forward {
                along
            } else {
                runway.length_m - along
            })
        };
        (to > from).then(|| RollPath {
            start: position(from),
            end: position(to),
            length_m: to - from,
            speed_ms: speed_kt * METRES_PER_SECOND_PER_KNOT,
        })
    }
}

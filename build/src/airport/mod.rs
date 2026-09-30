//! Airport ground operations (dev4 Stage 2C `airport_traffic`): the ground legs of a window's
//! Stage 1 day files ([`legs`]) projected onto the aeroway lines ([`lines`], [`project`]), each
//! leg's pass energy per metre (`physics::emission::airport`) summed per line and period as an
//! average day of the window, weighted as the boxes weigh the days, and written per square
//! ([`file`]) for the sources builder. One pass over the days serves every square of the scope.

pub mod file;
pub mod legs;
pub mod lines;
pub mod project;
pub mod strips;

use crate::boxes::Window;
use crate::dev4::{Dev4, Square};
use file::LineTraffic;
use legs::{GroundLeg, Mover, read_ground_legs};
use lines::Aeroways;
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS, energy};
use physics::emission::airport::{
    GroundOperation, aircraft_pass_energy_db, vehicle_pass_energy_db,
};
use project::LineIndex;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use tiles::COMPLETION_MARKER;
use tiles::geo::{Mercator, TileId};

/// Legs projected at once: the hits of one chunk are held together.
const CHUNK_LEGS: usize = 1 << 20;
/// An airport's movement categories: aircraft arriving or departing on its runways and ground
/// vehicles; the same bits shifted up mark movements only the secondary provider saw.
const ARRIVAL: u8 = 1;
const DEPARTURE: u8 = 2;
const VEHICLE: u8 = 4;
const SECONDARY_SHIFT: u8 = 3;
/// A leg on a runway that is not a take-off roll and slower than this taxis: backtracking,
/// crossing the runway or turning off at the end of a landing roll, at idle thrust (taxi levels).
/// dev4 took every leg on a runway as a roll, 15 dB above taxiing at 15 kt, and counted a flight
/// that crossed a runway as an arrival; a movement now needs a leg this fast on the runway.
const ROLL_MIN_KT: f64 = 40.0;

/// What a leg does on a line: a runway's slow legs taxi unless they are a take-off roll.
fn operation_of_leg(line: GroundOperation, leg: &GroundLeg) -> GroundOperation {
    match line {
        GroundOperation::RunwayRoll if !leg.departure && leg.speed_kt < ROLL_MIN_KT => {
            GroundOperation::Taxi
        }
        operation => operation,
    }
}

/// What a window gathers: per line, period and band the sound energy per metre of an average day
/// (linear, pW s per metre), and per airport its arrivals, departures and ground vehicles of an
/// average day.
pub struct Traffic {
    pub energy: Vec<[[f64; BANDS]; PERIODS]>,
    pub movements: Vec<[f64; 3]>,
}

/// One leg on one line: the line, its weighted energy per metre per band, its movement bits.
struct Hit {
    line: u32,
    energy: [f64; BANDS],
    movement: u8,
}

/// The lines a leg runs on with its energy on each (none when it has no weight or does not move).
fn leg_hits(
    leg: &GroundLeg,
    aeroways: &Aeroways,
    index: &LineIndex,
    weight: f64,
    scratch: &mut Vec<(u32, f64)>,
) -> Vec<Hit> {
    if weight <= 0.0 || usize::from(leg.period) >= PERIODS {
        return Vec::new();
    }
    index.project(leg.start, leg.end, scratch);
    let mut hits = Vec::with_capacity(scratch.len());
    for &(line, overlap_m) in scratch.iter() {
        let operation = operation_of_leg(aeroways.lines[line as usize].operation, leg);
        let (pass, movement) = match leg.mover {
            Mover::Aircraft { class } => (
                aircraft_pass_energy_db(class, operation, leg.departure, leg.speed_kt),
                match (operation, leg.departure) {
                    (GroundOperation::RunwayRoll, _) if leg.speed_kt < ROLL_MIN_KT => 0,
                    (GroundOperation::RunwayRoll, true) => DEPARTURE,
                    (GroundOperation::RunwayRoll, false) => ARRIVAL,
                    (GroundOperation::Taxi, _) => 0,
                },
            ),
            Mover::Vehicle(vehicle) => (vehicle_pass_energy_db(vehicle, leg.speed_kt), VEHICLE),
        };
        let Some(pass) = pass else { continue };
        // The leg's share of the line: a pass along all of it leaves its energy on every metre.
        let share = weight * overlap_m / index.length_m(line);
        hits.push(Hit {
            line,
            energy: pass.map(|level| share * energy(level)),
            movement: if leg.secondary_only {
                movement << SECONDARY_SHIFT
            } else {
                movement
            },
        });
    }
    hits
}

impl Traffic {
    pub fn new(aeroways: &Aeroways) -> Self {
        Traffic {
            energy: vec![[[0.0; BANDS]; PERIODS]; aeroways.lines.len()],
            movements: vec![[0.0; 3]; aeroways.airports.len()],
        }
    }

    /// Adds one day's legs: primary movements weighted `weight`, the ones only the secondary
    /// provider saw `secondary_weight`. A movement is one flight (or vehicle) of the day.
    pub fn add_day(
        &mut self,
        aeroways: &Aeroways,
        index: &LineIndex,
        legs: &[GroundLeg],
        (weight, secondary_weight): (f64, f64),
    ) {
        let mut movements: HashMap<(u32, u64), u8> = HashMap::new();
        for chunk in legs.chunks(CHUNK_LEGS) {
            let hits: Vec<Vec<Hit>> = chunk
                .par_iter()
                .map_init(Vec::new, |scratch, leg| {
                    let leg_weight = if leg.secondary_only {
                        secondary_weight
                    } else {
                        weight
                    };
                    leg_hits(leg, aeroways, index, leg_weight, scratch)
                })
                .collect();
            for (leg, hits) in chunk.iter().zip(hits) {
                for hit in hits {
                    let line = &mut self.energy[hit.line as usize][usize::from(leg.period)];
                    for (total, value) in line.iter_mut().zip(hit.energy) {
                        *total += value;
                    }
                    if hit.movement != 0 {
                        let airport = aeroways.lines[hit.line as usize].airport;
                        *movements.entry((airport, leg.flight_id)).or_default() |= hit.movement;
                    }
                }
            }
        }
        for ((airport, _), bits) in movements {
            for (category, bit) in [ARRIVAL, DEPARTURE, VEHICLE].into_iter().enumerate() {
                self.movements[airport as usize][category] += if bits & bit != 0 {
                    weight
                } else if bits & (bit << SECONDARY_SHIFT) != 0 {
                    secondary_weight
                } else {
                    0.0
                };
            }
        }
    }

    /// A line's sound power per metre (dB re 1 pW/m) per period and band; `-inf` is silence.
    pub fn power_db(&self, line: usize) -> [[f64; BANDS]; PERIODS] {
        std::array::from_fn(|period| {
            self.energy[line][period].map(|value| {
                let power = value / (PERIOD_HOURS[period] * 3_600.0);
                if power > 0.0 {
                    10.0 * power.log10()
                } else {
                    f64::NEG_INFINITY
                }
            })
        })
    }
}

/// A day's weights as the boxes take them: primary movements count 1/baseline on baseline days,
/// the ones only the secondary provider saw 1/increment on increment days.
pub fn day_weights(window: &Window, day: &str) -> (f64, f64) {
    let share = |days: &[String]| {
        if days.iter().any(|listed| listed == day) {
            1.0 / days.len() as f64
        } else {
            0.0
        }
    };
    (share(&window.baseline_days), share(&window.increment_days))
}

/// The z9 square holding a point (latitude, longitude).
fn square_of(point: [f64; 2]) -> Square {
    let (x, y) = TileId::containing(Mercator::from_degrees(point[0], point[1])).z9();
    Square { x, y }
}

/// The traffic pass: the lines of `squares` and their neighbours (a leg is shared among every line
/// it runs on, a neighbour's too), every day of the window read once from `segments_dir`, one
/// file per square with traffic under `out` (a fresh directory) and the completion marker last.
/// Returns the number of files written.
pub fn build(
    dev4: &Dev4,
    segments_dir: &Path,
    window: &Window,
    squares: &[Square],
    out: &Path,
) -> Result<usize, String> {
    if out.join(COMPLETION_MARKER).exists() {
        return Err(format!("{} holds a complete traffic pass", out.display()));
    }
    let mut loaded: Vec<Square> = squares.iter().flat_map(|s| s.with_neighbours()).collect();
    loaded.sort();
    loaded.dedup();
    let mut aeroways = Aeroways::read(dev4, &loaded)?;
    let index_of = |aeroways: &Aeroways| {
        LineIndex::new(&aeroways.lines.iter().map(|l| l.ends).collect::<Vec<_>>())
    };
    let osm_index = index_of(&aeroways);
    eprintln!(
        "airport traffic: {} lines of {} airports in {} squares",
        aeroways.lines.len(),
        aeroways.airports.len(),
        loaded.len()
    );
    let within: HashSet<Square> = loaded.iter().copied().collect();
    let keep = |start: [f64; 2], end: [f64; 2]| {
        within.contains(&square_of(start)) || within.contains(&square_of(end))
    };
    let mut days: Vec<&String> = window
        .baseline_days
        .iter()
        .chain(&window.increment_days)
        .collect();
    days.sort();
    days.dedup();
    // First pass: the airstrips missing from OSM.
    let read_day = |day: &str| {
        let path = segments_dir.join("segments").join(format!("{day}.arrow"));
        read_ground_legs(&path, &keep)
    };
    let mut discovery = strips::Discovery::default();
    for day in &days {
        discovery.add_day(&read_day(day)?, &osm_index);
    }
    let found = discovery.strips();
    aeroways.add_strips(&found, square_of);
    let index = index_of(&aeroways);
    eprintln!(
        "airport traffic: {} airstrips missing from OSM",
        found.len()
    );
    let mut traffic = Traffic::new(&aeroways);
    for day in &days {
        let started = std::time::Instant::now();
        let legs = read_day(day)?;
        traffic.add_day(&aeroways, &index, &legs, day_weights(window, day));
        eprintln!(
            "airport traffic: {day}: {} ground legs in {:.1} s",
            legs.len(),
            started.elapsed().as_secs_f64()
        );
    }
    let mut by_square: BTreeMap<Square, Vec<LineTraffic>> = BTreeMap::new();
    for (line_index, line) in aeroways.lines.iter().enumerate() {
        let power_db = traffic.power_db(line_index);
        if power_db
            .iter()
            .flatten()
            .all(|level| *level == f64::NEG_INFINITY)
        {
            continue;
        }
        let airport = line.airport as usize;
        by_square.entry(line.square).or_default().push(LineTraffic {
            osm_id: line.osm_id,
            segment: line.segment,
            ends: line.ends,
            operation: line.operation,
            airport: aeroways.airports[airport].clone(),
            movements_per_day: traffic.movements[airport],
            power_db,
        });
    }
    let metadata = HashMap::from([
        ("baseline_days".into(), window.baseline_days.join(",")),
        ("increment_days".into(), window.increment_days.join(",")),
    ]);
    for (square, lines) in &by_square {
        file::write(out, *square, lines, &metadata)?;
    }
    let note = format!(
        "airport traffic of {} squares from {} baseline and {} increment days",
        squares.len(),
        window.baseline_days.len(),
        window.increment_days.len()
    );
    crate::output::mark_complete(out, &note)?;
    Ok(by_square.len())
}

#[cfg(test)]
mod tests;

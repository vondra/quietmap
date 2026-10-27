//! Airport ground operations (dev4 Stage 2C `airport_traffic`): the ground legs of a window's
//! Stage 1 day files ([`legs`]) projected onto the aeroway lines ([`lines`], [`project`]). A leg
//! rolling on a runway is its flight's take-off or landing roll ([`rolls`]), heard as Doc 29
//! segments from the `aircraft-rolls` tiles ([`roll_tiles`]); every other leg taxis or drives, its
//! pass energy per metre (`physics::emission::airport`) summed per line and period as an average
//! day of the window, weighted as the boxes weigh the days, and written per square ([`file`]) for
//! the sources builder. One pass over the days serves every square of the scope.

pub mod file;
pub mod legs;
pub mod lines;
pub mod low_ends;
pub mod project;
pub mod roll_tiles;
pub mod rolls;
pub mod runways;
pub mod strips;

use crate::boxes::Window;
use crate::dev4::{Dev4, Square};
use file::LineTraffic;
use legs::{DayLegs, GroundLeg, Mover, read_ground_legs};
use lines::{Aeroway, Aeroways};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS, energy};
use physics::emission::airport::{aircraft_taxi_energy_db, vehicle_pass_energy_db};
use project::LineIndex;
use rayon::prelude::*;
use rolls::Stretch;
use runways::{RollKey, Runways};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use tiles::COMPLETION_MARKER;
use tiles::geo::{Mercator, TileId};

/// Legs projected at once: the hits of one chunk are held together.
const CHUNK_LEGS: usize = 1 << 20;
/// A leg on a runway that is not a take-off roll and slower than this taxis: backtracking,
/// crossing the runway or turning off at the end of a landing roll, at idle thrust (taxi levels).
/// dev4 took every leg on a runway as a roll, 15 dB above taxiing at 15 kt, and counted a flight
/// that crossed a runway as an arrival; a roll now needs a leg this fast on the runway.
pub const ROLL_MIN_KT: f64 = 40.0;

/// Whether an aircraft leg rolls: a take-off leg or one this fast (on a runway).
fn rolls(leg: &GroundLeg) -> bool {
    leg.departure || leg.speed_kt >= ROLL_MIN_KT
}

/// What a window gathers: per line, period and band the sound energy per metre of an average day
/// (linear, pW s per metre) of its taxiing aircraft and ground vehicles; per airport its arrivals,
/// departures and ground vehicles of an average day; per roll its movements of an average day.
pub struct Traffic {
    pub energy: Vec<[[f64; BANDS]; PERIODS]>,
    pub movements: Vec<[f64; 3]>,
    pub rolls: BTreeMap<RollKey, [f64; PERIODS]>,
}

/// One leg on one line: the line and its weighted energy per metre per band.
struct Hit {
    line: u32,
    energy: [f64; BANDS],
}

/// What a leg leaves: its roll (an aircraft rolling along a runway: the runway, whether it heads
/// from the runway's first end, its class) or its energy on every line it runs on (none when it
/// has no weight or does not move).
enum LegHits {
    Roll(u32, bool, u8),
    Lines(Vec<Hit>),
}

fn leg_hits(
    leg: &GroundLeg,
    (aeroways, index, runways): (&Aeroways, &LineIndex, &Runways),
    weight: f64,
    scratch: &mut Vec<(u32, f64)>,
) -> LegHits {
    if weight <= 0.0 || usize::from(leg.period) >= PERIODS {
        return LegHits::Lines(Vec::new());
    }
    index.project(leg.start, leg.end, scratch);
    // The runway the leg runs along the most.
    if let Mover::Aircraft { class } = leg.mover
        && rolls(leg)
        && let Some(((runway, forward), _)) = (scratch.iter())
            .filter_map(|&(line, overlap_m)| {
                let line = &aeroways.lines[line as usize];
                (line.aeroway == Aeroway::Runway)
                    .then(|| runways.rolling(line.osm_id, leg, class))
                    .flatten()
                    .map(|place| (place, overlap_m))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
    {
        return LegHits::Roll(runway, forward, class);
    }
    let pass = match leg.mover {
        Mover::Aircraft { class } => aircraft_taxi_energy_db(class, leg.speed_kt),
        Mover::Vehicle(vehicle) => vehicle_pass_energy_db(vehicle, leg.speed_kt),
    };
    let Some(pass) = pass else {
        return LegHits::Lines(Vec::new());
    };
    LegHits::Lines(
        scratch
            .iter()
            .map(|&(line, overlap_m)| {
                // The leg's share of the line: a pass along all of it leaves its energy on every
                // metre.
                let share = weight * overlap_m / index.length_m(line);
                Hit {
                    line,
                    energy: pass.map(|level| share * energy(level)),
                }
            })
            .collect(),
    )
}

impl Traffic {
    pub fn new(aeroways: &Aeroways) -> Self {
        Traffic {
            energy: vec![[[0.0; BANDS]; PERIODS]; aeroways.lines.len()],
            movements: vec![[0.0; 3]; aeroways.airports.len()],
            rolls: BTreeMap::new(),
        }
    }

    /// Adds one day's legs and the low ends of its flights: primary movements weighted `weight`,
    /// the ones only the secondary provider saw `secondary_weight`. A movement is one flight's
    /// take-off or landing roll ([`rolls`]), or one vehicle at an airport, a day.
    pub fn add_day(
        &mut self,
        ways: (&Aeroways, &LineIndex, &Runways),
        day: &DayLegs,
        (weight, secondary_weight): (f64, f64),
    ) {
        let DayLegs {
            legs,
            low_ends,
            lift_offs,
        } = day;
        let (aeroways, _, runways) = ways;
        let weight_of = |secondary_only: bool| {
            if secondary_only {
                secondary_weight
            } else {
                weight
            }
        };
        let mut stretches: HashMap<(u64, u16), Stretch> = HashMap::new();
        let mut vehicles: HashMap<(u32, u64), bool> = HashMap::new();
        for chunk in legs.chunks(CHUNK_LEGS) {
            let hits: Vec<LegHits> = chunk
                .par_iter()
                .map_init(Vec::new, |scratch, leg| {
                    leg_hits(leg, ways, weight_of(leg.secondary_only), scratch)
                })
                .collect();
            for (leg, hits) in chunk.iter().zip(hits) {
                match hits {
                    LegHits::Roll(runway, forward, class) => {
                        let stretch = stretches.entry((leg.flight_id, leg.episode)).or_default();
                        stretch.add(leg, runway, forward, class);
                    }
                    LegHits::Lines(hits) => {
                        for hit in hits {
                            let line = &mut self.energy[hit.line as usize][usize::from(leg.period)];
                            for (total, value) in line.iter_mut().zip(hit.energy) {
                                *total += value;
                            }
                            if matches!(leg.mover, Mover::Vehicle(_)) {
                                let airport = aeroways.lines[hit.line as usize].airport;
                                let primary = vehicles.entry((airport, leg.flight_id)).or_default();
                                *primary |= !leg.secondary_only;
                            }
                        }
                    }
                }
            }
        }
        let observed = rolls::observed(stretches, lift_offs);
        for movement in rolls::movements(observed, low_ends, runways) {
            let counted = weight_of(movement.secondary_only);
            if counted <= 0.0 || usize::from(movement.period) >= PERIODS {
                continue;
            }
            self.rolls.entry(movement.key).or_default()[usize::from(movement.period)] += counted;
            let airport = runways.airport(movement.key.runway) as usize;
            self.movements[airport][usize::from(!movement.key.landing)] += counted;
        }
        for ((airport, _), primary) in vehicles {
            self.movements[airport as usize][2] += weight_of(!primary);
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
    Square::of(TileId::containing(Mercator::from_degrees(
        point[0], point[1],
    )))
}

/// The traffic pass: the lines of `squares` and their neighbours (a leg is shared among every line
/// it runs on, a neighbour's too), every day of the window read once from `segments_dir`, one
/// file per square with traffic under `out` (a fresh directory), the rolls as `aircraft-rolls`
/// square files under `out/rolls` on the runways' elevations in `terrain_root` (a prepared year
/// root), and the completion marker last. Returns the number of line files written.
pub fn build(
    dev4: &Dev4,
    segments_dir: &Path,
    window: &Window,
    squares: &[Square],
    (terrain_root, out): (&Path, &Path),
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
        discovery.add_day(&read_day(day)?.legs, &osm_index);
    }
    let found = discovery.strips();
    aeroways.add_strips(&found, square_of);
    let index = index_of(&aeroways);
    let runways = Runways::new(&aeroways);
    eprintln!(
        "airport traffic: {} airstrips missing from OSM",
        found.len()
    );
    let mut traffic = Traffic::new(&aeroways);
    for day in &days {
        let started = std::time::Instant::now();
        let legs = read_day(day)?;
        traffic.add_day(
            (&aeroways, &index, &runways),
            &legs,
            day_weights(window, day),
        );
        eprintln!(
            "airport traffic: {day}: {} ground legs, {} low ends, in {:.1} s",
            legs.legs.len(),
            legs.low_ends.len(),
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
            aeroway: line.aeroway,
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
    let rolls_written =
        roll_tiles::write(&traffic.rolls, &runways, terrain_root, &out.join("rolls"))?;
    let note = format!(
        "airport traffic of {} squares from {} baseline and {} increment days: {} line files, {} \
         rolls",
        squares.len(),
        window.baseline_days.len(),
        window.increment_days.len(),
        by_square.len(),
        rolls_written
    );
    let marker = out.join(COMPLETION_MARKER);
    std::fs::write(&marker, note).map_err(|error| format!("{}: {error}", marker.display()))?;
    Ok(by_square.len())
}

#[cfg(test)]
mod tests;

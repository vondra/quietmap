//! The aircraft events table (PLAN section 6, B): at a receiver every flight counts once, at its
//! loudest moment (Doc 29 Eq. 4-8a, outdoors in the open); per band of that maximum level the
//! flights of an average day by the period of the moment, their mean height above the ground
//! there and the type flying most of them, and the helicopters of the lowest band. The weights are
//! the boxes': a flight whose primary segments pass a band counts on baseline days, one that
//! passes it only through what the secondary provider saw counts on increment days (the energy's
//! P/B + S/I estimator, a count in place of the energy).
//!
//! The builder ([`build_square`]) counts at every z16 cell of a square (`aircraft-events` tiles),
//! the checker at its points; both take every segment of a flight whose reach (its own LAmax
//! curve, the largest Delta_I) meets the receiver, so a louder power row along the track beats the
//! foot of the nearest segment as it does in the air.

use super::read::{FLAG_SECONDARY_ONLY, FlightSegment};
use super::shuffle::{self, DayRoles};
use super::{emission_of, place_atmosphere};
use crate::dev4::Square;
use crate::output::write_tile;
use physics::bands::PERIODS;
use physics::doc29::atmosphere::PlaceAtmosphere;
use physics::doc29::corrections::INSTALLATION_CORRECTION_MAX_DB;
use physics::doc29::segment::{SegmentEmission, closest_points, lmax_reach_m, receiver_lmax_db};
use physics::weather::WeatherTable;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
pub use tiles::aircraft_events::{BANDS, EVENT_BANDS_DB};
use tiles::aircraft_events::{BandCell, CELLS_PER_SIDE, EventCell, encode};
use tiles::geo::{LocalFrame, Mercator, TILES_PER_AXIS, TileId};
use tiles::terrain::Terrain;

/// A flight's moment at a receiver: its maximum level, the period it falls in and the aircraft's
/// height above the ground under the receiver (m; negative below it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    pub lmax_db: f64,
    pub period: u8,
    pub height_m: f64,
}

/// A flight's loudest moments of one day at one receiver: over its primary segments and over all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightPeaks {
    pub designator: [u8; 4],
    pub helicopter: bool,
    primary: Option<Peak>,
    any: Option<Peak>,
}

fn louder(a: Option<Peak>, b: Option<Peak>) -> Option<Peak> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.lmax_db > a.lmax_db { b } else { a }),
        (a, b) => a.or(b),
    }
}

impl FlightPeaks {
    pub fn new(designator: [u8; 4], helicopter: bool) -> Self {
        FlightPeaks {
            designator,
            helicopter,
            primary: None,
            any: None,
        }
    }

    /// Adds a segment's moment; `secondary` when only the secondary provider saw the segment.
    pub fn add(&mut self, peak: Peak, secondary: bool) {
        if !secondary {
            self.primary = louder(self.primary, Some(peak));
        }
        self.any = louder(self.any, Some(peak));
    }

    pub fn merge(&mut self, other: &FlightPeaks) {
        self.primary = louder(self.primary, other.primary);
        self.any = louder(self.any, other.any);
    }

    /// The moment that counts the flight at or above `threshold_db` and its weight: the primary
    /// segments' at the day's weight for them, else all segments' at the secondary's weight;
    /// none where that weight is 0 (the day lacks the role).
    fn counted(&self, threshold_db: f64, (primary, secondary): (f64, f64)) -> Option<(Peak, f64)> {
        let passes = |peak: Option<Peak>| peak.filter(|peak| peak.lmax_db >= threshold_db);
        match passes(self.primary) {
            Some(peak) => Some((peak, primary)),
            None => passes(self.any).map(|peak| (peak, secondary)),
        }
        .filter(|(_, weight)| *weight > 0.0)
    }
}

/// The table at one receiver, summed over days.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventCounts {
    /// Per band, flights an average day by period, and their weights times height (m).
    pub per_day: [[f64; PERIODS]; BANDS],
    pub height_sum: [f64; BANDS],
    /// Per band, each type's flights an average day (a few types a cell: a list).
    pub designators: [Vec<([u8; 4], f64)>; BANDS],
    /// Helicopters an average day in the lowest band.
    pub helicopters: f64,
}

/// Adds `weight` to `designator`'s entry of a list.
fn add_type(list: &mut Vec<([u8; 4], f64)>, designator: [u8; 4], weight: f64) {
    match list.iter_mut().find(|(listed, _)| *listed == designator) {
        Some((_, sum)) => *sum += weight,
        None => list.push((designator, weight)),
    }
}

impl EventCounts {
    /// Counts one flight of a day whose primary and secondary segments weigh `weights`.
    pub fn add(&mut self, flight: &FlightPeaks, weights: (f64, f64)) {
        for (band, &threshold) in EVENT_BANDS_DB.iter().enumerate() {
            let Some((peak, weight)) = flight.counted(threshold, weights) else {
                continue;
            };
            self.per_day[band][usize::from(peak.period).min(PERIODS - 1)] += weight;
            self.height_sum[band] += weight * peak.height_m;
            add_type(&mut self.designators[band], flight.designator, weight);
            if band == 0 && flight.helicopter {
                self.helicopters += weight;
            }
        }
    }

    pub fn merge(&mut self, other: EventCounts) {
        for band in 0..BANDS {
            for period in 0..PERIODS {
                self.per_day[band][period] += other.per_day[band][period];
            }
            self.height_sum[band] += other.height_sum[band];
            for &(designator, weight) in &other.designators[band] {
                add_type(&mut self.designators[band], designator, weight);
            }
        }
        self.helicopters += other.helicopters;
    }

    /// Flights an average day in `band`.
    pub fn flights(&self, band: usize) -> f64 {
        self.per_day[band].iter().sum()
    }

    /// The mean height (m) of `band`'s flights, NaN without any.
    pub fn mean_height_m(&self, band: usize) -> f64 {
        self.height_sum[band] / self.flights(band)
    }

    /// The designator flying most of `band`'s flights (equal weights by the designator).
    pub fn top_designator(&self, band: usize) -> Option<[u8; 4]> {
        self.designators[band]
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(designator, _)| *designator)
    }

    /// The cell a tile stores: per band the flights a day, those at night, their mean height and
    /// type.
    fn cell(&self) -> EventCell {
        EventCell {
            bands: std::array::from_fn(|band| {
                let per_day = self.flights(band);
                BandCell {
                    per_day: per_day as f32,
                    night_per_day: self.per_day[band][PERIODS - 1] as f32,
                    height_m: if per_day > 0.0 {
                        self.mean_height_m(band)
                            .round()
                            .clamp(f64::from(i16::MIN), f64::from(i16::MAX))
                            as i16
                    } else {
                        0
                    },
                    designator: self.top_designator(band),
                }
            }),
            helicopters_per_day: self.helicopters as f32,
        }
    }
}

/// Aircraft farther than this horizontally reach no cell's lowest band: the loudest row of any
/// class with the most Delta_I reaches 50 dB 18.4 km aside (a 747-400 departure at 5,250 m on the
/// production curves; evidence 2026-10-08, aircraft, review decision 3).
pub const EVENTS_REACH_M: f64 = 20_000.0;
/// The receiver above the ground (m), as the popup's.
const RECEIVER_HEIGHT_M: f64 = 4.0;
/// The most the NPD curves gain at a receiver's elevation (Doc 29 4.2.1): +0.05 dB at the Dead
/// Sea's 430 m below sea level, less everywhere above it. The bounds carry it.
const IMPEDANCE_MARGIN_DB: f64 = 0.1;
/// z16 cells per square side.
const CELLS: usize = 8 * CELLS_PER_SIDE;

/// Mercator x differences across the antimeridian.
fn wrap_x(dx: f64) -> f64 {
    let n = f64::from(TILES_PER_AXIS);
    (dx + n / 2.0).rem_euclid(n) - n / 2.0
}

/// The receivers of a square: each z16 cell's centre 4 m above the terrain, per row the local
/// frame's metres per Mercator unit, and the highest receiver.
struct Receivers {
    west: f64,
    north: f64,
    positions: Vec<Mercator>,
    altitudes_m: Vec<f64>,
    scales: Vec<[f64; 2]>,
    highest_m: f64,
    /// Per block of [`BLOCK`] x [`BLOCK`] cells (row-major), its highest receiver.
    block_tops_m: Vec<f64>,
}

/// Cells per side of a block, the unit a flight's segment is first bounded on.
const BLOCK: usize = 8;
const BLOCKS: usize = CELLS / BLOCK;

impl Receivers {
    fn new(square: Square, terrain_root: &Path) -> Result<Self, String> {
        let (west, north) = (f64::from(square.x * 8), f64::from(square.y * 8));
        let mut terrain_bytes = HashMap::new();
        for index in 0..64 {
            let tile = TileId {
                x: square.x * 8 + index % 8,
                y: square.y * 8 + index / 8,
            };
            let path = tiles::tile_path(terrain_root, tile, tiles::Kind::Terrain);
            if let Ok(bytes) = std::fs::read(path) {
                terrain_bytes.insert(tile, bytes);
            }
        }
        let terrain: HashMap<TileId, Terrain<'_>> = terrain_bytes
            .iter()
            .map(|(tile, bytes)| Terrain::parse(bytes).map(|terrain| (*tile, terrain)))
            .collect::<Result<_, _>>()
            .map_err(|error| error.to_string())?;
        let cell = |index: usize| Mercator {
            x: west + ((index % CELLS) as f64 + 0.5) / CELLS_PER_SIDE as f64,
            y: north + ((index / CELLS) as f64 + 0.5) / CELLS_PER_SIDE as f64,
        };
        let positions: Vec<Mercator> = (0..CELLS * CELLS).map(cell).collect();
        let altitudes_m: Vec<f64> = positions
            .par_iter()
            .map(|&position| {
                let ground = terrain
                    .get(&TileId::containing(position))
                    .and_then(|terrain| terrain.sample(position))
                    .map_or(0.0, |sample| sample.height_m);
                ground + RECEIVER_HEIGHT_M
            })
            .collect();
        let scales = (0..CELLS)
            .map(|row| {
                let frame = LocalFrame::at(positions[row * CELLS + CELLS / 2]);
                [frame.east_m_per_unit, frame.north_m_per_unit]
            })
            .collect();
        let highest_m = altitudes_m
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let block_tops_m = (0..BLOCKS * BLOCKS)
            .map(|block| {
                let (row, column) = (block / BLOCKS * BLOCK, block % BLOCKS * BLOCK);
                (0..BLOCK * BLOCK)
                    .map(|cell| altitudes_m[(row + cell / BLOCK) * CELLS + column + cell % BLOCK])
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .collect();
        Ok(Receivers {
            west,
            north,
            positions,
            altitudes_m,
            scales,
            highest_m,
            block_tops_m,
        })
    }

    /// The fewest metres per Mercator unit east and north within the square (its poleward row).
    fn least_scales(&self) -> [f64; 2] {
        self.scales.iter().fold([f64::INFINITY; 2], |least, scale| {
            [least[0].min(scale[0]), least[1].min(scale[1])]
        })
    }
}

/// The squares within [`EVENTS_REACH_M`] of `square` (more than its neighbours where squares are
/// narrower than the reach, near the poles), by the metres per Mercator unit at its poleward edge.
pub fn squares_within_reach(square: Square) -> Vec<Square> {
    let poleward = if square.y < 256 {
        square.y * 8
    } else {
        square.y * 8 + 8
    };
    let frame = LocalFrame::at(Mercator {
        x: f64::from(square.x * 8) + 4.0,
        y: f64::from(poleward).min(f64::from(TILES_PER_AXIS) - 1e-6),
    });
    let span = |metres_per_unit: f64| (EVENTS_REACH_M / (8.0 * metres_per_unit)).ceil() as i64;
    let (across, along) = (span(frame.east_m_per_unit), span(frame.north_m_per_unit));
    let mut squares = Vec::new();
    for dy in -along..=along {
        let y = i64::from(square.y) + dy;
        if !(0..512).contains(&y) {
            continue;
        }
        for dx in -across.min(255)..=across.min(255) {
            let x = (i64::from(square.x) + dx).rem_euclid(512);
            squares.push(Square {
                x: x as u32,
                y: y as u32,
            });
        }
    }
    squares
}

/// The squares a part's shuffle must hold for the events of `squares`: each with those within
/// reach (review decision 4: a halo by distance, the events written for the part's own squares).
pub fn halo(squares: &[Square]) -> Vec<Square> {
    let mut all: Vec<Square> = squares
        .iter()
        .flat_map(|&square| squares_within_reach(square))
        .collect();
    all.sort();
    all.dedup();
    all
}

/// A flight's loudest moments at the cells it reaches: per cell index, over its primary segments
/// and over all of them (only moments of the lowest band and above).
type CellMoments = HashMap<u32, [Option<Peak>; 2]>;

/// One block's loudest moments of the flight so far, and per provider the quietest of them over
/// its cells (-inf while a cell has none): no segment whose bound stays under it can change the
/// block.
struct BlockMoments {
    cells: [[Option<Peak>; 2]; BLOCK * BLOCK],
    floor_db: [f64; 2],
}

impl BlockMoments {
    fn new() -> Self {
        BlockMoments {
            cells: [[None; 2]; BLOCK * BLOCK],
            floor_db: [f64::NEG_INFINITY; 2],
        }
    }

    fn update_floor(&mut self) {
        self.floor_db = [0, 1].map(|provider| {
            self.cells
                .iter()
                .map(|cell| cell[provider].map_or(f64::NEG_INFINITY, |peak| peak.lmax_db))
                .fold(f64::INFINITY, f64::min)
        });
    }
}

/// A segment of the flight that can reach the lowest band somewhere: its emission, its ends,
/// its lowest altitude, its reach (m) and the most its maximum level can be anywhere (dB).
struct Candidate<'a> {
    segment: &'a FlightSegment,
    emission: SegmentEmission,
    ends: [Mercator; 2],
    lowest_m: f64,
    reach_m: f64,
    bound_db: f64,
}

fn flight_moments(
    segments: &[FlightSegment],
    receivers: &Receivers,
    place: &PlaceAtmosphere,
) -> CellMoments {
    let lowest_band = EVENT_BANDS_DB[0];
    let [least_east, least_north] = receivers.least_scales();
    // The square's box widened by the reach (Mercator units): a segment beyond it reaches none of
    // its cells, before its emission and reach are worked out.
    let margin = [EVENTS_REACH_M / least_east, EVENTS_REACH_M / least_north];
    let side = (CELLS / CELLS_PER_SIDE) as f64;
    let mut candidates: Vec<Candidate> = segments
        .iter()
        .filter_map(|segment| {
            let ends =
                [segment.start, segment.end].map(|end| Mercator::from_degrees(end[0], end[1]));
            let [x0, x1] = ends.map(|end| wrap_x(end.x - receivers.west));
            let [y0, y1] = ends.map(|end| end.y - receivers.north);
            if x0.max(x1) < -margin[0]
                || x0.min(x1) > side + margin[0]
                || y0.max(y1) < -margin[1]
                || y0.min(y1) > side + margin[1]
            {
                return None;
            }
            let (_, emission) = emission_of(segment)?;
            // The receivers' square's atmosphere, as the popup's flight list reads its place's.
            let emission = emission.in_atmosphere(place);
            let reach_slant_m = lmax_reach_m(&emission, lowest_band - IMPEDANCE_MARGIN_DB);
            let lowest_m = segment.start[2].min(segment.end[2]);
            let above_m = (lowest_m - receivers.highest_m).max(0.0);
            if reach_slant_m <= above_m {
                return None;
            }
            let reach_m = (reach_slant_m * reach_slant_m - above_m * above_m)
                .sqrt()
                .min(EVENTS_REACH_M);
            let bound_db = emission.read_npd(above_m.max(1.0)).lamax_db
                + INSTALLATION_CORRECTION_MAX_DB
                + IMPEDANCE_MARGIN_DB;
            Some(Candidate {
                segment,
                emission,
                ends,
                lowest_m,
                reach_m,
                bound_db,
            })
        })
        .collect();
    // The loudest first: their moments let the blocks skip the quieter segments.
    candidates.sort_by(|a, b| b.bound_db.total_cmp(&a.bound_db));
    let mut blocks: HashMap<u32, BlockMoments> = HashMap::new();
    for candidate in &candidates {
        let (segment, emission) = (candidate.segment, &candidate.emission);
        let secondary = segment.flags & FLAG_SECONDARY_ONLY != 0;
        let provider = usize::from(secondary);
        // The blocks within the reach of the segment's box.
        let blocks_of = |from: f64, to: f64, margin: f64| {
            let cells = CELLS_PER_SIDE as f64;
            let low = ((from.min(to) - margin) * cells / BLOCK as f64).floor();
            let high = ((from.max(to) + margin) * cells / BLOCK as f64).floor();
            (low.max(0.0) as usize, high.min(BLOCKS as f64 - 1.0))
        };
        let [start_position, end_position] = candidate.ends;
        let (x0, x1) = (
            wrap_x(start_position.x - receivers.west),
            wrap_x(end_position.x - receivers.west),
        );
        let (y0, y1) = (
            start_position.y - receivers.north,
            end_position.y - receivers.north,
        );
        let (first_column, last_column) = blocks_of(x0, x1, candidate.reach_m / least_east);
        let (first_row, last_row) = blocks_of(y0, y1, candidate.reach_m / least_north);
        if last_column < first_column as f64 || last_row < first_row as f64 {
            continue;
        }
        let local =
            |at: Mercator, scales: [f64; 2], position: Mercator, metres: f64, below: f64| {
                [
                    wrap_x(position.x - at.x) * scales[0],
                    (at.y - position.y) * scales[1],
                    metres - below,
                ]
            };
        for block_row in first_row..=last_row as usize {
            for block_column in first_column..=last_column as usize {
                let block = block_row * BLOCKS + block_column;
                // The block's centre and half diagonal, in its middle row's metres.
                let middle_row = block_row * BLOCK + BLOCK / 2;
                let scales = receivers.scales[middle_row];
                let centre = Mercator {
                    x: receivers.west
                        + (block_column * BLOCK) as f64 / CELLS_PER_SIDE as f64
                        + 0.5 * BLOCK as f64 / CELLS_PER_SIDE as f64,
                    y: receivers.north
                        + (block_row * BLOCK) as f64 / CELLS_PER_SIDE as f64
                        + 0.5 * BLOCK as f64 / CELLS_PER_SIDE as f64,
                };
                let side = BLOCK as f64 / CELLS_PER_SIDE as f64;
                let half_diagonal_m = 0.5 * (side * scales[0]).hypot(side * scales[1]);
                let horizontal_m = horizontal_distance_m(
                    local(centre, scales, start_position, segment.start[2], 0.0),
                    local(centre, scales, end_position, segment.end[2], 0.0),
                ) - half_diagonal_m;
                if horizontal_m > candidate.reach_m {
                    continue;
                }
                let top_m = receivers.block_tops_m[block];
                let nearest_m = horizontal_m
                    .max(0.0)
                    .hypot((candidate.lowest_m - top_m).max(0.0));
                let bound = emission.read_npd(nearest_m).lamax_db
                    + INSTALLATION_CORRECTION_MAX_DB
                    + IMPEDANCE_MARGIN_DB;
                if bound < lowest_band {
                    continue;
                }
                let state = blocks.entry(block as u32).or_insert_with(BlockMoments::new);
                if state.floor_db[provider] >= bound {
                    continue;
                }
                let mut changed = false;
                for (slot, cell) in state.cells.iter_mut().enumerate() {
                    let row = block_row * BLOCK + slot / BLOCK;
                    let column = block_column * BLOCK + slot % BLOCK;
                    let index = row * CELLS + column;
                    let (at, altitude_m) =
                        (receivers.positions[index], receivers.altitudes_m[index]);
                    let row_scales = receivers.scales[row];
                    let (start_m, end_m) = (
                        local(at, row_scales, start_position, segment.start[2], altitude_m),
                        local(at, row_scales, end_position, segment.end[2], altitude_m),
                    );
                    let horizontal_m = horizontal_distance_m(start_m, end_m);
                    if horizontal_m > candidate.reach_m {
                        continue;
                    }
                    // The nearest the segment comes bounds what it can reach here.
                    let nearest_m = horizontal_m.hypot((candidate.lowest_m - altitude_m).max(0.0));
                    let bound = emission.read_npd(nearest_m).lamax_db
                        + INSTALLATION_CORRECTION_MAX_DB
                        + IMPEDANCE_MARGIN_DB;
                    if bound < lowest_band
                        || cell[provider].is_some_and(|peak| peak.lmax_db >= bound)
                    {
                        continue;
                    }
                    let closest = closest_points(start_m, end_m);
                    let lmax_db = receiver_lmax_db(emission, &closest, altitude_m);
                    if lmax_db < lowest_band {
                        continue;
                    }
                    let peak = Some(Peak {
                        lmax_db,
                        period: segment.period,
                        height_m: closest.on_segment_m[2] + RECEIVER_HEIGHT_M,
                    });
                    if !secondary {
                        cell[0] = louder(cell[0], peak);
                    }
                    cell[1] = louder(cell[1], peak);
                    changed = true;
                }
                if changed {
                    state.update_floor();
                }
            }
        }
    }
    let mut moments = CellMoments::new();
    for (block, state) in blocks {
        let (block_row, block_column) = (block as usize / BLOCKS, block as usize % BLOCKS);
        for (slot, cell) in state.cells.iter().enumerate() {
            if cell[1].is_some() {
                let row = block_row * BLOCK + slot / BLOCK;
                let column = block_column * BLOCK + slot % BLOCK;
                moments.insert((row * CELLS + column) as u32, *cell);
            }
        }
    }
    moments
}

/// The horizontal distance (m) from the origin to a segment given in its frame.
pub(crate) fn horizontal_distance_m(start: [f64; 3], end: [f64; 3]) -> f64 {
    let (dx, dy) = (end[0] - start[0], end[1] - start[1]);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 0.0 {
        (-(start[0] * dx + start[1] * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (start[0] + t * dx).hypot(start[1] + t * dy)
}

/// A segment's identity in the shuffle: the same segment written to two squares' files (its
/// flight, ends, period and provider flags).
fn segment_key(segment: &FlightSegment) -> (u64, u8, u8, [u64; 6]) {
    let ends = [
        segment.start[0],
        segment.start[1],
        segment.start[2],
        segment.end[0],
        segment.end[1],
        segment.end[2],
    ];
    (
        segment.flight_id,
        segment.period,
        segment.flags,
        ends.map(f64::to_bits),
    )
}

/// Builds the `aircraft-events` tiles of one z9 square from the day files of the squares within
/// [`EVENTS_REACH_M`] under `shuffled` (`days`: the window's, with their roles) and the terrain of
/// `terrain_root`; writes the tiles with a flight above the lowest band under `out`; returns the
/// number written. An increment day must also be a baseline day: the shuffle writes a day's
/// primary segments only on baseline days, and without them a flight the primary saw would count
/// again through its secondary segments.
pub fn build_square(
    (shuffled, days): (&Path, &[(String, DayRoles)]),
    (square, weather): (Square, &WeatherTable),
    terrain_root: &Path,
    out: &Path,
) -> Result<usize, String> {
    let receivers = Receivers::new(square, terrain_root)?;
    let inputs = squares_within_reach(square);
    let place = place_atmosphere(weather, square);
    let (baseline, increment) = (
        days.iter()
            .filter(|(_, roles)| roles.baseline)
            .count()
            .max(1) as f64,
        days.iter()
            .filter(|(_, roles)| roles.increment)
            .count()
            .max(1) as f64,
    );
    let mut counts = vec![EventCounts::default(); CELLS * CELLS];
    for (day, roles) in days {
        if roles.increment && !roles.baseline {
            return Err(format!("aircraft events: {day} is an increment day only"));
        }
        let mut segments = Vec::new();
        for input in &inputs {
            segments.extend(shuffle::square_day(shuffled, *input, day)?);
        }
        segments.par_sort_unstable_by_key(segment_key);
        segments.dedup_by_key(|segment| segment_key(segment));
        let starts: Vec<usize> = (0..segments.len())
            .filter(|&index| {
                index == 0 || segments[index - 1].flight_id != segments[index].flight_id
            })
            .chain(std::iter::once(segments.len()))
            .collect();
        let flights: Vec<(&[FlightSegment], CellMoments)> = starts
            .par_windows(2)
            .map(|bounds| {
                let flight = &segments[bounds[0]..bounds[1]];
                (flight, flight_moments(flight, &receivers, &place))
            })
            .collect();
        let weights = (
            if roles.baseline { 1.0 / baseline } else { 0.0 },
            if roles.increment {
                1.0 / increment
            } else {
                0.0
            },
        );
        for (flight, moments) in flights {
            let first = &flight[0];
            let helicopter =
                emission_of(first).is_some_and(|(aircraft, _)| aircraft.helicopter.is_some());
            for (index, [primary, any]) in moments {
                let mut peaks = FlightPeaks::new(first.designator, helicopter);
                if let Some(peak) = primary {
                    peaks.add(peak, false);
                }
                if let Some(peak) = any {
                    peaks.add(peak, true);
                }
                counts[index as usize].add(&peaks, weights);
            }
        }
    }
    let mut written = 0;
    for tile_index in 0..64 {
        let (tile_column, tile_row) = (tile_index % 8, tile_index / 8);
        let cells: Vec<EventCell> = (0..CELLS_PER_SIDE * CELLS_PER_SIDE)
            .map(|cell| {
                let row = tile_row * CELLS_PER_SIDE + cell / CELLS_PER_SIDE;
                let column = tile_column * CELLS_PER_SIDE + cell % CELLS_PER_SIDE;
                counts[row * CELLS + column].cell()
            })
            .collect();
        if cells.iter().all(|cell| cell.bands[0].per_day == 0.0) {
            continue;
        }
        let tile = TileId {
            x: square.x * 8 + tile_column as u32,
            y: square.y * 8 + tile_row as u32,
        };
        write_tile(out, tile, tiles::Kind::AircraftEvents, &encode(&cells))?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxes::read::FLAG_DEPARTURE;
    use physics::weather::{COLUMNS, ROWS, WeatherNode, encode as encode_weather};
    use tiles::aircraft_events::AircraftEvents;

    /// The halo is the eight neighbours at mid latitudes and reaches further where squares are
    /// narrower than the reach: 3 x 3 squares around Prague, 5 x 5 at 80 degrees north (13 km wide).
    #[test]
    fn the_halo_follows_the_reach_not_the_neighbours() {
        assert_eq!(squares_within_reach(Square { x: 276, y: 173 }).len(), 9);
        assert_eq!(squares_within_reach(Square { x: 276, y: 57 }).len(), 25);
        let part = [Square { x: 276, y: 173 }, Square { x: 277, y: 173 }];
        assert_eq!(halo(&part).len(), 12);
    }

    /// A departure passing over a cell at 600 m at night and back over it at 900 m by day, its
    /// segments in its own square's file and its neighbour's: the cell counts it once, at the
    /// night pass, in every band; a cell 30 km away hears nothing.
    #[test]
    fn a_flight_counts_once_per_cell_at_its_loudest_pass() {
        let root = std::env::temp_dir().join(format!("qm-events-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let square = Square { x: 276, y: 173 };
        let (column, row) = (4 * CELLS_PER_SIDE + 8, 3 * CELLS_PER_SIDE + 8);
        let cell = Mercator {
            x: f64::from(square.x * 8) + (column as f64 + 0.5) / CELLS_PER_SIDE as f64,
            y: f64::from(square.y * 8) + (row as f64 + 0.5) / CELLS_PER_SIDE as f64,
        };
        let frame = LocalFrame::at(cell);
        let pass = |altitude: f64, period: u8| {
            (0..40).map(move |step| {
                let north = |k: f64| -4_000.0 + 200.0 * k;
                let point = |metres: f64| {
                    let (lat, lon) = frame.to_mercator([0.0, metres]).to_degrees();
                    [lat, lon, altitude]
                };
                FlightSegment {
                    flight_id: 0x4b_0001 << 40 | 1_767_830_400,
                    callsign: *b"CSA200  ",
                    designator: *b"A320",
                    profile: physics::doc29::profiles_generated::profile_idx("A320"),
                    period,
                    flags: FLAG_DEPARTURE,
                    start: point(north(f64::from(step))),
                    end: point(north(f64::from(step) + 1.0)),
                    pressure_altitude_m: [altitude; 2],
                    speed_kt: 200.0,
                    above_ground_m: altitude,
                    departure_field_m: 0.0,
                    ground_m: [0.0; 2],
                    acceleration_ms2: None,
                    ..FlightSegment::default()
                }
            })
        };
        let mut bytes = Vec::new();
        for segment in pass(600.0, 2).chain(pass(900.0, 0)) {
            shuffle::encode(&segment, &mut bytes);
        }
        for listed in [square, Square { x: 277, y: 173 }] {
            let path = root.join(format!("shuffle/{}/{}", listed.x, listed.y));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("2026-01-08.seg"), &bytes).unwrap();
        }
        let weather = WeatherTable::parse(&encode_weather(&vec![
            WeatherNode::default();
            ROWS * COLUMNS
        ]))
        .unwrap();
        let days = [(
            "2026-01-08".to_string(),
            DayRoles {
                baseline: true,
                increment: false,
            },
        )];
        let out = root.join("2026");
        let written = build_square(
            (&root.join("shuffle"), &days),
            (square, &weather),
            &root.join("no-terrain"),
            &out,
        )
        .unwrap();
        assert!(written >= 1);
        let tile = TileId::containing(cell);
        let bytes =
            std::fs::read(tiles::tile_path(&out, tile, tiles::Kind::AircraftEvents)).unwrap();
        let events = AircraftEvents::parse(&bytes).unwrap();
        let heard = events.cell(column % CELLS_PER_SIDE, row % CELLS_PER_SIDE);
        // The night pass overhead: its segment over the cell, 596 m above the receiver.
        let overhead = pass(600.0, 2).nth(20).unwrap();
        let (_, emission) = emission_of(&overhead).unwrap();
        let receiver_m = RECEIVER_HEIGHT_M;
        let local = |end: [f64; 3]| {
            let [east, north] = frame.to_metres(Mercator::from_degrees(end[0], end[1]));
            [east, north, end[2] - RECEIVER_HEIGHT_M]
        };
        let closest = closest_points(local(overhead.start), local(overhead.end));
        let loudest = receiver_lmax_db(&emission, &closest, receiver_m);
        assert!(loudest > 60.0, "{loudest}");
        for (band, threshold) in heard.bands.iter().zip(EVENT_BANDS_DB) {
            let expected = if loudest >= threshold {
                (1.0, 1.0, 600, Some(*b"A320"))
            } else {
                (0.0, 0.0, 0, None)
            };
            assert_eq!(
                (
                    band.per_day,
                    band.night_per_day,
                    band.height_m,
                    band.designator
                ),
                expected,
                "{threshold} dB, the loudest {loudest:.1} dB"
            );
        }
        assert_eq!(heard.helicopters_per_day, 0.0);
        let far = TileId {
            x: square.x * 8 + 7,
            y: tile.y,
        };
        assert!(!tiles::tile_path(&out, far, tiles::Kind::AircraftEvents).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    const B738: [u8; 4] = *b"B738";
    const DAY: (f64, f64) = (1.0 / 354.0, 1.0 / 11.0);

    fn peak(lmax_db: f64, period: u8, height_m: f64) -> Peak {
        Peak {
            lmax_db,
            period,
            height_m,
        }
    }

    /// A flight passing a receiver twice counts once per band, at its louder moment's period and
    /// height.
    #[test]
    fn a_flight_counts_once_at_its_loudest_moment() {
        let mut flight = FlightPeaks::new(B738, false);
        flight.add(peak(63.0, 0, 900.0), false);
        flight.add(peak(71.0, 1, 400.0), false);
        let mut counts = EventCounts::default();
        counts.add(&flight, DAY);
        for band in 0..BANDS {
            assert_eq!(counts.per_day[band], [0.0, 1.0 / 354.0, 0.0]);
            assert!((counts.mean_height_m(band) - 400.0).abs() < 1e-9);
            assert_eq!(counts.top_designator(band), Some(B738));
        }
    }

    /// Primary segments to 55 dB with a secondary gap fill to 65 dB: the primary counts the 50 dB
    /// band at a baseline day's weight, the gap fill the 60 dB band at an increment day's; on a
    /// baseline day that is no increment day the gap fill adds nothing.
    #[test]
    fn the_secondary_adds_only_what_the_primary_misses() {
        let mut flight = FlightPeaks::new(B738, false);
        flight.add(peak(55.0, 0, 1_000.0), false);
        flight.add(peak(65.0, 2, 700.0), true);
        let mut both = EventCounts::default();
        both.add(&flight, DAY);
        assert_eq!(both.per_day[0], [1.0 / 354.0, 0.0, 0.0]);
        assert_eq!(both.per_day[1], [0.0, 0.0, 1.0 / 11.0]);
        assert_eq!(both.flights(2), 0.0);
        let mut baseline_only = EventCounts::default();
        baseline_only.add(&flight, (1.0 / 354.0, 0.0));
        assert_eq!(baseline_only.flights(0), 1.0 / 354.0);
        assert_eq!(baseline_only.flights(1), 0.0);
    }

    /// A flight only the secondary provider saw weighs an increment day's 1/11, and nothing on a
    /// day that is no increment day; a helicopter counts in the lowest band's helicopters only.
    #[test]
    fn a_secondary_flight_counts_on_increment_days() {
        let mut flight = FlightPeaks::new(*b"EC35", true);
        flight.add(peak(66.0, 0, 150.0), true);
        let mut counts = EventCounts::default();
        counts.add(&flight, DAY);
        assert_eq!(counts.flights(0), 1.0 / 11.0);
        assert_eq!(counts.flights(1), 1.0 / 11.0);
        assert_eq!(counts.helicopters, 1.0 / 11.0);
        let mut none = EventCounts::default();
        none.add(&flight, (1.0 / 354.0, 0.0));
        assert_eq!(none, EventCounts::default());
    }
}

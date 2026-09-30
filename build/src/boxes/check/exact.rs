//! The exact sum of the check: every segment within reach of a point through the kernel, per
//! period, beyond the reach, per band and distance of its box pieces, and per flight.

use super::super::emission_of;
use super::super::place::{BoxKey, Placement, cut_into_pieces};
use super::super::read::FlightSegment;
use super::{BEYOND_REACH_M, REACH_M, Receiver};
use physics::bands::PERIODS;
use physics::doc29::screening::Unscreened;
use physics::doc29::segment::{SegmentEmission, SegmentGeometry, segment_sel_at_receiver};
use std::collections::HashMap;
use tiles::geo::Mercator;

/// Energy slots of one point: exact within the reach, beyond it, and the pieces of the first
/// band and above (each per period); per flight within the reach its exact SEL energy and its
/// loudest LAmax (dB).
#[derive(Clone)]
pub(super) struct Sums {
    pub(super) energy: [f64; SLOTS * PERIODS],
    pub(super) flights: HashMap<u64, (f64, f64)>,
    /// For a diagnosed point, per box and period the exact energy of its pieces, that energy times
    /// their mean altitude and over their d_lambda at the point.
    pub(super) per_box: Option<HashMap<BoxKey, [[f64; PERIODS]; 6]>>,
}

pub(super) const EXACT: usize = 0;
pub(super) const BEYOND: usize = PERIODS;
pub(super) const NEAR_GROUND: usize = 2 * PERIODS;
pub(super) const ALOFT: usize = 3 * PERIODS;
/// The exact energy by horizontal distance (of each box piece's middle), from this slot on.
pub(super) const BY_DISTANCE: usize = 4 * PERIODS;
const SLOTS: usize = 4 + DISTANCE_BANDS_M.len();
/// Upper edges of the distance bands the error is split into (m).
pub(super) const DISTANCE_BANDS_M: [f64; 3] = [2_000.0, 6_000.0, REACH_M];

/// The distance band of a horizontal distance within the reach.
pub(super) fn distance_band(distance_m: f64) -> usize {
    DISTANCE_BANDS_M
        .iter()
        .position(|&edge| distance_m < edge)
        .unwrap_or(DISTANCE_BANDS_M.len() - 1)
}

impl Sums {
    pub(super) fn new(diagnosed: bool) -> Self {
        Sums {
            energy: [0.0; SLOTS * PERIODS],
            flights: HashMap::new(),
            per_box: diagnosed.then(HashMap::new),
        }
    }

    pub(super) fn merge(&mut self, other: Sums) {
        for (a, b) in self.energy.iter_mut().zip(other.energy) {
            *a += b;
        }
        for (flight, (energy, lmax_db)) in other.flights {
            let entry = self
                .flights
                .entry(flight)
                .or_insert((0.0, f64::NEG_INFINITY));
            entry.0 += energy;
            entry.1 = entry.1.max(lmax_db);
        }
        if let (Some(mine), Some(theirs)) = (self.per_box.as_mut(), other.per_box) {
            for (key, sums) in theirs {
                let sum = mine.entry(key).or_insert([[0.0; PERIODS]; 6]);
                for (mine, theirs) in sum.iter_mut().zip(sums) {
                    for (a, b) in mine.iter_mut().zip(theirs) {
                        *a += b;
                    }
                }
            }
        }
    }
}

/// Adds one segment of weight `weight` to the sums of every point it reaches.
pub(super) fn add_segment(
    sums: &mut [Sums],
    receivers: &[Receiver],
    placement: &Placement,
    segment: &FlightSegment,
    weight: f64,
) {
    let mut emission: Option<Option<(bool, SegmentEmission)>> = None;
    let mut pieces = None;
    for (sums, receiver) in sums.iter_mut().zip(receivers) {
        let (start, end) = (receiver.local(segment.start), receiver.local(segment.end));
        let nearest = start[0].hypot(start[1]).min(end[0].hypot(end[1]));
        if nearest > BEYOND_REACH_M {
            continue;
        }
        let Some((helicopter, emission)) = emission.get_or_insert_with(|| {
            emission_of(segment)
                .map(|(aircraft, emission)| (aircraft.helicopter.is_some(), emission))
        }) else {
            return;
        };
        let period = usize::from(segment.period).min(PERIODS - 1);
        let ground = |metres: f64| metres - receiver.altitude_m;
        let geometry = SegmentGeometry {
            start_m: start,
            end_m: end,
            ground_under_start_m: ground(segment.ground_m[0]),
            ground_under_end_m: ground(segment.ground_m[1]),
        };
        let Some(sel) = segment_sel_at_receiver(emission, &geometry, &Unscreened) else {
            continue;
        };
        let energy = 10f64.powf(sel.sel_db / 10.0);
        if nearest > REACH_M {
            sums.energy[BEYOND + period] += weight * energy;
            continue;
        }
        sums.energy[EXACT + period] += weight * energy;
        // The loudest LAmax as the popup takes it: the NPD value at the closest point's slant.
        let closest = sel.closest.on_segment_m;
        let lmax_db = emission
            .read_npd(closest[0].hypot(closest[1]).hypot(closest[2]))
            .lamax_db;
        let flight = sums
            .flights
            .entry(segment.flight_id)
            .or_insert((0.0, f64::NEG_INFINITY));
        flight.0 += energy;
        flight.1 = flight.1.max(lmax_db);
        let point = |end: [f64; 3]| (Mercator::from_degrees(end[0], end[1]), end[2]);
        let (from, to) = (point(segment.start), point(segment.end));
        let pieces =
            pieces.get_or_insert_with(|| cut_into_pieces(placement, from, to, *helicopter));
        let length = (to.0.x - from.0.x).hypot(to.0.y - from.0.y);
        for piece in pieces.iter() {
            // The terrain under the piece's ends, along the segment's.
            let under = |at: Mercator| {
                let t = if length > 0.0 {
                    ((at.x - from.0.x).hypot(at.y - from.0.y) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ground(segment.ground_m[0] + t * (segment.ground_m[1] - segment.ground_m[0]))
            };
            let geometry = SegmentGeometry {
                start_m: receiver.metres(piece.start.0, piece.start.1),
                end_m: receiver.metres(piece.end.0, piece.end.1),
                ground_under_start_m: under(piece.start.0),
                ground_under_end_m: under(piece.end.0),
            };
            if let Some(sel) = segment_sel_at_receiver(emission, &geometry, &Unscreened) {
                let slot = if piece.key.band == 0 {
                    NEAR_GROUND
                } else {
                    ALOFT
                };
                let value = weight * 10f64.powf(sel.sel_db / 10.0);
                sums.energy[slot + period] += value;
                let middle = [
                    0.5 * (geometry.start_m[0] + geometry.end_m[0]),
                    0.5 * (geometry.start_m[1] + geometry.end_m[1]),
                ];
                let band = distance_band(middle[0].hypot(middle[1]));
                sums.energy[BY_DISTANCE + PERIODS * band + period] += value;
                if let Some(per_box) = sums.per_box.as_mut() {
                    let sum = per_box.entry(piece.key).or_insert([[0.0; PERIODS]; 6]);
                    sum[0][period] += value;
                    sum[1][period] += value * 0.5 * (piece.start.1 + piece.end.1);
                    sum[2][period] += value / sel.npd.scaled_distance_m;
                    sum[3][period] += value * middle[0];
                    sum[4][period] += value * middle[1];
                    let [a, b] = [geometry.start_m, geometry.end_m];
                    sum[5][period] += value * (b[0] - a[0]).hypot(b[1] - a[1]);
                }
            }
        }
    }
}

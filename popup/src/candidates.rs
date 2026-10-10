//! Every source of the read rings as a candidate: its geometry in the click's frame, the ground at
//! its ends, its closest horizontal distance and its upper bound per period.

use crate::obstacles::REFLECTION_MAX_DB;
use crate::scene::Ground;
use physics::bands::BANDS;
use physics::bands::energy;
use physics::bands::{PERIODS, lden_energy};
use physics::bound::{
    FAVOURABLE_GAIN_BOUND_DB, REACH_EDGE_LDEN_DB, ReceiverBound, Spread, emission_energy, reach_m,
    received_energy_bound,
};
use rayon::prelude::*;
use tiles::geo::TileId;
use tiles::sources::{Layer, Sources};

/// Attributes or pieces per parallel task of one tile.
const PARALLEL_CHUNK: usize = 4_096;

/// Ground sources beyond this horizontal distance are never evaluated, in any mode, so the answer
/// never depends on which tiles were read (dev4's ceiling was 11,622 m).
pub const GROUND_REACH_M: f64 = 12_000.0;

/// Where a candidate's display text lives: ring, tile index in the ring, attribute in the tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRef {
    pub ring: u16,
    pub tile: u16,
    pub attribute: u32,
}

/// What the physics needs of a source's attribute, with its A-weighted linear band energies
/// (per metre for lines), computed once per click.
pub struct SourceAttribute {
    pub layer: Layer,
    pub height_m: f64,
    pub ground_percent: u8,
    pub platform_half_width_m: f64,
    pub exclusion_radius_m: f64,
    pub footprint_id: u64,
    pub group_key: u64,
    pub energy: [[f64; BANDS]; PERIODS],
    /// Its Lden power (per metre for lines), which sets its reach.
    pub lden_power: f64,
}

/// Which attribute a candidate carries: one list per sources file read, then the index in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeRef {
    pub list: u32,
    pub index: u32,
}

/// The attributes of every sources file a click has read, one list per file, never copied.
#[derive(Default)]
pub struct Attributes {
    lists: Vec<Vec<SourceAttribute>>,
}

impl Attributes {
    /// Every file's attributes, in the order their candidates refer to them.
    pub fn lists(&self) -> &[Vec<SourceAttribute>] {
        &self.lists
    }

    /// Keeps a file's attributes; returns the list number its candidates refer to.
    pub fn push(&mut self, list: Vec<SourceAttribute>) -> u32 {
        self.lists.push(list);
        (self.lists.len() - 1) as u32
    }
}

impl std::ops::Index<AttributeRef> for Attributes {
    type Output = SourceAttribute;

    fn index(&self, at: AttributeRef) -> &SourceAttribute {
        &self.lists[at.list as usize][at.index as usize]
    }
}

/// A sources file's attributes and its candidates (their `attribute.list` still to be set).
pub type TileCandidates = (Vec<SourceAttribute>, Vec<Candidate>);

#[derive(Clone)]
pub struct Candidate {
    pub layer: Layer,
    /// A line piece, else a point.
    pub line: bool,
    pub attribute: AttributeRef,
    pub ends_m: [[f64; 2]; 2],
    pub ground_m: [f64; 2],
    pub distance_m: f64,
    pub bound: [f64; PERIODS],
    /// The bound's Lden-weighted energy: candidates are evaluated from the loudest.
    pub order: f64,
    pub group_key: u64,
    pub display: DisplayRef,
}

/// Horizontal distance from `receiver` to the segment `a`-`b`.
fn distance_from(receiver: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    distance_to_segment(
        [a[0] - receiver[0], a[1] - receiver[1]],
        [b[0] - receiver[0], b[1] - receiver[1]],
    )
}

/// Horizontal distance from the origin to the segment `a`-`b`.
fn distance_to_segment(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > 0.0 {
        (-(a[0] * dx + a[1] * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a[0] + t * dx).hypot(a[1] + t * dy)
}

impl Candidate {
    /// How the piece spreads (a line's length is its 3D length).
    fn spread(&self) -> Spread {
        if self.line {
            let [a, b] = self.ends_m;
            let horizontal = (b[0] - a[0]).hypot(b[1] - a[1]);
            Spread::Line {
                length_m: horizontal.hypot(self.ground_m[1] - self.ground_m[0]),
            }
        } else {
            Spread::Point
        }
    }

    /// Horizontal distance from `receiver` (click metres) to the piece.
    pub fn distance_from(&self, receiver: [f64; 2]) -> f64 {
        let [a, b] = self.ends_m;
        if self.line {
            distance_from(receiver, a, b)
        } else {
            (a[0] - receiver[0]).hypot(a[1] - receiver[1])
        }
    }

    /// The bound per period at horizontal distance `distance_m`.
    pub fn bound_at_distance(
        &self,
        distance_m: f64,
        source: &SourceAttribute,
        receiver_bound: &ReceiverBound,
    ) -> [f64; PERIODS] {
        received_energy_bound(&source.energy, self.spread(), distance_m, receiver_bound)
    }

    /// Distance, bound and order for a receiver at `receiver` (click metres); `false` when the
    /// piece lies beyond the reach.
    pub fn bound_at(
        &mut self,
        receiver: [f64; 2],
        source: &SourceAttribute,
        receiver_bound: &ReceiverBound,
    ) -> bool {
        self.distance_m = self.distance_from(receiver);
        self.bound = self.bound_at_distance(self.distance_m, source, receiver_bound);
        self.order = lden_energy(&self.bound);
        self.distance_m <= GROUND_REACH_M
    }

    /// The farthest any receiver can find the piece [`loud`]: its bound at the largest gain a
    /// receiver can have (the ground's and the reflection's) reaches the edge, at most
    /// [`GROUND_REACH_M`].
    pub fn reach_m(&self, source: &SourceAttribute) -> f64 {
        reach_m(
            source.lden_power,
            self.spread(),
            FAVOURABLE_GAIN_BOUND_DB + REFLECTION_MAX_DB,
        )
        .min(GROUND_REACH_M)
    }
}

/// Whether a piece whose bound at a receiver is `bound` is loud there: its bound reaches the edge.
/// The painter evaluates a loud piece exactly and estimates the quiet rest as this crate's
/// selection does (`paint::lattice`).
pub fn loud(bound: &[f64; PERIODS]) -> bool {
    lden_energy(bound) >= energy(REACH_EDGE_LDEN_DB)
}

/// The attributes of one sources file and its pieces within `reach_m` of the receiver at
/// `receiver` (click metres) as candidates; a candidate's `attribute` indexes the returned
/// attributes.
pub fn collect(
    sources: &Sources<'_>,
    tile: TileId,
    display: (u16, u16),
    ground: &Ground,
    receiver: [f64; 2],
    reach_m: f64,
    receiver_bound: &ReceiverBound,
) -> Result<TileCandidates, String> {
    // Both loops run in parallel chunks: a dense tile holds a million sources.
    let attributes = (0..sources.attribute_count())
        .into_par_iter()
        .with_min_len(PARALLEL_CHUNK)
        .map(|index| {
            let attribute = sources
                .attribute(index as u32)
                .map_err(|error| error.to_string())?;
            let energy = emission_energy(&attribute.emission);
            Ok(SourceAttribute {
                layer: attribute.layer,
                height_m: attribute.height_m,
                ground_percent: attribute.ground_percent,
                platform_half_width_m: attribute.platform_half_width_m,
                exclusion_radius_m: attribute.exclusion_radius_m,
                footprint_id: attribute.footprint_id,
                group_key: attribute.group_key,
                energy,
                lden_power: lden_energy(&energy.map(|bands| bands.iter().sum())),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let candidates = (0..sources.piece_count())
        .into_par_iter()
        .with_min_len(PARALLEL_CHUNK)
        .map(|index| -> Result<Option<Candidate>, String> {
            let piece = sources.piece(index).map_err(|error| error.to_string())?;
            let source = &attributes[piece.attribute as usize];
            let ends_m = piece.ends.map(|local| {
                let global = tile.global(local);
                ground
                    .frame
                    .metres_of_steps([global.x as f64, global.y as f64])
            });
            let near = if piece.is_line() {
                distance_from(receiver, ends_m[0], ends_m[1])
            } else {
                (ends_m[0][0] - receiver[0]).hypot(ends_m[0][1] - receiver[1])
            };
            if near > reach_m {
                return Ok(None);
            }
            let start_ground = ground.at(ends_m[0])?.height_m;
            let ground_m = if piece.is_line() {
                [start_ground, ground.at(ends_m[1])?.height_m]
            } else {
                [start_ground; 2]
            };
            let mut candidate = Candidate {
                layer: source.layer,
                line: piece.is_line(),
                order: 0.0,
                display: DisplayRef {
                    ring: display.0,
                    tile: display.1,
                    attribute: piece.attribute,
                },
                group_key: source.group_key,
                attribute: AttributeRef {
                    list: 0,
                    index: piece.attribute,
                },
                ends_m,
                ground_m,
                distance_m: near,
                bound: [0.0; PERIODS],
            };
            candidate.bound_at(receiver, source, receiver_bound);
            Ok(Some(candidate))
        })
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>, String>>()?;
    Ok((attributes, candidates))
}

/// The layers in answer order.
pub const LAYERS: [Layer; 6] = Layer::ALL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_takes_the_clamped_foot() {
        assert!((distance_to_segment([-5.0, 3.0], [5.0, 3.0]) - 3.0).abs() < 1e-12);
        assert!((distance_to_segment([4.0, 3.0], [9.0, 3.0]) - 5.0).abs() < 1e-12);
        assert!((distance_to_segment([3.0, 4.0], [3.0, 4.0]) - 5.0).abs() < 1e-12);
    }
}

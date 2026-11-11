//! Every source of the read rings as a candidate: its geometry in the click's frame, a bridge's deck
//! under it, its closest horizontal distance and its upper bound per period.

use crate::answer::RECEIVER_HEIGHT_M;
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
use tiles::geo::{LocalFrame, TileId};
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
    /// On a bridge: the deck's altitude (m) at the piece's ends, where its source stands instead of
    /// on the terrain.
    pub deck_m: Option<[f64; 2]>,
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
    let offset = closest_offset(a, b);
    offset[0].hypot(offset[1])
}

fn closest_offset(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let t = closest_fraction(a, b);
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

/// How far along the segment `a`-`b` (0 at `a`, 1 at `b`) its point nearest the origin lies.
fn closest_fraction(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = dx * dx + dy * dy;
    if length_sq > 0.0 {
        (-(a[0] * dx + a[1] * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl Candidate {
    /// How the piece spreads (a line carries its power per horizontal metre).
    pub fn spread(&self) -> Spread {
        if self.line {
            let [a, b] = self.ends_m;
            Spread::Line {
                length_m: (b[0] - a[0]).hypot(b[1] - a[1]),
            }
        } else {
            Spread::Point
        }
    }

    /// The ground under the piece's ends as its rays stand on it: the deck on a bridge, else the
    /// terrain with its fine detail (a point's both the same).
    pub fn traced_ground_m(&self, ground: &Ground) -> Result<[f64; 2], String> {
        let [a, b] = self.ends_m;
        Ok(match self.deck_m {
            Some(deck) => deck,
            None if self.line => [ground.at(a)?.height_m, ground.at(b)?.height_m],
            None => [ground.at(a)?.height_m; 2],
        })
    }

    /// The distance from a receiver at `receiver` (click metres) and `altitude_m` to the piece's
    /// emitters where it passes nearest, `height_m` above their ground: Kurze's distance to a line
    /// of them (`crate::percentiles`), over a road's axis the receiver's height above the vehicles.
    /// Their ground is a bridge's deck there, else the receiver's own: exact over the axis and on
    /// level ground, short beside a cutting and long beside an embankment. Across 168 roads in 14
    /// squares the terrain at the nearest point moved a road's Nden by at most 0.6 phon at 99 % of
    /// 2,686 clicks (1 phon on hillsides), and would be read for every pair of every painted pixel.
    pub fn slant_distance_from(&self, receiver: [f64; 2], altitude_m: f64, height_m: f64) -> f64 {
        let [a, b] = (self.ends_m).map(|end| [end[0] - receiver[0], end[1] - receiver[1]]);
        let fraction = closest_fraction(a, b);
        let [x, y] = [
            a[0] + fraction * (b[0] - a[0]),
            a[1] + fraction * (b[1] - a[1]),
        ];
        let vertical_m = match self.deck_m {
            Some([start, end]) => altitude_m - (start + fraction * (end - start)) - height_m,
            None => RECEIVER_HEIGHT_M - height_m,
        };
        (x * x + y * y + vertical_m * vertical_m).sqrt()
    }

    /// Horizontal distance from `receiver` (click metres) to the piece.
    pub fn distance_from(&self, receiver: [f64; 2]) -> f64 {
        let offset = self.offset_from(receiver);
        offset[0].hypot(offset[1])
    }

    /// Offset of the nearest source point, before taking its length.
    pub fn offset_from(&self, receiver: [f64; 2]) -> [f64; 2] {
        let [a, b] = self
            .ends_m
            .map(|end| [end[0] - receiver[0], end[1] - receiver[1]]);
        if self.line { closest_offset(a, b) } else { a }
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
/// The painter evaluates a loud piece exactly and blends the quiet rest (`paint::blocks`).
pub fn loud(bound: &[f64; PERIODS]) -> bool {
    loud_energy(lden_energy(bound))
}

/// Whether a bound of Lden energy `lden` is [`loud`].
pub fn loud_energy(lden: f64) -> bool {
    lden >= energy(REACH_EDGE_LDEN_DB)
}

/// The attributes of one sources file and its pieces within `reach_m` of the receiver at
/// `receiver` (click metres) as candidates; a candidate's `attribute` indexes the returned
/// attributes.
pub fn collect(
    sources: &Sources<'_>,
    tile: TileId,
    display: (u16, u16),
    frame: &LocalFrame,
    receiver: [f64; 2],
    reach_m: f64,
    receiver_bound: &ReceiverBound,
) -> Result<TileCandidates, String> {
    // The loops run in parallel chunks: a dense tile holds a million sources. Attributes share
    // emission rows (a city's buildings ten to a row), so a row's energies are computed once.
    let row_energies: Vec<[[f64; BANDS]; PERIODS]> = (0..sources.emission_row_count())
        .into_par_iter()
        .with_min_len(PARALLEL_CHUNK)
        .map(|row| emission_energy(&sources.emission_row(row)))
        .collect();
    let attributes = (0..sources.attribute_count())
        .into_par_iter()
        .with_min_len(PARALLEL_CHUNK)
        .map(|index| {
            let attribute = sources
                .attribute(index as u32)
                .map_err(|error| error.to_string())?;
            let energy = row_energies[sources
                .emission_row_of(index as u32)
                .map_err(|error| error.to_string())?];
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
                frame.metres_of_steps([global.x as f64, global.y as f64])
            });
            let near = if piece.is_line() {
                distance_from(receiver, ends_m[0], ends_m[1])
            } else {
                (ends_m[0][0] - receiver[0]).hypot(ends_m[0][1] - receiver[1])
            };
            if near > reach_m {
                return Ok(None);
            }
            let deck_m = sources.deck(index).map(|[start, end]| {
                if piece.is_line() {
                    [start, end]
                } else {
                    [start; 2]
                }
            });
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
                deck_m,
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

    /// Kurze's distance runs from the receiver to the emitters themselves: over a road's axis its
    /// height above them (the horizontal distance, none, read roads' axes a median 1.9 phon quiet
    /// in Nden, 168 roads in 14 squares), beside the road the hypotenuse, past a piece's end to
    /// that end, under a bridge up to the deck.
    #[test]
    fn a_line_is_heard_from_its_emitters_where_it_passes_nearest() {
        let piece = |deck_m| Candidate {
            layer: Layer::Road,
            line: true,
            attribute: AttributeRef { list: 0, index: 0 },
            ends_m: [[-100.0, 0.0], [100.0, 0.0]],
            deck_m,
            distance_m: 0.0,
            bound: [0.0; PERIODS],
            order: 0.0,
            group_key: 0,
            display: DisplayRef {
                ring: 0,
                tile: 0,
                attribute: 0,
            },
        };
        let (road, bridge) = (piece(None), piece(Some([320.0, 340.0])));
        let close = |distance: f64, expected: f64| assert!((distance - expected).abs() < 1e-9);
        close(road.slant_distance_from([0.0, 0.0], 304.0, 0.05), 3.95);
        close(
            road.slant_distance_from([30.0, 3.0], 304.0, 0.05),
            3.0f64.hypot(3.95),
        );
        close(
            road.slant_distance_from([110.0, 0.0], 304.0, 0.05),
            10.0f64.hypot(3.95),
        );
        close(bridge.slant_distance_from([50.0, 0.0], 304.0, 0.05), 31.05);
    }
}

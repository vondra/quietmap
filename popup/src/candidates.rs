//! Every source of the read rings as a candidate: its geometry in the click's frame, the ground at
//! its ends, its closest horizontal distance and its upper bound per period.

use crate::scene::Ground;
use physics::bands::BANDS;
use physics::bands::{PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy};
use physics::bound::{Spread, emission_energy, received_energy_bound};
use tiles::geo::TileId;
use tiles::sources::{Attribute, Layer, Piece, Sources};

/// Ground sources beyond this horizontal distance are never evaluated, in any mode, so the answer
/// never depends on which tiles were read (dev4's ceiling was 11,622 m).
pub const GROUND_REACH_M: f64 = 12_000.0;

/// Where a candidate's display text lives: ring, tile index in the ring, attribute in the tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRef {
    pub ring: usize,
    pub tile: usize,
    pub attribute: u32,
}

/// An attribute with its A-weighted linear band energies, computed once per click.
pub struct SourceAttribute {
    pub attribute: Attribute,
    pub energy: [[f64; BANDS]; PERIODS],
}

/// A sources file's attributes and its candidates (indexing those attributes).
pub type TileCandidates = (Vec<SourceAttribute>, Vec<Candidate>);

pub struct Candidate {
    pub layer: Layer,
    pub piece: Piece,
    /// Index into the click's attribute list.
    pub attribute: usize,
    pub ends_m: [[f64; 2]; 2],
    pub ground_m: [f64; 2],
    pub distance_m: f64,
    pub bound: [f64; PERIODS],
    /// The bound's Lden-weighted energy: candidates are evaluated from the loudest.
    pub order: f64,
    pub group_key: u64,
    pub display: DisplayRef,
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

/// The Lden weighting of per-period energies (the ordering key).
pub fn lden_weighted(periods: &[f64; PERIODS]) -> f64 {
    (0..PERIODS)
        .map(|p| PERIOD_HOURS[p] * periods[p] * energy(PERIOD_PENALTY_DB[p]))
        .sum::<f64>()
        / 24.0
}

/// The attributes of one sources file and its pieces within reach as candidates; a candidate's
/// `attribute` indexes the returned attributes.
pub fn collect(
    sources: &Sources<'_>,
    tile: TileId,
    display: (usize, usize),
    ground: &Ground<'_>,
    receiver_gain: &[f64; PERIODS],
) -> Result<TileCandidates, String> {
    let attributes = (0..sources.attribute_count())
        .map(|index| {
            let attribute = sources
                .attribute(index as u32)
                .map_err(|error| error.to_string())?;
            Ok(SourceAttribute {
                energy: emission_energy(&attribute.emission),
                attribute,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut candidates = Vec::with_capacity(sources.piece_count());
    for index in 0..sources.piece_count() {
        let piece = sources.piece(index).map_err(|error| error.to_string())?;
        let source = &attributes[piece.attribute as usize];
        let ends_m = piece.ends.map(|local| {
            let global = tile.global(local);
            ground
                .frame
                .metres_of_steps([global.x as f64, global.y as f64])
        });
        let distance_m = if piece.is_line() {
            distance_to_segment(ends_m[0], ends_m[1])
        } else {
            ends_m[0][0].hypot(ends_m[0][1])
        };
        if distance_m > GROUND_REACH_M {
            continue;
        }
        let ground_m = [
            ground.at(ends_m[0])?.height_m,
            ground.at(ends_m[1])?.height_m,
        ];
        let spread = if piece.is_line() {
            let horizontal = (ends_m[1][0] - ends_m[0][0]).hypot(ends_m[1][1] - ends_m[0][1]);
            Spread::Line {
                length_m: horizontal.hypot(ground_m[1] - ground_m[0]),
            }
        } else {
            Spread::Point
        };
        let bound = received_energy_bound(&source.energy, spread, distance_m, receiver_gain);
        candidates.push(Candidate {
            layer: source.attribute.layer,
            order: lden_weighted(&bound),
            display: DisplayRef {
                ring: display.0,
                tile: display.1,
                attribute: piece.attribute,
            },
            group_key: source.attribute.group_key,
            attribute: piece.attribute as usize,
            piece,
            ends_m,
            ground_m,
            distance_m,
            bound,
        });
    }
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

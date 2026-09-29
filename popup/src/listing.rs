//! The benchmark's piece listing (`qm-popup --pieces N`): the loudest evaluated pieces of each
//! layer with their emission, the buildings and walls on the ray from their closest point and the
//! terms of that ray, for piece-by-piece comparisons with dev4. Never part of a visitor's answer.

use crate::candidates::{AttributeRef, Attributes, Candidate, lden_weighted};
use crate::evaluate::{Receiver, Scratch, trace};
use crate::selection::LayerSelection;
use physics::bands::{BANDS, PERIODS, energy};
use tiles::sources::Layer;

/// One evaluated piece.
#[derive(Clone)]
pub struct EvaluatedPiece {
    pub layer: Layer,
    pub ends_m: [[f64; 2]; 2],
    pub distance_m: f64,
    pub energy: [f64; PERIODS],
    /// A-weighted emission per period (per metre for lines), linear.
    pub emission: [f64; PERIODS],
    pub group_key: u64,
    pub attribute: AttributeRef,
    /// Buildings and walls crossed by the ray from the piece's closest point: distance from the
    /// receiver (m), height (m) and footprint id, filled when listed.
    pub crossings: Vec<(f64, f64, u64)>,
    /// The source's own footprint (0: none).
    pub footprint_id: u64,
    /// The ray from the closest point, filled when listed.
    pub trace: Option<PieceTrace>,
}

/// The terms of one ray, each as an A-weighted attenuation over the piece's day emission spectrum
/// (dB), per state (homogeneous, favourable) where the state matters.
#[derive(Clone)]
pub struct PieceTrace {
    pub slant_m: f64,
    pub favourable_probability: [f64; PERIODS],
    pub boundary_db: [f64; 2],
    pub without_ground_db: [f64; 2],
    pub foliage_db: [f64; 2],
    pub air_db: f64,
    pub path_difference_m: [f64; 2],
}

impl EvaluatedPiece {
    pub fn of(candidate: &Candidate, attributes: &Attributes, energy: [f64; PERIODS]) -> Self {
        EvaluatedPiece {
            layer: candidate.layer,
            ends_m: candidate.ends_m,
            distance_m: candidate.distance_m,
            energy,
            emission: attributes[candidate.attribute]
                .energy
                .map(|bands| bands.iter().sum()),
            group_key: candidate.group_key,
            attribute: candidate.attribute,
            crossings: Vec::new(),
            footprint_id: attributes[candidate.attribute].footprint_id,
            trace: None,
        }
    }
}

/// The point of the segment `a`-`b` closest to `receiver`.
fn closest_point(receiver: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > 0.0 {
        (((receiver[0] - a[0]) * dx + (receiver[1] - a[1]) * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    [a[0] + t * dx, a[1] + t * dy]
}

/// The `count` loudest evaluated pieces of every layer, traced at `receiver`.
pub fn list_pieces(
    selections: &mut [LayerSelection],
    count: usize,
    receiver: &Receiver,
    attributes: &Attributes,
) -> Result<Vec<EvaluatedPiece>, String> {
    let mut listed = Vec::new();
    let mut scratch = Scratch::default();
    for selection in selections.iter_mut() {
        selection
            .pieces
            .sort_by(|a, b| lden_weighted(&b.energy).total_cmp(&lden_weighted(&a.energy)));
        for piece in selection.pieces.iter().take(count) {
            let mut piece = piece.clone();
            let from = closest_point(receiver.position, piece.ends_m[0], piece.ends_m[1]);
            let mut crossings = Vec::new();
            receiver
                .obstacles
                .crossings(from, receiver.position, &mut crossings)?;
            let length = (from[0] - receiver.position[0]).hypot(from[1] - receiver.position[1]);
            piece.crossings = crossings
                .iter()
                .map(|crossing| {
                    (
                        (1.0 - crossing.t) * length,
                        crossing.height_m,
                        crossing.footprint_id,
                    )
                })
                .collect();
            let source = &attributes[piece.attribute];
            let terms = trace(receiver, from, source, &mut scratch)?;
            let spectrum = source.energy[0];
            let weighted = |attenuation: &[f64; BANDS]| {
                let total: f64 = spectrum.iter().sum();
                let passed: f64 = (0..BANDS)
                    .map(|band| spectrum[band] * energy(-attenuation[band]))
                    .sum();
                -10.0 * (passed / total).log10()
            };
            piece.trace = Some(PieceTrace {
                slant_m: terms.transfer.slant_m,
                favourable_probability: terms.favourable_probability,
                boundary_db: [0, 1].map(|state| weighted(&terms.boundaries[state].attenuation_db)),
                without_ground_db: [0, 1]
                    .map(|state| weighted(&terms.boundaries[state].without_ground_db)),
                foliage_db: [0, 1].map(|state| weighted(&terms.foliage_db[state])),
                air_db: weighted(&terms.air_db),
                path_difference_m: [0, 1].map(|state| terms.boundaries[state].path_difference_m),
            });
            listed.push(piece);
        }
    }
    Ok(listed)
}

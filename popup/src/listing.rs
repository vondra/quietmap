//! The piece listing (`qm-popup --pieces N`, the segments view and the benchmark): the loudest
//! evaluated pieces of each layer with their emission, every ray they were summed over with what
//! it delivers, the buildings and walls on the ray from their closest point and the terms of that
//! ray.

use crate::candidates::{Attributes, Candidate};
use crate::evaluate::{Receiver, Scratch, source_rays, trace};
use crate::selection::LayerSelection;
use physics::bands::{BANDS, PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy, lden_energy};

/// Ground samples a listed piece's trace keeps.
pub const PROFILE_POINTS: usize = 48;

/// One evaluated piece.
#[derive(Clone)]
pub struct EvaluatedPiece {
    pub candidate: Candidate,
    pub energy: [f64; PERIODS],
    /// A-weighted emission per period (per metre for lines), linear.
    pub emission: [f64; PERIODS],
    /// Buildings and walls crossed by the ray from the piece's closest point: distance from the
    /// receiver (m), height (m) and footprint id, filled when listed.
    pub crossings: Vec<(f64, f64, u64)>,
    /// The source's own footprint (0: none).
    pub footprint_id: u64,
    /// The ray from the closest point, filled when listed.
    pub trace: Option<PieceTrace>,
    /// Every ray the piece was summed over, filled when listed.
    pub rays: Vec<ListedRay>,
}

/// One ray of a listed piece: the point it leaves from (click metres), the in-plane angle it
/// stands for (0 for a point), the energy it delivers per period (the piece's energy is their
/// sum) and its terms.
#[derive(Clone)]
pub struct ListedRay {
    pub from_m: [f64; 2],
    pub angle_rad: f64,
    pub energy: [f64; PERIODS],
    pub terms: Option<ListedTerms>,
}

/// A ray's terms as A-weighted attenuations over the piece's day emission spectrum (dB): ground
/// and screening together and screening alone per state (calm air, bent down), the air's
/// absorption, and its slant length (m).
#[derive(Clone, Copy)]
pub struct ListedTerms {
    pub boundary_db: [f64; 2],
    pub without_ground_db: [f64; 2],
    pub air_db: f64,
    pub slant_m: f64,
}

/// The terms of one ray, each as an A-weighted attenuation over the piece's day emission spectrum
/// (dB), per state (homogeneous, favourable) where the state matters.
#[derive(Clone)]
pub struct PieceTrace {
    /// The ground under the ray from the source: distance (m), altitude (m) and G, at most
    /// [`PROFILE_POINTS`] samples; the source and the receiver altitudes (m).
    pub profile: Vec<[f64; 3]>,
    pub source_altitude_m: f64,
    pub receiver_altitude_m: f64,
    pub slant_m: f64,
    pub favourable_probability: [f64; PERIODS],
    pub boundary_db: [f64; 2],
    pub without_ground_db: [f64; 2],
    pub air_db: f64,
    pub path_difference_m: [f64; 2],
    /// The ray on the map: from the piece's closest point to the receiver (click metres).
    pub ray_m: [[f64; 2]; 2],
}

impl EvaluatedPiece {
    pub fn of(candidate: &Candidate, attributes: &Attributes, energy: [f64; PERIODS]) -> Self {
        EvaluatedPiece {
            candidate: candidate.clone(),
            energy,
            emission: attributes[candidate.attribute]
                .energy
                .map(|bands| bands.iter().sum()),
            crossings: Vec::new(),
            footprint_id: attributes[candidate.attribute].footprint_id,
            trace: None,
            rays: Vec::new(),
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
            .sort_by(|a, b| lden_energy(&b.energy).total_cmp(&lden_energy(&a.energy)));
        for piece in selection.pieces.iter().take(count) {
            let mut piece = piece.clone();
            let source = &attributes[piece.candidate.attribute];
            source_rays(
                receiver,
                &piece.candidate,
                source,
                &mut scratch,
                &mut |ray| {
                    piece.rays.push(ListedRay {
                        from_m: ray.from_m,
                        angle_rad: ray.angle_rad,
                        energy: std::array::from_fn(|period| {
                            (0..BANDS)
                                .map(|band| {
                                    ray.weight
                                        * source.energy[period][band]
                                        * ray.transfer.periods[period][band]
                                })
                                .sum()
                        }),
                        terms: None,
                    })
                },
            )?;
            // The terms weigh the bands as the piece's Lden does (a source silent by day has terms).
            let spectrum: [f64; BANDS] = std::array::from_fn(|band| {
                (0..PERIODS)
                    .map(|p| {
                        PERIOD_HOURS[p] * energy(PERIOD_PENALTY_DB[p]) * source.energy[p][band]
                    })
                    .sum()
            });
            let weighted = |attenuation: &[f64; BANDS]| {
                let total: f64 = spectrum.iter().sum();
                let passed: f64 = (0..BANDS)
                    .map(|band| spectrum[band] * energy(-attenuation[band]))
                    .sum();
                -10.0 * (passed / total).log10()
            };
            // Every ray's own terms (the piece's nearest ray below also keeps its ground profile).
            for ray in &mut piece.rays {
                let terms = trace(receiver, ray.from_m, source, &mut scratch)?;
                ray.terms = Some(ListedTerms {
                    boundary_db: [0, 1]
                        .map(|state| weighted(&terms.boundaries[state].attenuation_db)),
                    without_ground_db: [0, 1]
                        .map(|state| weighted(&terms.boundaries[state].without_ground_db)),
                    air_db: weighted(&terms.air_db),
                    slant_m: terms.transfer.slant_m,
                });
            }
            let [a, b] = piece.candidate.ends_m;
            let from = closest_point(receiver.position, a, b);
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
            let terms = trace(receiver, from, source, &mut scratch)?;
            let ground = scratch.profile();
            let step = ground.t.len().div_ceil(PROFILE_POINTS).max(1);
            let mut profile: Vec<[f64; 3]> = (0..ground.t.len())
                .filter(|&k| k % step == 0 || k + 1 == ground.t.len())
                .map(|k| {
                    [
                        ground.t[k] * ground.horizontal_m,
                        ground.ground_m[k],
                        ground.ground_factor[k],
                    ]
                })
                .collect();
            profile.dedup_by(|a, b| a[0] == b[0]);
            piece.trace = Some(PieceTrace {
                source_altitude_m: ground.ground_m.first().copied().unwrap_or(0.0)
                    + source.height_m,
                receiver_altitude_m: receiver.altitude_m,
                profile,
                slant_m: terms.transfer.slant_m,
                favourable_probability: terms.favourable_probability,
                boundary_db: [0, 1].map(|state| weighted(&terms.boundaries[state].attenuation_db)),
                without_ground_db: [0, 1]
                    .map(|state| weighted(&terms.boundaries[state].without_ground_db)),
                air_db: weighted(&terms.air_db),
                path_difference_m: [0, 1].map(|state| terms.boundaries[state].path_difference_m),
                ray_m: [from, receiver.position],
            });
            listed.push(piece);
        }
    }
    Ok(listed)
}

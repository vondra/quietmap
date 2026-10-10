//! A source's sound path (`qm-popup --source KEYS`, an opened row's pieces): every piece of the
//! asked sources, each evaluated with certainty, loudest first, with its emission, its own sound
//! path account and the buildings and walls on the ray from its closest point; the
//! [`TRACED_PIECES`] loudest also with every ray they were summed over, what it delivers and its
//! terms, and the ground under the ray from the closest point; one asked piece (`--piece K`) with
//! the ground and walls under each of its rays.

use crate::candidates::{Attributes, Candidate};
use crate::evaluate::{Path, Received, Receiver, Scratch, period_sums, source_rays};
use crate::selection::LayerSelection;
use physics::bands::{BANDS, PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS, energy, lden_energy};
use physics::ray::containing_footprints;

/// Ground samples a listed piece's trace keeps.
pub const PROFILE_POINTS: usize = 48;
/// The loudest pieces listed with their rays and ground (the update stays under 100 KB).
pub const TRACED_PIECES: usize = 24;

/// One evaluated piece.
#[derive(Clone)]
pub struct EvaluatedPiece {
    pub candidate: Candidate,
    pub energy: [f64; PERIODS],
    /// How its sound reaches the receiver, and by what its rays bend over (`Received::edges`).
    pub path: Path,
    pub edges: [[f64; 3]; 3],
    /// A-weighted emission per period (per metre for lines), linear.
    pub emission: [f64; PERIODS],
    /// The ray from the closest point, filled for the traced pieces.
    pub trace: Option<PieceTrace>,
    /// Every ray the piece was summed over, filled for the traced pieces.
    pub rays: Vec<ListedRay>,
}

/// One ray of a listed piece: the point it leaves from (click metres), the in-plane angle it
/// stands for (0 for a point), the energy it delivers per period (the piece's energy is their
/// sum), the terms it was summed with, and for the asked piece its ground and walls.
#[derive(Clone)]
pub struct ListedRay {
    pub from_m: [f64; 2],
    pub angle_rad: f64,
    pub energy: [f64; PERIODS],
    pub terms: ListedTerms,
    pub profile: Option<RayProfile>,
}

/// The ground and the walls under one ray as the evaluation saw them: from the source, the
/// distance (m), altitude (m) and G of at most [`PROFILE_POINTS`] samples; the source's altitude
/// (m); each wall's distance (m), height above the ground (m) and whether a building's (else a
/// barrier's). No wall where the skyline showed none could reach the line of sight.
#[derive(Clone)]
pub struct RayProfile {
    pub ground: Vec<[f64; 3]>,
    pub source_altitude_m: f64,
    pub walls: Vec<(f64, f64, bool)>,
}

/// At most [`PROFILE_POINTS`] samples of the ground under a ray (distance, altitude, G): the source's
/// two (its ground and the probe a few metres out that finds a berm), the receiver's, and between
/// them the highest of each stretch, so a crest that screens is never thinned away.
fn sampled(ground: &physics::profile::Profile) -> Vec<[f64; 3]> {
    let count = ground.t.len();
    let at = |k: usize| {
        [
            ground.t[k] * ground.horizontal_m,
            ground.ground_m[k],
            ground.ground_factor[k],
        ]
    };
    if count <= PROFILE_POINTS {
        return (0..count).map(at).collect();
    }
    let stretches = PROFILE_POINTS - 3;
    let inner = count - 3;
    let highest = (0..stretches).filter_map(|stretch| {
        let (low, high) = (
            2 + inner * stretch / stretches,
            2 + inner * (stretch + 1) / stretches,
        );
        (low..high).max_by(|&a, &b| ground.ground_m[a].total_cmp(&ground.ground_m[b]))
    });
    [0, 1]
        .into_iter()
        .chain(highest)
        .chain([count - 1])
        .map(at)
        .collect()
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

/// Where a listed piece's nearest ray runs on the map, from its closest point to the receiver
/// (click metres), and the receiver's altitude (m).
#[derive(Clone)]
pub struct PieceTrace {
    pub ray_m: [[f64; 2]; 2],
    pub receiver_altitude_m: f64,
}

impl EvaluatedPiece {
    pub fn of(candidate: &Candidate, attributes: &Attributes, received: &Received) -> Self {
        EvaluatedPiece {
            candidate: candidate.clone(),
            energy: period_sums(&received.bands),
            path: received.path,
            edges: received.edges,
            emission: attributes[candidate.attribute]
                .energy
                .map(|bands| bands.iter().sum()),
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

/// How the asked source's sound arrives, over all its pieces: their count, and their rays'
/// Lden-weighted energy after the air, after the screening and after the boundary, calm, by what
/// the rays bend over in calm air (`Received::edges`: nothing, a building or wall, terrain).
#[derive(Clone, Default)]
pub struct Arrival {
    pub pieces: usize,
    pub edges: [[f64; 3]; 3],
}

/// The names of [`Arrival`]'s edges, in order.
pub const EDGES: [&str; 3] = ["open", "buildings", "terrain"];

/// The kept pieces of every layer (the asked source's) at `receiver`: how they arrive, and the
/// [`TRACED_PIECES`] loudest listed, or only the `piece`th loudest with every ray's ground and
/// walls.
pub fn list_source(
    selections: &mut [LayerSelection],
    receiver: &Receiver,
    attributes: &Attributes,
    piece: Option<usize>,
) -> Result<(Vec<EvaluatedPiece>, Arrival), String> {
    let mut listed: Vec<EvaluatedPiece> = selections
        .iter_mut()
        .flat_map(|selection| selection.pieces.drain(..))
        .collect();
    listed.sort_by(|a, b| lden_energy(&b.energy).total_cmp(&lden_energy(&a.energy)));
    let mut arrival = Arrival::default();
    let mut scratch = Scratch::default();
    let asked = piece;
    for (rank, piece) in listed.iter_mut().enumerate() {
        let source = &attributes[piece.candidate.attribute];
        arrival.pieces += 1;
        for (total, piece) in arrival.edges.iter_mut().zip(&piece.edges) {
            for (sum, value) in total.iter_mut().zip(piece) {
                *sum += value;
            }
        }
        let profiled = asked == Some(rank);
        if !profiled && (asked.is_some() || rank >= TRACED_PIECES) {
            continue;
        }
        {
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
            // Every ray with the terms the evaluation summed it with (the piece's nearest ray below
            // also keeps its ground profile).
            source_rays(
                receiver,
                &piece.candidate,
                source,
                &mut scratch,
                &mut |ray| {
                    let terms = &ray.terms;
                    let profile = profiled.then(|| {
                        let mut own_walls = Vec::new();
                        containing_footprints(ray.crossings, source.footprint_id, &mut own_walls);
                        RayProfile {
                            ground: sampled(ray.profile),
                            source_altitude_m: ray.profile.ground_m.first().copied().unwrap_or(0.0)
                                + source.height_m,
                            // The walls that screen it: not those of the building it stands in.
                            walls: ray
                                .crossings
                                .iter()
                                .filter(|wall| own_walls.binary_search(&wall.footprint_id).is_err())
                                .map(|wall| {
                                    (
                                        wall.t * ray.profile.horizontal_m,
                                        wall.height_m,
                                        wall.building,
                                    )
                                })
                                .collect(),
                        }
                    });
                    piece.rays.push(ListedRay {
                        profile,
                        from_m: ray.from_m,
                        angle_rad: ray.angle_rad,
                        energy: std::array::from_fn(|period| {
                            (0..BANDS)
                                .map(|band| {
                                    ray.weight
                                        * source.energy[period][band]
                                        * terms.transfer.periods[period][band]
                                })
                                .sum()
                        }),
                        terms: ListedTerms {
                            boundary_db: [0, 1]
                                .map(|state| weighted(&terms.boundaries[state].attenuation_db)),
                            without_ground_db: [0, 1]
                                .map(|state| weighted(&terms.boundaries[state].without_ground_db)),
                            air_db: weighted(&terms.air_db),
                            slant_m: terms.transfer.slant_m,
                        },
                    })
                },
            )?;
            let [a, b] = piece.candidate.ends_m;
            piece.trace = Some(PieceTrace {
                ray_m: [closest_point(receiver.position, a, b), receiver.position],
                receiver_altitude_m: receiver.altitude_m,
            });
        }
    }
    match asked {
        Some(rank) => listed = listed.into_iter().nth(rank).into_iter().collect(),
        None => listed.truncate(TRACED_PIECES),
    }
    Ok((listed, arrival))
}

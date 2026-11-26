//! A pixel's paint blended between a block's exact points: its own sources by their free field, the rest in the logarithm, its loudness carried along the block's slope of loudness against energy.

use crate::exact::LAYERS;
use crate::levels::Painted;

/// An exact point a block's pixels are blended from: its paint and, of its layers' energies, what
/// the block's own sources (crossing it or the eight around) deliver there, with their free field
/// there ([`crate::free_field`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub paint: Painted,
    pub own: [f64; LAYERS],
    pub free: [f64; LAYERS],
}

/// The floors added to an energy and a loudness before their logarithm (0 dB, a sixteenth of a sone:
/// the stored silence): silence then blends too, and values far under a floor nearly linearly.
pub(crate) const ENERGY_FLOOR: f64 = 1.0;
pub(crate) const SONE_FLOOR: f64 = 1.0 / 16.0;

/// A layer's energies and the total's, each in the logarithm.
pub(crate) fn log_energies(energies: &[f64; LAYERS]) -> [f64; LAYERS + 1] {
    let total: f64 = energies.iter().sum();
    std::array::from_fn(|at| (energies.get(at).copied().unwrap_or(total) + ENERGY_FLOOR).ln())
}

/// An exact point as the pixels blended from it read it, each logarithm taken once for all of
/// them (taken again at every blended pixel they were a tenth of a light square's work on the
/// cores): its layers' energies and their total, its loudness, each layer's energy without the
/// block's own sources' and theirs, and their free field, plain too.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Heard {
    pub paint: Painted,
    energies: [f64; LAYERS + 1],
    ndens: [f64; LAYERS + 1],
    rest: [f64; LAYERS],
    own: [f64; LAYERS],
    free_ln: [f64; LAYERS],
    free: [f64; LAYERS],
}

impl Heard {
    pub fn of(anchor: &Anchor) -> Self {
        let (energies, ndens) = &anchor.paint;
        Heard {
            paint: anchor.paint,
            free: anchor.free,
            energies: log_energies(energies),
            ndens: ndens.map(|sone| (sone + SONE_FLOOR).ln()),
            rest: std::array::from_fn(|layer| {
                ((energies[layer] - anchor.own[layer]).max(0.0) + ENERGY_FLOOR).ln()
            }),
            own: anchor.own.map(|energy| (energy + ENERGY_FLOOR).ln()),
            free_ln: anchor.free.map(f64::ln),
        }
    }
}

/// Per layer and for the total how a block's loudness follows its energy from one lattice point
/// to another: the least-squares slope of log loudness on log energy over its `lattice`. Between
/// neighbouring pixels the same sources sound, so a pixel's loudness is its neighbours' moved
/// along this slope to its own energy: at the pixels the old painter computed in refused patches
/// of two rural squares (153,000 of 1 sone) that was 0.21-0.24 phon rms from their own loudness
/// where the patch's probe confirmed it (88-95 % of the patches), 0.5-1.0 % of them over 1 phon.
/// Where the energy hardly varies the slope is Stevens' 0.3 (a doubling of loudness every 10 dB).
pub(crate) fn loudness_slopes(lattice: &[Option<Heard>]) -> [f64; LAYERS + 1] {
    const STEVENS: f64 = 0.3;
    let heard: Vec<&Heard> = lattice.iter().flatten().collect();
    let count = heard.len() as f64;
    std::array::from_fn(|at| {
        let x = |k: usize| heard[k].energies[at];
        let y = |k: usize| heard[k].ndens[at];
        let mean_x = (0..heard.len()).map(x).sum::<f64>() / count;
        let mean_y = (0..heard.len()).map(y).sum::<f64>() / count;
        let spread: f64 = (0..heard.len()).map(|k| (x(k) - mean_x).powi(2)).sum();
        let together: f64 = (0..heard.len())
            .map(|k| (x(k) - mean_x) * (y(k) - mean_y))
            .sum();
        // Under a spread of 0.2 dB rms over the lattice the slope is its noise.
        if heard.len() < 4 || spread < 0.05 * count / 25.0 {
            STEVENS
        } else {
            (together / spread).clamp(0.0, 1.2)
        }
    })
}

/// How far beyond its patch's lattice points' energies a pixel's may lie for its loudness to be
/// carried from them (half a decibel). A carry between the points' energies interpolates along the
/// slope; beyond them it extrapolates, beside a road's axis or into a shadow: of the pixels the old
/// painter computed in confirmed patches of a suburb's and a city's square, 3-5 % lay beyond, and
/// their carried loudness missed their own by over 1 phon at 26-47 % of them, the others' at
/// 0.9-1.9 % (private evidence 2026-11-12/fable-speed, `carry_guard_study.py`).
const CARRIED_BEYOND: f64 = 0.115_129_254_649_702_28;
/// Margin when carrying loudness alone onto pixels whose exact ray energies are already known
/// (`1.5 dB = ln(10^(1.5 / 10))`). Chosen below one 2 dB map loudness bin (widened from 0.5 dB
/// because exact energies already capture local attenuation) to avoid full loudness synthesis on
/// refused patches; the combined release kept 97.68 % of visible popup clicks within 1 phon and
/// 99.97 % within 3 phon in the 54-square world sample (2026-11-26).
pub(crate) const CARRIED_LOUDNESS_BEYOND: f64 = 0.345_387_763_949_106_84;

/// A layer under this Lden energy at a pixel and at its lattice points (20 dB) is not asked to lie
/// within their range: no visitor sees it (carried so, no layer under 20 dB reads over 0.27 sone
/// in the world sample's 56 million cells).
const HEARD_FROM: f64 = 100.0;

/// Maximum energy spread across a refused patch's outdoor lattice points
/// (`7.5 dB = ln(10^(7.5 / 10))`) for carrying loudness on any layer or total reaching
/// [`CARRIED_SPREAD_FROM`] (`32 dB = 10^(32 / 10)`, matching `visible::tolerance::TOTAL_FROM_DB`
/// just below the 35 dB visible floor). Chosen just under the 8-21 dB shadow-cliff spreads seen
/// at >10 phon outdoor map-to-map outliers across the 56.6M assessed pixels of the 54-square
/// world sample (2026-11-26); checking the eight remaining >10 phon map-to-map outliers against
/// the popup showed all five outdoor pixels within 0.1-3.6 phon of the popup (the other three
/// were indoor pixels of two small buildings).
const CARRIED_MAX_SPREAD: f64 = 1.726_938_819_745_534_2;
const CARRIED_SPREAD_FROM: f64 = 1_584.893_192_461_114;

/// Whether the outdoor points `around` a refused patch stay within [`CARRIED_MAX_SPREAD`] of each
/// other in every layer (and total) reaching [`CARRIED_SPREAD_FROM`].
pub(crate) fn within_carried_spread(around: [Option<&Heard>; 4]) -> bool {
    let from = (CARRIED_SPREAD_FROM + ENERGY_FLOOR).ln();
    (0..=LAYERS).all(|at| {
        let (low, high) = (around.iter().flatten())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), point| {
                (low.min(point.energies[at]), high.max(point.energies[at]))
            });
        high < from || high - low <= CARRIED_MAX_SPREAD
    })
}

/// Whether a pixel's energies (`here`, [`log_energies`]) lie within the range of the outdoor
/// points' `around` it, widened by `margin`, in the total and in every layer heard there.
pub(crate) fn within_lattice_range(
    around: [Option<&Heard>; 4],
    here: &[f64; LAYERS + 1],
    margin: f64,
) -> bool {
    let heard_from = (HEARD_FROM + ENERGY_FLOOR).ln();
    (0..=LAYERS).all(|at| {
        let (low, high) = (around.iter().flatten())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), point| {
                (low.min(point.energies[at]), high.max(point.energies[at]))
            });
        (at < LAYERS && here[at].max(high) < heard_from)
            || (here[at] >= low - margin && here[at] <= high + margin)
    })
}

/// The loudness among four lattice points (`weights` theirs there, bilinear) of a pixel of the
/// energies `here` ([`log_energies`]): each outdoor point's loudness moved along `slopes` to the
/// energy there, blended in the logarithm by the outdoor points' weights; `None` where none of
/// them counts. A town's patches mostly have a lattice point in a building, and the others carry
/// as well.
pub(crate) fn carried_loudness(
    around: [Option<&Heard>; 4],
    weights: [f64; 4],
    here: &[f64; LAYERS + 1],
    slopes: &[f64; LAYERS + 1],
) -> Option<[f64; LAYERS + 1]> {
    let outdoor: f64 = (around.iter().zip(weights))
        .filter(|(point, _)| point.is_some())
        .map(|(_, weight)| weight)
        .sum();
    if outdoor <= 0.0 {
        return None;
    }
    let mut moved = [0.0; LAYERS + 1];
    for (point, weight) in around.iter().zip(weights) {
        let Some(point) = point else {
            continue;
        };
        for at in 0..=LAYERS {
            moved[at] +=
                weight / outdoor * (point.ndens[at] + slopes[at] * (here[at] - point.energies[at]));
        }
    }
    Some(moved.map(|level| (level.exp() - SONE_FLOOR).max(0.0)))
}

/// The paint among four exact points (`weights` theirs there, bilinear) of a pixel whose block's
/// own sources have the free field `free` there; `None` where its energies lie beyond the points'
/// ([`within_lattice_range`]) or it stands nearer own sources than every point (their free field
/// over [`CARRIED_BEYOND`] beyond the points') where they could be heard (they or their free field
/// alone reach [`HEARD_FROM`]: no point may have heard them): a pass paints it.
/// Their excess nearer than any point is unknown. Per layer the own sources' energy blended
/// between the points in the logarithm over the energy's floor, as the rest of the layer is,
/// times their free field at the pixel over the points' free field blended so: one rule for every
/// cell, so two cells agree along the edge they share, and a source no point hears adds nothing
/// however near the pixel stands (its free field's ratio alone once made 35 dB of silence). A
/// road's field falls as 1/r from its axis, and the middle of a patch 12 to 36 m from a road read
/// 1.25 dB loud blended in energy and 0.6 dB in the logarithm, where its free field is exact and
/// its excess changes slowly. On 64 blocks of each of 18 squares, every loud source traced at
/// every pixel, a block then computes 9.9 points instead of 28.3 in the lightest class of ground
/// squares and 29.5 instead of 49.1 in the next, the blended pixels as near the exact sum (0.18
/// against 0.21 and 0.15 against 0.15 dB rms; private evidence 2026-11-18/fable-speed-2). The
/// loudness is the points' moved to the pixel's energies ([`carried_loudness`]): between points
/// of equal energies their blend in the logarithm.
pub(crate) fn blend(
    around: [&Heard; 4],
    weights: [f64; 4],
    free: &[f64; LAYERS],
    slopes: &[f64; LAYERS + 1],
) -> Option<Painted> {
    let between =
        |values: [f64; 4]| -> f64 { values.iter().zip(weights).map(|(v, w)| v * w).sum() };
    let mut energies = [0.0; LAYERS];
    for layer in 0..LAYERS {
        energies[layer] =
            (between(around.map(|point| point.rest[layer])).exp() - ENERGY_FLOOR).max(0.0);
        if free[layer] == 0.0 {
            continue;
        }
        let heard = between(around.map(|point| point.own[layer])).exp() - ENERGY_FLOOR;
        let own = heard * free[layer] / between(around.map(|point| point.free_ln[layer])).exp();
        let nearest = (around.iter()).fold(0.0f64, |most, point| most.max(point.free[layer]));
        // Not finite: a point end-on to the block's only piece of the layer has no free field.
        if !own.is_finite()
            || (own.max(free[layer]) >= HEARD_FROM && free[layer] > nearest * CARRIED_BEYOND.exp())
        {
            return None;
        }
        energies[layer] += own;
    }
    let here = log_energies(&energies);
    let around = around.map(Some);
    if !within_lattice_range(around, &here, CARRIED_BEYOND) {
        return None;
    }
    carried_loudness(around, weights, &here, slopes).map(|ndens| (energies, ndens))
}

#[cfg(test)]
#[path = "lattice_blend_tests.rs"]
mod tests;

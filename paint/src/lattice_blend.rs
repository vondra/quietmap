//! Lattice points' paint blended in the logarithm, and a pixel's loudness carried from them along the block's slope of loudness against energy.

use crate::exact::LAYERS;
use crate::levels::Painted;
use crate::paint::bilinear;

/// The floors added to an energy and a loudness before their logarithm (0 dB, a sixteenth of a sone:
/// the stored silence): silence then blends too, and values far under a floor nearly linearly.
pub(crate) const ENERGY_FLOOR: f64 = 1.0;
pub(crate) const SONE_FLOOR: f64 = 1.0 / 16.0;

/// A layer's energies and the total's, each in the logarithm.
fn log_energies(energies: &[f64; LAYERS]) -> [f64; LAYERS + 1] {
    let total: f64 = energies.iter().sum();
    std::array::from_fn(|at| (energies.get(at).copied().unwrap_or(total) + ENERGY_FLOOR).ln())
}

/// Per layer and for the total how a block's loudness follows its energy from one lattice point
/// to another: the least-squares slope of log loudness on log energy over its `lattice`. Between
/// neighbouring pixels the same sources sound, so a pixel's loudness is its neighbours' moved
/// along this slope to its own energy: at the pixels the old painter computed in refused patches
/// of two rural squares (153,000 of 1 sone) that was 0.21-0.24 phon rms from their own loudness
/// where the patch's probe confirmed it (88-95 % of the patches), 0.5-1.0 % of them over 1 phon.
/// Where the energy hardly varies the slope is Stevens' 0.3 (a doubling of loudness every 10 dB).
pub(crate) fn loudness_slopes(lattice: &[Option<Painted>]) -> [f64; LAYERS + 1] {
    const STEVENS: f64 = 0.3;
    let heard: Vec<([f64; LAYERS + 1], &[f64; LAYERS + 1])> = (lattice.iter().flatten())
        .map(|(energies, ndens)| (log_energies(energies), ndens))
        .collect();
    let count = heard.len() as f64;
    std::array::from_fn(|at| {
        let x = |k: usize| heard[k].0[at];
        let y = |k: usize| (heard[k].1[at] + SONE_FLOOR).ln();
        let mean_x = (0..heard.len()).map(x).sum::<f64>() / count;
        let mean_y = (0..heard.len()).map(y).sum::<f64>() / count;
        let spread: f64 = (0..heard.len()).map(|k| (x(k) - mean_x).powi(2)).sum();
        let together: f64 = (0..heard.len())
            .map(|k| (x(k) - mean_x) * (y(k) - mean_y))
            .sum();
        // Under a spread of 1 dB a point over the lattice the slope is its noise.
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

/// A layer under this Lden energy at a pixel and at its lattice points (20 dB) is not asked to lie
/// within their range: no visitor sees it.
const HEARD_FROM: f64 = 100.0;

/// Whether a pixel's `energies` lie within the range of its patch's outdoor lattice points'
/// (`around`), widened by [`CARRIED_BEYOND`], in the total and in every layer heard there.
pub(crate) fn within_lattice_range(
    around: &[Option<Painted>; 4],
    energies: &[f64; LAYERS],
) -> bool {
    let here = log_energies(energies);
    let there: Vec<[f64; LAYERS + 1]> = (around.iter().flatten())
        .map(|(energies, _)| log_energies(energies))
        .collect();
    let heard_from = (HEARD_FROM + ENERGY_FLOOR).ln();
    (0..=LAYERS).all(|at| {
        let (low, high) = (there.iter())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), point| {
                (low.min(point[at]), high.max(point[at]))
            });
        (at < LAYERS && here[at].max(high) < heard_from)
            || (here[at] >= low - CARRIED_BEYOND && here[at] <= high + CARRIED_BEYOND)
    })
}

/// The loudness among four lattice points (`weights` theirs there, bilinear) from `energies` there:
/// each outdoor point's loudness moved along `slopes` to the energy there, blended in the logarithm
/// by the outdoor points' weights; `None` where none of them counts. A town's patches mostly have
/// a lattice point in a building, and the others carry as well.
pub(crate) fn carried_loudness(
    around: [Option<Painted>; 4],
    weights: [f64; 4],
    energies: &[f64; LAYERS],
    slopes: &[f64; LAYERS + 1],
) -> Option<[f64; LAYERS + 1]> {
    let outdoor: f64 = (around.iter().zip(weights))
        .filter(|(point, _)| point.is_some())
        .map(|(_, weight)| weight)
        .sum();
    if outdoor <= 0.0 {
        return None;
    }
    let here = log_energies(energies);
    let mut moved = [0.0; LAYERS + 1];
    for (point, weight) in around.iter().zip(weights) {
        let Some((there, ndens)) = point else {
            continue;
        };
        let there = log_energies(there);
        for at in 0..=LAYERS {
            moved[at] += weight / outdoor
                * ((ndens[at] + SONE_FLOOR).ln() + slopes[at] * (here[at] - there[at]));
        }
    }
    Some(moved.map(|level| (level.exp() - SONE_FLOOR).max(0.0)))
}

/// Four lattice points' paint blended in the logarithm of each energy and loudness, with a floor
/// that lets silence blend (0 dB, a sixteenth of a sone: the stored silence): a road's field
/// between lattice points falls as 1/r, and the middle of a patch 12 to 36 m from a road reads
/// 1.25 dB loud blended in energy, 0.6 dB in the logarithm.
pub(crate) fn blend(around: [Painted; 4], fx: f64, fy: f64) -> Painted {
    let of = |value: &dyn Fn(&Painted) -> f64, floor: f64| {
        let level = bilinear(around.map(|paint| (value(&paint) + floor).ln()), fx, fy);
        (level.exp() - floor).max(0.0)
    };
    (
        std::array::from_fn(|layer| of(&|paint| paint.0[layer], ENERGY_FLOOR)),
        std::array::from_fn(|layer| of(&|paint| paint.1[layer], SONE_FLOOR)),
    )
}

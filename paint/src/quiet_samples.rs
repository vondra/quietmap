//! A block's lattice of exact samples: which patches blend from it, which carry its loudness, and each pixel's paint from it.
//!
//! A patch's pixels take the lattice's blend only after the patch's probe agreed with it as far as
//! a visitor sees ([`crate::visible`]); a refused patch keeps its exact lattice and probe values,
//! and where its probe confirms it, its other pixels take their loudness from the lattice and their
//! own energies ([`crate::lattice_blend::carried_loudness`]). An empirical safeguard verified
//! against the independent etalon, not a bound on unsampled rays.

use crate::exact::LAYERS;
use crate::lattice_blend::{blend, carried_loudness, within_lattice_range};
use crate::levels::Painted;
use crate::paint::corner_weights;
use crate::visible::{loudness_visibly_agrees, visibly_agrees};

/// A block's samples: its lattice axis, its 41 values (`sample_positions`; `None` in a building or
/// not painted), and its 16 patches accepted for blending.
pub(crate) struct QuietSamples {
    pub axis: [usize; 5],
    pub values: Vec<Option<Painted>>,
    pub accepted: [bool; 16],
    /// The patches not accepted whose pixels take their loudness from the lattice and their own
    /// energies ([`QuietSamples::loudness_from`]), and each layer's and the total's slope for it.
    pub carried: [bool; 16],
    pub slopes: [f64; LAYERS + 1],
}

impl QuietSamples {
    fn patch_at(&self, x: usize, y: usize) -> [usize; 2] {
        let interval = |at: usize| {
            self.axis
                .partition_point(|&sample| sample <= at)
                .saturating_sub(1)
                .min(3)
        };
        [interval(x), interval(y)]
    }

    pub fn at(&self, x: usize, y: usize) -> Painted {
        let [i, j] = self.patch_at(x, y);
        let fraction =
            |at, low| (at - self.axis[low]) as f64 / (self.axis[low + 1] - self.axis[low]) as f64;
        let (fx, fy) = (fraction(x, i), fraction(y, j));
        blend(
            lattice_corners([i, j]).map(|at| self.values[at].unwrap()),
            fx,
            fy,
        )
    }

    /// The loudness of the pixel (x, y) from its `energies` (each layer's Lden energy) where its
    /// patch carries it and they lie within its lattice points' range: the patch's outdoor lattice
    /// points' loudness blended, each first taken to the pixel's energy along the block's slope
    /// (loudness against energy, both in the logarithm).
    pub fn loudness_from(
        &self,
        x: usize,
        y: usize,
        energies: &[f64; LAYERS],
    ) -> Option<[f64; LAYERS + 1]> {
        let [i, j] = self.patch_at(x, y);
        let around = lattice_corners([i, j]).map(|at| self.values[at]);
        if !self.carried[j * 4 + i] || !within_lattice_range(&around, energies) {
            return None;
        }
        self.carried_at([i, j], (x, y), energies)
    }

    /// The loudness `energies` carry at (x, y) of patch (i, j) from its outdoor lattice points
    /// ([`carried_loudness`]); `None` where all four stand in buildings.
    fn carried_at(
        &self,
        [i, j]: [usize; 2],
        (x, y): (usize, usize),
        energies: &[f64; LAYERS],
    ) -> Option<[f64; LAYERS + 1]> {
        let fraction =
            |at, low| (at - self.axis[low]) as f64 / (self.axis[low + 1] - self.axis[low]) as f64;
        let weights = corner_weights(fraction(x, i), fraction(y, j));
        let around = lattice_corners([i, j]).map(|at| self.values[at]);
        carried_loudness(around, weights, energies, &self.slopes)
    }

    pub fn accepts(&self, x: usize, y: usize) -> bool {
        let [i, j] = self.patch_at(x, y);
        self.accepted[j * 4 + i]
    }

    pub fn complete(&self) -> bool {
        self.accepted.iter().all(|&accepted| accepted)
    }

    /// How many of its patches are blended, and how many of the others carry their loudness.
    pub fn patches(&self) -> (usize, usize) {
        let carried = (self.accepted.iter().zip(&self.carried))
            .filter(|&(&accepted, &carried)| !accepted && carried)
            .count();
        (
            self.accepted.iter().filter(|&&accepted| accepted).count(),
            carried,
        )
    }

    /// Whether (x, y) stands inside patch (i, j), off its lattice lines: a probe on a line (a
    /// one-pixel interval's) is no independent check of the patch.
    fn is_interior(&self, [i, j]: [usize; 2], (x, y): (usize, usize)) -> bool {
        x > self.axis[i] && x < self.axis[i + 1] && y > self.axis[j] && y < self.axis[j + 1]
    }

    /// Whether the patch's probe at (x, y) agrees with the lattice's blend there, as far as a
    /// visitor can see.
    pub fn agrees_with_probe(
        &self,
        patch: [usize; 2],
        at: (usize, usize),
        heard: &Painted,
    ) -> bool {
        let known = (lattice_corners(patch).iter()).all(|&corner| self.values[corner].is_some());
        self.is_interior(patch, at) && known && visibly_agrees(heard, &self.at(at.0, at.1))
    }

    /// Whether the patch's probe at (x, y) hears the loudness its energies carry from the
    /// lattice, as far as a visitor can see.
    pub fn carries_probe(&self, patch: [usize; 2], at: (usize, usize), heard: &Painted) -> bool {
        self.is_interior(patch, at)
            && (self.carried_at(patch, at, &heard.0))
                .is_some_and(|carried| loudness_visibly_agrees(&heard.1, &carried))
    }

    fn sample_index(&self, x: usize, y: usize) -> Option<usize> {
        if let (Some(i), Some(j)) = (
            self.axis.iter().position(|&at| at == x),
            self.axis.iter().position(|&at| at == y),
        ) {
            return Some(j * 5 + i);
        }
        let centres: [usize; 4] = std::array::from_fn(|at| (self.axis[at] + self.axis[at + 1]) / 2);
        if let (Some(i), Some(j)) = (
            centres.iter().position(|&at| at == x),
            centres.iter().position(|&at| at == y),
        ) {
            return Some(25 + j * 4 + i);
        }
        None
    }

    pub fn has_paint(&self, x: usize, y: usize) -> bool {
        self.sample_index(x, y)
            .map_or_else(|| self.accepts(x, y), |at| self.values[at].is_some())
    }

    /// The paint of the pixel (x, y) where the block knows it: a lattice point's own; a probe's
    /// as its patch paints the pixels around it (the blend where the patch is accepted, its own
    /// energies with the carried loudness where the patch carries, else its own); another pixel's
    /// blend in an accepted patch. A probe that kept its own value in an accepted patch stood
    /// alone among its neighbours by half a phon at 2.3 % of the probes on even ground, seven
    /// times as often as other pixels (the world sample, 2026-11-12).
    pub fn known_at(&self, x: usize, y: usize) -> Option<Painted> {
        match self.sample_index(x, y) {
            Some(at) if at < 25 => self.values[at],
            Some(at) => {
                let (energies, ndens) = self.values[at]?;
                Some(match self.accepts(x, y) {
                    true => self.at(x, y),
                    false => (
                        energies,
                        self.loudness_from(x, y, &energies).unwrap_or(ndens),
                    ),
                })
            }
            None => self.accepts(x, y).then(|| self.at(x, y)),
        }
    }
}

fn lattice_corners([i, j]: [usize; 2]) -> [usize; 4] {
    let first = j * 5 + i;
    [first, first + 1, first + 5, first + 6]
}

/// A block of `b` pixels' sample axis (five lattice lines) and its 41 sample positions: the 25
/// lattice points row by row, then the 16 patches' probes.
pub(crate) fn sample_positions(b: usize) -> ([usize; 5], Vec<(usize, usize)>) {
    let axis = [0, b / 4, b / 2, 3 * b / 4, b - 1];
    let centres: [usize; 4] = std::array::from_fn(|at| (axis[at] + axis[at + 1]) / 2);
    let positions: Vec<_> = axis
        .into_iter()
        .flat_map(|y| axis.into_iter().map(move |x| (x, y)))
        .chain(
            centres
                .into_iter()
                .flat_map(|y| centres.into_iter().map(move |x| (x, y))),
        )
        .collect();
    (axis, positions)
}

#[cfg(test)]
#[path = "quiet_samples_tests.rs"]
mod tests;

//! A block's pixels painted without a pass: its exact samples and the pixels blended between them, and which refused patches carry its lattice's loudness.
//!
//! A pixel takes a lattice's blend ([`crate::lattice_blend::blend`]) only after the probes of its
//! block or its patch agreed with that blend as far as a visitor sees ([`crate::visible`]), where
//! no screening edge crosses; a refused patch keeps its exact lattice values, and where its probe
//! confirms it, its other pixels take their loudness from the lattice and their own energies
//! ([`crate::lattice_blend::carried_loudness`]). A probe pixel paints as its patch does.
//! An empirical safeguard verified against the independent etalon, not a bound on unsampled rays.

use crate::exact::LAYERS;
use crate::lattice_blend::{
    Anchor, Heard, blend, carried_loudness, log_energies, loudness_slopes, within_lattice_range,
};
use crate::levels::Painted;
use crate::paint::corner_weights;
use crate::visible::{Loudest, loudness_visibly_agrees, visibly_agrees};

/// Exact points on lattice lines across a block, row-major (`None`: in a building or not painted),
/// with the slopes its pixels' loudness is carried along. The lines are in pixels along either
/// axis, a pixel's centre at its number: the block's corners stand half a pixel before its first
/// pixel and after its last.
pub(crate) struct Lattice {
    lines: Vec<f64>,
    heard: Vec<Option<Heard>>,
    slopes: [f64; LAYERS + 1],
}

impl Lattice {
    pub fn new(lines: &[f64], anchors: &[Option<Anchor>]) -> Self {
        let heard: Vec<Option<Heard>> = (anchors.iter())
            .map(|anchor| anchor.as_ref().map(Heard::of))
            .collect();
        Lattice {
            lines: lines.to_vec(),
            slopes: loudness_slopes(&heard),
            heard,
        }
    }

    /// The interval of the lines holding pixel `at` and where the pixel stands in it.
    fn interval(&self, at: usize) -> (usize, f64) {
        let lines = &self.lines;
        let interval = (lines.partition_point(|&line| line <= at as f64))
            .saturating_sub(1)
            .min(lines.len() - 2);
        let width = lines[interval + 1] - lines[interval];
        (interval, (at as f64 - lines[interval]) / width)
    }

    /// The four points around pixel (x, y) and the pixel's weights among them.
    fn around(&self, x: usize, y: usize) -> ([Option<&Heard>; 4], [f64; 4]) {
        let ((i, fx), (j, fy)) = (self.interval(x), self.interval(y));
        let side = self.lines.len();
        let first = j * side + i;
        let around = [first, first + 1, first + side, first + side + 1];
        (
            around.map(|at| self.heard[at].as_ref()),
            corner_weights(fx, fy),
        )
    }

    /// The paint the lattice blends at pixel (x, y), `free` the block's own sources' free field
    /// there; `None` where a point around it is missing or its energies lie beyond theirs.
    pub fn blend_at(&self, x: usize, y: usize, free: &[f64; LAYERS]) -> Option<Painted> {
        let ([a, b, c, d], weights) = self.around(x, y);
        blend([a?, b?, c?, d?], weights, free, &self.slopes)
    }

    /// The loudest among the points around pixel (x, y).
    fn loudest_around(&self, x: usize, y: usize) -> Loudest {
        let (around, _) = self.around(x, y);
        Loudest::of(around.iter().flatten().map(|point| &point.paint))
    }

    /// Whether `probe`, exact at pixel (x, y), agrees with the lattice's blend there as far as a
    /// visitor can see of the loudest point around it.
    pub fn agrees_with(&self, x: usize, y: usize, probe: &Anchor) -> bool {
        (self.blend_at(x, y, &probe.free)).is_some_and(|blended| {
            visibly_agrees(&probe.paint, &blended, &self.loudest_around(x, y))
        })
    }

    /// The loudness `energies` carry at pixel (x, y) from the outdoor points around it
    /// ([`carried_loudness`]): `None` where all four stand in buildings, or with `within` where
    /// the energies lie beyond the points' ([`within_lattice_range`]).
    fn carried_at(
        &self,
        (x, y): (usize, usize),
        energies: &[f64; LAYERS],
        within: bool,
    ) -> Option<[f64; LAYERS + 1]> {
        let (around, weights) = self.around(x, y);
        let here = log_energies(energies);
        if within && !within_lattice_range(around, &here) {
            return None;
        }
        carried_loudness(around, weights, &here, &self.slopes)
    }
}

/// A block's own sources' free field at its pixel (x, y), where its ground can be read.
pub(crate) type OwnFreeField<'a> = dyn Fn(usize, usize) -> Option<[f64; LAYERS]> + 'a;

/// A block's pixels painted without a pass, row-major (`None`: a pass paints it, or it stands in
/// a building); its fine lattice where some refused patch carries its loudness, with those
/// patches; and how many of its sixteen patches blend.
pub(crate) struct QuietSamples {
    side: usize,
    pixels: Vec<Option<Painted>>,
    lattice: Option<Lattice>,
    carried: [bool; 16],
    blended: usize,
}

impl QuietSamples {
    /// A block of `side` pixels every pixel of which `lattice` blends (its corners, or its coarse
    /// lattice, whose points are their own blend), `free(x, y)` the block's own sources' free
    /// field at a pixel: a pixel without one or beyond the lattice points' range is left to a pass.
    pub fn blended_whole(side: usize, lattice: &Lattice, free: &OwnFreeField) -> Self {
        let pixels = (0..side * side)
            .map(|at| lattice.blend_at(at % side, at / side, &free(at % side, at / side)?))
            .collect();
        QuietSamples {
            side,
            pixels,
            lattice: None,
            carried: [false; 16],
            blended: 16,
        }
    }

    /// A block of `side` pixels settled patch by patch from its 41 `samples`
    /// ([`sample_positions`]). A patch whose probe agrees with the lattice's blend and which no
    /// screening edge crosses (`edge_free` of its number) blends its pixels; any other keeps its
    /// samples, its probe painted as its pixels will be: with the lattice's loudness carried to
    /// its own energies where the probe confirms that carry. A probe that kept its own value in a
    /// blended patch stood alone among its neighbours by half a phon at 2.3 % of the probes on
    /// even ground, seven times as often as other pixels (the world sample, 2026-11-12).
    pub fn patched(
        side: usize,
        samples: &[Option<Anchor>],
        edge_free: &dyn Fn(usize) -> bool,
        free: &OwnFreeField,
    ) -> Self {
        let (axis, positions) = sample_positions(side);
        let lattice = Lattice::new(&axis.map(|at| at as f64), &samples[..25]);
        let mut pixels = vec![None; side * side];
        for (&(x, y), anchor) in positions.iter().zip(samples) {
            pixels[y * side + x] = anchor.map(|anchor| anchor.paint);
        }
        let (mut carried, mut blended) = ([false; 16], 0);
        for (cell, &(x, y)) in positions[25..].iter().enumerate() {
            let (i, j) = (cell % 4, cell / 4);
            let Some(probe) = samples[25 + cell] else {
                continue;
            };
            // A probe on a lattice line (a one-pixel interval's) is no independent check.
            if !(x > axis[i] && x < axis[i + 1] && y > axis[j] && y < axis[j + 1]) {
                continue;
            }
            if lattice.agrees_with(x, y, &probe) && edge_free(cell) {
                blended += 1;
                let within = |low: usize, high: usize, last: bool| low..high + usize::from(last);
                for py in within(axis[j], axis[j + 1], j == 3) {
                    for px in within(axis[i], axis[i + 1], i == 3) {
                        if !(axis.contains(&px) && axis.contains(&py)) {
                            pixels[py * side + px] =
                                free(px, py).and_then(|free| lattice.blend_at(px, py, &free));
                        }
                    }
                }
                continue;
            }
            let (energies, ndens) = probe.paint;
            carried[cell] = (lattice.carried_at((x, y), &energies, false)).is_some_and(|carried| {
                loudness_visibly_agrees(&ndens, &carried, &lattice.loudest_around(x, y).ndens)
            });
            if carried[cell] {
                let carried = lattice.carried_at((x, y), &energies, true);
                pixels[y * side + x] = Some((energies, carried.unwrap_or(ndens)));
            }
        }
        let keeps_lattice = carried.iter().any(|&carried| carried);
        QuietSamples {
            side,
            pixels,
            lattice: keeps_lattice.then_some(lattice),
            carried,
            blended,
        }
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
        let lattice = self.lattice.as_ref()?;
        let (patch_x, patch_y) = (lattice.interval(x).0, lattice.interval(y).0);
        if !self.carried[patch_y * 4 + patch_x] {
            return None;
        }
        lattice.carried_at((x, y), energies, true)
    }

    /// The paint of the pixel (x, y) where the block knows it without a pass.
    pub fn known_at(&self, x: usize, y: usize) -> Option<Painted> {
        self.pixels[y * self.side + x]
    }

    /// Whether every pixel of the block is painted.
    pub fn complete(&self) -> bool {
        self.pixels.iter().all(Option::is_some)
    }

    /// Whether any pixel of the block is painted.
    pub fn paints_a_pixel(&self) -> bool {
        self.pixels.iter().any(Option::is_some)
    }

    /// How many of its patches are blended, and how many of the others carry their loudness.
    pub fn patches(&self) -> (usize, usize) {
        let carried = self.carried.iter().filter(|&&carried| carried).count();
        (self.blended, carried)
    }
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

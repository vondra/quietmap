//! Per-tile A-weighted linear energy accumulator.
//!
//! One tile = `TILE_PX × TILE_PX × 3 periods` cells of f32 linear
//! energy. Sources add into it via [`Self::add_energy_at`]. The three
//! periods collapse to one Lden byte per cell at write time via
//! [`crate::wire_hm3::collapse_lden_u8`] which reads the `energy`
//! field directly.

use crate::grid::TILE_PX;

pub const NUM_PERIODS: usize = 3;

pub struct TileAccumulator {
    /// Row-major: `[py][px][period]`. Linear A-weighted energy, summed
    /// across sources. Keep as `Vec<f32>` (not `[[f32; 3]; TILE_PX^2]`)
    /// so `Vec::new()` doesn't blow the stack on creation.
    pub energy: Vec<f32>,
}

impl TileAccumulator {
    pub fn new() -> Self {
        Self {
            energy: vec![0.0; TILE_PX * TILE_PX * NUM_PERIODS],
        }
    }

    #[inline]
    pub fn add_energy_at(&mut self, py: u32, px: u32, period: u8, e: f32) {
        let idx = (py as usize * TILE_PX + px as usize) * NUM_PERIODS + period as usize;
        self.energy[idx] += e;
    }

    /// Add `other`'s energy element-wise into `self`. Used by the
    /// rayon `fold + reduce` parallelisation — per-thread accumulators
    /// merge into one before collapse. Energy sums commute in the
    /// linear domain, so order of merge is irrelevant.
    pub fn merge_from(&mut self, other: &TileAccumulator) {
        debug_assert_eq!(self.energy.len(), other.energy.len());
        for (dst, src) in self.energy.iter_mut().zip(other.energy.iter()) {
            *dst += *src;
        }
    }
}

impl Default for TileAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// One coarse receiver lattice (`n × n` nodes spanning the tile) for
/// far-field airborne segments. Receiver terrain and building screening are
/// sampled at these same nodes; the resulting screened energy field is then
/// bilinearly expanded instead of evaluating all 512² receivers. Summation
/// commutes with linear interpolation, so expanding the per-thread sum once
/// equals expanding every segment.
///
/// `n` is kept runtime (not a const generic) so the [`CoarseLevels`] ladder
/// is a plain array and the scatter hot path picks a level from a runtime
/// `best_slant` band without monomorphised fan-out. The bilinear error of a
/// node cell is `≈ 8.69·(cell/2)²/slant²` dB, so a coarser `n` (bigger
/// cell) stays inside the same error budget at proportionally larger slant
/// — the basis for the distance-adaptive ladder.
pub struct CoarseLattice {
    n: usize,
    /// Row-major `[ci][cj][period]` linear energy.
    energy: Vec<f32>,
}

impl CoarseLattice {
    /// `n` nodes per side (`n ≥ 2`; n=2 = whole-tile corners). A real
    /// `assert!` (not `debug_assert!`) because this is a runtime-sized
    /// public type and an `n < 2` would index out of bounds in `expand_into`.
    pub fn new(n: usize) -> Self {
        assert!(n >= 2, "CoarseLattice needs n >= 2 nodes per side, got {n}");
        Self {
            n,
            energy: vec![0.0; n * n * NUM_PERIODS],
        }
    }

    /// Wrap a raw `[ci][cj][period]` energy buffer (e.g. produced by the GPU
    /// `airborne_coarse` kernel) as a lattice so it can `expand_into` the fine
    /// grid with the same bilinear path the CPU far scatter uses.
    pub fn from_energy(n: usize, energy: Vec<f32>) -> Self {
        assert!(n >= 2, "CoarseLattice needs n >= 2 nodes per side, got {n}");
        assert_eq!(
            energy.len(),
            n * n * NUM_PERIODS,
            "energy len must be n*n*3"
        );
        Self { n, energy }
    }

    /// Nodes per side — the scatter loop bound (the lattice `n`).
    #[inline]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Fine pixel index sampled by coarse node `i` (rounded so the
    /// endpoints land exactly on pixel 0 and `TILE_PX-1`).
    #[inline]
    pub fn coarse_pixel(&self, i: usize) -> usize {
        (i * (TILE_PX - 1) + (self.n - 1) / 2) / (self.n - 1)
    }

    #[inline]
    pub fn add_energy_at(&mut self, ci: usize, cj: usize, period: u8, e: f32) {
        let idx = (ci * self.n + cj) * NUM_PERIODS + period as usize;
        self.energy[idx] += e;
    }

    pub fn merge_from(&mut self, other: &CoarseLattice) {
        debug_assert_eq!(self.n, other.n);
        debug_assert_eq!(self.energy.len(), other.energy.len());
        for (dst, src) in self.energy.iter_mut().zip(other.energy.iter()) {
            *dst += *src;
        }
    }

    /// Bilinearly upsample this lattice and add it into `fine`. Done once
    /// per tile after the per-thread lattices are reduced.
    pub fn expand_into(&self, fine: &mut TileAccumulator) {
        let n = self.n;
        // Per-fine-pixel bracketing coarse index + interpolation weight,
        // precomputed once per axis (same mapping for rows and columns).
        let mut coarse_lo = [0usize; TILE_PX];
        let mut frac = [0.0f32; TILE_PX];
        for (p, (lo_p, frac_p)) in coarse_lo.iter_mut().zip(frac.iter_mut()).enumerate() {
            let mut ci = (p * (n - 1)) / (TILE_PX - 1);
            if ci > n - 2 {
                ci = n - 2;
            }
            // Rounding in `coarse_pixel` can place p just outside
            // [pos(ci), pos(ci+1)]; nudge to the bracketing interval.
            while ci > 0 && p < self.coarse_pixel(ci) {
                ci -= 1;
            }
            while ci < n - 2 && p >= self.coarse_pixel(ci + 1) {
                ci += 1;
            }
            let p_lo = self.coarse_pixel(ci);
            let p_hi = self.coarse_pixel(ci + 1);
            *lo_p = ci;
            *frac_p = if p_hi > p_lo {
                (p - p_lo) as f32 / (p_hi - p_lo) as f32
            } else {
                0.0
            };
        }

        for py in 0..TILE_PX {
            let ciy = coarse_lo[py];
            let fy = frac[py];
            let r0 = ciy * n;
            let r1 = (ciy + 1) * n;
            for px in 0..TILE_PX {
                let cix = coarse_lo[px];
                let fx = frac[px];
                let i00 = (r0 + cix) * NUM_PERIODS;
                let i01 = (r0 + cix + 1) * NUM_PERIODS;
                let i10 = (r1 + cix) * NUM_PERIODS;
                let i11 = (r1 + cix + 1) * NUM_PERIODS;
                let dst = (py * TILE_PX + px) * NUM_PERIODS;
                for k in 0..NUM_PERIODS {
                    let top =
                        self.energy[i00 + k] + (self.energy[i01 + k] - self.energy[i00 + k]) * fx;
                    let bot =
                        self.energy[i10 + k] + (self.energy[i11 + k] - self.energy[i10 + k]) * fx;
                    fine.energy[dst + k] += top + (bot - top) * fy;
                }
            }
        }
    }
}

/// Far-field stride ladder, coarsest band last. Index = the `best_slant`
/// routing band: 0 ⇒ <2 km (stride 8 px), 1 ⇒ 2–8 km (stride 32 px),
/// 2 ⇒ >8 km (stride 128 px). The near (exact per-pixel) path is NOT a
/// level here — it bypasses the lattices entirely. Single source of truth
/// for the lattice sizes; the matching slant band edges live next to the
/// airborne kernel's `NEAR_SLANT_M`.
/// 512@z12 shift: node counts doubled from [33, 9, 3] so the pixel
/// strides — and with them the PHYSICAL node spacing and the bilinear error
/// budget per band — stay exactly as validated (docs/validation).
pub const COARSE_LEVELS_N: [usize; 3] = [65, 17, 5];

/// The set of coarse lattices, one per [`COARSE_LEVELS_N`] band, carried by
/// the airborne rayon fold/reduce beside the fine [`TileAccumulator`]. Each
/// far segment scatters onto exactly ONE level (chosen by `best_slant`);
/// after the reduce all levels expand-and-sum into the fine grid. Linear
/// energy is additive and bilinear interpolation commutes with summation,
/// so summing the per-level expansions equals expanding every segment on
/// its own lattice — no double counting.
pub struct CoarseLevels([CoarseLattice; 3]);

impl CoarseLevels {
    pub fn new() -> Self {
        Self(COARSE_LEVELS_N.map(CoarseLattice::new))
    }

    #[inline]
    pub fn add_energy_at(&mut self, level: usize, ci: usize, cj: usize, period: u8, e: f32) {
        self.0[level].add_energy_at(ci, cj, period, e);
    }

    /// Nodes per side of `level` — the scatter loop bound.
    #[inline]
    pub fn n(&self, level: usize) -> usize {
        self.0[level].n
    }

    /// Fine pixel sampled by node `i` of `level`.
    #[inline]
    pub fn coarse_pixel(&self, level: usize, i: usize) -> usize {
        self.0[level].coarse_pixel(i)
    }

    pub fn merge_from(&mut self, other: &CoarseLevels) {
        for (a, b) in self.0.iter_mut().zip(other.0.iter()) {
            a.merge_from(b);
        }
    }

    /// Expand every level into `fine` and sum. One pass per level (3×),
    /// once per tile — negligible beside the per-segment scatter.
    pub fn expand_into(&self, fine: &mut TileAccumulator) {
        for lattice in &self.0 {
            lattice.expand_into(fine);
        }
    }
}

impl Default for CoarseLevels {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The historical 256-pixel ladder plus the degenerate-but-valid n=2
    /// (whole-tile corners) — exercised so the type stays future-safe.
    const NS: [usize; 4] = [33, 9, 3, 2];

    #[test]
    fn coarse_pixel_endpoints_land_on_corners() {
        for &n in &NS {
            let lat = CoarseLattice::new(n);
            assert_eq!(lat.coarse_pixel(0), 0, "n={n}");
            assert_eq!(lat.coarse_pixel(n - 1), TILE_PX - 1, "n={n}");
        }
    }

    #[test]
    fn coarse_pixel_monotonic_nondecreasing() {
        for &n in &NS {
            let lat = CoarseLattice::new(n);
            for i in 0..n - 1 {
                assert!(
                    lat.coarse_pixel(i) <= lat.coarse_pixel(i + 1),
                    "n={n} i={i}: {} > {}",
                    lat.coarse_pixel(i),
                    lat.coarse_pixel(i + 1)
                );
            }
        }
    }

    #[test]
    fn expand_bracketing_is_in_bounds_for_all_n() {
        // Replays expand_into's per-axis bracketing and asserts every fine
        // pixel maps to a valid [ci, ci+1] interval with frac in [0,1] and
        // no out-of-range node — the small-N safety proof, encoded.
        for &n in &NS {
            let lat = CoarseLattice::new(n);
            for p in 0..TILE_PX {
                let mut ci = (p * (n - 1)) / (TILE_PX - 1);
                if ci > n - 2 {
                    ci = n - 2;
                }
                while ci > 0 && p < lat.coarse_pixel(ci) {
                    ci -= 1;
                }
                while ci < n - 2 && p >= lat.coarse_pixel(ci + 1) {
                    ci += 1;
                }
                assert!(ci <= n - 2, "n={n} p={p}: ci {ci} > n-2");
                let lo = lat.coarse_pixel(ci);
                let hi = lat.coarse_pixel(ci + 1);
                assert!(
                    lo <= p && p <= hi,
                    "n={n} p={p} not bracketed by [{lo},{hi}]"
                );
                // expand_into indexes node (ci+1); must stay < n.
                assert!(ci + 1 < n, "n={n} p={p}: ci+1 {} out of range", ci + 1);
            }
        }
    }

    #[test]
    fn expand_constant_field_is_exact() {
        // Bilinear of a constant is the constant — catches weight bugs at
        // small N.
        for &n in &NS {
            let mut lat = CoarseLattice::new(n);
            for ci in 0..n {
                for cj in 0..n {
                    lat.add_energy_at(ci, cj, 1, 4.0);
                }
            }
            let mut fine = TileAccumulator::new();
            lat.expand_into(&mut fine);
            for py in 0..TILE_PX {
                for px in 0..TILE_PX {
                    let v = fine.energy[(py * TILE_PX + px) * NUM_PERIODS + 1];
                    assert!((v - 4.0).abs() < 1e-4, "n={n} ({py},{px}) = {v}");
                }
            }
        }
    }

    #[test]
    fn expand_affine_field_is_exact() {
        // Bilinear interpolation reproduces an affine field exactly, even
        // with the uneven node spacing `coarse_pixel` rounding produces.
        // The decisive small-N correctness guard (run at N=3 and N=9).
        let f = |fx: f64, fy: f64| 1.0 + 0.01 * fx + 0.02 * fy;
        for &n in &NS {
            let mut lat = CoarseLattice::new(n);
            for ci in 0..n {
                for cj in 0..n {
                    let py = lat.coarse_pixel(ci) as f64;
                    let px = lat.coarse_pixel(cj) as f64;
                    lat.add_energy_at(ci, cj, 0, f(px, py) as f32);
                }
            }
            let mut fine = TileAccumulator::new();
            lat.expand_into(&mut fine);
            for py in 0..TILE_PX {
                for px in 0..TILE_PX {
                    let got = fine.energy[(py * TILE_PX + px) * NUM_PERIODS];
                    let want = f(px as f64, py as f64) as f32;
                    assert!(
                        (got - want).abs() < 1e-2,
                        "n={n} ({py},{px}) got {got} want {want}"
                    );
                }
            }
        }
    }

    #[test]
    fn levels_expand_and_sum_independently() {
        // Each level holds a different constant; the fine grid must be their
        // sum everywhere — proves per-level expand-and-sum is additive.
        let mut levels = CoarseLevels::new();
        for (lvl, val) in [(0usize, 1.0f32), (1, 2.0), (2, 4.0)] {
            let n = levels.n(lvl);
            for ci in 0..n {
                for cj in 0..n {
                    levels.add_energy_at(lvl, ci, cj, 2, val);
                }
            }
        }
        let mut fine = TileAccumulator::new();
        levels.expand_into(&mut fine);
        for py in 0..TILE_PX {
            for px in 0..TILE_PX {
                let v = fine.energy[(py * TILE_PX + px) * NUM_PERIODS + 2];
                assert!((v - 7.0).abs() < 1e-3, "({py},{px}) = {v}");
            }
        }
    }
}

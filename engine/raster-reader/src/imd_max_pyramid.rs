//! Max-pooled imperviousness pyramid over a [`FusedGrid`]'s IMD plane — sound
//! per-chunk IMD maxima for the scatter byte-stop's ground bound (M3a) without
//! marching the ray.
//!
//! Level 0 is the grid's own `imd` plane; level L pools the max over each
//! `2^L × 2^L` block. A query for a (fractional, clamped) raster-coordinate box
//! returns the max over all cells whose bilinear quads the box touches — the
//! queried value is an UPPER bound on every `lookup_fused_rc` IMD sample taken
//! inside the box, which is the soundness direction the bound needs: a HIGHER
//! IMD bound is a LOWER ground-factor bound, and the byte-stop bound may only
//! ever over-read energy.

use crate::fused_grid::FusedPixel;

/// Pyramid over one `rows × cols` u8 plane. Levels shrink by 2× per axis; the
/// top level is 1×1 (or 2×1 / 1×2 when a dimension is not a power of two —
/// odd edges pool over the cells that exist, which preserves the containment
/// property "a level-L cell's footprint ⊇ every contained level-(L−1) cell's").
#[derive(Clone)]
pub struct ImdMaxPyramid {
    levels: Vec<Vec<u8>>,
    cols: Vec<usize>,
}

impl ImdMaxPyramid {
    /// Build from a grid's pixel plane. Cost is one pass per level (~⅓ of the
    /// plane's bytes total) at halo-build time, amortized over every pair the
    /// tile prices.
    pub fn from_imd_plane(data: &[FusedPixel], rows: usize, cols: usize) -> Self {
        let mut levels = vec![data.iter().map(|p| p.imd).collect::<Vec<u8>>()];
        let mut cols = vec![cols];
        let mut rows_stack = vec![rows];
        while cols[cols.len() - 1] > 1 || rows_stack[rows_stack.len() - 1] > 1 {
            let prev = &levels[levels.len() - 1];
            let (pr, pc) = (rows_stack[rows_stack.len() - 1], cols[cols.len() - 1]);
            let (cr, cc) = (pr.div_ceil(2), pc.div_ceil(2));
            let mut cur = vec![0u8; cr * cc];
            for r in 0..cr {
                for c in 0..cc {
                    let mut m = 0u8;
                    for dr in 0..2 {
                        for dc in 0..2 {
                            let (rr, ccx) = (r * 2 + dr, c * 2 + dc);
                            if rr < pr && ccx < pc {
                                m = m.max(prev[rr * pc + ccx]);
                            }
                        }
                    }
                    cur[r * cc + c] = m;
                }
            }
            levels.push(cur);
            cols.push(cc);
            rows_stack.push(cr);
        }
        Self { levels, cols }
    }

    /// Max over the cell box `[r_lo..=r_hi] × [c_lo..=c_hi]` (inclusive cell
    /// indices, assumed pre-clamped to the plane). EXACT: the box is tiled by
    /// aligned dyadic blocks (overlapping allowed — `max` does not double
    /// count), each read as one or a few cells at its matching pyramid level.
    pub fn max_over_cell_box(&self, r_lo: usize, r_hi: usize, c_lo: usize, c_hi: usize) -> u8 {
        debug_assert!(r_lo <= r_hi && c_lo <= c_hi);
        let mut m = 0u8;
        for (ra, rk) in Self::dyadic_intervals(r_lo, r_hi) {
            for (ca, ck) in Self::dyadic_intervals(c_lo, c_hi) {
                // The row interval is `2^rk` wide and `ra`-aligned; the col one
                // likewise. At level L = min(rk, ck) both are axis-aligned
                // runs of one or more cells — read them all.
                let l = (rk.min(ck)) as usize;
                let lc = self.cols[l];
                let r0 = ra >> l;
                let r1 = (ra + (1 << rk) - 1) >> l;
                let c0 = ca >> l;
                let c1 = (ca + (1 << ck) - 1) >> l;
                let plane = &self.levels[l];
                for r in r0..=r1 {
                    let row = &plane[r * lc..(r + 1) * lc];
                    m = m.max(*row[c0..=c1].iter().max().unwrap());
                }
            }
        }
        m
    }

    /// Decompose `[lo..=hi]` into ≤ 2·log₂(span) aligned dyadic intervals
    /// `(start, k)` — each covers `[start .. start + 2^k − 1]` with `start` a
    /// multiple of `2^k`, which is exactly the footprint of one (or a run of)
    /// pyramid level-`k` cells.
    fn dyadic_intervals(lo: usize, hi: usize) -> impl Iterator<Item = (usize, u32)> {
        let mut out: [(usize, u32); 64] = [(0, 0); 64];
        let mut n = 0usize;
        let mut lo = lo;
        while lo <= hi {
            // Largest 2^k that starts at lo (alignment) and stays inside hi.
            let align = lo.trailing_zeros();
            let room = (hi - lo + 1).ilog2();
            let k = align.min(room);
            out[n] = (lo, k);
            n += 1;
            lo += 1usize << k;
        }
        out.into_iter().take(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic LCG so the brute-force check needs no `rand` dependency.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    fn grid(rows: usize, cols: usize, seed: u64) -> (Vec<FusedPixel>, Lcg) {
        let mut rng = Lcg(seed);
        let mut data = Vec::with_capacity(rows * cols);
        for _ in 0..rows * cols {
            let imd = (rng.next() % 101) as u8;
            data.push(FusedPixel {
                elevation: 0.0,
                building: 0,
                forest: 0,
                imd,
                _pad: 0,
            });
        }
        (data, rng)
    }

    /// The whole point of the structure: the pyramid's answer equals the
    /// brute-force cell max for EVERY box, at every size and offset — a wrong
    /// level choice or an off-by-one in the covering set is an unsound bound.
    #[test]
    fn every_box_matches_the_brute_force_max() {
        for &(rows, cols) in &[(1usize, 1usize), (2, 2), (7, 5), (64, 64), (100, 37)] {
            let (data, mut rng) = grid(rows, cols, rows as u64 * 1000 + cols as u64);
            let py = ImdMaxPyramid::from_imd_plane(&data, rows, cols);
            for _ in 0..400 {
                let r_lo = (rng.next() % rows as u64) as usize;
                let c_lo = (rng.next() % cols as u64) as usize;
                let r_hi = r_lo + (rng.next() % (rows - r_lo) as u64) as usize;
                let c_hi = c_lo + (rng.next() % (cols - c_lo) as u64) as usize;
                let mut brute = 0u8;
                for r in r_lo..=r_hi {
                    for c in c_lo..=c_hi {
                        brute = brute.max(data[r * cols + c].imd);
                    }
                }
                assert_eq!(
                    py.max_over_cell_box(r_lo, r_hi, c_lo, c_hi),
                    brute,
                    "box ({r_lo}..={r_hi}, {c_lo}..={c_hi}) of {rows}×{cols}"
                );
            }
        }
    }

    /// The top level is the whole-plane max: a full-extent box reads one cell.
    #[test]
    fn full_extent_box_is_the_global_max() {
        let (data, _) = grid(33, 21, 7);
        let py = ImdMaxPyramid::from_imd_plane(&data, 33, 21);
        let global = data.iter().map(|p| p.imd).max().unwrap();
        assert_eq!(py.max_over_cell_box(0, 32, 0, 20), global);
    }
}

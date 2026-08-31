//! Cruise coarse-field: compute the cruise contribution once on a coarse
//! Mercator grid (default z9, ≈ 98 m/px), then bilinearly upsample into the
//! z12 output tiles.
//!
//! Cruise is a smooth, terrain-screen-free field (the Doc 29 airborne kernel
//! has no diffraction; cruise segments clear every receiver by ≥ 7.6 km), and
//! it is **overhead-bound, not eval-bound**: the per-tile cost is the per-row
//! bucket prefilter + the per-in-reach-bucket terrain sample (5 raster reads),
//! NOT the 9-node far-field lattice evals. Computing it per z12 tile re-runs
//! that fixed overhead for every one of the 64 z12 children that tile a single
//! z9 extent. This module runs it ONCE per z9 tile instead — `64×` fewer
//! tiles, offset by a larger reach cap (the z9 tile's bigger half-diagonal),
//! for a net ~3–4× fewer terrain samples (the dominant cost).
//!
//! **Seam-free by construction.** The z9 receiver grid is a single global
//! Mercator pixel lattice shared by every z12 tile and every region: a z9
//! pixel's energy is a pure function of `(pixel lat/lon/alt, cruise buckets
//! within the 16 km reach)`, and any region whose z12 child brackets that
//! pixel sits within ~26 km of it — well inside its `grid_disk(1)` ring (~68 km
//! coverage), so it has the identical bucket set and computes the identical
//! energy. Two neighbouring z12 tiles (even across a region boundary) sampling
//! the same global z9 pixel therefore read the same value, and the bilinear
//! upsample of a shared continuous grid is continuous — no per-tile plateau,
//! no region-edge seam. (Contrast the historical per-z12 lattice, which only
//! avoided plateaus because adjacent tiles happened to share edge pixels.)
//!
//! Energy lives in the linear domain throughout (raw per-period f32, NOT a
//! collapsed Lden byte): bilinear interpolation commutes with the linear
//! energy-sum, so the coarse field + upsample equals scattering every bucket
//! directly onto the fine grid. The visibility floor is applied only at the
//! z12 collapse (`collapse_lden_u8` in `region_runner`), never at z9 — a z9
//! threshold would re-introduce a coarse on/off boundary after upsample.

use std::collections::HashMap;
use std::f64::consts::PI;

use noise_compute::compute::aircraft_v6::CruiseRowView;
use raster_reader::fused_tile_z13::FusedTileZ13;
use raster_reader::RealRasters;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};
use crate::cruise::{scatter_tile, ScatterStats};
use crate::grid::TILE_PX;

/// Mercator zoom the cruise field is computed at. z12 base − 3 = one z9 tile
/// per 8×8 z12 block; with 512-px tiles the z9 pixel (≈ 98 m at Praha) is the
/// SAME physical grid as the pre-shift z10@256, so cost and quality carry
/// over unchanged: ~8× finer than the historical 3×3-per-base-tile node
/// spacing (~1.57 km), corridors stay visible, per-bucket overhead runs 64×
/// fewer times. One level coarser would halve cost again at ~0.3 dB upsample
/// error near low-cruise descents — re-measure first.
pub const CRUISE_COMPUTE_ZOOM: u8 = 9;

/// Local-pixel decomposition of a global pixel index: `g >> TILE_SHIFT` is
/// the tile, `g & (TILE_PX-1)` the pixel — derived from TILE_PX so the tile
/// side can never drift apart from the shift/mask pair again.
const TILE_SHIFT: u32 = (TILE_PX as u32).trailing_zeros();

/// One z9 coarse-field tile: raw per-period linear energy, row-major
/// `[py][px][period]` (same layout as [`TileAccumulator::energy`]).
type Z9Energy = Box<[f32]>;

/// The region's computed z9 cruise field, keyed by z9 tile `(x, y)`.
/// Built lazily: a z9 tile is computed the first time any z12 child brackets
/// it (which pulls in the E/S/SE halo neighbours automatically).
pub struct CruiseField<'a> {
    grid: HashMap<(u32, u32), Z9Energy>,
    rasters: &'a RealRasters,
    /// Borrowed for the build's lifetime; the region's `grid_disk(1)` cruise ring.
    cruise: &'a [CruiseRowView<'a>],
    /// Row terrain sampled once per region build — a row's segment geometry is
    /// tile-independent, but its ~25 km reach spans several z9 tiles, each of
    /// which used to re-sample the same five raster points.
    row_terrain: Vec<Option<noise_compute::emission::aircraft::SegmentTerrain>>,
    pub stats: ScatterStats,
}

impl<'a> CruiseField<'a> {
    pub fn new(rasters: &'a RealRasters, cruise: &'a [CruiseRowView<'a>]) -> Self {
        Self {
            grid: HashMap::new(),
            rasters,
            cruise,
            row_terrain: crate::cruise::precompute_row_terrain(cruise, rasters),
            stats: ScatterStats::default(),
        }
    }

    /// Compute (or fetch) the z9 energy tile at `(x10, y10)`.
    fn z9_tile(&mut self, x10: u32, y10: u32) -> &Z9Energy {
        if !self.grid.contains_key(&(x10, y10)) {
            // Cruise's synth half-segment reaches ~25 km past the tile and reads
            // the full raster store, so a 0-halo standalone tile is correct (no
            // path-profile ray-march). Build the receiver lattice at z9.
            let tile = FusedTileZ13::build_receiver_altitude_only(
                CRUISE_COMPUTE_ZOOM,
                x10,
                y10,
                self.rasters,
            );
            let mut accum = TileAccumulator::new();
            let s = scatter_tile(&tile, self.cruise, &self.row_terrain, &mut accum);
            self.stats.merge(&s);
            self.grid
                .insert((x10, y10), accum.energy.into_boxed_slice());
        }
        &self.grid[&(x10, y10)]
    }

    /// Bilinearly upsample the z9 cruise field into the z12 tile's accumulator,
    /// reading each z12 receiver pixel's four bracketing z9 pixels (which may
    /// straddle z9-tile boundaries — those neighbour tiles are computed on
    /// demand). Adds into `accum` so airborne can sum into the same tile after.
    pub fn upsample_into(&mut self, tile: &FusedTileZ13, accum: &mut TileAccumulator) {
        // Per-axis, resolve each z12 receiver pixel to the GLOBAL z9 pixels it
        // brackets + the interpolation fraction. The lower bracket can be -1 (just
        // inside a tile's W/N edge); X wraps cyclically (the −180°/+180° seam is
        // the SAME column, so a date-line tile interpolates continuously across
        // it), Y clamps (latitude does not wrap at the poles). A z12 tile (~3 km)
        // spans ≤2 z9 tiles per axis, so the whole tile's receivers touch a ≤2×2
        // block of z9 tiles — `axis_brackets` returns the bracket-per-pixel plus
        // the (≤2) distinct tile ids, indexed by a 0/1 slot in each bracket.
        let (bx, x_tiles) = axis_brackets(&tile.rx_lon, lon_to_global_px, wrap_x);
        let (by, y_tiles) = axis_brackets(&tile.rx_lat, lat_to_global_px, clamp_y);

        // Realise the ≤4 bracketing z9 tiles up front (computing any missing one),
        // then borrow them into a 2×2 slot grid so the bilinear inner loop indexes
        // slices directly — no HashMap probe per pixel. When an axis touches only
        // one z9 tile its slot-1 id is `None`; every bracket on that axis then has
        // slot 0, so the slot-1 cells are never indexed — fill them with the slot-0
        // tile to keep the grid total. `clamp(sx, present_x)` collapses the index.
        for &tx in x_tiles.iter().flatten() {
            for &ty in y_tiles.iter().flatten() {
                self.z9_tile(tx, ty);
            }
        }
        let max_sx = x_tiles[1].is_some() as usize;
        let max_sy = y_tiles[1].is_some() as usize;
        let cell = |sx: usize, sy: usize| -> &Z9Energy {
            let tx = x_tiles[sx.min(max_sx)].expect("slot-0 tile always present");
            let ty = y_tiles[sy.min(max_sy)].expect("slot-0 tile always present");
            &self.grid[&(tx, ty)]
        };
        let tiles = [[cell(0, 0), cell(1, 0)], [cell(0, 1), cell(1, 1)]];

        // Pixel-lattice loops kept as range indices: `py`/`px` index the bracket
        // tables `by`/`bx` AND compute the flat output offset `(py*TILE_PX+px)*…`,
        // so enumerate() would not remove the manual indexing. Bilinear-blend math
        // and its f64 order are byte-exact — left verbatim.
        #[allow(clippy::needless_range_loop)]
        for py in 0..TILE_PX {
            let r = by[py];
            #[allow(clippy::needless_range_loop)]
            for px in 0..TILE_PX {
                let c = bx[px];
                let e00 = read3(tiles[r.lo_slot][c.lo_slot], r.lo_local, c.lo_local);
                let e10 = read3(tiles[r.lo_slot][c.hi_slot], r.lo_local, c.hi_local);
                let e01 = read3(tiles[r.hi_slot][c.lo_slot], r.hi_local, c.lo_local);
                let e11 = read3(tiles[r.hi_slot][c.hi_slot], r.hi_local, c.hi_local);
                let dst = (py * TILE_PX + px) * NUM_PERIODS;
                for k in 0..NUM_PERIODS {
                    let top = e00[k] + (e10[k] - e00[k]) * c.frac;
                    let bot = e01[k] + (e11[k] - e01[k]) * c.frac;
                    accum.energy[dst + k] += top + (bot - top) * r.frac;
                }
            }
        }
    }
}

/// One axis bracket: the lower/upper z9 local-pixel the receiver falls between,
/// each tagged with a 0/1 SLOT into the axis's ≤2 distinct tiles, plus the
/// interpolation fraction toward the upper neighbour.
#[derive(Clone, Copy)]
struct Bracket {
    lo_slot: usize,
    lo_local: usize,
    hi_slot: usize,
    hi_local: usize,
    frac: f32,
}

/// The 3 period energies at local `(ly, lx)` of a z9 energy tile.
#[inline]
fn read3(energy: &Z9Energy, ly: usize, lx: usize) -> [f32; NUM_PERIODS] {
    let base = (ly * TILE_PX + lx) * NUM_PERIODS;
    [energy[base], energy[base + 1], energy[base + 2]]
}

/// Resolve every pixel along one axis to its bracket, and collect the (≤2)
/// distinct z9 tiles the axis touches. `to_global` maps a coordinate to a
/// fractional world pixel; `norm` wraps (X) or clamps (Y) a signed global pixel
/// to `(tile, local)`. The first distinct tile seen is slot 0, the second slot 1
/// — a z12 tile straddles at most one z9-tile boundary per axis.
fn axis_brackets(
    coords: &[f64; TILE_PX],
    to_global: impl Fn(f64) -> f64,
    norm: impl Fn(i64) -> (u32, usize),
) -> ([Bracket; TILE_PX], [Option<u32>; 2]) {
    let mut tiles: [Option<u32>; 2] = [None, None];
    let mut slot_of = |t: u32| -> usize {
        if tiles[0] == Some(t) {
            0
        } else if tiles[0].is_none() {
            tiles[0] = Some(t);
            0
        } else if tiles[1] == Some(t) {
            1
        } else if tiles[1].is_none() {
            tiles[1] = Some(t);
            1
        } else {
            // A 3rd distinct z9 tile on one axis means the output tile spans
            // >2 z9 tiles — only possible if output zoom < CRUISE_COMPUTE_ZOOM−1.
            // Production builds z12 (8× finer than z9), so this never fires; it
            // guards a future coarse-output-zoom misuse rather than silently
            // corrupting the slot grid.
            panic!(
                "cruise upsample: output tile spans >2 z9 tiles per axis \
                 (output zoom too coarse for CRUISE_COMPUTE_ZOOM={CRUISE_COMPUTE_ZOOM})"
            );
        }
    };
    let mut out = [Bracket {
        lo_slot: 0,
        lo_local: 0,
        hi_slot: 0,
        hi_local: 0,
        frac: 0.0,
    }; TILE_PX];
    for (i, b) in out.iter_mut().enumerate() {
        let (lo, frac) = bracket(to_global(coords[i]));
        let (lt, ll) = norm(lo);
        let (ht, hl) = norm(lo + 1);
        *b = Bracket {
            lo_slot: slot_of(lt),
            lo_local: ll,
            hi_slot: slot_of(ht),
            hi_local: hl,
            frac,
        };
    }
    (out, tiles)
}

/// Global signed X pixel → (z9 tile x, local px), wrapping cyclically.
#[inline]
fn wrap_x(g: i64) -> (u32, usize) {
    let g = g.rem_euclid(WORLD_PX) as u32;
    (g >> TILE_SHIFT, (g & (TILE_PX as u32 - 1)) as usize)
}

/// Global signed Y pixel → (z9 tile y, local py), clamping to the poles.
#[inline]
fn clamp_y(g: i64) -> (u32, usize) {
    let g = g.clamp(0, WORLD_PX - 1) as u32;
    (g >> TILE_SHIFT, (g & (TILE_PX as u32 - 1)) as usize)
}

/// World width/height in z9 pixels (`256 × 2^10`). Mercator is square in
/// pixel space, so this is both the X period (for wrap) and the Y extent
/// (for clamp).
const WORLD_PX: i64 = (TILE_PX as i64) * (1 << CRUISE_COMPUTE_ZOOM);

/// World-pixel X (at [`CRUISE_COMPUTE_ZOOM`]) of a longitude. Continuous,
/// pixel-centre convention applied by the caller's `bracket`.
#[inline]
fn lon_to_global_px(lon: f64) -> f64 {
    (lon + 180.0) / 360.0 * WORLD_PX as f64
}

/// World-pixel Y (at [`CRUISE_COMPUTE_ZOOM`]) of a latitude (Web Mercator).
#[inline]
fn lat_to_global_px(lat: f64) -> f64 {
    let lat_rad = lat.to_radians();
    let merc = (lat_rad.tan() + 1.0 / lat_rad.cos()).ln();
    (1.0 - merc / PI) / 2.0 * WORLD_PX as f64
}

/// Bracket a fractional world-pixel coordinate `g` to `(lower_pixel, frac)`
/// for pixel-CENTRE bilinear sampling: centres are at `integer + 0.5`, so the
/// lower neighbour is `floor(g - 0.5)` and `frac ∈ [0,1)` is the offset toward
/// the upper neighbour. The lower pixel is signed (−1 just inside a W/N edge);
/// `sample_period` wraps (X) or clamps (Y) it into range.
#[inline]
fn bracket(g: f64) -> (i64, f32) {
    let c = g - 0.5;
    let lo = c.floor();
    let frac = (c - lo) as f32;
    (lo as i64, frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_centres_and_fractions() {
        // Pixel centre at world-px 10.5 → lower pixel 10, frac 0.
        assert_eq!(bracket(10.5), (10, 0.0));
        // Half-way between centres 10.5 and 11.5 → lower 10, frac 0.5.
        let (lo, f) = bracket(11.0);
        assert_eq!(lo, 10);
        assert!((f - 0.5).abs() < 1e-6);
        // Just below a centre rounds to the previous bracket.
        let (lo, f) = bracket(11.4);
        assert_eq!(lo, 10);
        assert!((f - 0.9).abs() < 1e-5);
        // Just inside the W/N edge the lower neighbour is −1 (wrapped/clamped
        // later, not at bracket time).
        let (lo, f) = bracket(0.2);
        assert_eq!(lo, -1);
        assert!((f - 0.7).abs() < 1e-5);
    }

    #[test]
    fn global_px_tile_decomposition_matches_xyz() {
        // A z9 tile's centre pixel must decode back to that tile via the
        // TILE_SHIFT/mask pair (same physical spot as the old z10 552/346).
        let z = CRUISE_COMPUTE_ZOOM;
        let bbox = crate::grid::tile_bbox(z, 276, 173);
        let mid = (TILE_PX / 2) as u32;
        let cx = bracket(lon_to_global_px(crate::grid::pixel_lon(&bbox, mid))).0;
        let cy = bracket(lat_to_global_px(crate::grid::pixel_lat(&bbox, mid))).0;
        assert_eq!(cx >> TILE_SHIFT, 276, "centre pixel x tile");
        assert_eq!(cy >> TILE_SHIFT, 173, "centre pixel y tile");
    }

    #[test]
    fn antimeridian_wraps_x_poles_clamp_y() {
        // X is cyclic: the lower neighbour of global pixel 0 is WORLD_PX−1
        // (the +180° column), not a clamp to 0. Replays sample_period's
        // normalisation (without touching the grid).
        assert_eq!((-1i64).rem_euclid(WORLD_PX), WORLD_PX - 1);
        assert_eq!(WORLD_PX.rem_euclid(WORLD_PX), 0);
        // The easternmost z12 pixel (lon → +180⁻) brackets a pixel whose upper
        // neighbour is WORLD_PX → wraps to global x 0 (tile 0, local 0).
        let g = lon_to_global_px(179.9999);
        let (lo, _) = bracket(g);
        let hi = (lo + 1).rem_euclid(WORLD_PX);
        assert_eq!(
            hi >> TILE_SHIFT,
            0,
            "east-edge upper neighbour wraps to tile x 0"
        );
        assert_eq!(hi & (TILE_PX as i64 - 1), 0, "…local pixel 0");
        // Y clamps (no pole wrap): a below-0 row pins to 0, an over-max pins
        // to the last valid row.
        assert_eq!((-3i64).clamp(0, WORLD_PX - 1), 0);
        assert_eq!((WORLD_PX + 5).clamp(0, WORLD_PX - 1), WORLD_PX - 1);
    }
}

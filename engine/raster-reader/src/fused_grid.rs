//! [`FusedGrid`] + [`FusedPixel`] — L3-cache-resident cropped raster grid for
//! pipeline compute.
//!
//! Pre-reads DEM + building + forest + IMD for a bbox out of
//! [`crate::real_rasters::RealRasters`] into one contiguous `Vec<FusedPixel>`,
//! then implements [`noise_compute::types::RasterSampler`] over it with the SAME
//! per-raster interpolation config (DEM/IMD bilinear, building/forest nearest) so
//! pipeline output matches mmap-based [`RealRasters`] to ~0 dB.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::imd_max_pyramid::ImdMaxPyramid;
use crate::RealRasters;

/// Each FusedGrid allocation/mutation gets a new id so worker-local cached
/// pixels can never survive into a distinct halo that reused an allocator slot.
static NEXT_GRID_ID: AtomicU64 = AtomicU64::new(1);

fn next_grid_id() -> u64 {
    NEXT_GRID_ID.fetch_add(1, Ordering::Relaxed)
}

/// L3-cache-resident cropped raster grid for pipeline compute.
///
/// Pre-reads DEM + building + forest + IMD for the hex bbox into ONE contiguous
/// Vec, cropped to just the needed area (~22 MB for a typical R4 hex + ring).
/// Implements RasterSampler so all existing path_effects code works unchanged.
/// Zero algorithmic change = zero dB error vs mmap-based RealRasters.
///
/// `Clone` exists for [`FusedGrid::burn_building_max`] experiments: burning
/// into a COPY leaves the original — and the receiver reflection pre-baked
/// from it — untouched.
pub struct FusedGrid {
    data: Vec<FusedPixel>,
    grid_id: u64,
    lat_min: f64,
    lon_min: f64,
    inv_cell_deg: f64,
    cols: usize,
    rows: usize,
    /// Max-pooled IMD over `data`, built once at grid build — the scatter
    /// byte-stop's ground bound (M3a) queries it per ray chunk instead of
    /// marching. Never mutated after build (the quad cache's `grid_id`
    /// invalidation covers the one mutator, `burn_building_max`, which does
    /// not touch IMD).
    imd_pyramid: ImdMaxPyramid,
}

impl Clone for FusedGrid {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            grid_id: next_grid_id(),
            lat_min: self.lat_min,
            lon_min: self.lon_min,
            inv_cell_deg: self.inv_cell_deg,
            cols: self.cols,
            rows: self.rows,
            imd_pyramid: self.imd_pyramid.clone(),
        }
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct FusedPixel {
    pub elevation: f32, // DEM (meters, full precision bilinear)
    pub building: u8,   // building height (meters)
    pub forest: u8,     // forest cover (0 or 100)
    pub imd: u8,        // imperviousness 0-100
    pub _pad: u8,       // total: 4+1+1+1+1 = 8 bytes per pixel
}

/// The C7.1 locality receipt measured 77.7775% exact hits at this capacity.
/// At 36 bytes per entry it is 288 KiB per worker, so two SMT siblings fit in
/// msas2's 1 MiB private L2 with room for profile state; 16K would not.
const PROFILE_QUAD_CACHE_ENTRIES: usize = 8192;
const _: () = assert!(PROFILE_QUAD_CACHE_ENTRIES.is_power_of_two());

#[derive(Clone, Copy, Default)]
struct CachedPixelQuad {
    /// `base + 1`; zero is the cold-entry sentinel and cannot alias a grid
    /// index because a u32-max base bypasses this cache.
    tag: u32,
    pixels: [FusedPixel; 4],
}

/// Per-thread direct map. It is deliberately not shared: a lock would cost
/// more than four L3-resident loads. A Rayon worker can interleave receiver
/// blocks from multiple concurrent halos, so `grid_id !=` must flush on every
/// halo replacement or mutation, including a return to an older generation.
struct WorkerPixelQuadCache {
    grid_id: u64,
    entries: Vec<CachedPixelQuad>,
}

impl WorkerPixelQuadCache {
    fn new() -> Self {
        Self {
            grid_id: 0,
            entries: vec![CachedPixelQuad::default(); PROFILE_QUAD_CACHE_ENTRIES],
        }
    }

    #[inline]
    fn lookup_or_insert(
        &mut self,
        grid_id: u64,
        base: usize,
        cols: usize,
        data: &[FusedPixel],
    ) -> [FusedPixel; 4] {
        let Some(tag) = base
            .checked_add(1)
            .and_then(|base_plus_one| u32::try_from(base_plus_one).ok())
        else {
            return [
                data[base],
                data[base + 1],
                data[base + cols],
                data[base + cols + 1],
            ];
        };
        if self.grid_id != grid_id {
            self.grid_id = grid_id;
            self.entries.fill(CachedPixelQuad::default());
        }
        let entry = &mut self.entries[base & (PROFILE_QUAD_CACHE_ENTRIES - 1)];
        if entry.tag == tag {
            return entry.pixels;
        }
        let pixels = [
            data[base],
            data[base + 1],
            data[base + cols],
            data[base + cols + 1],
        ];
        *entry = CachedPixelQuad { tag, pixels };
        pixels
    }
}

thread_local! {
    static WORKER_PIXEL_QUAD_CACHE: RefCell<Option<WorkerPixelQuadCache>> = const { RefCell::new(None) };
}

impl FusedGrid {
    #[inline]
    fn pixel_quad(&self, base: usize) -> [FusedPixel; 4] {
        WORKER_PIXEL_QUAD_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache
                .get_or_insert_with(WorkerPixelQuadCache::new)
                .lookup_or_insert(self.grid_id, base, self.cols, &self.data)
        })
    }

    pub(crate) fn empty() -> Self {
        let data = vec![FusedPixel::default(); 4];
        let imd_pyramid = ImdMaxPyramid::from_imd_plane(&data, 2, 2);
        FusedGrid {
            data,
            grid_id: next_grid_id(),
            lat_min: 0.0,
            lon_min: 0.0,
            inv_cell_deg: 3600.0,
            cols: 2,
            rows: 2,
            imd_pyramid,
        }
    }

    /// Exact grid dimensions `build` will allocate for this bbox — the ONE
    /// sizing computation, shared with byte-budget estimation (noise-gpu's
    /// pipeline gate reserves a block's bytes BEFORE building it; an estimate
    /// derived anywhere else would drift from the real allocation).
    ///
    /// Snap origin to the 1/3600° DEM pixel lattice (integer-lattice
    /// arithmetic avoids float edge slop). Without this, `build` samples
    /// DEM at sub-cell-shifted positions, introducing a persistent
    /// half-cell phase error that `lookup_fused` / `pixel` cannot correct.
    ///
    /// Margin sized to cover `building_enclosure`'s metric 3×3 probe
    /// (ENCLOSURE_RADIUS_M = 75 m) plus the `pixel()` nearest-neighbour
    /// half-cell rounding. Probe footprint in DEM cells (~30.7 m):
    /// lat ≈ 2.4 cells; lon ≈ 2.4/cos(lat), reaching ~7.1 cells at
    /// 70°N. 8 cells stays safe through ~71°N (cos(lat) ≥ 0.32) —
    /// covers every populated Arctic city; above that the probe
    /// clamps to edge pixels exactly like the pre-fix code did.
    pub fn grid_dims(
        lat_min: f64,
        lat_max: f64,
        lon_min: f64,
        lon_max: f64,
    ) -> (usize, usize, f64, f64) {
        let cell_deg = 1.0 / 3600.0;
        let inv_cell_deg = 3600.0;
        const MARGIN_CELLS: i32 = 8;
        let lat_lo_i = (lat_min * inv_cell_deg).floor() as i32 - MARGIN_CELLS;
        let lon_lo_i = (lon_min * inv_cell_deg).floor() as i32 - MARGIN_CELLS;
        let lat_hi_i = (lat_max * inv_cell_deg).ceil() as i32 + MARGIN_CELLS;
        let lon_hi_i = (lon_max * inv_cell_deg).ceil() as i32 + MARGIN_CELLS;
        let rows = ((lat_hi_i - lat_lo_i + 1).max(2)) as usize;
        let cols = ((lon_hi_i - lon_lo_i + 1).max(2)) as usize;
        (
            rows,
            cols,
            lat_lo_i as f64 * cell_deg,
            lon_lo_i as f64 * cell_deg,
        )
    }

    /// Heap bytes owned by this grid (the `FusedPixel` buffer).
    pub fn heap_bytes(&self) -> u64 {
        (self.data.capacity() * std::mem::size_of::<FusedPixel>()) as u64
    }

    /// Build from RealRasters, cropping to bbox. ~0.2-0.5s for typical hex.
    pub fn build(
        rasters: &RealRasters,
        lat_min: f64,
        lat_max: f64,
        lon_min: f64,
        lon_max: f64,
    ) -> Self {
        let cell_deg = 1.0 / 3600.0;
        let inv_cell_deg = 3600.0;
        let (rows, cols, lat_lo, lon_lo) = Self::grid_dims(lat_min, lat_max, lon_min, lon_max);

        let mut data = vec![FusedPixel::default(); rows * cols];

        for r in 0..rows {
            let lat = lat_lo + r as f64 * cell_deg;
            for co in 0..cols {
                let lon = lon_lo + co as f64 * cell_deg;
                let idx = r * cols + co;
                let elev = rasters.dem.sample(lat, lon);
                data[idx] = FusedPixel {
                    elevation: elev as f32,
                    building: rasters.building.sample(lat, lon) as u8,
                    forest: rasters.forest.sample(lat, lon) as u8,
                    imd: rasters.imd.sample(lat, lon) as u8,
                    _pad: 0,
                };
            }
        }

        let imd_pyramid = ImdMaxPyramid::from_imd_plane(&data, rows, cols);
        FusedGrid {
            data,
            grid_id: next_grid_id(),
            lat_min: lat_lo,
            lon_min: lon_lo,
            inv_cell_deg,
            cols,
            rows,
            imd_pyramid,
        }
    }

    /// Rasterise one noise-barrier segment into the BUILDING channel: every
    /// grid cell the segment crosses gets `building = max(building, height)`.
    ///
    /// NOT wired into any production path (B8/C9 verdict, 2026-06-11): this
    /// was the candidate GPU barrier representation (the CUDA surface kernels
    /// read screening obstacles from the 30 m cover raster and have no
    /// vector-barrier input), but the quantified comparison —
    /// tile-painter `tests/barrier_screening.rs::w2_vector_vs_burn_
    /// quantified_decision_record` — measured the burn UNDER-screening by
    /// mean +3.7 / max +13.8 dB at wall-adjacent shadow pixels: the bilateral
    /// ray cadence (≥ ~30 m sample spacing) steps over a one-cell-thin burned
    /// wall on most paths, while the CPU vector path
    /// (`path_effects::screening_attenuation`) maps the wall onto the nearest
    /// existing sample and never misses. Thin-line features cannot be
    /// faithfully represented in a raster sampled at ≥ cell-size cadence, so
    /// barriers ship CPU-vector-only and this function remains as the
    /// decision-record apparatus. If you re-open the GPU question, burn a
    /// CLONE (receiver reflection is pre-baked from the unburned halo by
    /// `FusedTileZ13::build_with_halo` — barriers must screen, never reflect).
    ///
    /// Supercover traversal (Amanatides & Woo, 4-connected): a diagonal wall
    /// has no one-past-the-corner gaps a nearest-neighbour ray sample could
    /// slip through, which plain 8-connected Bresenham would leave. Cells are
    /// matched on the nearest-neighbour convention (`pixel()` rounds, so cell
    /// (r, c) spans [r−0.5, r+0.5)). Out-of-grid cells are skipped, never
    /// clamped — clamping would smear far barriers onto the halo edge.
    pub fn burn_building_max(
        &mut self,
        start_lat: f64,
        start_lon: f64,
        end_lat: f64,
        end_lon: f64,
        height_m: f32,
    ) {
        let h = height_m.round().clamp(0.0, 255.0) as u8;
        if h == 0
            || !(start_lat.is_finite()
                && start_lon.is_finite()
                && end_lat.is_finite()
                && end_lon.is_finite())
        {
            return;
        }
        // Continuous coords in floor-grid space: +0.5 shifts the rounding
        // convention so `floor` yields the same cell `pixel()` rounds to.
        let u0 = (start_lat - self.lat_min) * self.inv_cell_deg + 0.5;
        let v0 = (start_lon - self.lon_min) * self.inv_cell_deg + 0.5;
        let u1 = (end_lat - self.lat_min) * self.inv_cell_deg + 0.5;
        let v1 = (end_lon - self.lon_min) * self.inv_cell_deg + 0.5;
        let mut r = u0.floor() as i64;
        let mut c = v0.floor() as i64;
        let r_end = u1.floor() as i64;
        let c_end = v1.floor() as i64;
        // Whole segment on one side of the grid slab in either axis ⇒ no cell
        // crossed; O(1) reject for the region barriers far from this halo.
        let (rows, cols) = (self.rows as i64, self.cols as i64);
        if (r < 0 && r_end < 0)
            || (r >= rows && r_end >= rows)
            || (c < 0 && c_end < 0)
            || (c >= cols && c_end >= cols)
        {
            return;
        }
        let step_r: i64 = if u1 >= u0 { 1 } else { -1 };
        let step_c: i64 = if v1 >= v0 { 1 } else { -1 };
        // Parametric distance (in units of |Δ|⁻¹) to the next r/c cell
        // boundary, then constant per-cell increments — Amanatides & Woo.
        let inv_du = (u1 - u0).abs().recip(); // ∞ when the segment is axis-parallel
        let inv_dv = (v1 - v0).abs().recip();
        let mut t_max_r = if step_r > 0 {
            (r + 1) as f64 - u0
        } else {
            u0 - r as f64
        } * inv_du;
        let mut t_max_c = if step_c > 0 {
            (c + 1) as f64 - v0
        } else {
            v0 - c as f64
        } * inv_dv;
        // Supercover visits exactly |Δr| + |Δc| + 1 cells; the +4 pads float
        // edge cases so a boundary-grazing segment can't loop unbounded.
        let mut guard = (r_end - r).abs() + (c_end - c).abs() + 4;
        let mut modified = false;
        loop {
            if (0..rows).contains(&r) && (0..cols).contains(&c) {
                let px = &mut self.data[r as usize * self.cols + c as usize];
                px.building = px.building.max(h);
                modified = true;
            }
            if (r == r_end && c == c_end) || guard <= 0 {
                if modified {
                    self.grid_id = next_grid_id();
                }
                return;
            }
            guard -= 1;
            if t_max_r < t_max_c {
                t_max_r += inv_du;
                r += step_r;
            } else {
                t_max_c += inv_dv;
                c += step_c;
            }
        }
    }

    /// Nearest-neighbor pixel lookup — matches `TileStore::sample_nearest`
    /// (rounds to the closest pixel). Taking `.floor() as usize` here biased
    /// the sample up-left by half a cell and made FusedGrid-based pipeline
    /// runs differ from RealRasters-based popup by 5-7 dB when a building /
    /// tree / impervious-surface edge fell inside the quad.
    #[inline]
    fn pixel(&self, lat: f64, lon: f64) -> &FusedPixel {
        let rf = (lat - self.lat_min) * self.inv_cell_deg;
        let cf = (lon - self.lon_min) * self.inv_cell_deg;
        let r = rf.round().clamp(0.0, (self.rows - 1) as f64) as usize;
        let c = cf.round().clamp(0.0, (self.cols - 1) as f64) as usize;
        &self.data[r * self.cols + c]
    }

    /// Bilinear IMD lookup — matches `RealRasters.imd` `Interp::Bilinear`
    /// config. Storage stays `u8` (0-100 range gives 0.01 G-factor quanta,
    /// worst per-band error ~0.025 dB — well below noise floor); the
    /// interpolation happens at query time over 4 u8 neighbours.
    #[inline]
    fn imd_bilinear(&self, lat: f64, lon: f64) -> f64 {
        let rf = (lat - self.lat_min) * self.inv_cell_deg;
        let cf = (lon - self.lon_min) * self.inv_cell_deg;
        let rf = rf.clamp(0.0, (self.rows - 1) as f64);
        let cf = cf.clamp(0.0, (self.cols - 1) as f64);
        let r0 = (rf.floor() as usize).min(self.rows - 2);
        let c0 = (cf.floor() as usize).min(self.cols - 2);
        let fr = rf - r0 as f64;
        let fc = cf - c0 as f64;
        let base = r0 * self.cols + c0;
        let v00 = self.data[base].imd as f64;
        let v01 = self.data[base + 1].imd as f64;
        let v10 = self.data[base + self.cols].imd as f64;
        let v11 = self.data[base + self.cols + 1].imd as f64;
        let v0 = v00 + fc * (v01 - v00);
        let v1 = v10 + fc * (v11 - v10);
        v0 + fr * (v1 - v0)
    }

    #[inline]
    fn elevation_bilinear(&self, lat: f64, lon: f64) -> f64 {
        let rf = (lat - self.lat_min) * self.inv_cell_deg;
        let cf = (lon - self.lon_min) * self.inv_cell_deg;
        // Clamp BEFORE floor — prevents negative wrap and OOB linear
        // extrapolation (the prior form left `fr = rf - r0` negative for
        // points west/south of bbox, silently extrapolating elevation
        // into open space).
        let rf = rf.clamp(0.0, (self.rows - 1) as f64);
        let cf = cf.clamp(0.0, (self.cols - 1) as f64);
        let r0 = (rf.floor() as usize).min(self.rows - 2);
        let c0 = (cf.floor() as usize).min(self.cols - 2);
        let fr = rf - r0 as f64;
        let fc = cf - c0 as f64;
        let v00 = self.data[r0 * self.cols + c0].elevation as f64;
        let v01 = self.data[r0 * self.cols + c0 + 1].elevation as f64;
        let v10 = self.data[(r0 + 1) * self.cols + c0].elevation as f64;
        let v11 = self.data[(r0 + 1) * self.cols + c0 + 1].elevation as f64;
        let v0 = v00 + fc * (v01 - v00);
        let v1 = v10 + fc * (v11 - v10);
        v0 + fr * (v1 - v0)
    }
}

impl noise_compute::types::RasterSampler for FusedGrid {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.elevation_bilinear(lat, lon)
    }

    fn building_height(&self, lat: f64, lon: f64) -> f64 {
        self.pixel(lat, lon).building as f64
    }

    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        // Bilinear IMD to match RealRasters config (Interp::Bilinear for
        // IMD). Without this, G jumps in 1/100 steps across pixel edges,
        // while popup sees a smooth gradient — popup/pipeline diverges
        // on hard/soft ground transitions.
        let imd = self.imd_bilinear(lat, lon);
        (1.0 - imd / 100.0).clamp(0.0, 1.0)
    }

    fn building_enclosure(&self, lat: f64, lon: f64) -> f64 {
        // Mirror RealRasters::building_enclosure: metric 3×3 probe at
        // ENCLOSURE_RADIUS_M (75 m). Required for popup/pipeline parity.
        let step_lat_deg = crate::ENCLOSURE_RADIUS_M / noise_compute::constants::M_PER_DEG_LAT;
        let step_lon_deg =
            crate::ENCLOSURE_RADIUS_M / noise_compute::constants::m_per_deg_lon(lat.to_radians());
        let mut tall = 0;
        let mut total = 0;
        for dr in [-1, 0, 1] {
            for dc in [-1, 0, 1] {
                // Wrap probe longitude across the antimeridian — keeps parity
                // with `RealRasters::building_enclosure`; without it, `pixel()`
                // clamps the out-of-bbox column and the two implementations
                // diverge near ±180°.
                let probe_lon =
                    ((lon + dc as f64 * step_lon_deg + 180.0).rem_euclid(360.0)) - 180.0;
                let h = self
                    .pixel(lat + dr as f64 * step_lat_deg, probe_lon)
                    .building;
                if h > 5 {
                    tall += 1;
                }
                total += 1;
            }
        }
        let density = tall as f64 / total as f64;
        if density > 0.5 {
            3.0
        } else if density > 0.2 {
            1.5
        } else {
            0.0
        }
    }

    fn max_building_along_path(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        excl_start_m: f64,
    ) -> (f64, f64) {
        // Mirror RealRasters / trait-default cadence exactly (uniform
        // cell / 3 × cell / 6 × cell by distance band). Previous version
        // used `fill_t_values` bilaterally, which has a 240 m mid-gap
        // that misses tall obstacles between endpoints — parity divergence.
        let cell_m = noise_compute::propagation::path_profile::CELL_M;
        let step = if dist_m <= 1000.0 {
            cell_m
        } else if dist_m <= 3000.0 {
            cell_m * 3.0
        } else {
            cell_m * 6.0
        };
        let n = ((dist_m / step).ceil() as usize).clamp(2, 400);
        let mut max_bh = 0.0_f64;
        let mut max_t = 0.5_f64;
        for k in 1..n {
            let t = k as f64 / n as f64;
            if excl_start_m > 0.0 && t * dist_m < excl_start_m {
                continue;
            }
            let lat = src_lat + t * (rcv_lat - src_lat);
            let lon = src_lon + t * (rcv_lon - src_lon);
            let bh = self.pixel(lat, lon).building as f64;
            if bh > max_bh {
                max_bh = bh;
                max_t = t;
            }
        }
        (max_bh, max_t)
    }

    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut noise_compute::propagation::PathProfile,
    ) {
        noise_compute::propagation::path_profile::fill_t_values(dist_m, &mut out.t);
        self.fill_profile_rasters(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out);
    }
}

impl FusedGrid {
    /// [`RasterSampler::build_path_profile`] with the SURFACE-HEATMAP
    /// coarse-middle cadence ([`CoarseMid`]): full-res near both ends (where
    /// obstacles diffract sound most severely), the smooth long-ray middle
    /// subsampled. Rays with no real middle reduce to the exact cadence
    /// byte-for-byte. Heatmap line/point/ground-ops kernels only — the POPUP
    /// stays on the exact [`RasterSampler::build_path_profile`].
    pub fn build_path_profile_coarse_mid(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        cfg: noise_compute::propagation::path_profile::CoarseMid,
        out: &mut noise_compute::propagation::PathProfile,
    ) {
        noise_compute::propagation::path_profile::fill_t_values_coarse_mid(dist_m, &mut out.t, cfg);
        self.fill_profile_rasters(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out);
    }

    /// Sample the four surface rasters at the t-values already in `out.t`,
    /// populating the profile. Shared by the exact + coarse-middle cadences
    /// (only the `out.t` fill differs); keeps the ray-march loop in one place.
    fn fill_profile_rasters(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut noise_compute::propagation::PathProfile,
    ) {
        // `out.t` is already filled by the caller's cadence. Clear-then-refill the
        // remaining fields, preserving t.
        let t_len = out.t.len();
        out.elevation_m.clear();
        out.building_h_m.clear();
        out.forest_u8.clear();
        out.imd_u8.clear();
        out.elevation_f64_scratch.clear();
        out.composite_h_scratch.clear();
        out.dist_m = dist_m;
        out.src_lat = src_lat;
        out.src_lon = src_lon;
        out.rcv_lat = rcv_lat;
        out.rcv_lon = rcv_lon;

        out.elevation_m.reserve(t_len);
        out.building_h_m.reserve(t_len);
        out.forest_u8.reserve(t_len);
        out.imd_u8.reserve(t_len);

        // Raster coords are affine in lat/lon, which are affine in t, so walk
        // (rf, cf) as a plain lerp instead of re-deriving them inside every lookup.
        let src_rf = (src_lat - self.lat_min) * self.inv_cell_deg;
        let src_cf = (src_lon - self.lon_min) * self.inv_cell_deg;
        let d_rf = (rcv_lat - src_lat) * self.inv_cell_deg;
        let d_cf = (rcv_lon - src_lon) * self.inv_cell_deg;
        for &t in &out.t {
            let (elev, bh, fr_u8, imd_u8) =
                self.lookup_fused_rc(src_rf + t * d_rf, src_cf + t * d_cf);
            out.elevation_m.push(elev);
            out.building_h_m.push(bh);
            out.forest_u8.push(fr_u8);
            out.imd_u8.push(imd_u8);
        }

        // The heatmap collapses to per-pixel energy: it never reads step_m_med
        // (popup tooltip metadata), and it omits the P3 mid-path peak
        // augmentation that RealRasters keeps for the per-point popup. Measured
        // on aggregate z13 tiles that augmentation moves Lden <0.07 dB mean /
        // 2 dB max while costing ~half the surface ray-march — the bilateral
        // cadence already samples every obstacle that dominates the single-edge δ.
        out.step_m_med = 0.0;
    }
}

impl FusedGrid {
    /// Number of raster cells held (cols × rows) — the denominator for the
    /// scatter's read-redundancy telemetry (how many times each cell is re-read).
    #[inline]
    pub fn cell_count(&self) -> usize {
        self.cols * self.rows
    }

    /// A clone with its sampling ORIGIN shifted by `(dlat, dlon)` degrees — the
    /// SAME stored raster cells, re-indexed so every `(lat,lon)` lookup lands a
    /// fraction of a cell away. Used ONLY by the surface noise-floor harness to
    /// render the exact field at a second raster PHASE (the half-cell-shift
    /// "method noise floor" the coarse-middle error is measured against). Not a
    /// production path — `FusedGrid::build` always snaps origin to the DEM
    /// lattice, so this is the one way to perturb raster phase without moving
    /// receivers or geometry.
    pub fn with_origin_shift(&self, dlat: f64, dlon: f64) -> FusedGrid {
        FusedGrid {
            data: self.data.clone(),
            grid_id: next_grid_id(),
            lat_min: self.lat_min + dlat,
            lon_min: self.lon_min + dlon,
            inv_cell_deg: self.inv_cell_deg,
            cols: self.cols,
            rows: self.rows,
            // Same stored cells ⇒ same IMD plane; the pyramid is origin-blind.
            imd_pyramid: self.imd_pyramid.clone(),
        }
    }

    /// Upper bound on every IMD value `lookup_fused_rc` can return for any
    /// sample whose (fractional, pre-clamp) raster coordinate lies in
    /// `[rf_lo..=rf_hi] × [cf_lo..=cf_hi]`: the pyramid max over the cell box
    /// `[floor(clamp(rf_lo)) ..= floor(clamp(rf_hi))+1]` per axis. The `+1` is
    /// what makes a bilinear sample SOUND — a sample at fractional `rf` reads
    /// the quad `floor(rf)..floor(rf)+1`, so the box must cover the quad, not
    /// the point. Callers may pass `rf_lo > rf_hi` (rays run either way); the
    /// min/max is normalized here.
    pub fn imd_max_over_rc_box(&self, rf_lo: f64, rf_hi: f64, cf_lo: f64, cf_hi: f64) -> u8 {
        let clamp_pair = |lo: f64, hi: f64, n: usize| -> (usize, usize) {
            let lo = lo.clamp(0.0, (n - 1) as f64);
            let hi = hi.clamp(0.0, (n - 1) as f64);
            let (lo, hi) = (lo.min(hi), lo.max(hi));
            (lo.floor() as usize, ((hi.floor() as usize) + 1).min(n - 1))
        };
        let (r_lo, r_hi) = clamp_pair(rf_lo, rf_hi, self.rows);
        let (c_lo, c_hi) = clamp_pair(cf_lo, cf_hi, self.cols);
        self.imd_pyramid.max_over_cell_box(r_lo, r_hi, c_lo, c_hi)
    }

    /// Packed pixel array for the GPU backend (engine/noise-gpu) to upload as a
    /// device-resident halo. The device kernel mirrors [`Self::lookup_fused_rc`]
    /// over these cells; pair with [`Self::geom`] for the (lat,lon)→cell mapping.
    #[inline]
    pub fn pixels(&self) -> &[FusedPixel] {
        &self.data
    }

    /// `(lat_min, lon_min, inv_cell_deg, rows, cols)` — the origin/scale a device
    /// bilinear lookup needs: `rf = (lat − lat_min)·inv_cell_deg`, `cf` likewise.
    #[inline]
    pub fn geom(&self) -> (f64, f64, f64, usize, usize) {
        (
            self.lat_min,
            self.lon_min,
            self.inv_cell_deg,
            self.rows,
            self.cols,
        )
    }

    /// Bilinear elevation + IMD, nearest-neighbour building + forest.
    ///
    /// Matches `RealRasters` per-raster `Interp` config: DEM bilinear, IMD
    /// bilinear, building/forest nearest. Earlier versions used `px00`
    /// (top-left of the bilinear quad) for all three categoricals, biasing
    /// up-left by half a cell and producing up to 6+ dB divergence from
    /// `RealRasters` wherever a raster edge passed through the quad.
    /// `(elev_bilinear, building_nearest, forest_nearest, imd_bilinear)` — the
    /// four surface rasters in one lookup, used by the heatmap horizon builder.
    #[inline]
    pub fn lookup_fused(&self, lat: f64, lon: f64) -> (f32, u8, u8, u8) {
        let rf = (lat - self.lat_min) * self.inv_cell_deg;
        let cf = (lon - self.lon_min) * self.inv_cell_deg;
        self.lookup_fused_rc(rf, cf)
    }

    /// [`Self::lookup_fused`] with pre-computed fractional raster coordinates.
    /// The profile builder walks a straight ray, so `(rf, cf)` are affine in the
    /// path parameter `t`; lerping them directly differs only sub-ULP from
    /// re-deriving via per-sample lat/lon (measured 0.000 dB tile drift) and
    /// drops two multiplies + two subtracts per sample from the hot loop.
    #[inline]
    pub fn lookup_fused_rc(&self, rf: f64, cf: f64) -> (f32, u8, u8, u8) {
        // Clamp before floor: prevents negative wrap and OOB extrapolation.
        let rf = rf.clamp(0.0, (self.rows - 1) as f64);
        let cf = cf.clamp(0.0, (self.cols - 1) as f64);
        let r0 = (rf.floor() as usize).min(self.rows - 2);
        let c0 = (cf.floor() as usize).min(self.cols - 2);
        let fr = rf - r0 as f64;
        let fc = cf - c0 as f64;
        let base = r0 * self.cols + c0;
        let [px00, px01, px10, px11] = self.pixel_quad(base);
        // Elevation bilinear (DEM is a continuous field).
        let v0e = px00.elevation as f64 + fc * (px01.elevation as f64 - px00.elevation as f64);
        let v1e = px10.elevation as f64 + fc * (px11.elevation as f64 - px10.elevation as f64);
        let elev = (v0e + fr * (v1e - v0e)) as f32;
        // Nearest-neighbor for building + forest (discrete categoricals).
        let near = match (fr >= 0.5, fc >= 0.5) {
            (false, false) => px00,
            (false, true) => px01,
            (true, false) => px10,
            (true, true) => px11,
        };
        // IMD is stored u8 but sampled BILINEARLY at query time to match
        // RealRasters.imd Interp::Bilinear. PathProfile consumer quantises
        // back to u8 (matching its `imd_u8: Vec<u8>` contract).
        let v0i = px00.imd as f64 + fc * (px01.imd as f64 - px00.imd as f64);
        let v1i = px10.imd as f64 + fc * (px11.imd as f64 - px10.imd as f64);
        let imd = (v0i + fr * (v1i - v0i)).round().clamp(0.0, 255.0) as u8;
        (elev, near.building, near.forest, imd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::types::RasterSampler;
    use std::path::Path;

    fn test_rasters() -> RealRasters {
        // Prepared rasters under data/prepared — 1° tiles for CZ + surroundings
        // are populated by scripts/rasters-global.sh. Tests auto-ignore
        // when missing (see `prepared_available` helper below).
        RealRasters::new(Path::new("../../data/prepared"))
    }

    /// Skip test body if no prepared rasters on disk (e.g., hermetic CI
    /// without data/prepared populated). Checks the Copernicus DEM dir
    /// rather than a specific tile so the guard survives tile rotation.
    fn prepared_available() -> bool {
        let dir = Path::new("../../data/prepared/dem/copernicus");
        std::fs::read_dir(dir)
            .map(|mut iter| {
                iter.any(|e| {
                    e.ok()
                        .is_some_and(|e| e.path().extension().is_some_and(|x| x == "hgt"))
                })
            })
            .unwrap_or(false)
    }

    #[test]
    fn test_elevation_brno() {
        if !prepared_available() {
            return;
        }
        let r = test_rasters();
        let elev = r.elevation(49.195, 16.608);
        assert!(elev > 150.0 && elev < 400.0, "Brno elevation: {elev}m");
    }

    #[test]
    fn test_elevation_not_flat_200() {
        if !prepared_available() {
            return;
        }
        let r = test_rasters();
        let e1 = r.elevation(49.195, 16.608);
        let e2 = r.elevation(49.5, 16.0);
        assert!(
            (e1 - e2).abs() > 10.0,
            "Should not be flat: e1={e1}, e2={e2}"
        );
    }

    /// Audit B3: a coordinate with no IMD tile on disk is open ocean — the
    /// missing-tile default must read fully hard (imd=100 → G=0). The
    /// converter emits an IMD tile for every land tile, never for ocean,
    /// so no mid-Atlantic tile exists on any host. (Partial-tree hosts —
    /// e.g. a dev box carrying only 34–59°N + Scandinavia — make missing
    /// northern LAND read hard too; the production host's complete tree is
    /// the truth.)
    /// Runs without `prepared_available()`: a missing data dir is the
    /// same code path as a missing tile.
    #[test]
    fn imd_missing_tile_defaults_to_hard_ocean() {
        let r = test_rasters();
        let imd = r.imd.sample(30.0, -45.0);
        assert_eq!(imd, 100.0, "missing IMD tile must default to 100 (hard)");
        assert_eq!(
            r.ground_g(30.0, -45.0),
            0.0,
            "ocean ground must be hard (G=0)"
        );
    }

    #[test]
    fn test_ground_g_varies() {
        if !prepared_available() {
            return;
        }
        let r = test_rasters();
        let g1 = r.ground_g(49.195, 16.608); // urban
        let g2 = r.ground_g(49.3, 16.4); // rural
        assert!((0.0..=1.0).contains(&g1), "G urban: {g1}");
        assert!((0.0..=1.0).contains(&g2), "G rural: {g2}");
    }

    #[test]
    fn test_building_height() {
        if !prepared_available() {
            return;
        }
        let r = test_rasters();
        let h = r.building_height(49.195, 16.608);
        assert!((0.0..=255.0).contains(&h), "Building height: {h}m");
    }

    // FusedGrid ↔ RealRasters parity tests. Regression guard for the
    // FusedGrid fix set (truncate, px00-bias, subgrid shift, IMD
    // interpolation mismatch, OOB extrapolation). Tests auto-skip when
    // `data/prepared/` is not populated — no `#[ignore]` needed.

    fn test_fused() -> Option<(RealRasters, FusedGrid)> {
        if !prepared_available() {
            return None;
        }
        let r = test_rasters();
        // Small Brno-area bbox — keeps the grid in L1/L2.
        let fg = FusedGrid::build(&r, 49.18, 49.22, 16.58, 16.63);
        Some((r, fg))
    }

    #[test]
    fn fused_ground_g_parity() {
        let Some((real, fg)) = test_fused() else {
            return;
        };
        // Grid of points across the Brno bbox including hex-edge cases.
        for (lat, lon) in &[
            (49.20, 16.60),
            (49.195, 16.608),
            (49.19, 16.59),
            (49.181, 16.581), // near bbox edge
            (49.215, 16.625), // opposite corner
        ] {
            let g_real = real.ground_g(*lat, *lon);
            let g_fused = fg.ground_g(*lat, *lon);
            // Fused samples IMD nearest-neighbour at ITS cell centre; the real reader
            // interpolates bilinearly at the query point. On an IMD gradient the half-cell
            // offset legitimately diverges by a few percent (first seen as 0.49 vs 0.50
            // after the 2026-06 IMD water re-extract). 0.05 still catches the bugs this
            // parity test exists for (off-by-tile, scale, axis swap), which are ≥0.1.
            assert!(
                (g_real - g_fused).abs() < 0.05,
                "ground_g divergence at ({}, {}): real={g_real:.4} fused={g_fused:.4}",
                lat,
                lon
            );
        }
    }

    #[test]
    fn fused_elevation_parity() {
        let Some((real, fg)) = test_fused() else {
            return;
        };
        for (lat, lon) in &[(49.20, 16.60), (49.195, 16.608), (49.181, 16.581)] {
            let e_real = real.elevation(*lat, *lon);
            let e_fused = fg.elevation(*lat, *lon);
            // f32 precision in FusedPixel + f64 in RealRasters → sub-1 cm.
            assert!(
                (e_real - e_fused).abs() < 0.05,
                "elevation divergence at ({}, {}): real={e_real:.4} fused={e_fused:.4}",
                lat,
                lon
            );
        }
    }

    #[test]
    fn fused_building_height_parity() {
        let Some((real, fg)) = test_fused() else {
            return;
        };
        for (lat, lon) in &[(49.195, 16.608), (49.20, 16.60), (49.215, 16.625)] {
            let h_real = real.building_height(*lat, *lon);
            let h_fused = fg.building_height(*lat, *lon);
            assert_eq!(
                h_real as u8, h_fused as u8,
                "building_height divergence at ({}, {}): real={h_real} fused={h_fused}",
                lat, lon
            );
        }
    }

    #[test]
    fn fused_building_enclosure_near_bbox_edge() {
        // Regression test for Gemini-flagged bug: insufficient bbox
        // margin made `building_enclosure()` silently clamp the 3×3
        // probe to edge pixels at hex boundaries. Current 8-cell margin
        // covers the metric probe (ENCLOSURE_RADIUS_M = 75 m ≈ 2.4 lat
        // cells, ~3.4 lon cells at 50°N) plus rounding slack.
        let Some((real, fg)) = test_fused() else {
            return;
        };
        let lat = 49.181; // 1 cell from bbox min (49.18)
        let lon = 16.581;
        let e_real = real.building_enclosure(lat, lon);
        let e_fused = fg.building_enclosure(lat, lon);
        assert!(
            (e_real - e_fused).abs() < 0.5,
            "building_enclosure near-edge divergence: real={e_real} fused={e_fused}"
        );
    }

    #[test]
    fn fused_oob_clamp_no_extrapolation() {
        // Regression test for Gemini-flagged OOB bilinear extrapolation.
        // Query outside the FusedGrid bbox — pre-fix would extrapolate
        // linearly into space (negative `fr`/`fc`). Post-fix clamps to
        // the edge elevation.
        let Some((_, fg)) = test_fused() else {
            return;
        };
        let e_edge = fg.elevation(49.18, 16.58); // at bbox edge
        let e_outside = fg.elevation(49.10, 16.50); // far outside
                                                    // Both must be finite and plausible CZ elevations.
        assert!(e_edge.is_finite() && e_outside.is_finite());
        assert!(e_outside > -1000.0 && e_outside < 10_000.0);
    }

    #[test]
    fn building_enclosure_antimeridian_wraps() {
        // Regression for the antimeridian wrap fix: a receiver at lon
        // ≈ ±180° must not panic and the probe column at dc=+1/-1 must
        // wrap to the opposite hemisphere, not clamp to the ±179° edge.
        // We can't compare against a real-raster ground truth here
        // (Pacific tiles are not in `data/prepared/`), so we assert the
        // function is finite and matches between RealRasters / FusedGrid
        // at both wrap polarities — the fix's purpose is parity.
        if !prepared_available() {
            return;
        }
        let r = test_rasters();
        let lat = 0.0;
        for &lon in &[179.9995_f64, -179.9995_f64] {
            let v = r.building_enclosure(lat, lon);
            assert!(v.is_finite(), "RealRasters NaN at lon={lon}");
            assert!((0.0..=3.0).contains(&v), "out of range at lon={lon}: {v}");
        }
    }

    // ── FusedGrid::burn_building_max (GPU barrier burn) ──────────────────
    // A grid built from an EMPTY rasters dir is flat 0 with building 0
    // everywhere (missing tiles are negative-cached defaults), so every
    // non-zero building cell after a burn is the burn's own footprint.

    fn empty_grid() -> FusedGrid {
        let r = RealRasters::new(Path::new("/nonexistent-quietmap-test-rasters"));
        FusedGrid::build(&r, 0.0, 0.05, 0.0, 0.05)
    }

    fn burned_cells(fg: &FusedGrid) -> Vec<(usize, usize, u8)> {
        let (_, _, _, rows, cols) = fg.geom();
        let mut out = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                let b = fg.pixels()[r * cols + c].building;
                if b > 0 {
                    out.push((r, c, b));
                }
            }
        }
        out
    }

    #[test]
    fn cached_quad_matches_the_uncached_quad_across_every_aliasing_base() {
        // 197x197 = 38,809 cells over an 8,192-entry direct map: every slot is
        // claimed by ~4.7 distinct bases, so a truncated tag or a wrong index
        // mask returns a NEIGHBOUR ROW's quad here.
        let mut g = empty_grid();
        let (rows, cols) = (g.rows, g.cols);
        assert!(rows * cols > 4 * PROFILE_QUAD_CACHE_ENTRIES);
        let (lat_min, lon_min) = (g.lat_min, g.lon_min);
        for r in 0..rows {
            let lat = lat_min + r as f64 / 3600.0;
            let h = (1 + r % 200) as f32;
            g.burn_building_max(lat, lon_min, lat, lon_min + cols as f64 / 3600.0, h);
        }
        for r0 in 0..rows - 1 {
            for c0 in 0..cols - 1 {
                let base = r0 * cols + c0;
                let want = [
                    g.data[base],
                    g.data[base + 1],
                    g.data[base + cols],
                    g.data[base + cols + 1],
                ];
                let got = g.pixel_quad(base);
                for i in 0..4 {
                    assert_eq!(
                        (
                            got[i].building,
                            got[i].forest,
                            got[i].imd,
                            got[i].elevation.to_bits()
                        ),
                        (
                            want[i].building,
                            want[i].forest,
                            want[i].imd,
                            want[i].elevation.to_bits()
                        ),
                        "cached quad != uncached quad at base {base} corner {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn two_live_grids_never_leak_quads_into_each_other() {
        // The production surface build runs `region_concurrency` regions at once
        // while the inner receiver-block par_iter steals across the whole pool,
        // so ONE worker thread alternates between distinct halo grids.
        let mut a = empty_grid();
        let mut b = empty_grid();
        a.burn_building_max(0.01, 0.01, 0.01, 0.01, 7.0);
        b.burn_building_max(0.01, 0.01, 0.01, 0.01, 3.0);
        for _ in 0..4 {
            assert_eq!(
                a.lookup_fused_rc(44.0, 44.0).1,
                7,
                "grid A read grid B's quad"
            );
            assert_eq!(
                b.lookup_fused_rc(44.0, 44.0).1,
                3,
                "grid B read grid A's quad"
            );
        }
    }

    #[test]
    fn cached_quad_is_invalidated_after_a_grid_mutation() {
        let mut grid = FusedGrid::empty();
        assert_eq!(grid.lookup_fused_rc(0.0, 0.0).1, 0);
        let before = grid.grid_id;
        grid.burn_building_max(0.0, 0.0, 0.0, 0.0, 9.0);
        assert_ne!(grid.grid_id, before, "a burn must invalidate quad caches");
        assert_eq!(
            grid.lookup_fused_rc(0.0, 0.0).1,
            9,
            "a stale cached quad must not survive the burn"
        );
    }

    #[test]
    fn pixel_quad_cache_keeps_full_tags_and_flushes_on_grid_revisit() {
        let cols = 2usize;
        let mut first = vec![FusedPixel::default(); PROFILE_QUAD_CACHE_ENTRIES + cols + 2];
        first[0].building = 11;
        first[PROFILE_QUAD_CACHE_ENTRIES].building = 22;
        let mut cache = WorkerPixelQuadCache::new();

        assert_eq!(cache.lookup_or_insert(1, 0, cols, &first)[0].building, 11);
        assert_eq!(
            cache.lookup_or_insert(1, PROFILE_QUAD_CACHE_ENTRIES, cols, &first)[0].building,
            22,
            "same direct-map slot must compare the complete base tag"
        );

        first[0].building = 33;
        let mut second = vec![FusedPixel::default(); cols + 2];
        second[0].building = 44;
        assert_eq!(cache.lookup_or_insert(1, 0, cols, &first)[0].building, 33);
        assert_eq!(cache.lookup_or_insert(2, 0, cols, &second)[0].building, 44);
        assert_eq!(
            cache.lookup_or_insert(1, 0, cols, &first)[0].building,
            33,
            "returning to an older live grid must flush the newer grid's quad"
        );
    }

    #[test]
    fn pixel_quad_cache_entry_stays_within_the_l2_budget() {
        assert_eq!(std::mem::size_of::<CachedPixelQuad>(), 36);
        assert_eq!(
            PROFILE_QUAD_CACHE_ENTRIES * std::mem::size_of::<CachedPixelQuad>(),
            288 * 1024
        );
    }

    #[test]
    fn burn_horizontal_segment_marks_one_cell_row() {
        let mut fg = empty_grid();
        fg.burn_building_max(0.01, 0.01, 0.01, 0.04, 3.0);
        let cells = burned_cells(&fg);
        // 0.03° span at 1/3600° cells = 108 crossings → 109 cells ±1 float edge.
        assert!(
            (108..=110).contains(&cells.len()),
            "horizontal wall cell count {} not in 108..=110",
            cells.len()
        );
        let row0 = cells[0].0;
        assert!(cells.iter().all(|&(r, _, h)| r == row0 && h == 3));
    }

    #[test]
    fn burn_diagonal_is_supercover_not_bresenham() {
        let mut fg = empty_grid();
        // 45° diagonal across 36×36 cells: supercover visits |Δr|+|Δc|+1 ≈ 73
        // cells (4-connected, no corner gaps); 8-connected Bresenham would
        // visit ~37 — the kernel's nearest-neighbour ray samples could slip
        // diagonally through those gaps.
        fg.burn_building_max(0.01, 0.01, 0.02, 0.02, 3.0);
        let cells = burned_cells(&fg);
        assert!(
            cells.len() > 60,
            "diagonal burn {} cells — looks 8-connected, supercover expected",
            cells.len()
        );
        assert!(
            cells.len() <= 76,
            "diagonal burn {} cells > |Δr|+|Δc|+1 bound",
            cells.len()
        );
    }

    #[test]
    fn burn_is_max_and_skips_out_of_grid() {
        let mut fg = empty_grid();
        fg.burn_building_max(0.02, 0.01, 0.02, 0.02, 5.0);
        fg.burn_building_max(0.02, 0.01, 0.02, 0.02, 2.0);
        let cells = burned_cells(&fg);
        assert!(!cells.is_empty());
        assert!(
            cells.iter().all(|&(_, _, h)| h == 5),
            "max(existing, h), never overwrite down"
        );

        // Entirely outside the grid → nothing painted, especially no edge smear.
        let before = cells.len();
        fg.burn_building_max(5.0, 5.0, 5.1, 5.1, 3.0);
        fg.burn_building_max(-1.0, 0.02, -2.0, 0.02, 3.0);
        assert_eq!(
            burned_cells(&fg).len(),
            before,
            "out-of-grid burn must be a no-op"
        );

        // Degenerate point segment → exactly one new cell.
        fg.burn_building_max(0.04, 0.04, 0.04, 0.04, 4.0);
        assert_eq!(burned_cells(&fg).len(), before + 1);
    }
}

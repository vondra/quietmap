//! Per-tile raster cache aligned with the Mercator receiver lattice (z12
//! base with 512-px tiles — the same physical lattice as the pre-2026-07
//! z13@256) plus an optional WGS84 30 m halo sized by the caller.
//!
//! The inner core (`TILE_PX × TILE_PX` Mercator-aligned cells) matches the
//! output HM tile lattice 1:1 — receiver `(py, px)` indexes every layer
//! (DEM, buildings, forest, IMD, pre-baked receiver altitude/refl) without
//! any lat/lon math in the hot loop.
//!
//! Path-profile rays exiting the inner bbox fall through to the halo
//! `FusedGrid` (lat/lon lookup at native source-raster resolution).
//!
//! Builds in ~ms per tile (samples 262 144 inner + ~1.5M halo cells from
//! `RealRasters` mmap). Lives in L3 for the duration of one tile's
//! compute, then drops.

use std::sync::Arc;

use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};
use noise_compute::constants::DEFAULT_RECEIVER_HEIGHT;
use noise_compute::types::RasterSampler;

use crate::fused_grid::FusedPixel;
use crate::{FusedGrid, RealRasters};

/// Side length of one output tile in receiver pixels. 512 since the 2026-07
/// shift — lockstep with tile-painter grid.rs and the CUDA TPX.
pub const TILE_PX: usize = 512;

/// Equatorial m/px at zoom 0. = 2 π R / TILE_PX with R = 6 378 137 — the
/// divisor IS the tile side (512), so the physical pixel size at the base
/// zoom is unchanged by the 512@z12 shift (z12·512 ≡ z13·256 lattice).
/// If TILE_PX ever changes again, this constant MUST scale inversely or
/// every physical pixel size silently corrupts all propagation physics.
const EQUATORIAL_M_PER_PX_Z0: f64 = 78_271.516_964_020_5;

/// Mercator tile bbox in lat/lon (EPSG:4326).
#[derive(Debug, Clone, Copy)]
pub struct TileBbox {
    pub west_lon: f64,
    pub east_lon: f64,
    pub north_lat: f64,
    pub south_lat: f64,
}

impl TileBbox {
    pub fn from_xyz(zoom: u8, x: u32, y: u32) -> Self {
        use std::f64::consts::PI;
        let n = (1u64 << zoom) as f64;
        let xf = x as f64;
        let yf = y as f64;
        let west_lon = xf / n * 360.0 - 180.0;
        let east_lon = (xf + 1.0) / n * 360.0 - 180.0;
        let north_lat = (PI * (1.0 - 2.0 * yf / n)).sinh().atan().to_degrees();
        let south_lat = (PI * (1.0 - 2.0 * (yf + 1.0) / n))
            .sinh()
            .atan()
            .to_degrees();
        TileBbox {
            west_lon,
            east_lon,
            north_lat,
            south_lat,
        }
    }
}

#[inline]
fn mercator_y_from_lat(lat: f64) -> f64 {
    let lat_rad = lat.to_radians();
    (lat_rad.tan() + 1.0 / lat_rad.cos()).ln()
}

#[inline]
fn lat_from_mercator_y(merc: f64) -> f64 {
    use std::f64::consts::PI;
    (2.0 * merc.exp().atan() - PI / 2.0).to_degrees()
}

#[inline]
fn pixel_lat(bbox: &TileBbox, py: u32) -> f64 {
    let n = TILE_PX as f64;
    let north_merc = mercator_y_from_lat(bbox.north_lat);
    let south_merc = mercator_y_from_lat(bbox.south_lat);
    let frac = (py as f64 + 0.5) / n;
    let merc = north_merc + frac * (south_merc - north_merc);
    lat_from_mercator_y(merc)
}

#[inline]
fn pixel_lon(bbox: &TileBbox, px: u32) -> f64 {
    let n = TILE_PX as f64;
    let frac = (px as f64 + 0.5) / n;
    bbox.west_lon + frac * (bbox.east_lon - bbox.west_lon)
}

/// Mercator pixel side at the given latitude and zoom.
#[inline]
pub fn tile_pixel_size_m(zoom: u8, lat: f64) -> f64 {
    EQUATORIAL_M_PER_PX_Z0 * lat.to_radians().cos() / ((1u64 << zoom) as f64)
}

/// Per-tile raster cache for the aircraft ground-ops V1 kernel.
///
/// `inner_*` arrays are row-major `TILE_PX × TILE_PX` cells aligned with
/// the Mercator receiver lattice at `zoom`. `halo` is the WGS84
/// `FusedGrid` covering the tile bbox by the caller-selected reach.
pub struct FusedTileZ13 {
    pub zoom: u8,
    pub tile_x: u32,
    pub tile_y: u32,
    pub bbox: TileBbox,

    /// Receiver latitudes — one per pixel row.
    pub rx_lat: [f64; TILE_PX],
    /// Receiver longitudes — one per pixel column.
    pub rx_lon: [f64; TILE_PX],

    /// DEM elevation at every pixel centre.
    pub inner_elev_m: Vec<f32>,
    /// Forest cover 0/100 (WorldCover, nearest).
    pub inner_forest: Vec<u8>,
    /// Imperviousness 0..=100 (bilinear).
    pub inner_imd: Vec<u8>,

    /// Receiver altitude = DEM + END facade height (4 m). Pre-baked.
    pub rx_alt_m: Vec<f32>,
    /// Pre-baked receiver-side `building_enclosure` reflection bonus.
    pub rx_refl_db: Vec<f32>,

    /// Halo covering the tile bbox plus the caller-selected extension.
    /// `Arc`-wrapped so one [`TileBatch::build`] can share the same
    /// halo across N×N adjacent tiles instead of rebuilding it
    /// N² times — the halo dominates per-tile build cost.
    pub halo: Arc<FusedGrid>,
}

impl FusedTileZ13 {
    /// Build a standalone tile with its own freshly built halo extended
    /// by `halo_m`.
    pub fn build(zoom: u8, tile_x: u32, tile_y: u32, halo_m: f64, rasters: &RealRasters) -> Self {
        let bbox = TileBbox::from_xyz(zoom, tile_x, tile_y);
        let halo = Arc::new(build_halo_for(rasters, &bbox, halo_m));
        Self::build_with_halo(zoom, tile_x, tile_y, rasters, halo)
    }

    /// One DEM-only halo for any receiver bbox plus the airborne horizon
    /// reach. Its lattice and interpolation are identical to a full halo;
    /// only channels that aircraft never read are left zero.
    pub fn build_elevation_halo(
        receiver_bbox: &TileBbox,
        halo_m: f64,
        rasters: &RealRasters,
    ) -> Arc<FusedGrid> {
        let (lat_min, lat_max, lon_min, lon_max) = halo_bbox_for(receiver_bbox, halo_m);
        Arc::new(FusedGrid::build_elevation_only(
            rasters, lat_min, lat_max, lon_min, lon_max,
        ))
    }

    /// [`Self::build_with_halo`] leaving `rx_refl_db` zeroed for the caller to
    /// vector-bake (`source_loader_structure::bake_tile_vector_rx_refl`). Zero is
    /// the neutral value: an unpainted tile never reads it.
    pub fn build_with_halo_opt_rx_refl(
        zoom: u8,
        tile_x: u32,
        tile_y: u32,
        rasters: &RealRasters,
        halo: Arc<FusedGrid>,
    ) -> Self {
        let bbox = TileBbox::from_xyz(zoom, tile_x, tile_y);

        let rx_lat: [f64; TILE_PX] = std::array::from_fn(|i| pixel_lat(&bbox, i as u32));
        let rx_lon: [f64; TILE_PX] = std::array::from_fn(|i| pixel_lon(&bbox, i as u32));

        let n = TILE_PX * TILE_PX;
        let mut inner_elev_m = vec![0.0_f32; n];
        let mut inner_forest = vec![0_u8; n];
        let mut inner_imd = vec![0_u8; n];
        let mut rx_alt_m = vec![0.0_f32; n];
        let rx_refl_db = vec![0.0_f32; n];

        // Sample inner core directly from `RealRasters` tile stores —
        // the same mmap pages were just warmed building the halo, so
        // this is entirely cache-hot. `rx_refl_db` reads from the
        // (possibly shared) halo: denser than the 3×3-on-mmap probe
        // and one fewer cache hop.
        //
        // Pixel-lattice loops: `py`/`px` index the receiver lat/lon vectors AND
        // compute the flat output offset `idx = py*TILE_PX + px`, so enumerate()
        // would not remove the manual indexing. Kept verbatim (raster sampling
        // feeds the byte-exact heatmap kernel).
        #[allow(clippy::needless_range_loop)]
        for py in 0..TILE_PX {
            let lat = rx_lat[py];
            let row_base = py * TILE_PX;
            #[allow(clippy::needless_range_loop)]
            for px in 0..TILE_PX {
                let lon = rx_lon[px];
                let idx = row_base + px;
                let elev = rasters.dem.sample(lat, lon) as f32;
                inner_elev_m[idx] = elev;
                inner_forest[idx] = rasters.forest.sample(lat, lon) as u8;
                inner_imd[idx] = rasters.imd.sample(lat, lon).round() as u8;
                rx_alt_m[idx] = elev + DEFAULT_RECEIVER_HEIGHT as f32;
            }
        }

        FusedTileZ13 {
            zoom,
            tile_x,
            tile_y,
            bbox,
            rx_lat,
            rx_lon,
            inner_elev_m,
            inner_forest,
            inner_imd,
            rx_alt_m,
            rx_refl_db,
            halo,
        }
    }

    /// Build only the inner core; reuse an externally provided halo.
    ///
    /// The halo's bbox must cover every propagation query the caller will
    /// make, usually because it was built once for the enclosing [`TileBatch`].
    pub fn build_with_halo(
        zoom: u8,
        tile_x: u32,
        tile_y: u32,
        rasters: &RealRasters,
        halo: Arc<FusedGrid>,
    ) -> Self {
        Self::build_with_halo_opt_rx_refl(zoom, tile_x, tile_y, rasters, halo)
    }

    /// Build the receiver lattice and altitude only. This is for NPD aircraft heatmap paths only: airborne
    /// and cruise read `rx_lat`, `rx_lon`, and `rx_alt_m`; cruise samples segment terrain from `RealRasters`
    /// directly. Surface layers and airport ground-ops must use the full constructor because they consume
    /// reflection, screening, vegetation, and ground rasters.
    pub fn build_receiver_altitude_only(
        zoom: u8,
        tile_x: u32,
        tile_y: u32,
        rasters: &RealRasters,
    ) -> Self {
        let bbox = TileBbox::from_xyz(zoom, tile_x, tile_y);

        let rx_lat: [f64; TILE_PX] = std::array::from_fn(|i| pixel_lat(&bbox, i as u32));
        let rx_lon: [f64; TILE_PX] = std::array::from_fn(|i| pixel_lon(&bbox, i as u32));

        let n = TILE_PX * TILE_PX;
        let mut inner_elev_m = vec![0.0_f32; n];
        let inner_forest = vec![0_u8; n];
        let inner_imd = vec![0_u8; n];
        let mut rx_alt_m = vec![0.0_f32; n];
        let rx_refl_db = vec![0.0_f32; n];

        // A z12 output tile usually lies inside one 1° DEM tile. Retain that mmap across the
        // whole receiver walk: `sample()` would reacquire the TileStore slot mutex, clone its Arc,
        // and update two LRU atomics for every one of the 262,144 pixels. `sample_cached()` runs
        // the identical configured bilinear interpolation while doing that lookup only when the
        // walk actually crosses a 1° boundary.
        let mut cached_dem_key = (i32::MIN, i32::MIN);
        let mut cached_dem_tile = None;
        // `py`/`px` drive both the lat/lon reads and `idx = py*TILE_PX + px`.
        #[allow(clippy::needless_range_loop)]
        for py in 0..TILE_PX {
            let lat = rx_lat[py];
            let row_base = py * TILE_PX;
            #[allow(clippy::needless_range_loop)]
            for px in 0..TILE_PX {
                let lon = rx_lon[px];
                let idx = row_base + px;
                let elev =
                    rasters
                        .dem
                        .sample_cached(lat, lon, &mut cached_dem_key, &mut cached_dem_tile)
                        as f32;
                inner_elev_m[idx] = elev;
                rx_alt_m[idx] = elev + DEFAULT_RECEIVER_HEIGHT as f32;
            }
        }

        FusedTileZ13 {
            zoom,
            tile_x,
            tile_y,
            bbox,
            rx_lat,
            rx_lon,
            inner_elev_m,
            inner_forest,
            inner_imd,
            rx_alt_m,
            rx_refl_db,
            halo: Arc::new(FusedGrid::empty()),
        }
    }

    /// Return true if `(lat, lon)` lies inside the inner-core bbox.
    #[inline]
    pub fn bbox_contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.bbox.south_lat
            && lat <= self.bbox.north_lat
            && lon >= self.bbox.west_lon
            && lon <= self.bbox.east_lon
    }

    /// Map `(lat, lon)` inside the bbox to inner-core flat index. Linear
    /// fractional interpolation across the Mercator pixel grid; sub-metre
    /// error across one base heatmap tile.
    #[inline]
    pub fn latlon_to_inner_idx(&self, lat: f64, lon: f64) -> usize {
        let n = TILE_PX as f64;
        let lat_frac = (self.bbox.north_lat - lat) / (self.bbox.north_lat - self.bbox.south_lat);
        let lon_frac = (lon - self.bbox.west_lon) / (self.bbox.east_lon - self.bbox.west_lon);
        let py = (lat_frac * n).floor().clamp(0.0, (TILE_PX - 1) as f64) as usize;
        let px = (lon_frac * n).floor().clamp(0.0, (TILE_PX - 1) as f64) as usize;
        py * TILE_PX + px
    }

    /// Receiver altitude (DEM + 4 m) at pixel.
    #[inline]
    pub fn rx_alt(&self, py: u32, px: u32) -> f32 {
        self.rx_alt_m[py as usize * TILE_PX + px as usize]
    }

    /// Pre-baked enclosure reflection bonus (0/1.5/3 dB) at pixel.
    #[inline]
    pub fn rx_refl(&self, py: u32, px: u32) -> f32 {
        self.rx_refl_db[py as usize * TILE_PX + px as usize]
    }
}

impl RasterSampler for FusedTileZ13 {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        if self.bbox_contains(lat, lon) {
            self.inner_elev_m[self.latlon_to_inner_idx(lat, lon)] as f64
        } else {
            self.halo.elevation(lat, lon)
        }
    }

    /// Clamped to `[0, 1]` like the other two implementations of this formula
    /// (`RealRasters::ground_g`, `FusedGrid::ground_g`) — `inner_imd` is filled
    /// by an `as u8` saturating cast of the raster sample, so a nodata or
    /// out-of-spec IMD cell above 100 would otherwise make G NEGATIVE. That is
    /// not merely a wrong level: literal CNOSSOS ground uses clamped path and
    /// source factors, and is bounded below by `GROUND_HARD_FLOOR_DB` only
    /// across `[0, 1]`.
    /// `scatter_band::budget_ub_lden` prices every pair's upper bound on exactly
    /// that floor (`GROUND_GAIN_UB_DB`, constants.rs). A negative G breaks
    /// `ub ≥ exact`, which the byte-stop asserts in RELEASE — so an unclamped
    /// cell here would abort a paint rather than mis-shade one pixel.
    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        if self.bbox_contains(lat, lon) {
            let imd = self.inner_imd[self.latlon_to_inner_idx(lat, lon)] as f64;
            (1.0 - imd / 100.0).clamp(0.0, 1.0)
        } else {
            self.halo.ground_g(lat, lon)
        }
    }

    fn building_enclosure(&self, lat: f64, lon: f64) -> f64 {
        // The 75 m metric probe spans cells beyond the inner core for
        // edge receivers. Every non-empty halo covers the 75 m probe, so it
        // sees real building data regardless of receiver position.
        self.halo.building_enclosure(lat, lon)
    }

    /// Without this override, `build_default` (trait fallback) zeroes
    /// `forest_u8` per sample — biased +3 dB vs popup in M8 parity. The halo
    /// runs the canonical bilateral walk over the caller-sized reach. The
    /// heatmap profile omits the popup's P3 peak augmentation — see
    /// `FusedGrid::build_path_profile`.
    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut noise_compute::propagation::PathProfile,
    ) {
        self.halo
            .build_path_profile(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out);
    }
}

impl FusedTileZ13 {
    /// [`RasterSampler::build_path_profile`] with the SURFACE-HEATMAP
    /// coarse-middle cadence — full-res near both ends, the smooth long-ray
    /// middle subsampled (see [`FusedGrid::build_path_profile_coarse_mid`]).
    /// Heatmap line/point/ground-ops scatter only; the popup is untouched.
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
        self.halo
            .build_path_profile_coarse_mid(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, cfg, out);
    }
}

/// Compute the halo bbox covering `inner_bbox` extended by `halo_m` on
/// each side. Production layers pass their own reach so the halo — and thus
/// the L3 working set — shrinks to what each source type actually needs.
fn halo_bbox_for(inner_bbox: &TileBbox, halo_m: f64) -> (f64, f64, f64, f64) {
    let centre_lat = (inner_bbox.north_lat + inner_bbox.south_lat) * 0.5;
    let halo_lat_deg = halo_m / M_PER_DEG_LAT;
    let halo_lon_deg = halo_m / m_per_deg_lon(centre_lat.to_radians()).max(1.0);
    (
        inner_bbox.south_lat - halo_lat_deg,
        inner_bbox.north_lat + halo_lat_deg,
        inner_bbox.west_lon - halo_lon_deg,
        inner_bbox.east_lon + halo_lon_deg,
    )
}

fn build_halo_for(rasters: &RealRasters, inner_bbox: &TileBbox, halo_m: f64) -> FusedGrid {
    let (lat_min, lat_max, lon_min, lon_max) = halo_bbox_for(inner_bbox, halo_m);
    FusedGrid::build(rasters, lat_min, lat_max, lon_min, lon_max)
}

/// A `batch_n × batch_n` block of [`FusedTileZ13`] tiles that share one
/// halo [`FusedGrid`] covering the entire block plus the requested reach on each
/// side. The shared halo is built once and reused across all tiles in
/// the block — the dominant cost saving versus N² per-tile halo builds.
///
/// Construction is sequential; the caller usually parallelises across
/// batches, not within them (each tile's scatter already uses rayon).
pub struct TileBatch {
    pub zoom: u8,
    pub base_x: u32,
    pub base_y: u32,
    pub batch_n: u32,
    pub tiles: Vec<FusedTileZ13>,
}

impl TileBatch {
    /// Heap bytes owned per tile: the six inner per-pixel vectors
    /// (3×f32 + 3×u8 = 15 B/px); the shared halo is counted once at
    /// batch level, never per tile.
    const INNER_BYTES_PER_PX: u64 = 15;

    /// The bbox `build` sizes its shared halo with — the ONE definition,
    /// shared with `estimate_heap_bytes` so the byte-gate's pre-image can
    /// never drift from the real allocation.
    fn batch_bbox(zoom: u8, base_x: u32, base_y: u32, batch_n: u32) -> TileBbox {
        let nw = TileBbox::from_xyz(zoom, base_x, base_y);
        let se = TileBbox::from_xyz(zoom, base_x + batch_n - 1, base_y + batch_n - 1);
        TileBbox {
            west_lon: nw.west_lon,
            east_lon: se.east_lon,
            north_lat: nw.north_lat,
            south_lat: se.south_lat,
        }
    }

    /// Actual heap bytes resident for this batch: inner vectors, each
    /// tile's own struct (the inline `rx_lat`/`rx_lon` receiver arrays live
    /// there), the `tiles` Vec, and the shared halo once. Feeds noise-gpu's
    /// process-wide pipeline byte gate, which corrects its pre-build
    /// reservation to this value.
    pub fn heap_bytes(&self) -> u64 {
        let inner: u64 = self
            .tiles
            .iter()
            .map(|t| {
                (t.inner_elev_m.capacity() * 4
                    + t.rx_alt_m.capacity() * 4
                    + t.rx_refl_db.capacity() * 4
                    + t.inner_forest.capacity()
                    + t.inner_imd.capacity()) as u64
            })
            .sum();
        let tiles_vec = (self.tiles.capacity() * std::mem::size_of::<FusedTileZ13>()) as u64;
        let halo = self.tiles.first().map(|t| t.halo.heap_bytes()).unwrap_or(0);
        inner + tiles_vec + halo
    }

    /// Exact pre-build size of `build(zoom, base, batch_n, halo_m)`: the
    /// same bbox → `FusedGrid::grid_dims` sizing the build itself performs
    /// (one source of truth), the fixed 15 B/px inner vectors, and the
    /// per-tile struct storage. Lets a byte-budget gate reserve BEFORE
    /// building; `heap_bytes()` afterwards only corrects allocator slack
    /// (normally zero — `vec![x; n]` and `with_capacity` are exact).
    pub fn estimate_heap_bytes(
        zoom: u8,
        base_x: u32,
        base_y: u32,
        batch_n: u32,
        halo_m: f64,
    ) -> u64 {
        let batch_bbox = Self::batch_bbox(zoom, base_x, base_y, batch_n);
        let (lat_min, lat_max, lon_min, lon_max) = halo_bbox_for(&batch_bbox, halo_m);
        let (rows, cols, _, _) = FusedGrid::grid_dims(lat_min, lat_max, lon_min, lon_max);
        let halo_bytes = (rows * cols * std::mem::size_of::<FusedPixel>()) as u64;
        let n_tiles = (batch_n as u64) * (batch_n as u64);
        let inner_bytes = n_tiles * (TILE_PX * TILE_PX) as u64 * Self::INNER_BYTES_PER_PX;
        let tiles_bytes = n_tiles * std::mem::size_of::<FusedTileZ13>() as u64;
        halo_bytes + inner_bytes + tiles_bytes
    }

    /// Build a batch whose north-west tile is at `(base_x, base_y)`.
    ///
    /// Tiles are stored in row-major (y-then-x) order: index
    /// `dy * batch_n + dx` → tile `(base_x + dx, base_y + dy)`.
    pub fn build(
        zoom: u8,
        base_x: u32,
        base_y: u32,
        batch_n: u32,
        halo_m: f64,
        rasters: &RealRasters,
    ) -> Self {
        Self::build_opt_rx_refl(zoom, base_x, base_y, batch_n, halo_m, rasters)
    }

    /// [`Self::build`] leaving `rx_refl_db` for the caller's vector pre-bake
    /// (`bake_tile_vector_rx_refl`); unrequested tiles are never painted and
    /// keep the neutral 0 dB.
    pub fn build_opt_rx_refl(
        zoom: u8,
        base_x: u32,
        base_y: u32,
        batch_n: u32,
        halo_m: f64,
        rasters: &RealRasters,
    ) -> Self {
        assert!(batch_n >= 1, "batch_n must be ≥ 1");
        let batch_bbox = Self::batch_bbox(zoom, base_x, base_y, batch_n);
        let halo = Arc::new(build_halo_for(rasters, &batch_bbox, halo_m));

        let mut tiles = Vec::with_capacity((batch_n * batch_n) as usize);
        for dy in 0..batch_n {
            for dx in 0..batch_n {
                tiles.push(FusedTileZ13::build_with_halo_opt_rx_refl(
                    zoom,
                    base_x + dx,
                    base_y + dy,
                    rasters,
                    halo.clone(),
                ));
            }
        }

        TileBatch {
            zoom,
            base_x,
            base_y,
            batch_n,
            tiles,
        }
    }

    /// Build a batch for NPD aircraft layers, which only need receiver latitude, longitude, and altitude.
    pub fn build_receiver_altitude_only(
        zoom: u8,
        base_x: u32,
        base_y: u32,
        batch_n: u32,
        rasters: &RealRasters,
    ) -> Self {
        assert!(batch_n >= 1, "batch_n must be ≥ 1");
        let mut tiles = Vec::with_capacity((batch_n * batch_n) as usize);
        for dy in 0..batch_n {
            for dx in 0..batch_n {
                tiles.push(FusedTileZ13::build_receiver_altitude_only(
                    zoom,
                    base_x + dx,
                    base_y + dy,
                    rasters,
                ));
            }
        }
        TileBatch {
            zoom,
            base_x,
            base_y,
            batch_n,
            tiles,
        }
    }

    /// Build the NPD receiver lattice while sharing a DEM halo for airborne
    /// receiver horizons and vector-building edge elevations.
    pub fn build_receiver_altitude_with_halo(
        zoom: u8,
        base_x: u32,
        base_y: u32,
        batch_n: u32,
        halo_m: f64,
        rasters: &RealRasters,
    ) -> Self {
        assert!(batch_n >= 1, "batch_n must be ≥ 1");
        let batch_bbox = Self::batch_bbox(zoom, base_x, base_y, batch_n);
        let halo = FusedTileZ13::build_elevation_halo(&batch_bbox, halo_m, rasters);
        Self::build_receiver_altitude_with_shared_halo(zoom, base_x, base_y, batch_n, rasters, halo)
    }

    /// Build NPD receiver tiles against an already-covered elevation halo.
    /// Region painters use this to share one 8 km grid across every block.
    pub fn build_receiver_altitude_with_shared_halo(
        zoom: u8,
        base_x: u32,
        base_y: u32,
        batch_n: u32,
        rasters: &RealRasters,
        halo: Arc<FusedGrid>,
    ) -> Self {
        assert!(batch_n >= 1, "batch_n must be ≥ 1");
        let mut tiles = Vec::with_capacity((batch_n * batch_n) as usize);
        for dy in 0..batch_n {
            for dx in 0..batch_n {
                let mut tile = FusedTileZ13::build_receiver_altitude_only(
                    zoom,
                    base_x + dx,
                    base_y + dy,
                    rasters,
                );
                tile.halo = halo.clone();
                tiles.push(tile);
            }
        }
        TileBatch {
            zoom,
            base_x,
            base_y,
            batch_n,
            tiles,
        }
    }
}

/// Read /sys L3 size and pick a default batch dimension N.
///
/// Working set per batch ≈ 17 MB halo (Praha) + N² × 2 MB inner (512² cells
/// since the 2026-07 shift; was 0.5 MB at 256²). N=2/3/4 → ~25/35/49 MB —
/// the three buckets still land inside the canonical Ryzen / EPYC L3 sizes
/// on our dev servers; everything is fallthrough-safe.
///
/// Override at runtime with a CLI flag — this is the default if none.
pub fn default_batch_size() -> u32 {
    match read_l3_size_mb() {
        Some(mb) if mb <= 40 => 2,
        Some(mb) if mb <= 70 => 3,
        _ => 4,
    }
}

fn read_l3_size_mb() -> Option<u32> {
    let raw = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cache/index3/size").ok()?;
    parse_cache_size_mb(raw.trim())
}

fn parse_cache_size_mb(s: &str) -> Option<u32> {
    // Linux exposes values like "32M", "96M", "32768K".
    let (digits, suffix) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = digits.parse().ok()?;
    let mb = match suffix {
        "" => n / (1024 * 1024),
        "K" | "KB" => n / 1024,
        "M" | "MB" => n,
        "G" | "GB" => n * 1024,
        _ => return None,
    };
    u32::try_from(mb).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_HALO_M: f64 = 8_000.0;
    use std::path::PathBuf;

    struct TempPreparedRaster {
        root: PathBuf,
    }

    impl Drop for TempPreparedRaster {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn receiver_dem_fixture() -> TempPreparedRaster {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "quiet-map-receiver-dem-{}-{unique}",
            std::process::id()
        ));
        let dem = root.join("dem/copernicus");
        std::fs::create_dir_all(&dem).expect("create DEM fixture directory");
        for (name, values) in [
            ("N49E015.hgt", [100_i16, 120, 140, 160]),
            ("N49E016.hgt", [200_i16, 220, 240, 260]),
        ] {
            let bytes: Vec<u8> = values.into_iter().flat_map(i16::to_be_bytes).collect();
            std::fs::write(dem.join(name), bytes).expect("write DEM fixture");
        }
        TempPreparedRaster { root }
    }

    fn dir_has_dem_tiles(root: &std::path::Path) -> bool {
        ["rasters/dem", "dem/copernicus", "dem/srtm"]
            .iter()
            .any(|rel| {
                std::fs::read_dir(root.join(rel))
                    .map(|mut iter| {
                        iter.any(|e| {
                            e.ok()
                                .is_some_and(|e| e.path().extension().is_some_and(|x| x == "hgt"))
                        })
                    })
                    .unwrap_or(false)
            })
    }

    fn data_root() -> Option<PathBuf> {
        // Cargo runs tests from the crate root, so resolve relative to it.
        // Dev4 year dir first (`<prepared>/2026`), then the legacy dev1
        // `<prepared>` root. Returns None (callers skip) when no DEM tiles
        // are on disk — an empty checkout dir is not usable data.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        for rel in ["../../data/prepared/2026", "../../data/prepared"] {
            let p = PathBuf::from(manifest_dir).join(rel);
            if dir_has_dem_tiles(&p) {
                return Some(p);
            }
        }
        None
    }

    #[test]
    fn praha_tile_bbox_round_trips() {
        // Praha LKPR (50.10°N, 14.26°E) → z=12 tile (2210, 1386).
        let bbox = TileBbox::from_xyz(12, 2210, 1386);
        assert!(
            bbox.south_lat > 49.9 && bbox.north_lat < 50.3,
            "lat range {:.3}..{:.3}",
            bbox.south_lat,
            bbox.north_lat
        );
        assert!(
            bbox.west_lon > 14.0 && bbox.east_lon < 14.5,
            "lon range {:.3}..{:.3}",
            bbox.west_lon,
            bbox.east_lon
        );
    }

    #[test]
    fn pixel_size_at_praha_base_zoom() {
        // z12 with 512-px tiles = the same physical lattice as the old
        // z13@256, so the pixel size at Praha must still be ~12.3 m.
        let px_m = tile_pixel_size_m(12, 50.10);
        assert!(
            (px_m - 12.3).abs() < 0.5,
            "z=12 px size at Praha = {px_m:.3} m"
        );
    }

    #[test]
    fn pixel_centres_inside_bbox() {
        let bbox = TileBbox::from_xyz(12, 2210, 1386);
        let lat = pixel_lat(&bbox, 256);
        let lon = pixel_lon(&bbox, 256);
        assert!(lat <= bbox.north_lat && lat >= bbox.south_lat);
        assert!(lon >= bbox.west_lon && lon <= bbox.east_lon);
    }

    #[test]
    fn latlon_round_trip_via_inner_idx() {
        let bbox = TileBbox::from_xyz(12, 2210, 1386);
        let bbox_centre_lat = (bbox.north_lat + bbox.south_lat) * 0.5;
        let bbox_centre_lon = (bbox.east_lon + bbox.west_lon) * 0.5;
        let n = TILE_PX as f64;
        let lat_frac = (bbox.north_lat - bbox_centre_lat) / (bbox.north_lat - bbox.south_lat);
        let lon_frac = (bbox_centre_lon - bbox.west_lon) / (bbox.east_lon - bbox.west_lon);
        let py = (lat_frac * n).floor() as usize;
        let px = (lon_frac * n).floor() as usize;
        assert_eq!(py, TILE_PX / 2);
        assert_eq!(px, TILE_PX / 2);
    }

    #[test]
    fn build_smoke() {
        let Some(root) = data_root() else {
            eprintln!("data/prepared not present; skipping FusedTileZ13 smoke build");
            return;
        };
        let rasters = RealRasters::new(&root);
        let tile = FusedTileZ13::build(12, 2246, 1411, TEST_HALO_M, &rasters);
        assert_eq!(tile.zoom, 12);
        assert_eq!(tile.inner_elev_m.len(), TILE_PX * TILE_PX);
        assert_eq!(tile.rx_alt_m.len(), TILE_PX * TILE_PX);
        // Praha DEM around 200-400 m; receiver alt = DEM + 4 m.
        let mid = tile.rx_alt(256, 256);
        assert!(
            mid > 100.0 && mid < 500.0,
            "Praha tile centre alt = {mid} m"
        );
    }

    #[test]
    fn receiver_altitude_cache_matches_direct_sampling_across_dem_boundary() {
        let fixture = receiver_dem_fixture();
        let rasters = RealRasters::new(&fixture.root);
        // z12/x2230 straddles 16°E, exercising the cached sampler's key transition between
        // N49E015 and N49E016 rather than validating only the common one-source-tile case.
        let cache_touches_before = rasters.dem.cache_touch_count();
        let cached_started = std::time::Instant::now();
        let tile = FusedTileZ13::build_receiver_altitude_only(12, 2230, 1403, &rasters);
        let cached_elapsed = cached_started.elapsed();
        let cache_touches = rasters.dem.cache_touch_count() - cache_touches_before;
        assert!(
            cache_touches <= (2 * TILE_PX) as u64,
            "receiver walk performed {cache_touches} cache-slot lookups; expected at most two \
             1° DEM transitions per row"
        );
        let direct_started = std::time::Instant::now();
        for py in 0..TILE_PX {
            for px in 0..TILE_PX {
                let idx = py * TILE_PX + px;
                let direct = rasters.dem.sample(tile.rx_lat[py], tile.rx_lon[px]) as f32;
                assert_eq!(
                    tile.inner_elev_m[idx].to_bits(),
                    direct.to_bits(),
                    "DEM mismatch at receiver ({py},{px})"
                );
                assert_eq!(
                    tile.rx_alt_m[idx].to_bits(),
                    (direct + DEFAULT_RECEIVER_HEIGHT as f32).to_bits(),
                    "receiver altitude mismatch at ({py},{px})"
                );
            }
        }
        eprintln!(
            "receiver altitude: cached builder {cached_elapsed:?}, direct parity walk {:?}",
            direct_started.elapsed()
        );
    }

    #[test]
    fn parse_cache_size_handles_linux_formats() {
        assert_eq!(parse_cache_size_mb("32M"), Some(32));
        assert_eq!(parse_cache_size_mb("96M"), Some(96));
        assert_eq!(parse_cache_size_mb("32768K"), Some(32));
        assert_eq!(parse_cache_size_mb("1G"), Some(1024));
        assert_eq!(parse_cache_size_mb("nope"), None);
        assert_eq!(parse_cache_size_mb("32X"), None);
    }

    #[test]
    fn batch_shares_halo_across_all_tiles() {
        let Some(root) = data_root() else {
            eprintln!("data/prepared not present; skipping TileBatch smoke");
            return;
        };
        let rasters = RealRasters::new(&root);
        let batch = TileBatch::build(12, 2210, 1386, 2, TEST_HALO_M, &rasters);
        assert_eq!(batch.tiles.len(), 4);
        // All four tiles must point at the same FusedGrid allocation.
        let halo0 = Arc::as_ptr(&batch.tiles[0].halo);
        for t in &batch.tiles[1..] {
            assert_eq!(Arc::as_ptr(&t.halo), halo0, "halo not shared");
        }
        // Row-major tile ordering: (dx, dy) → idx dy*N + dx.
        assert_eq!(batch.tiles[0].tile_x, 2210);
        assert_eq!(batch.tiles[0].tile_y, 1386);
        assert_eq!(batch.tiles[1].tile_x, 2211);
        assert_eq!(batch.tiles[1].tile_y, 1386);
        assert_eq!(batch.tiles[2].tile_x, 2210);
        assert_eq!(batch.tiles[2].tile_y, 1387);
        assert_eq!(batch.tiles[3].tile_x, 2211);
        assert_eq!(batch.tiles[3].tile_y, 1387);
    }
}

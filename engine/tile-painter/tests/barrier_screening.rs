//! B8/C9 barrier wiring proofs on a synthetic flat-DEM tile.
//!
//! Fixture: a real `FusedTileZ13` built from an EMPTY rasters dir (missing
//! tiles are negative-cached defaults → elevation 0, buildings 0, forest 0,
//! IMD 100 → G = 0), one N-S line/point source west of an N-S 3 m noise wall
//! (chain of ~60 m microsegments, the real `barriers.arrow` shape), receivers
//! on the whole 512² lattice at DEM + 4 m.
//!
//! 1. Wall effect: kernel energy WITH barriers < WITHOUT, behind the wall.
//! 2. Popup↔kernel parity: the kernel's per-pixel energy equals a manual
//!    replay of the same physics calling `path_effects::screening_attenuation`
//!    directly with the same slice — proves the kernels feed barriers to the
//!    shared popup kernel unchanged.
//! 3. /gg W2 quantified comparison: CPU vector-barrier arm vs the
//!    GPU-burn-equivalent arm (wall burned into the halo's building channel
//!    via `FusedGrid::burn_building_max`, empty vector slice) — per-pixel Δ
//!    stats against the plan's ≤1.0 dB wall-adjacent gate. MEASURED VERDICT
//!    (2026-06-11): the gate fails decisively (mean +3.7 / max +5.9 dB
//!    under-screening at 45 m road→wall; max +13.8 dB at the D11-like 27 m),
//!    because the bilateral ray cadence (~30–245 m sample spacing) steps over
//!    a one-cell (~20 m lon at 50°N) burned wall column on most paths, while
//!    the vector path maps the wall onto the nearest EXISTING sample and
//!    never misses. Thin-line features cannot be faithfully burned into a
//!    raster sampled at ≥ cell-size cadence — so the BURN is rejected; the GPU
//!    line kernel instead screens the same VECTOR slice behind `QM_GPU_BARRIERS`
//!    (the `w2_gpu_vector_crossings_match_cpu_oracle` arm below pins its
//!    crossings to the CPU oracle; divergence documented in SPEC §3.6 and
//!    gpu_surface.rs). The test stays as the decision record: it asserts
//!    the measured divergence, so if cadence/raster resolution ever changes
//!    enough for the burn to become viable, it fails loudly and the decision
//!    must be revisited. Run with `--nocapture` for the stats table.

use std::f64::consts::{LN_10, PI};
use std::path::Path;
use std::sync::Arc;

use noise_compute::constants::{ALPHA_ATM, A_WEIGHTING, M_PER_DEG_LAT, M_PER_DEG_LON_EQ};
use noise_compute::propagation::geo::{finite_line_correction, point_to_segment_full};
use noise_compute::propagation::iso9613::{fast_exp_f64, ground_atten_db};
use noise_compute::propagation::path_effects;
use noise_compute::propagation::PathProfile;
use noise_compute::types::{Barrier, RasterSampler, BARRIER_PATH_HORIZON_M};
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_barrier::{BarrierData, BarrierSeg};
use tile_painter::source_point::PointRow;
use tile_painter::{scatter_line, scatter_point};

const NUM_BANDS: usize = 8;

/// Flat synthetic tile: z12 2211/1386 (~50.07°N, 14.30°E, Praha latitudes so
/// lon cells are ~19.9 m), 2 km halo, all rasters at their missing-tile
/// defaults.
fn flat_tile() -> FusedTileZ13 {
    let rasters = RealRasters::new(Path::new("/nonexistent-quietmap-barrier-fixture"));
    FusedTileZ13::build(12, 2211, 1386, 2_000.0, &rasters)
}

fn centre(tile: &FusedTileZ13) -> (f64, f64) {
    (
        (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5,
        (tile.bbox.west_lon + tile.bbox.east_lon) * 0.5,
    )
}

fn m_to_deg_lat(m: f64) -> f64 {
    m / M_PER_DEG_LAT
}

fn m_to_deg_lon(m: f64, lat: f64) -> f64 {
    m / (111_320.0 * lat.to_radians().cos())
}

/// N-S wall at `wall_lon` spanning ±extent_m around `c_lat`, split into
/// ~60 m microsegments like real `barriers.arrow` rows.
fn wall_segments(c_lat: f64, wall_lon: f64, extent_m: f64, height_m: f32) -> Vec<BarrierSeg> {
    let seg_len_m = 60.0;
    let n = ((2.0 * extent_m) / seg_len_m).round() as usize;
    (0..n)
        .map(|i| {
            let a = c_lat + m_to_deg_lat(-extent_m + i as f64 * seg_len_m);
            let b = c_lat + m_to_deg_lat(-extent_m + (i + 1) as f64 * seg_len_m);
            BarrierSeg {
                osm_id: 1_354_881_685 + i as i64,
                start_lat: a,
                start_lon: wall_lon,
                end_lat: b,
                end_lon: wall_lon,
                height_m,
            }
        })
        .collect()
}

/// N-S road through the tile centre at `road_lon` (one 800 m LineRow, flat
/// 90 dB/m bands in every period).
fn road_line(c_lat: f64, road_lon: f64) -> LineRow {
    let half = m_to_deg_lat(400.0);
    let em: [f32; NUM_BANDS] = [10f32.powf(9.0); NUM_BANDS]; // 90 dB → linear
    LineRow {
        start_lat: c_lat - half,
        start_lon: road_lon,
        end_lat: c_lat + half,
        end_lon: road_lon,
        length_m: 800.0,
        max_distance_m: 10_000.0,
        source_height_m: 0.05,
        bridge: false,
        emission_lin: [em, em, em],
    }
}

fn px_of(tile: &FusedTileZ13, lon: f64) -> usize {
    let f = (lon - tile.bbox.west_lon) / (tile.bbox.east_lon - tile.bbox.west_lon);
    ((f * TILE_PX as f64).floor()).clamp(0.0, (TILE_PX - 1) as f64) as usize
}

fn py_of(tile: &FusedTileZ13, lat: f64) -> usize {
    let f = (tile.bbox.north_lat - lat) / (tile.bbox.north_lat - tile.bbox.south_lat);
    ((f * TILE_PX as f64).floor()).clamp(0.0, (TILE_PX - 1) as f64) as usize
}

fn day_energy(acc: &TileAccumulator, py: usize, px: usize) -> f64 {
    acc.energy[(py * TILE_PX + px) * 3] as f64
}

/// Wall effect + popup↔kernel parity for the LINE kernel.
#[test]
fn line_kernel_applies_vector_barriers() {
    let tile = flat_tile();
    let (c_lat, c_lon) = centre(&tile);
    let road_lon = c_lon;
    let wall_lon = c_lon + m_to_deg_lon(45.0, c_lat);
    let line = road_line(c_lat, road_lon);
    let lines = vec![line];
    let segs = wall_segments(c_lat, wall_lon, 360.0, 3.0);
    let barriers = BarrierData::from_segments(segs).for_tile(&tile.bbox, 10_000.0);
    assert!(!barriers.is_empty());

    let mut acc_no = TileAccumulator::new();
    scatter_line::scatter_tile(&tile, &lines, &[], None, &mut acc_no);
    let mut acc_wall = TileAccumulator::new();
    scatter_line::scatter_tile(&tile, &lines, &barriers, None, &mut acc_wall);

    // Shadow receiver ~60 m east of the wall at the wall's mid-latitude.
    let rx_lon = wall_lon + m_to_deg_lon(60.0, c_lat);
    let (py, px) = (py_of(&tile, c_lat), px_of(&tile, rx_lon));
    let e_no = day_energy(&acc_no, py, px);
    let e_wall = day_energy(&acc_wall, py, px);
    assert!(e_no > 0.0 && e_wall > 0.0);
    let drop_db = 10.0 * (e_no / e_wall).log10();
    assert!(
        drop_db > 1.0,
        "3 m wall must screen the shadow pixel by >1 dB, got {drop_db:.2} dB"
    );

    // Popup↔kernel parity: manual replay of scatter_band's exact op sequence
    // for that pixel, calling the shared path_effects kernel directly with
    // the same barrier slice.
    let l = &lines[0];
    let rx_lat = tile.rx_lat[py];
    let rx_lon = tile.rx_lon[px];
    let pts = point_to_segment_full(
        rx_lat,
        rx_lon,
        l.start_lat,
        l.start_lon,
        l.end_lat,
        l.end_lon,
    );
    let dist_m = pts.d_endpoint_m;
    assert!(dist_m <= l.max_distance_m);
    let idx = py * TILE_PX + px;
    let rx_alt = tile.rx_alt_m[idx] as f64;
    let src_alt = tile.elevation(pts.cp_lat, pts.cp_lon) + l.source_height_m;
    let d_slant = (dist_m * dist_m + (src_alt - rx_alt).powi(2))
        .sqrt()
        .max(1.0);
    let flc = finite_line_correction(l.length_m as f64, dist_m, pts.fraction.clamp(0.0, 1.0));
    let refl = tile.rx_refl_db[idx] as f64;
    let base_db = refl + flc - 10.0 * (2.0 * PI * d_slant).log10();
    let atm_d_km = d_slant / 1000.0;

    let mut profile = PathProfile::new();
    tile.build_path_profile(pts.cp_lat, pts.cp_lon, rx_lat, rx_lon, dist_m, &mut profile);
    let ground_g = path_effects::ground_g_from_profile(&profile);
    let terrain = path_effects::terrain_attenuation(&mut profile, src_alt, rx_alt);
    let screening = path_effects::screening_attenuation(
        &mut profile,
        &barriers,
        path_effects::ObstacleInput::CANDIDATES_OFF,
        src_alt,
        rx_alt,
        0.0,
        &terrain,
    );
    assert!(
        screening.iter().any(|&s| s > 0.0),
        "direct screening_attenuation must see the wall on this path"
    );
    let veg = path_effects::vegetation_attenuation_path(&profile);
    let mut expected = 0.0f64;
    for i in 0..NUM_BANDS {
        let a_gr = ground_atten_db(i, ground_g);
        let a_bar = terrain[i] + screening[i];
        let gob = if a_bar > 0.0 { a_gr.max(a_bar) } else { a_gr };
        let path_db = base_db - ALPHA_ATM[i] * atm_d_km - gob - veg[i];
        expected +=
            l.emission_lin[0][i] as f64 * fast_exp_f64((path_db + A_WEIGHTING[i]) * LN_10 * 0.1);
    }
    let rel = (e_wall - expected).abs() / expected.max(f64::MIN_POSITIVE);
    assert!(
        rel < 1e-6,
        "kernel pixel energy {e_wall:.6e} != direct path_effects replay {expected:.6e} (rel {rel:.2e})"
    );
}

/// Wall effect + popup↔kernel parity for the POINT kernel.
#[test]
fn point_kernel_applies_vector_barriers() {
    let tile = flat_tile();
    let (c_lat, c_lon) = centre(&tile);
    let src_lon = c_lon;
    let wall_lon = c_lon + m_to_deg_lon(45.0, c_lat);
    let em: [f32; NUM_BANDS] = [10f32.powf(10.0); NUM_BANDS]; // 100 dB bands
    let point = PointRow {
        lat: c_lat,
        lon: src_lon,
        source_height_m: 0.5, // below the 3 m wall so the LOS to a 4 m receiver is cut
        max_distance_m: 5_000.0,
        exclusion_radius_m: 0.0,
        max_day_emission_db: 100.0,
        emission_lin: [em, em, em],
    };
    let points = vec![point];
    let segs = wall_segments(c_lat, wall_lon, 360.0, 3.0);
    let barriers = BarrierData::from_segments(segs).for_tile(&tile.bbox, 10_000.0);

    let mut acc_no = TileAccumulator::new();
    scatter_point::scatter_tile(&tile, &points, &[], None, &mut acc_no);
    let mut acc_wall = TileAccumulator::new();
    scatter_point::scatter_tile(&tile, &points, &barriers, None, &mut acc_wall);

    let rx_lon = wall_lon + m_to_deg_lon(60.0, c_lat);
    let (py, px) = (py_of(&tile, c_lat), px_of(&tile, rx_lon));
    let e_no = day_energy(&acc_no, py, px);
    let e_wall = day_energy(&acc_wall, py, px);
    assert!(e_no > 0.0 && e_wall > 0.0);
    let drop_db = 10.0 * (e_no / e_wall).log10();
    assert!(
        drop_db > 1.0,
        "3 m wall must screen the shadow pixel by >1 dB, got {drop_db:.2} dB"
    );

    // Manual replay of scatter_point::scatter_band for that pixel.
    let p = &points[0];
    let rx_lat = tile.rx_lat[py];
    let rx_lon = tile.rx_lon[px];
    let dist_m = noise_compute::propagation::geo::flat_dist(rx_lat, rx_lon, p.lat, p.lon);
    let idx = py * TILE_PX + px;
    let rx_alt = tile.rx_alt_m[idx] as f64;
    let src_alt = tile.elevation(p.lat, p.lon) + p.source_height_m;
    let prop_dist =
        noise_compute::propagation::geo::effective_area_source_dist(dist_m, p.exclusion_radius_m);
    let d_slant = noise_compute::propagation::geo::slant_dist(prop_dist, src_alt, rx_alt).max(1.0);
    let refl = tile.rx_refl_db[idx] as f64;
    let base_db = refl - (20.0 * d_slant.log10() + 11.0);
    let atm_d_km = d_slant / 1000.0;
    let mut profile = PathProfile::new();
    tile.build_path_profile(p.lat, p.lon, rx_lat, rx_lon, dist_m, &mut profile);
    let ground_g = tile.ground_g(rx_lat, rx_lon);
    let terrain = path_effects::terrain_attenuation(&mut profile, src_alt, rx_alt);
    let screening = path_effects::screening_attenuation(
        &mut profile,
        &barriers,
        path_effects::ObstacleInput::CANDIDATES_OFF,
        src_alt,
        rx_alt,
        p.exclusion_radius_m,
        &terrain,
    );
    assert!(screening.iter().any(|&s| s > 0.0));
    let veg = path_effects::vegetation_attenuation_path(&profile);
    let mut expected = 0.0f64;
    for i in 0..NUM_BANDS {
        let a_gr = ground_atten_db(i, ground_g);
        let a_bar = terrain[i] + screening[i];
        let gob = if a_bar > 0.0 { a_gr.max(a_bar) } else { a_gr };
        let path_db = base_db - ALPHA_ATM[i] * atm_d_km - gob - veg[i];
        expected +=
            p.emission_lin[0][i] as f64 * fast_exp_f64((path_db + A_WEIGHTING[i]) * LN_10 * 0.1);
    }
    let rel = (e_wall - expected).abs() / expected.max(f64::MIN_POSITIVE);
    assert!(
        rel < 1e-6,
        "kernel pixel energy {e_wall:.6e} != direct path_effects replay {expected:.6e} (rel {rel:.2e})"
    );
}

/// CPU oracle arm: verbatim replica of the barrier crossing scan in
/// `path_effects::screening_attenuation_with_meta` §1 (f64 midpoint cosine, no
/// clamp; `obstacle_index::segment_intersection_t`). Returns one
/// `(chainage, height)` per wall segment the ray actually crosses.
fn cpu_crossings(
    src: (f64, f64),
    rcv: (f64, f64),
    dist_m: f64,
    barriers: &[Barrier],
) -> Vec<(f64, f32)> {
    let (src_lat, src_lon) = src;
    let (rcv_lat, rcv_lon) = rcv;
    let meters_per_deg_lon = M_PER_DEG_LON_EQ * ((src_lat + rcv_lat) * 0.5).to_radians().cos();
    let path_dx_m = (rcv_lon - src_lon) * meters_per_deg_lon;
    let path_dy_m = (rcv_lat - src_lat) * M_PER_DEG_LAT;
    let mut out = Vec::new();
    for b in barriers {
        if b.dist_m > dist_m + BARRIER_PATH_HORIZON_M {
            break;
        }
        let x0 = (b.start_lon - src_lon) * meters_per_deg_lon;
        let y0 = (b.start_lat - src_lat) * M_PER_DEG_LAT;
        let x1 = (b.end_lon - src_lon) * meters_per_deg_lon;
        let y1 = (b.end_lat - src_lat) * M_PER_DEG_LAT;
        if let Some(t) = ray_segment_t(path_dx_m, path_dy_m, x0, y0, x1, y1) {
            out.push((t, b.height_m));
        }
    }
    out
}

/// GPU arm: replica of the scatter.cu `barrier_best_candidate` scan — the SAME
/// intersection with the kernel's one deviation: an f32 midpoint cosine clamped
/// at 0.01 (`__cosf` house style).
fn gpu_crossings(
    src: (f64, f64),
    rcv: (f64, f64),
    dist_m: f64,
    barriers: &[Barrier],
) -> Vec<(f64, f32)> {
    let (cplat, cplon) = src;
    let (rlat, rlon) = rcv;
    let mut out = Vec::new();
    if barriers.is_empty() || barriers[0].dist_m > dist_m + BARRIER_PATH_HORIZON_M {
        return out;
    }
    let ray_mid = (cplat + rlat) * 0.5 * (std::f64::consts::PI / 180.0);
    let ray_mlon = M_PER_DEG_LON_EQ * f64::from((ray_mid as f32).cos().max(0.01f32));
    let pdx = (rlon - cplon) * ray_mlon;
    let pdy = (rlat - cplat) * M_PER_DEG_LAT;
    for b in barriers {
        if b.dist_m > dist_m + BARRIER_PATH_HORIZON_M {
            break;
        }
        let x0 = (b.start_lon - cplon) * ray_mlon;
        let y0 = (b.start_lat - cplat) * M_PER_DEG_LAT;
        let x1 = (b.end_lon - cplon) * ray_mlon;
        let y1 = (b.end_lat - cplat) * M_PER_DEG_LAT;
        if let Some(t) = ray_segment_t(pdx, pdy, x0, y0, x1, y1) {
            out.push((t, b.height_m));
        }
    }
    out
}

/// `obstacle_index::segment_intersection_t` / scatter.cu `seg_isect_t` with the
/// ray anchored at the origin — the one primitive both lanes run on a wall.
fn ray_segment_t(dx: f64, dy: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<f64> {
    let (ex, ey) = (x1 - x0, y1 - y0);
    let denom = dx * ey - dy * ex;
    if denom == 0.0 {
        return None;
    }
    let t = (x0 * ey - y0 * ex) / denom;
    let u = (x0 * dy - y0 * dx) / denom;
    (t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u)).then_some(t)
}

/// W2 GPU-vector arm — the spike's host-side unit check, now on Fix 3's exact
/// geometry. The CUDA port's only NEW math is the ray×wall intersection (the
/// δ race + single-edge are the kernel's already-validated candidate path). So
/// pin exactly that: replay the kernel's scan in Rust against a verbatim
/// replica of the CPU's, over every audible pixel of the burn-record fixture at
/// both spacings, and require the same crossings at the same chainages.
/// Identical crossings ⇒ identical candidates ⇒ identical screening — the
/// inverse of the burn record's cadence-miss (an intersection cannot step over
/// a wall). The chainage tolerance is the kernel's `__cosf` lon scale: 1e-6
/// relative on a scale factor is ≤ 1 cm of a 10 km path.
#[test]
fn w2_gpu_vector_crossings_match_cpu_oracle() {
    for spacing_m in [45.0, 27.0] {
        let tile = flat_tile();
        let (c_lat, c_lon) = centre(&tile);
        let wall_lon = c_lon + m_to_deg_lon(spacing_m, c_lat);
        let line = road_line(c_lat, c_lon);
        let segs = wall_segments(c_lat, wall_lon, 360.0, 3.0);
        let barriers = BarrierData::from_segments(segs).for_tile(&tile.bbox, 10_000.0);
        assert!(!barriers.is_empty());

        let mut pixels_with_hits = 0usize;
        let mut crossings_total = 0usize;
        let mut mismatched_pixels = 0usize;
        let mut worst_dt = 0.0f64;
        for py in 0..TILE_PX {
            let rx_lat = tile.rx_lat[py];
            for px in 0..TILE_PX {
                let rx_lon = tile.rx_lon[px];
                let pts = point_to_segment_full(
                    rx_lat,
                    rx_lon,
                    line.start_lat,
                    line.start_lon,
                    line.end_lat,
                    line.end_lon,
                );
                let dist_m = pts.d_endpoint_m;
                if dist_m < 30.0 {
                    continue; // screening gate (n<3 || dist<30) — kernel parity
                }
                let src = (pts.cp_lat, pts.cp_lon);
                let rcv = (rx_lat, rx_lon);
                let cpu = cpu_crossings(src, rcv, dist_m, &barriers);
                let gpu = gpu_crossings(src, rcv, dist_m, &barriers);
                let same = cpu.len() == gpu.len()
                    && cpu.iter().zip(&gpu).all(|(a, b)| {
                        worst_dt = worst_dt.max((a.0 - b.0).abs());
                        (a.0 - b.0).abs() < 1e-6 && a.1 == b.1
                    });
                if !same {
                    mismatched_pixels += 1;
                    if mismatched_pixels <= 5 {
                        eprintln!(
                            "mismatch @py={py} px={px} dist={dist_m:.1} m: cpu={cpu:?} gpu={gpu:?}"
                        );
                    }
                }
                if !cpu.is_empty() {
                    pixels_with_hits += 1;
                    crossings_total += cpu.len();
                }
            }
        }
        println!(
            "W2 GPU-vector crossing parity @ {spacing_m} m: {pixels_with_hits} pixels cross the \
             wall ({crossings_total} crossings), {mismatched_pixels} mismatched pixels of {} \
             compared, worst |Δt| {worst_dt:.2e}",
            TILE_PX * TILE_PX
        );
        // The wall spans 720 m of a 512 px tile, so every pixel east of it on a
        // path from the road crosses it; half the tile is the vacuity guard.
        assert!(
            pixels_with_hits > 3_500,
            "fixture must exercise the crossing broadly, got {pixels_with_hits} pixels"
        );
        assert_eq!(
            mismatched_pixels, 0,
            "GPU intersection replica must match the CPU oracle at {spacing_m} m"
        );
    }
}

struct DeltaStats {
    n: usize,
    /// Mean of SIGNED Δ = 10·log10(E_burn / E_vector); positive ⇒ the burn
    /// arm reads hotter (under-screens vs the popup-parity vector arm).
    mean_signed: f64,
    p95_abs: f64,
    max_abs: f64,
}

fn delta_stats(deltas: &[f64]) -> DeltaStats {
    if deltas.is_empty() {
        return DeltaStats {
            n: 0,
            mean_signed: 0.0,
            p95_abs: 0.0,
            max_abs: 0.0,
        };
    }
    let n = deltas.len();
    let mut abs: Vec<f64> = deltas.iter().map(|d| d.abs()).collect();
    abs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    DeltaStats {
        n,
        mean_signed: deltas.iter().sum::<f64>() / n as f64,
        p95_abs: abs[((n as f64 * 0.95) as usize).min(n - 1)],
        max_abs: abs[n - 1],
    }
}

/// One W2 arm pair at a given road→wall spacing: returns per-pixel signed
/// Δ dB = 10·log10(E_burn/E_vector) partitioned into (all, shadow-side
/// wall-adjacent ≤150 m, source-side wall-adjacent ≤150 m), plus the
/// presence-flip count.
fn run_w2_comparison(spacing_m: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>, usize) {
    let tile_v = flat_tile();
    let (c_lat, c_lon) = centre(&tile_v);
    let road_lon = c_lon;
    let wall_lon = c_lon + m_to_deg_lon(spacing_m, c_lat);
    let lines = vec![road_line(c_lat, road_lon)];
    let segs = wall_segments(c_lat, wall_lon, 360.0, 3.0);

    // GPU-burn-equivalent arm: wall burned into a fresh tile's halo building
    // channel (rx_refl_db was pre-baked at build → unburned, exactly the
    // semantics a production GPU burn would have had), EMPTY vector slice.
    let mut tile_b = flat_tile();
    {
        let halo = Arc::get_mut(&mut tile_b.halo).expect("fixture tile owns its halo");
        for s in &segs {
            halo.burn_building_max(s.start_lat, s.start_lon, s.end_lat, s.end_lon, s.height_m);
        }
    }
    let mut acc_b = TileAccumulator::new();
    scatter_line::scatter_tile(&tile_b, &lines, &[], None, &mut acc_b);

    // Vector arm: the SAME wall as exact midpoint barriers through the popup
    // kernel (burn borrowed `segs` above; the vector arm consumes it now).
    let barriers = BarrierData::from_segments(segs).for_tile(&tile_v.bbox, 10_000.0);
    let mut acc_v = TileAccumulator::new();
    scatter_line::scatter_tile(&tile_v, &lines, &barriers, None, &mut acc_v);

    let wall_n = c_lat + m_to_deg_lat(360.0);
    let wall_s = c_lat - m_to_deg_lat(360.0);
    let mut all = Vec::new();
    let mut shadow_adj = Vec::new();
    let mut source_adj = Vec::new();
    let mut presence_flips = 0usize;
    for py in 0..TILE_PX {
        for px in 0..TILE_PX {
            let ev = day_energy(&acc_v, py, px);
            let eb = day_energy(&acc_b, py, px);
            if (ev > 0.0) != (eb > 0.0) {
                presence_flips += 1;
                continue;
            }
            if ev <= 0.0 {
                continue;
            }
            let d = 10.0 * (eb / ev).log10(); // signed: + ⇒ burn hotter
            all.push(d);
            let rx_lat = tile_v.rx_lat[py];
            let rx_lon = tile_v.rx_lon[px];
            let pts = point_to_segment_full(rx_lat, rx_lon, wall_s, wall_lon, wall_n, wall_lon);
            if pts.d_endpoint_m <= 150.0 {
                if rx_lon >= wall_lon {
                    shadow_adj.push(d);
                } else {
                    source_adj.push(d);
                }
            }
        }
    }
    (all, shadow_adj, source_adj, presence_flips)
}

/// /gg W2: quantified CPU-vector vs GPU-burn comparison — the C9 DECISION
/// RECORD. The plan's viability gate was max |Δ| ≤ 1.0 dB at wall-adjacent
/// SHADOW pixels (the user-facing quiet side); measured 2026-06-11 it fails
/// ~6× over (the cadence steps over the one-cell burned wall — see module
/// docs), so B8 shipped CPU-only. The assertions pin the measured failure
/// mode: if this test ever starts failing because the burn arm AGREES with
/// the vector arm (max ≤ 1 dB), the sampling model has changed enough that
/// the GPU burn decision must be re-evaluated — do that, don't just flip the
/// assert.
#[test]
fn w2_vector_vs_burn_quantified_decision_record() {
    let (all45, shadow45, src45, flips45) = run_w2_comparison(45.0);
    let (all27, shadow27, src27, flips27) = run_w2_comparison(27.0);

    println!("W2 CPU-vector vs GPU-burn-equivalent Δ dB = 10·log10(E_burn/E_vector)");
    println!("(flat DEM, 3 m wall, 720 m chain of 60 m segs; + ⇒ burn under-screens)");
    println!("  spacing  set                      n   mean(signed)  p95|Δ|  max|Δ|");
    for (label, name, deltas) in [
        ("45 m", "all audible", &all45),
        ("45 m", "shadow ≤150 m", &shadow45),
        ("45 m", "source side ≤150 m", &src45),
        ("27 m", "all audible", &all27),
        ("27 m", "shadow ≤150 m", &shadow27),
        ("27 m", "source side ≤150 m", &src27),
    ] {
        let s = delta_stats(deltas);
        println!(
            "  {:<8} {:<20} {:>6}  {:>9.3}     {:>6.3}  {:>6.3}",
            label, name, s.n, s.mean_signed, s.p95_abs, s.max_abs
        );
    }
    println!("  presence flips: 45 m = {flips45}, 27 m = {flips27}");

    assert_eq!(flips45, 0, "burn must not create/destroy audible pixels");
    assert_eq!(flips27, 0, "burn must not create/destroy audible pixels");

    let gate45 = delta_stats(&shadow45);
    let gate27 = delta_stats(&shadow27);
    assert!(
        gate45.n > 100,
        "shadow set unexpectedly small: {}",
        gate45.n
    );
    // The decision facts: the burn UNDER-screens (positive mean — tiles
    // would stay hot behind walls, the exact B8 failure mode), far past the
    // 1.0 dB viability gate, at both a cell-separated and the D11-like
    // road→wall spacing; the source side stays clean (the divergence is
    // confined to the shadow the burn fails to cast).
    assert!(
        gate45.mean_signed > 1.0 && gate45.max_abs > 1.0,
        "GPU-burn arm now within {:.3} dB mean / {:.3} dB max of the vector arm at 45 m — \
         the W2 gate would PASS; re-evaluate wiring the burn into gpu_surface (plan §B8/W2)",
        gate45.mean_signed,
        gate45.max_abs
    );
    assert!(
        gate27.max_abs > 1.0,
        "GPU-burn arm within {:.3} dB max of vector at 27 m — re-evaluate the burn decision",
        gate27.max_abs
    );
    let src_worst = delta_stats(&src45).max_abs.max(delta_stats(&src27).max_abs);
    assert!(
        src_worst < 0.25,
        "source-side pixels should be untouched by the burn, max |Δ| {src_worst:.3} dB"
    );
}

//! B8/C9 barrier wiring proofs on a synthetic flat-DEM tile.
//!
//! Fixture: a real `FusedTileZ13` built from an EMPTY rasters dir (missing
//! tiles are negative-cached defaults → elevation 0, buildings 0, forest 0,
//! IMD 100 → G = 0), one N-S line/point source west of an N-S 3 m noise wall,
//! receivers on the whole 512² lattice at DEM + 4 m. The wall is a chain of
//! ~60 m `ObstacleKind::Barrier` polyline microsegments inside the receiver's
//! `ObstacleIndex` — the shape the structures.arrow kind=1 rows load into
//! since the `types::Barrier` slice channel was deleted (e57941d3).
//!
//! 1. Wall effect: kernel energy WITH the wall indexed < WITHOUT, behind the
//!    wall.
//! 2. Popup↔kernel parity (the POINT kernel, whose cp-ray path is exact): the
//!    kernel's per-pixel energy equals a manual replay of the same physics
//!    calling `path_effects::screening_attenuation` directly with the wall's
//!    crossings collected from the same index — proves the kernel feeds walls
//!    to the shared popup kernel unchanged. The LINE kernel keeps only the
//!    effect half: with the wall in the index it runs the 5-bucket angular
//!    quadrature (`seg_sampling`), which a single-ray replay cannot reproduce
//!    to the old 1e-6 (the per-bucket integration is pinned by noise-compute's
//!    own seg_sampling fixtures).
//! 3. THE BURN IS REJECTED — decision record, no longer an executable arm.
//!    MEASURED 2026-06-11, CPU vector-barrier arm vs a GPU-burn-equivalent arm
//!    (the wall burned into the halo's building channel, empty vector slice):
//!    the ≤1.0 dB wall-adjacent gate failed decisively — mean +3.7 / max +5.9 dB
//!    under-screening at 45 m road→wall, max +13.8 dB at the D11-like 27 m,
//!    because the bilateral ray cadence (~30–245 m sample spacing) steps over
//!    a one-cell (~20 m lon at 50°N) burned wall column on most paths, while
//!    the vector path maps the wall onto the nearest EXISTING sample and
//!    never misses. Thin-line features cannot be faithfully burned into a
//!    raster sampled at ≥ cell-size cadence — so the BURN is rejected. The
//!    GPU surface kernel's barrier slice scan (formerly gated by
//!    `QM_GPU_BARRIERS`, its crossings pinned by the deleted
//!    `w2_gpu_vector_crossings_match_cpu_oracle` host replica) is removed in
//!    the GPU wave; its successor reads the same index. Run with
//!    `--nocapture` for the stats table.

use std::f64::consts::LN_10;
use std::path::Path;
use std::sync::Arc;

use noise_compute::constants::{ALPHA_ATM, A_WEIGHTING, M_PER_DEG_LAT};
use noise_compute::propagation::iso9613::{fast_exp_f64, legacy_ground_atten_db};
use noise_compute::propagation::obstacle_index::{ObstacleIndex, ObstacleKind, ObstacleSet};
use noise_compute::propagation::path_effects;
use noise_compute::propagation::PathProfile;
use noise_compute::types::RasterSampler;
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::source_line::LineRow;
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

/// N-S wall at `wall_lon` spanning ±extent_m around `c_lat` as a chain of
/// ~60 m `ObstacleKind::Barrier` polylines in one index — the shape the
/// structures.arrow kind=1 microsegment rows take on load.
fn wall_set(c_lat: f64, wall_lon: f64, extent_m: f64, height_m: f32) -> ObstacleSet {
    let seg_len_m = 60.0;
    let n = ((2.0 * extent_m) / seg_len_m).round() as usize;
    let mut builder = ObstacleIndex::builder(c_lat, wall_lon);
    for i in 0..n {
        let a = c_lat + m_to_deg_lat(-extent_m + i as f64 * seg_len_m);
        let b = c_lat + m_to_deg_lat(-extent_m + (i + 1) as f64 * seg_len_m);
        builder.add_polyline(
            &[(a, wall_lon), (b, wall_lon)],
            height_m,
            ObstacleKind::Barrier,
            i as u32,
        );
    }
    ObstacleSet {
        indexes: vec![Arc::new(builder.build())],
    }
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

/// Wall effect for the LINE kernel: the indexed wall must screen the shadow
/// pixel. (The exact-replay half moved to the point kernel — with the wall in
/// the index the line path runs the angular quadrature, which is not a single
/// ray.)
#[test]
fn line_kernel_applies_vector_barriers() {
    let tile = flat_tile();
    let (c_lat, c_lon) = centre(&tile);
    let road_lon = c_lon;
    let wall_lon = c_lon + m_to_deg_lon(45.0, c_lat);
    let line = road_line(c_lat, road_lon);
    let lines = vec![line];
    let walls = wall_set(c_lat, wall_lon, 360.0, 3.0);
    assert!(walls.edge_count() > 0);

    let mut acc_no = TileAccumulator::new();
    scatter_line::scatter_tile(&tile, &lines, &ObstacleSet::empty(), &mut acc_no);
    let mut acc_wall = TileAccumulator::new();
    scatter_line::scatter_tile(&tile, &lines, &walls, &mut acc_wall);

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
}

/// Wall effect + popup↔kernel parity for the POINT kernel: the kernel's
/// per-pixel energy equals a manual replay of the same physics, calling the
/// shared `path_effects` screening with the wall's crossings collected from
/// the same index the kernel ran on.
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
    let walls = wall_set(c_lat, wall_lon, 360.0, 3.0);

    let mut acc_no = TileAccumulator::new();
    scatter_point::scatter_tile(&tile, &points, &ObstacleSet::empty(), &mut acc_no);
    let mut acc_wall = TileAccumulator::new();
    scatter_point::scatter_tile(&tile, &points, &walls, &mut acc_wall);

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

    // Manual replay of scatter_point::scatter_band for that pixel: the wall's
    // crossings come out of the SAME index, then feed the SAME
    // `screening_attenuation` the popup calls. The kernel's pruned crossing
    // walk is output-neutral against this unpruned one (CellPrune only skips
    // cells that provably cannot reach the candidate race's floor).
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
    let (terrain, terrain_delta_m) =
        path_effects::terrain_attenuation(&mut profile, src_alt, rx_alt);
    let mut wall_crossings = Vec::new();
    walls.crossings(p.lat, p.lon, rx_lat, rx_lon, &mut wall_crossings);
    assert!(
        !wall_crossings.is_empty(),
        "the index must hand the wall to this path"
    );
    let screening = path_effects::screening_attenuation(
        &mut profile,
        path_effects::ObstacleInput {
            candidates: &wall_crossings,
        },
        src_alt,
        rx_alt,
        p.exclusion_radius_m,
        &terrain,
        terrain_delta_m,
    );
    assert!(screening.iter().any(|&s| s > 0.0));
    let veg = path_effects::vegetation_attenuation_path(&profile);
    let mut expected = 0.0f64;
    for i in 0..NUM_BANDS {
        let a_gr = legacy_ground_atten_db(i, ground_g);
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

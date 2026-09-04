//! A painted aircraft tile must carry the same bytes whatever the core count.
//!
//! Splitting a tile's scatter over SOURCES and merging per-thread `f32` accumulators
//! makes every cell's summation order depend on rayon's work stealing, and that order
//! decides the quantised byte whenever a cell sits on the 0.5 dB HM3 step. Measured on
//! the world's Dobris R4 on 2026-09-04: two runs of one cruise binary, same host, same
//! inputs, disagreed in 2 painted cells of 44 040 192. Both painters therefore split
//! the fine grid over RECEIVER rows and the coarse lattices into fixed source parts;
//! this test pins that property by painting one synthetic tile with 1 and with 8 rayon
//! threads and demanding identical bits.

use std::sync::Arc;

use noise_compute::compute::aircraft_v6::views::{BBox, SubSegmentSlice};
use noise_compute::compute::aircraft_v6::{AirborneRowView, CruiseRowView};
use noise_compute::emission::aircraft::{ClassWeights, SegmentTerrain};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use raster_reader::fused_grid::FusedGrid;
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBbox};
use tile_painter::accumulator::TileAccumulator;
use tile_painter::grid::{pixel_lat, pixel_lon, tile_bbox, TILE_PX};
use tile_painter::source_loader_structure::InteriorEstimate;

/// A z13 tile over central Bohemia, flat terrain — the receiver lattice is all the
/// NPD aircraft painters read.
const ZOOM: u8 = 13;
const TILE_X: u32 = 4415;
const TILE_Y: u32 = 2787;
const TERRAIN_M: f32 = 300.0;

fn flat_tile() -> FusedTileZ13 {
    // `tile_painter::grid` and `raster_reader::fused_tile_z13` each carry a `TileBbox`;
    // the receiver lattice is only meaningful if the two agree.
    let grid_bbox = tile_bbox(ZOOM, TILE_X, TILE_Y);
    let bbox = TileBbox::from_xyz(ZOOM, TILE_X, TILE_Y);
    assert_eq!(
        (bbox.west_lon, bbox.east_lon, bbox.north_lat, bbox.south_lat),
        (
            grid_bbox.west_lon,
            grid_bbox.east_lon,
            grid_bbox.north_lat,
            grid_bbox.south_lat
        ),
    );
    let cells = TILE_PX * TILE_PX;
    FusedTileZ13 {
        zoom: ZOOM,
        tile_x: TILE_X,
        tile_y: TILE_Y,
        rx_lat: std::array::from_fn(|py| pixel_lat(&grid_bbox, py as u32)),
        rx_lon: std::array::from_fn(|px| pixel_lon(&grid_bbox, px as u32)),
        inner_elev_m: vec![TERRAIN_M; cells],
        inner_forest: vec![0; cells],
        inner_imd: vec![0; cells],
        rx_alt_m: vec![TERRAIN_M + 4.0; cells],
        rx_refl_db: vec![0.0; cells],
        halo: Arc::new(FusedGrid::empty()),
        bbox,
    }
}

/// A reproducible per-source offset. The sources have to land at COMPARABLE energies
/// with differing low bits — a wide spread would be order-independent for the opposite
/// reason (a term below `max * 2^-24` is a no-op wherever it is added).
fn jitter(i: usize, salt: u64) -> f64 {
    ((i as u64).wrapping_mul(2_654_435_761).wrapping_add(salt) % 997) as f64 * 1.0e-5
}

fn paint<F>(threads: usize, paint_tile: F) -> Vec<u32>
where
    F: Fn(&mut TileAccumulator) + Sync + Send,
{
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("test rayon pool")
        .install(|| {
            let mut accum = TileAccumulator::new();
            paint_tile(&mut accum);
            accum.energy.iter().map(|e| e.to_bits()).collect()
        })
}

fn assert_bit_identical(one_thread: &[u32], eight_threads: &[u32], layer: &str) {
    assert!(
        one_thread.iter().any(|&bits| bits != 0),
        "{layer}: the fixture painted no energy at all"
    );
    let differing = one_thread
        .iter()
        .zip(eight_threads)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing,
        0,
        "{layer}: {differing} of {} energy cells depend on the core count",
        one_thread.len()
    );
}

#[test]
fn cruise_tile_bytes_do_not_depend_on_the_core_count() {
    let tile = flat_tile();
    let centre_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
    let centre_lon = (tile.bbox.east_lon + tile.bbox.west_lon) * 0.5;

    // 2 048 R7 buckets inside the 16 km reach. Two of them sit below the far-field
    // gate and take the exact per-pixel path; the rest broadcast onto the lattice.
    const BUCKETS: usize = 2_048;
    let rows: Vec<CruiseRowView<'static>> = (0..BUCKETS)
        .map(|i| {
            let lat = centre_lat + 0.06 * (jitter(i, 11) * 1.0e5 / 997.0 - 0.5);
            let lon = centre_lon + 0.09 * (jitter(i, 23) * 1.0e5 / 997.0 - 0.5);
            let cell = h3o::LatLng::new(lat, lon)
                .expect("fixture lat/lon")
                .to_cell(h3o::Resolution::Seven);
            let near = i % 1_024 == 0;
            CruiseRowView {
                r7_hex: u64::from(cell),
                class: 0,
                rep_profile_idx: 0,
                fl_bin: 0,
                period: (i % 3) as u8,
                sum_length_m: 40_000.0 + (i % 97) as f32,
                rep_len_m: 900.0 + (i % 31) as f32,
                rep_alt_m: if near { 4_000.0 } else { 11_000.0 },
                rep_speed_kt: 430.0 + (i % 41) as f32,
                source_id: 0,
                origin: 0,
                unique_count: 1,
                top_candidates: &[],
            }
        })
        .collect();
    let terrain = vec![
        Some(SegmentTerrain {
            start_elev: TERRAIN_M as f64,
            q1_elev: TERRAIN_M as f64,
            mid_elev: TERRAIN_M as f64,
            q3_elev: TERRAIN_M as f64,
            end_elev: TERRAIN_M as f64,
        });
        rows.len()
    ];

    let scatter = |accum: &mut TileAccumulator| {
        let stats = tile_painter::cruise::scatter_tile(&tile, &rows, &terrain, accum);
        assert!(
            stats.buckets_broadcast > 0
                && stats.buckets_in_reach > stats.buckets_broadcast
                && stats.pairs_evaluated > stats.pairs_below_threshold,
            "fixture must exercise both the broadcast lattice and the exact path: {stats:?}"
        );
    };
    assert_bit_identical(&paint(1, scatter), &paint(8, scatter), "cruise");
}

#[test]
fn airborne_coarse_bands_do_not_depend_on_the_core_count() {
    let tile = flat_tile();
    let centre_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
    let centre_lon = (tile.bbox.east_lon + tile.bbox.west_lon) * 0.5;

    // Three sub-segments per flight, one per far-field stride band: ~1 km, ~4 km and
    // ~11 km of best slant from the tile. The exact per-pixel path is deliberately not
    // exercised here — it would make the test build all 262 144 receiver horizons —
    // and it shares its receiver-row split with the cruise painter's, tested above.
    const FLIGHTS: usize = 512;
    const BAND_OFFSET_DEG: [f64; 3] = [0.010, 0.040, 0.110];
    let columns: Vec<Vec<Vec<f32>>> = (0..FLIGHTS)
        .map(|i| {
            let start_lat: Vec<f32> = BAND_OFFSET_DEG
                .iter()
                .map(|d| (centre_lat + d + jitter(i, 11)) as f32)
                .collect();
            let end_lat: Vec<f32> = BAND_OFFSET_DEG
                .iter()
                .map(|d| (centre_lat + d + 0.01 + jitter(i, 17)) as f32)
                .collect();
            let start_lon: Vec<f32> = BAND_OFFSET_DEG
                .iter()
                .map(|d| (centre_lon + d + jitter(i, 23)) as f32)
                .collect();
            let end_lon: Vec<f32> = BAND_OFFSET_DEG
                .iter()
                .map(|d| (centre_lon + d + 0.01 + jitter(i, 29)) as f32)
                .collect();
            let alt: Vec<f32> = (0..3)
                .map(|b| 900.0 + 200.0 * b as f32 + (jitter(i, 31) * 1.0e4) as f32)
                .collect();
            vec![start_lat, start_lon, alt.clone(), end_lat, end_lon, alt]
        })
        .collect();
    let period = [0u8, 1, 2];
    let date_id = [10i16; 3];
    let flags = [1u8; 3];
    let speed_kt = [220.0f32; 3];
    let length_m = [1_500.0f32; 3];
    let terrain_elev = [TERRAIN_M; 3];
    let rows: Vec<AirborneRowView<'_>> = (0..FLIGHTS)
        .map(|i| AirborneRowView {
            flight_id: noise_compute::flight_id::pack_synth(i as u64),
            callsign: "TEST",
            aircraft_type: *b"A320",
            profile_idx: (i % 8) as u8,
            source_id: 0,
            origin: 0,
            sub_segments: SubSegmentSlice {
                start_lat: &columns[i][0],
                start_lon: &columns[i][1],
                start_alt_m: &columns[i][2],
                end_lat: &columns[i][3],
                end_lon: &columns[i][4],
                end_alt_m: &columns[i][5],
                speed_kt: &speed_kt,
                length_m: &length_m,
                period: &period,
                date_id: &date_id,
                flags: &flags,
                terrain_start_elev_m: &terrain_elev,
                terrain_end_elev_m: &terrain_elev,
            },
            bbox: BBox {
                min_lat: (centre_lat - 0.2) as f32,
                max_lat: (centre_lat + 0.2) as f32,
                min_lon: (centre_lon - 0.3) as f32,
                max_lon: (centre_lon + 0.3) as f32,
            },
        })
        .collect();

    let obstacles = ObstacleSet::empty();
    let interior = InteriorEstimate::bake(&tile, &obstacles);
    let class_weights = ClassWeights::uniform();
    let scatter = |accum: &mut TileAccumulator| {
        let stats = tile_painter::airborne::scatter_tile(
            &tile,
            &rows,
            &class_weights,
            &obstacles,
            &interior,
            accum,
        );
        assert_eq!(stats.sub_near, 0, "fixture must stay on the coarse path");
        assert!(
            stats.coarse_band.iter().all(|&band| band > 0),
            "fixture must exercise every far-field stride band: {stats:?}"
        );
    };
    assert_bit_identical(&paint(1, scatter), &paint(8, scatter), "airborne");
}

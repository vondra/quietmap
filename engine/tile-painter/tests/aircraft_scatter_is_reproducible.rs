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

use noise_compute::compute::aircraft_v6::views::BBox;
use noise_compute::compute::aircraft_v6::CruiseRowView;
use noise_compute::emission::aircraft::{ClassWeights, SegmentTerrain};
use noise_compute::propagation::obstacle_index::ObstacleSet;
use raster_reader::fused_grid::FusedGrid;
use raster_reader::fused_tile_z13::{FusedTileZ13, TileBbox};
use tile_painter::accumulator::TileAccumulator;
use tile_painter::grid::{pixel_lat, pixel_lon, tile_bbox, TILE_PX};
use tile_painter::source_loader_structure::InteriorEstimate;

mod aircraft_source_fixture;
use aircraft_source_fixture::{
    airborne_rows, cruise_row, flight_columns, jitter, SubSegmentScalars, TERRAIN_M,
};

/// A z13 tile over central Bohemia, flat terrain — the receiver lattice is all the
/// NPD aircraft painters read.
const ZOOM: u8 = 13;
const TILE_X: u32 = 4415;
const TILE_Y: u32 = 2787;

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

/// Flat terrain under every cruise row, in row order.
fn flat_cruise_terrain(rows: usize) -> Vec<Option<SegmentTerrain>> {
    vec![
        Some(SegmentTerrain {
            start_elev: TERRAIN_M as f64,
            q1_elev: TERRAIN_M as f64,
            mid_elev: TERRAIN_M as f64,
            q3_elev: TERRAIN_M as f64,
            end_elev: TERRAIN_M as f64,
        });
        rows
    ]
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
            cruise_row(
                i,
                centre_lat + 0.06 * (jitter(i, 11) * 1.0e5 / 997.0 - 0.5),
                centre_lon + 0.09 * (jitter(i, 23) * 1.0e5 / 997.0 - 0.5),
                i % 1_024 == 0,
            )
        })
        .collect();
    let terrain = flat_cruise_terrain(rows.len());

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

    // Three sub-segments per flight, one per far-field stride band: ~1 km, ~4 km and ~11 km
    // of best slant from the tile.
    const FLIGHTS: usize = 512;
    const BAND_OFFSET_DEG: [f64; 3] = [0.010, 0.040, 0.110];
    let columns = flight_columns(
        FLIGHTS,
        &BAND_OFFSET_DEG,
        |i, band| 900.0 + 200.0 * band as f32 + (jitter(i, 31) * 1.0e4) as f32,
        centre_lat,
        centre_lon,
    );
    let scalars = SubSegmentScalars::new(BAND_OFFSET_DEG.len());
    let rows = airborne_rows(
        &columns,
        &scalars,
        BBox {
            min_lat: (centre_lat - 0.2) as f32,
            max_lat: (centre_lat + 0.2) as f32,
            min_lon: (centre_lon - 0.3) as f32,
            max_lon: (centre_lon + 0.3) as f32,
        },
    );

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
    assert_bit_identical(&paint(1, scatter), &paint(8, scatter), "airborne coarse");
}

#[test]
fn airborne_exact_path_does_not_depend_on_the_core_count() {
    let tile = flat_tile();
    let centre_lat = (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5;
    let centre_lon = (tile.bbox.east_lon + tile.bbox.west_lon) * 0.5;

    // Low overflights right across the tile: the clamped CPA is inside the half-diagonal and
    // the segments clear the receivers by ~300 m, so every one is under NEAR_SLANT_M and takes
    // the exact 262 144-pixel path — the receiver-row split the coarse fixture cannot reach.
    // Few flights on purpose. The cost of this test is the screening grid — the exact path
    // makes it build a horizon at all 262 144 receivers, twice, which is ~70 s of the debug
    // gate and does not move with the flight count (measured: 1 flight 70.6 s, 8 flights
    // 71.2 s). Eight is enough to make a cell's sum order-sensitive.
    const FLIGHTS: usize = 8;
    const OVERHEAD_DEG: [f64; 1] = [0.0];
    let columns = flight_columns(
        FLIGHTS,
        &OVERHEAD_DEG,
        |i, _| 600.0 + (jitter(i, 41) * 1.0e4) as f32,
        centre_lat,
        centre_lon,
    );
    let scalars = SubSegmentScalars::new(OVERHEAD_DEG.len());
    let rows = airborne_rows(
        &columns,
        &scalars,
        BBox {
            min_lat: (centre_lat - 0.02) as f32,
            max_lat: (centre_lat + 0.02) as f32,
            min_lon: (centre_lon - 0.02) as f32,
            max_lon: (centre_lon + 0.02) as f32,
        },
    );

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
        assert_eq!(
            stats.sub_near, FLIGHTS as u64,
            "fixture must put every sub-segment on the exact per-pixel path: {stats:?}"
        );
        assert!(
            stats.pairs_evaluated > stats.pairs_below_threshold,
            "fixture must paint energy, not only floor rejections: {stats:?}"
        );
    };
    assert_bit_identical(&paint(1, scatter), &paint(8, scatter), "airborne exact");
}

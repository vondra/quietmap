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

/// Per-flight sub-segment columns, in `SubSegmentSlice` order:
/// `[start_lat, start_lon, start_alt_m, end_lat, end_lon, end_alt_m]`. Owned by the test
/// because the row views borrow them.
type FlightColumns = [Vec<f32>; 6];

/// One flight per index, one sub-segment per entry of `offset_deg` — the sub-segment starts
/// that far north-east of the tile centre and runs 0.01° further, at `altitude_m`.
fn flight_columns(
    flights: usize,
    offset_deg: &[f64],
    altitude_m: impl Fn(usize, usize) -> f32,
    centre_lat: f64,
    centre_lon: f64,
) -> Vec<FlightColumns> {
    (0..flights)
        .map(|i| {
            let axis = |base: f64, extra: f64, salt: u64| -> Vec<f32> {
                offset_deg
                    .iter()
                    .map(|d| (base + d + extra + jitter(i, salt)) as f32)
                    .collect()
            };
            let alt: Vec<f32> = (0..offset_deg.len()).map(|b| altitude_m(i, b)).collect();
            [
                axis(centre_lat, 0.0, 11),
                axis(centre_lon, 0.0, 23),
                alt.clone(),
                axis(centre_lat, 0.01, 17),
                axis(centre_lon, 0.01, 29),
                alt,
            ]
        })
        .collect()
}

/// The per-sub-segment scalar columns every fixture shares. `flags & 1` = departure; the
/// terrain elevations are the tile's, so the endpoint ground-stale gate passes.
struct SubSegmentScalars {
    period: Vec<u8>,
    date_id: Vec<i16>,
    flags: Vec<u8>,
    speed_kt: Vec<f32>,
    length_m: Vec<f32>,
    terrain_elev_m: Vec<f32>,
}

impl SubSegmentScalars {
    fn new(sub_segments: usize) -> Self {
        Self {
            period: (0..sub_segments).map(|i| (i % 3) as u8).collect(),
            date_id: vec![10; sub_segments],
            flags: vec![1; sub_segments],
            speed_kt: vec![220.0; sub_segments],
            length_m: vec![1_500.0; sub_segments],
            terrain_elev_m: vec![TERRAIN_M; sub_segments],
        }
    }
}

fn airborne_rows<'a>(
    columns: &'a [FlightColumns],
    scalars: &'a SubSegmentScalars,
    bbox: BBox,
) -> Vec<AirborneRowView<'a>> {
    columns
        .iter()
        .enumerate()
        .map(|(i, flight)| AirborneRowView {
            flight_id: noise_compute::flight_id::pack_synth(i as u64),
            callsign: "TEST",
            aircraft_type: *b"A320",
            profile_idx: (i % 8) as u8,
            source_id: 0,
            origin: 0,
            sub_segments: SubSegmentSlice {
                start_lat: &flight[0],
                start_lon: &flight[1],
                start_alt_m: &flight[2],
                end_lat: &flight[3],
                end_lon: &flight[4],
                end_alt_m: &flight[5],
                speed_kt: &scalars.speed_kt,
                length_m: &scalars.length_m,
                period: &scalars.period,
                date_id: &scalars.date_id,
                flags: &scalars.flags,
                terrain_start_elev_m: &scalars.terrain_elev_m,
                terrain_end_elev_m: &scalars.terrain_elev_m,
            },
            bbox,
        })
        .collect()
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

//! One bug class: a paint narrowed by a `tiles=x,y,side` window must carry the same bytes as
//! the whole-cell paint the window was carved out of.
//!
//! A windowed cell still loads and screens against its whole `grid_disk(1)` source ring, so the
//! only way a tile could change is if some SHARED structure were built from the tile set instead
//! of from the cell. The CPU aircraft painter has exactly one such structure: the cruise coarse
//! field, computed lazily on a global z9 lattice and bilinearly upsampled into each base-zoom
//! tile. This paints real energy into two fields — one asked for a 2x2 window, one asked for the
//! 4x4 square around it — and demands identical bits for the tiles they share.
//!
//! The other shared structure is the batch a tile belongs to, which decides the terrain halo it
//! reads; that a window never moves a tile between batches is pinned in
//! `region_runner::group_tiles_into_batches`.

use noise_compute::compute::aircraft_v6::CruiseRowView;
use raster_reader::fused_tile_z13::FusedTileZ13;
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::cruise_field::CruiseField;
use tile_painter::stream_tile_window::TileWindow;

/// The release check's own 4x4 z13 square over Dobris, and the 2x2 window inside it.
const ZOOM: u8 = 13;
const SQUARE: TileWindow = TileWindow {
    x: 4414,
    y: 2786,
    side: 4,
};
const WINDOW: TileWindow = TileWindow {
    x: 4415,
    y: 2787,
    side: 2,
};

fn tiles_of(window: TileWindow) -> Vec<(u32, u32)> {
    (window.x..window.x + window.side)
        .flat_map(|x| (window.y..window.y + window.side).map(move |y| (x, y)))
        .collect()
}

/// A reproducible per-source offset, so the buckets land at comparable energies with differing
/// low bits — the only shape in which a summation-order or lattice change is visible at all.
fn jitter(i: usize, salt: u64) -> f64 {
    ((i as u64).wrapping_mul(2_654_435_761).wrapping_add(salt) % 997) as f64 * 1.0e-5
}

/// Cruise buckets spread over the square, half of them inside the near gate. Their R7 cells and
/// energies are fixed by the index alone, so both fields see the identical source set.
fn cruise_rows(centre_lat: f64, centre_lon: f64) -> Vec<CruiseRowView<'static>> {
    const BUCKETS: usize = 1_024;
    (0..BUCKETS)
        .map(|i| {
            let lat = centre_lat + 0.06 * (jitter(i, 11) * 1.0e5 / 997.0 - 0.5);
            let lon = centre_lon + 0.09 * (jitter(i, 23) * 1.0e5 / 997.0 - 0.5);
            let cell = h3o::LatLng::new(lat, lon)
                .expect("fixture lat/lon")
                .to_cell(h3o::Resolution::Seven);
            CruiseRowView {
                r7_hex: u64::from(cell),
                class: 0,
                rep_profile_idx: 0,
                fl_bin: 0,
                period: (i % 3) as u8,
                sum_length_m: 40_000.0 + (i % 97) as f32,
                rep_len_m: 900.0 + (i % 31) as f32,
                rep_alt_m: if i % 512 == 0 { 4_000.0 } else { 11_000.0 },
                rep_speed_kt: 430.0 + (i % 41) as f32,
                source_id: 0,
                origin: 0,
                unique_count: 1,
                top_candidates: &[],
            }
        })
        .collect()
}

/// Paint `tiles` through one freshly built cruise field and return each tile's raw per-period
/// energy bits, keyed by tile — the bytes before the shared HM3 collapse.
fn paint(rasters: &RealRasters, rows: &[CruiseRowView<'_>], tiles: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut field = CruiseField::new(rasters, rows);
    tiles
        .iter()
        .map(|&(x, y)| {
            let tile = FusedTileZ13::build_receiver_altitude_only(ZOOM, x, y, rasters);
            let mut accum = TileAccumulator::new();
            field.upsample_into(&tile, &mut accum);
            accum.energy.iter().map(|energy| energy.to_bits()).collect()
        })
        .collect()
}

#[test]
fn a_windowed_cruise_paint_carries_the_whole_squares_bytes() {
    // No raster tree: every receiver sits at sea level, which is all the cruise kernel reads
    // from terrain here and keeps the fixture independent of any host's data mount.
    let rasters = RealRasters::new(&std::env::temp_dir().join("quietmap-absent-rasters"));
    let square = tiles_of(SQUARE);
    let window = tiles_of(WINDOW);
    assert_eq!(square.len(), 16);
    assert_eq!(
        WINDOW.select(square.clone()).unwrap(),
        window,
        "the window must be a sub-square of the release check's square"
    );

    let first = FusedTileZ13::build_receiver_altitude_only(ZOOM, SQUARE.x, SQUARE.y, &rasters);
    let rows = cruise_rows(
        (first.bbox.north_lat + first.bbox.south_lat) * 0.5,
        (first.bbox.east_lon + first.bbox.west_lon) * 0.5,
    );

    let whole = paint(&rasters, &rows, &square);
    let windowed = paint(&rasters, &rows, &window);
    assert!(
        whole.iter().flatten().any(|&bits| bits != 0),
        "the fixture painted no cruise energy at all"
    );
    for (tile, painted) in window.iter().zip(&windowed) {
        let position = square
            .iter()
            .position(|candidate| candidate == tile)
            .expect("a windowed tile is one of the square's tiles");
        assert_eq!(
            painted, &whole[position],
            "tile {tile:?} paints different bytes inside the window than inside the square"
        );
    }
}

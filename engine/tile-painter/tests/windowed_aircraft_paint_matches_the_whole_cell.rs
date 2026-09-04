//! Two bug classes of the `tiles=x,y,side` window, for the CPU aircraft painter.
//!
//! 1. A narrowed paint differs from the whole-cell paint it was carved out of. A windowed cell
//!    still loads and screens against its whole `grid_disk(1)` ring, so a tile can only change if
//!    some SHARED structure were built from the tile set instead of from the cell. This painter
//!    has exactly one: the cruise coarse field, computed lazily on a global z9 Mercator lattice
//!    and bilinearly upsampled into each base-zoom tile. Two fields are built here — one asked for
//!    a 2x2 window, one for the 4x4 square around it — and the tiles they share must agree bit for
//!    bit. (The other shared structure is the batch a tile belongs to, which decides the terrain
//!    halo it reads; that a window never moves a tile between batches is pinned in
//!    `region_runner::group_tiles_into_batches`.)
//!
//! 2. The tile set leaks into ADMISSION. Byte equality alone cannot see that: hand both paints the
//!    same rows and they agree however narrow admission became. So a documented half of the source
//!    set here sits OUTSIDE the painted square and inside the reach of the tiles inside it, and
//!    each test asserts those far sources really do light the windowed tiles. With them in the
//!    fixture, any narrowing of admission to the painted window would change the windowed bytes
//!    and test 1 would fail too.

use noise_compute::compute::aircraft_v6::views::BBox;
use noise_compute::compute::aircraft_v6::CruiseRowView;
use noise_compute::emission::aircraft::ClassWeights;
use noise_compute::propagation::obstacle_index::ObstacleSet;
use raster_reader::fused_tile_z13::FusedTileZ13;
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::cruise_field::CruiseField;
use tile_painter::grid::tile_bbox;
use tile_painter::source_loader_structure::InteriorEstimate;
use tile_painter::stream_tile_window::TileWindow;

mod aircraft_source_fixture;
use aircraft_source_fixture::{
    airborne_rows, cruise_row, flight_columns, jitter, SubSegmentScalars,
};

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
/// How far NORTH of the square's own bounding box the "outside" sources sit: 0.02 degrees is
/// about 2.2 km past the edge — clear of every tile the window paints, and close enough to stay
/// inside the cruise reach (16 km) and the airborne far-field bands, whose own fixture in
/// `aircraft_scatter_is_reproducible` admits sub-segments out to ~14 km.
const OUTSIDE_DEG: f64 = 0.02;

fn tiles_of(window: TileWindow) -> Vec<(u32, u32)> {
    (window.x..window.x + window.side)
        .flat_map(|x| (window.y..window.y + window.side).map(move |y| (x, y)))
        .collect()
}

/// The 4x4 square's own bounding box as `(south, north, west, east)`.
fn square_extent() -> (f64, f64, f64, f64) {
    let (mut south, mut north, mut west, mut east) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (x, y) in tiles_of(SQUARE) {
        let bbox = tile_bbox(ZOOM, x, y);
        south = south.min(bbox.south_lat);
        north = north.max(bbox.north_lat);
        west = west.min(bbox.west_lon);
        east = east.max(bbox.east_lon);
    }
    (south, north, west, east)
}

/// No raster tree: every receiver sits at sea level, which is all these kernels read from terrain
/// here and keeps the fixture independent of any host's data mount.
fn absent_rasters() -> RealRasters {
    RealRasters::new(&std::env::temp_dir().join("quietmap-absent-rasters"))
}

/// Cruise buckets, half inside the square and half `OUTSIDE_DEG` past its north-east corner.
/// `inside_only` drops the far half — the paint an admission narrowed to the painted window
/// would produce.
fn cruise_rows(inside_only: bool) -> Vec<CruiseRowView<'static>> {
    const BUCKETS: usize = 1_024;
    let (south, north, west, east) = square_extent();
    let (centre_lat, centre_lon) = ((south + north) * 0.5, (west + east) * 0.5);
    (0..BUCKETS)
        .filter(|i| !(inside_only && i % 2 == 1))
        .map(|i| {
            let (lat, lon) = if i % 2 == 0 {
                (
                    centre_lat + 0.02 * (jitter(i, 11) * 1.0e5 / 997.0 - 0.5),
                    centre_lon + 0.03 * (jitter(i, 23) * 1.0e5 / 997.0 - 0.5),
                )
            } else {
                (
                    north + OUTSIDE_DEG + 0.01 * jitter(i, 11) * 1.0e5 / 997.0,
                    centre_lon + 0.03 * (jitter(i, 23) * 1.0e5 / 997.0 - 0.5),
                )
            };
            cruise_row(i, lat, lon, i % 512 == 0)
        })
        .collect()
}

/// Paint `tiles` through one freshly built cruise field and return each tile's raw per-period
/// energy bits, keyed by tile — the bytes before the shared HM3 collapse.
fn paint_cruise(
    rasters: &RealRasters,
    rows: &[CruiseRowView<'_>],
    tiles: &[(u32, u32)],
) -> Vec<Vec<u32>> {
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
    let rasters = absent_rasters();
    let square = tiles_of(SQUARE);
    let window = tiles_of(WINDOW);
    assert_eq!(square.len(), 16);
    assert_eq!(
        WINDOW.select(square.clone()).unwrap(),
        window,
        "the window must be a sub-square of the release check's square"
    );

    let rows = cruise_rows(false);
    let (_, north, _, _) = square_extent();
    let far_north = window
        .iter()
        .map(|&(x, y)| tile_bbox(ZOOM, x, y).north_lat)
        .fold(f64::MIN, f64::max);
    assert!(
        north + OUTSIDE_DEG > far_north,
        "half the fixture's buckets must sit north of every tile the window paints"
    );
    let whole = paint_cruise(&rasters, &rows, &square);
    let windowed = paint_cruise(&rasters, &rows, &window);
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

    // The admission half: the far buckets must actually light these tiles, so a narrowing of
    // admission to the painted window could not slip past the comparison above.
    let inside_only = paint_cruise(&rasters, &cruise_rows(true), &window);
    assert_ne!(
        windowed, inside_only,
        "a cruise bucket outside the painted square must still light the tiles inside it"
    );
}

#[test]
fn a_windowed_airborne_paint_still_admits_the_sources_outside_it() {
    let rasters = absent_rasters();
    let (_, north, _, _) = square_extent();
    let (window_x, window_y) = tiles_of(WINDOW)[0];
    let tile = FusedTileZ13::build_receiver_altitude_only(ZOOM, window_x, window_y, &rasters);
    let obstacles = ObstacleSet::empty();
    let interior = InteriorEstimate::bake(&tile, &obstacles);
    let class_weights = ClassWeights::uniform();

    // One flight group over the painted tile and one beyond the square's north-east corner; both
    // sit in the far-field stride bands, not on the exact per-pixel path.
    const FLIGHTS: usize = 128;
    const BAND_OFFSET_DEG: [f64; 2] = [0.010, 0.040];
    let scalars = SubSegmentScalars::new(BAND_OFFSET_DEG.len());
    let altitude =
        |i: usize, band: usize| 900.0 + 200.0 * band as f32 + (jitter(i, 31) * 1.0e4) as f32;
    let near_columns = flight_columns(
        FLIGHTS,
        &BAND_OFFSET_DEG,
        altitude,
        (tile.bbox.north_lat + tile.bbox.south_lat) * 0.5,
        (tile.bbox.east_lon + tile.bbox.west_lon) * 0.5,
    );
    let far_columns = flight_columns(
        FLIGHTS,
        &BAND_OFFSET_DEG,
        altitude,
        north + OUTSIDE_DEG,
        (tile.bbox.east_lon + tile.bbox.west_lon) * 0.5,
    );
    assert!(
        north + OUTSIDE_DEG > tile.bbox.north_lat,
        "the far flights must start north of every tile the window paints"
    );
    let wide = BBox {
        min_lat: (tile.bbox.south_lat - 0.3) as f32,
        max_lat: (tile.bbox.north_lat + 0.3) as f32,
        min_lon: (tile.bbox.west_lon - 0.4) as f32,
        max_lon: (tile.bbox.east_lon + 0.4) as f32,
    };
    let inside_rows = airborne_rows(&near_columns, &scalars, wide);
    let far_rows = airborne_rows(&far_columns, &scalars, wide);

    let scatter = |rows: &[_]| {
        let mut accum = TileAccumulator::new();
        let stats = tile_painter::airborne::scatter_tile(
            &tile,
            rows,
            &class_weights,
            &obstacles,
            &interior,
            &mut accum,
        );
        assert_eq!(stats.sub_near, 0, "fixture must stay on the coarse path");
        let bits: Vec<u32> = accum.energy.iter().map(|energy| energy.to_bits()).collect();
        (bits, stats)
    };

    // The flights over the tile establish that the fixture paints at all.
    let (inside_only, inside_stats) = scatter(&inside_rows);
    assert!(
        inside_only.iter().any(|&bits| bits != 0)
            && inside_stats.pairs_evaluated > inside_stats.pairs_below_threshold,
        "fixture must paint energy, not only floor rejections: {inside_stats:?}"
    );

    // The claim: the flights that never enter the painted square light it anyway. An admission
    // narrowed to the tiles a request paints would drop them and leave this tile silent — which
    // is also what would break the byte comparison in the cruise test above.
    let (far_only, far_stats) = scatter(&far_rows);
    assert!(
        far_only.iter().any(|&bits| bits != 0),
        "a flight outside the painted square must still light the tiles inside it: {far_stats:?}"
    );
    assert!(
        far_stats.coarse_band.iter().sum::<u64>() > 0,
        "the far flights must be admitted on the far-field path: {far_stats:?}"
    );
}

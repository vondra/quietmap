//! Regression cases for cell/window receiver preparation and bounded dateline halos.

use super::horizon_halo::{cell_horizon_rectangles, tiles_bbox};
use super::*;
use noise_compute::emission::aircraft::RECEIVER_HORIZON_MAX_M;
use tile_painter::region_runner::region_tiles;

/// Dobris and the 4x4 z13 window the release check paints inside it.
const DOBRIS: u64 = 0x841e309ffffffff;
const ZOOM: u8 = 13;
const WINDOW: [(u32, u32); 4] = [(4414, 2786), (4415, 2786), (4414, 2787), (4415, 2787)];

/// A dateline cell needs two compact halos, not a globe-wide allocation. Cells
/// outside Mercator have no receivers; high-latitude rectangles remain supported.
#[test]
fn dateline_cells_use_two_compact_rectangles_and_polar_cells_have_no_receivers() {
    // All three are cells of the prepared world. 84bb005ffffffff (-41.4, 179.8) carries
    // airborne.arrow and owns 129 z13 tiles spanning the full 360 degrees; 8403205ffffffff
    // (89.5 N) is above the Mercator cut and owns none; 8401515ffffffff (Svalbard, 78 N)
    // owns 1527 tiles inside one 2.07-degree-wide rectangle.
    const ANTIMERIDIAN: u64 = 0x84bb005ffffffff;
    const ABOVE_MERCATOR: u64 = 0x8403205ffffffff;
    const HIGH_LATITUDE: u64 = 0x8401515ffffffff;

    let seam_tiles = region_tiles(ANTIMERIDIAN, ZOOM);
    assert_eq!(seam_tiles.len(), 129);
    let rectangles = cell_horizon_rectangles(ZOOM, ANTIMERIDIAN).unwrap();
    assert_eq!(rectangles.len(), 2);
    assert!(rectangles
        .iter()
        .all(|bbox| bbox.east_lon - bbox.west_lon < 1.0));

    let polar = tiles_bbox(ZOOM, &region_tiles(ABOVE_MERCATOR, ZOOM))
        .expect_err("a cell above the Mercator cut owns no tile");
    assert!(format!("{polar:#}").contains("no tile"));

    let arctic = tiles_bbox(ZOOM, &region_tiles(HIGH_LATITUDE, ZOOM))
        .expect("a high-latitude cell away from the seam is one rectangle");
    assert!(arctic.north_lat > 77.0 && arctic.east_lon - arctic.west_lon < 5.0);
}

/// One bug class, at the CALL SITE: a windowed request prepares different receivers than the
/// whole-cell request it was carved out of. This runs the production block builder twice for
/// one cell — once with every tile the cell owns, once with a window of them — and demands
/// that each kept tile come back in the same block, at the same batch origin, over the same
/// terrain halo, with a bit-identical receiver lattice and the same interior stamp. z11 keeps
/// the whole-cell side to nine tiles; the code path is the one z13 production runs.
#[test]
fn a_windowed_request_prepares_the_same_receivers_as_the_whole_cell() {
    use tile_painter::stream_tile_window::TileWindow;

    const COARSE_ZOOM: u8 = 11;
    const BATCH_N: u32 = 3;
    let rasters = RealRasters::new(&std::env::temp_dir().join("quietmap-absent-rasters"));
    let obstacles = ObstacleSet::empty();
    let owned = region_tiles(DOBRIS, COARSE_ZOOM);
    assert_eq!(owned.len(), 9, "the fixture cell must own a small tile set");
    let window = TileWindow {
        x: owned.iter().map(|tile| tile.0).min().unwrap(),
        y: owned.iter().map(|tile| tile.1).min().unwrap(),
        side: 2,
    };
    let windowed = window.select(owned.clone()).unwrap();
    assert!(windowed.len() < owned.len() && !windowed.is_empty());

    let whole_blocks =
        prepare_receiver_blocks(&rasters, COARSE_ZOOM, BATCH_N, DOBRIS, &owned, &obstacles)
            .unwrap();
    let windowed_blocks = prepare_receiver_blocks(
        &rasters,
        COARSE_ZOOM,
        BATCH_N,
        DOBRIS,
        &windowed,
        &obstacles,
    )
    .unwrap();
    assert!(windowed_blocks.len() < whole_blocks.len());

    let stamped = |interior: &InteriorEstimate| {
        use raster_reader::fused_tile_z13::TILE_PX;
        let mut cells = vec![200u8; TILE_PX * TILE_PX];
        interior.apply(&mut cells);
        cells
    };
    let find = |blocks: &[PrepBlock], tile: (u32, u32)| {
        let block = blocks
            .iter()
            .find(|block| block.btiles.contains(&tile))
            .expect("every requested tile is prepared exactly once");
        let slot = block.btiles.iter().position(|&t| t == tile).unwrap();
        let receivers = block.tile_refs()[slot];
        (
            (block.bx, block.by),
            (block.batch.base_x, block.batch.base_y),
            receivers.rx_lat,
            receivers.rx_lon,
            receivers.rx_alt_m.clone(),
            receivers.inner_elev_m.clone(),
            stamped(&block.interiors[slot]),
            block.batch.tiles[0].halo.packed_elevation_grid(),
        )
    };
    for &tile in &windowed {
        let whole = find(&whole_blocks, tile);
        let narrowed = find(&windowed_blocks, tile);
        assert_eq!(whole.0, narrowed.0, "tile {tile:?} changed block");
        assert_eq!(whole.1, narrowed.1, "tile {tile:?} changed batch origin");
        assert_eq!(
            whole.2, narrowed.2,
            "tile {tile:?} changed receiver latitudes"
        );
        assert_eq!(
            whole.3, narrowed.3,
            "tile {tile:?} changed receiver longitudes"
        );
        assert_eq!(
            whole.4, narrowed.4,
            "tile {tile:?} changed receiver altitudes"
        );
        assert_eq!(whole.5, narrowed.5, "tile {tile:?} changed terrain");
        assert_eq!(
            whole.6, narrowed.6,
            "tile {tile:?} changed its interior stamp"
        );
        let (whole_halo, narrowed_halo) = (&whole.7, &narrowed.7);
        assert_eq!(
            (
                whole_halo.lat_min,
                whole_halo.lon_min,
                whole_halo.rows,
                whole_halo.cols
            ),
            (
                narrowed_halo.lat_min,
                narrowed_halo.lon_min,
                narrowed_halo.rows,
                narrowed_halo.cols
            ),
            "tile {tile:?} marched its horizon over a different halo lattice"
        );
    }
    // Every tile the window dropped is still prepared by the whole-cell request, so the
    // narrowing removed receivers and nothing else.
    for tile in owned.iter().filter(|tile| !windowed.contains(tile)) {
        assert!(whole_blocks.iter().any(|block| block.btiles.contains(tile)));
    }
}

/// One bug class: the painted tile set leaks into ADMISSION. `region_candidates` builds its
/// admit envelope from the CELL, so a windowed paint must still admit every source the whole
/// cell admits — including one that sits outside the painted window entirely. If the envelope
/// were ever re-derived from the tiles a request paints, this flight would vanish from a
/// windowed cell and its tiles would go quiet against the whole-cell reference.
#[test]
fn admission_comes_from_the_cell_and_never_from_the_painted_window() {
    use noise_compute::compute::aircraft_v6::views::{BBox, SubSegmentSlice};
    use noise_compute::compute::aircraft_v6::AirborneRowView;
    use noise_gpu::airborne::region_candidates;

    // The window's own bbox, and a flight one tile-width north-east of its corner — inside
    // the cell and inside the admit reach, outside everything the window paints.
    let window = tiles_bbox(ZOOM, &WINDOW).unwrap();
    let far_lat = (window.north_lat + 0.05) as f32;
    let far_lon = (window.east_lon + 0.06) as f32;
    assert!(
        f64::from(far_lat) > window.north_lat && f64::from(far_lon) > window.east_lon,
        "the fixture flight must lie outside the painted window"
    );
    let columns: [Vec<f32>; 6] = [
        vec![far_lat],
        vec![far_lon],
        vec![900.0],
        vec![far_lat + 0.01],
        vec![far_lon + 0.01],
        vec![900.0],
    ];
    let speed = vec![220.0f32];
    let length = vec![1_500.0f32];
    let period = vec![0u8];
    let date = vec![10i16];
    let flags = vec![1u8];
    let terrain = vec![300.0f32];
    let views = vec![AirborneRowView {
        flight_id: noise_compute::flight_id::pack_synth(1),
        callsign: "TEST",
        aircraft_type: *b"A320",
        profile_idx: 0,
        source_id: 0,
        origin: 0,
        sub_segments: SubSegmentSlice {
            start_lat: &columns[0],
            start_lon: &columns[1],
            start_alt_m: &columns[2],
            end_lat: &columns[3],
            end_lon: &columns[4],
            end_alt_m: &columns[5],
            speed_kt: &speed,
            length_m: &length,
            period: &period,
            date_id: &date,
            flags: &flags,
            terrain_start_elev_m: &terrain,
            terrain_end_elev_m: &terrain,
        },
        bbox: BBox {
            min_lat: far_lat,
            max_lat: far_lat + 0.01,
            min_lon: far_lon,
            max_lon: far_lon + 0.01,
        },
    }];
    assert_eq!(
        region_candidates(&views, DOBRIS, ZOOM).len(),
        1,
        "the cell must admit a source outside the window it happens to paint"
    );
}

/// One bug class: a paint narrowed by a `tiles=` window differs from the whole-cell paint.
/// Every block of a cell marches its receiver horizons over ONE shared elevation halo. A halo
/// anchored to the tiles a request paints would start on a different lattice origin, and a
/// `FusedGrid` reconstructs each sample's lat/lon from its own origin — so every sample of a
/// windowed paint would move by an ULP and its tiles could no longer be compared, byte for
/// byte, against the whole-cell reference. Pin that the cell's halo is strictly the larger
/// grid and that a window-anchored one really would sit somewhere else.
#[test]
fn the_shared_horizon_halo_spans_the_cell_not_the_painted_window() {
    // No raster tree: elevations read as sea level, which leaves the grid GEOMETRY —
    // the whole subject of this test — exactly as production builds it.
    let rasters = RealRasters::new(&std::env::temp_dir().join("quietmap-absent-rasters"));
    let owned = region_tiles(DOBRIS, ZOOM);
    assert!(WINDOW.iter().all(|tile| owned.contains(tile)));

    let cell = cell_horizon_halos(&rasters, ZOOM, DOBRIS).unwrap()[0].packed_elevation_grid();
    let window_anchored = FusedTileZ13::build_elevation_halo(
        &tiles_bbox(ZOOM, &WINDOW).unwrap(),
        RECEIVER_HORIZON_MAX_M,
        &rasters,
    )
    .packed_elevation_grid();

    assert!(
        cell.rows > window_anchored.rows && cell.cols > window_anchored.cols,
        "the cell's halo must be the larger grid: {}x{} against {}x{}",
        cell.rows,
        cell.cols,
        window_anchored.rows,
        window_anchored.cols
    );
    assert!(
        cell.lat_min <= window_anchored.lat_min && cell.lon_min <= window_anchored.lon_min,
        "the cell's halo must start south and west of any window inside it"
    );
    assert_ne!(
        (cell.lat_min, cell.lon_min),
        (window_anchored.lat_min, window_anchored.lon_min),
        "a window-anchored halo starts on a different lattice origin, which is exactly \
         the difference that would move every sample of a windowed paint"
    );
    assert_eq!(cell.inv_cell_deg, window_anchored.inv_cell_deg);
}

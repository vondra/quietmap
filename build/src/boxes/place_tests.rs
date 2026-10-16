//! Placement of flight pieces: bands by clearance, and a segment cut into chained pieces that
//! cover it exactly.

use super::*;
use physics::doc29::box_geometry::BOX_EDGE_LEVEL_STEP_DB;

const PRAGUE: TileId = TileId { x: 2212, y: 1387 };

#[test]
fn a_point_takes_the_highest_band_its_clearance_reaches() {
    let terrain = HashMap::new();
    let placement = Placement::new(&HashSet::from([PRAGUE]), &terrain, BOX_EDGE_LEVEL_STEP_DB);
    let bands = placement.bands(PRAGUE).to_vec();
    let centre = PRAGUE.centre();
    let (low, ground) = placement.box_of(centre, 20.0, false);
    assert_eq!((low.band, low.tile, ground), (0, PRAGUE, 0.0));
    assert_eq!(bands[0].zoom, 19);
    for altitude in [60.0, 400.0, 1_000.0, 9_000.0] {
        let (key, _) = placement.box_of(centre, altitude, true);
        let band = bands[usize::from(key.band)];
        assert!(band.clearance_m <= altitude, "{altitude}: {band:?}");
        let next = bands.get(usize::from(key.band) + 1);
        assert!(
            next.is_none_or(|next| next.clearance_m > altitude),
            "{altitude}"
        );
        assert!(key.helicopter);
        let scale = f64::from(1u32 << (band.zoom - 12));
        assert_eq!(key.cell[0], (centre.x * scale) as u32);
    }
}

#[test]
fn a_segment_is_cut_into_chained_pieces_covering_it() {
    let terrain = HashMap::new();
    let placement = Placement::new(&HashSet::from([PRAGUE]), &terrain, BOX_EDGE_LEVEL_STEP_DB);
    let centre = PRAGUE.centre();
    let units = 300.0 / cell_edge_m(PRAGUE, 12);
    let start = (
        Mercator {
            x: centre.x - units,
            y: centre.y,
        },
        150.0,
    );
    let end = (
        Mercator {
            x: centre.x + units,
            y: centre.y + 0.2 * units,
        },
        250.0,
    );
    let pieces = cut_into_pieces(&placement, start, end, false);
    assert!(
        pieces.len() >= 6,
        "{} pieces over 600 m of 49-98 m boxes",
        pieces.len()
    );
    assert_eq!(pieces[0].start, start);
    assert_eq!(pieces.last().unwrap().end, end);
    for pair in pieces.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
        assert_ne!(pair[0].key, pair[1].key);
    }
    // Climbing from 150 m to 250 m, the pieces rise through the bands.
    assert!(pieces.last().unwrap().key.band > pieces[0].key.band);
}

#[test]
fn pieces_end_at_tile_edges() {
    let neighbour = TileId {
        x: PRAGUE.x + 1,
        ..PRAGUE
    };
    let terrain = HashMap::new();
    let placement = Placement::new(
        &HashSet::from([PRAGUE, neighbour]),
        &terrain,
        BOX_EDGE_LEVEL_STEP_DB,
    );
    let at = |x: f64| {
        (
            Mercator {
                x,
                y: f64::from(PRAGUE.y) + 0.3,
            },
            2_000.0,
        )
    };
    let edge = f64::from(neighbour.x);
    let pieces = cut_into_pieces(&placement, at(edge - 0.05), at(edge + 0.05), false);
    let crossing = pieces
        .windows(2)
        .find(|pair| pair[0].key.tile != pair[1].key.tile)
        .expect("the segment crosses into the neighbour");
    assert_eq!(crossing[0].end.0.x, edge);
    assert_eq!(crossing[1].start.0.x, edge);
    for piece in &pieces {
        let tile = f64::from(piece.key.tile.x);
        assert!(piece.start.0.x >= tile && piece.end.0.x <= tile + 1.0);
    }
}

/// A steep descent from 2,000 m to 20 m crosses bands from coarse to fine within a coarse box's
/// step; every piece ends within an eighth of its box's edge of the box (the flight list's bounds
/// take no more).
#[test]
fn a_descent_keeps_every_piece_within_an_eighth_of_its_box() {
    let terrain = HashMap::new();
    let placement = Placement::new(&HashSet::from([PRAGUE]), &terrain, BOX_EDGE_LEVEL_STEP_DB);
    let centre = PRAGUE.centre();
    let units = 400.0 / cell_edge_m(PRAGUE, 12);
    let start = (
        Mercator {
            x: centre.x - units,
            y: centre.y - 0.3 * units,
        },
        2_000.0,
    );
    let end = (
        Mercator {
            x: centre.x + units,
            y: centre.y + 0.3 * units,
        },
        20.0,
    );
    let pieces = cut_into_pieces(&placement, start, end, false);
    let bands = placement.bands(PRAGUE).to_vec();
    assert!(pieces.len() > 10, "{} pieces", pieces.len());
    for piece in &pieces {
        let zoom = bands[usize::from(piece.key.band)].zoom;
        let edge = 1.0 / f64::from(1u32 << (zoom - 12));
        for (end, _) in [piece.start, piece.end] {
            for (coordinate, cell) in [(end.x, piece.key.cell[0]), (end.y, piece.key.cell[1])] {
                let west = f64::from(cell) * edge;
                let outside = (west - coordinate).max(coordinate - west - edge);
                assert!(
                    outside <= edge / 8.0 + 1e-9,
                    "a piece of a zoom {zoom} box {} edges past it",
                    outside / edge
                );
            }
        }
    }
}

/// A tile's cut is the whole cut's pieces in that tile, whatever the segment crosses: long and
/// short, through a corner, along an edge, climbing through bands (the builders cut tile by tile).
#[test]
fn a_tile_cut_is_the_whole_cut_within_it() {
    let tiles: HashSet<TileId> = [0, 1, 2].iter().flat_map(|&ring| PRAGUE.ring(ring)).collect();
    let terrain = HashMap::new();
    let placement = Placement::new(&tiles, &terrain, BOX_EDGE_LEVEL_STEP_DB);
    let corner = Mercator {
        x: f64::from(PRAGUE.x),
        y: f64::from(PRAGUE.y),
    };
    let at = |dx: f64, dy: f64| Mercator {
        x: corner.x + dx,
        y: corner.y + dy,
    };
    for (start, end) in [
        ((at(-1.3, -0.7), 40.0), (at(1.6, 1.9), 3_000.0)),
        ((at(-0.5, -0.5), 900.0), (at(0.5, 0.5), 900.0)),
        ((at(0.0, -1.5), 120.0), (at(0.0, 1.5), 60.0)),
        ((at(0.2, 0.3), 30.0), (at(0.25, 0.31), 35.0)),
        ((at(1.99, -0.99), 11_000.0), (at(-1.99, 0.99), 9_000.0)),
    ] {
        let whole = cut_into_pieces(&placement, start, end, false);
        let mut joined = Vec::new();
        for tile in &tiles {
            let within = cut_within_tile(&placement, start, end, false, *tile);
            let expected: Vec<BoxPiece> = whole
                .iter()
                .filter(|piece| piece.key.tile == *tile)
                .copied()
                .collect();
            assert_eq!(within, expected, "{tile:?} of {start:?} to {end:?}");
            joined.extend(within);
        }
        assert_eq!(joined.len(), whole.len());
    }
}

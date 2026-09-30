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

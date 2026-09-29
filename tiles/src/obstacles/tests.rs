//! Obstacles format: round trip, the cell table, the supercover walk and corrupt files.

use super::*;
use crate::geo::{GlobalSteps, TILES_PER_AXIS, TileId};

fn outline_at(parsed: &Obstacles, index: usize) -> Outline {
    let record = parsed.outline(index);
    Outline {
        footprint_id: record.footprint_id,
        kind: record.kind,
        envelope: record.envelope,
        height_m: record.height_m,
        vertices: (0..record.vertex_count)
            .map(|k| parsed.vertex(record.first_vertex + k))
            .collect(),
    }
}

/// A 600-step block with a 200-step courtyard, and a wall whose last edge lies east of the tile.
fn courtyard_block_and_wall() -> Vec<Outline> {
    let ring = |kind, half: i16| Outline {
        footprint_id: 0x0114_00ad_0000_0007,
        kind,
        envelope: EnvelopeClass::Residential,
        height_m: 29.0,
        vertices: vec![
            [-half, -half],
            [half, -half],
            [half, half],
            [-half, half],
            [-half, -half],
        ],
    };
    vec![
        ring(OutlineKind::Exterior, 300),
        ring(OutlineKind::Hole, 100),
        Outline {
            footprint_id: 0x0114_00ad_0000_0009,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Outdoor,
            height_m: 3.6,
            vertices: vec![[16_000, 10], [16_300, 20], [16_600, 30], [32_767, -32_768]],
        },
    ]
}

fn runs_of(parsed: &Obstacles, column: usize, row: usize) -> Vec<Run> {
    parsed.cell_runs(row * 128 + column).collect()
}

fn run(outline: u32, first_edge: u16, edge_count: u16) -> Run {
    Run {
        outline,
        first_edge,
        edge_count,
    }
}

#[test]
fn obstacles_round_trip() {
    let outlines = courtyard_block_and_wall();
    let bytes = encode(&outlines);
    let parsed = Obstacles::parse(&bytes).unwrap();
    assert_eq!((parsed.outline_count(), parsed.vertex_count()), (3, 14));
    for (index, outline) in outlines.iter().enumerate() {
        assert_eq!(&outline_at(&parsed, index), outline);
    }
    // The block's west edge (edge 3, x = -300) runs down column 62 from row 65 to row 62; the
    // corner cells also hold the edge before it (top, consecutive: one run) or edge 0 (bottom).
    assert_eq!(runs_of(&parsed, 62, 65), vec![run(0, 2, 2)]);
    assert_eq!(runs_of(&parsed, 62, 64), vec![run(0, 3, 1)]);
    assert_eq!(runs_of(&parsed, 62, 62), vec![run(0, 0, 1), run(0, 3, 1)]);
    assert_eq!(runs_of(&parsed, 63, 63), vec![run(1, 0, 1), run(1, 3, 1)]);
    // The wall's first two edges share column 127, its last edge lies beyond the tile: stored
    // whole, listed only where it crosses the tile's cells.
    assert_eq!(runs_of(&parsed, 126, 64), vec![run(2, 0, 1)]);
    assert_eq!(runs_of(&parsed, 127, 64), vec![run(2, 0, 2)]);
    let listed = (0..CELLS)
        .flat_map(|cell| parsed.cell_runs(cell))
        .filter(|run| run.outline == 2);
    assert_eq!(listed.count(), 2);
    assert_eq!(Obstacles::parse(&encode(&[])).unwrap().run_count(), 0);
}

#[test]
fn a_supercover_steps_the_row_first_on_a_tie_and_stays_between_its_end_cells() {
    let cells = |start, end| {
        let mut visited = Vec::new();
        for_each_edge_cell(start, end, |cell| visited.push(cell));
        visited
    };
    assert_eq!(
        cells([0, 0], [512, 512]),
        vec![[0, 0], [0, 1], [1, 1], [1, 2], [2, 2]]
    );
    assert_eq!(cells([5, 5], [5, 5]), vec![[0, 0]]);
    assert_eq!(cells([-1, -1], [-300, -1]), vec![[-1, -1], [-2, -1]]);
    // Ending exactly on a lattice corner while moving north ties at t = 1; dev4's walk stepped
    // the row there, out of the box, and wandered on. This walk finishes along the end row.
    assert_eq!(cells([0, 511], [256, 256]), vec![[0, 1], [1, 1]]);
}

#[test]
fn an_outline_is_listed_in_every_tile_whose_cells_it_crosses() {
    let tile = TileId { x: 2212, y: 1387 };
    let east_edge = tile.global([16_383, 0]);
    let across = [
        east_edge,
        GlobalSteps {
            x: east_edge.x + 2,
            ..east_edge
        },
    ];
    assert_eq!(
        tiles_crossed(&across),
        vec![tile, TileId { x: 2213, y: 1387 }]
    );
    let dateline = [
        GlobalSteps { x: -10, y: 5_000 },
        GlobalSteps { x: 10, y: 5_000 },
    ];
    assert_eq!(
        tiles_crossed(&dateline),
        vec![
            TileId { x: 0, y: 0 },
            TileId {
                x: TILES_PER_AXIS - 1,
                y: 0
            }
        ]
    );
    let beyond_the_pole = [GlobalSteps { x: 0, y: -600 }, GlobalSteps { x: 0, y: -300 }];
    assert!(tiles_crossed(&beyond_the_pole).is_empty());
}

#[test]
fn corrupt_files_are_rejected() {
    let bytes = encode(&courtyard_block_and_wall());
    let parsed = Obstacles::parse(&bytes).unwrap();
    assert!(Obstacles::parse(&bytes[..bytes.len() - 1]).is_err());
    assert!(Obstacles::parse(&[bytes.as_slice(), &[0]].concat()).is_err());
    let rejected = |changes: &[(usize, u8)]| {
        let mut copy = bytes.clone();
        for &(at, value) in changes {
            copy[at] = value;
        }
        Obstacles::parse(&copy).is_err()
    };
    let runs = HEADER_BYTES + 4 * (CELLS + 1);
    let outlines = runs + RUN_BYTES * parsed.run_count();
    assert!(rejected(&[(0, b'x')]), "magic");
    assert!(rejected(&[(8, 4)]), "outline count against the length");
    assert!(rejected(&[(25, 1)]), "reserved header byte");
    assert!(
        rejected(&[(HEADER_BYTES + 4, 0xff)]),
        "cell offsets out of order"
    );
    // The first run is the block's edge 0 in cell (62, 62).
    assert!(rejected(&[(runs, 7)]), "a run naming a missing outline");
    assert!(
        rejected(&[(runs + 6, 5)]),
        "a run past its outline's last edge"
    );
    assert!(
        !rejected(&[(runs + 6, 4)]),
        "a run up to the last edge is fine"
    );
    assert!(rejected(&[(outlines + 16, 3)]), "outline kind");
    assert!(rejected(&[(outlines + 17, 6)]), "envelope class");
    assert!(
        rejected(&[(outlines + 14, 0), (outlines + 15, 0)]),
        "zero height"
    );
    assert!(
        rejected(&[(outlines + 12, 4)]),
        "a ring that no longer closes"
    );
    assert!(
        rejected(&[(outlines + OUTLINE_BYTES, 0xff)]),
        "footprint ids out of order"
    );
}

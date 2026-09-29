//! Screening outline rules (dev4 `obstacle_index/tests/construction.rs`).

use super::*;

fn at(x: i64, y: i64) -> GlobalSteps {
    GlobalSteps { x, y }
}

/// A closed square ring of `half` steps around (x, y), as structures.arrow stores rings.
fn ring(x: i64, y: i64, half: i64) -> Vec<GlobalSteps> {
    let corners = [
        (-half, -half),
        (half, -half),
        (half, half),
        (-half, half),
        (-half, -half),
    ];
    corners.iter().map(|&(dx, dy)| at(x + dx, y + dy)).collect()
}

fn building(parts: Vec<Vec<Vec<GlobalSteps>>>) -> RowGeometry {
    RowGeometry::Building {
        envelope: EnvelopeClass::Residential,
        parts,
    }
}

fn outlines_of(height_m: f64, geometry: RowGeometry) -> Vec<ScreeningOutline> {
    let mut out = Vec::new();
    row_outlines(7, height_m, geometry, &mut out);
    out
}

#[test]
fn building_heights_are_clamped_at_828_m_and_wall_heights_never() {
    let block = outlines_of(31_231.0, building(vec![vec![ring(0, 0, 50)]]));
    assert_eq!(
        block.iter().map(|o| o.height_m).collect::<Vec<_>>(),
        vec![828.0]
    );
    let wall = outlines_of(900.0, RowGeometry::Wall(vec![at(0, 0), at(100, 0)]));
    assert_eq!(
        (wall[0].height_m, wall[0].kind, wall[0].envelope),
        (900.0, OutlineKind::Wall, EnvelopeClass::Outdoor)
    );
}

#[test]
fn rings_close_once_and_holes_follow_their_exterior() {
    let open = ring(0, 0, 50)[..4].to_vec();
    let outlines = outlines_of(
        12.0,
        building(vec![vec![open, ring(0, 0, 10)], vec![ring(500, 0, 20)]]),
    );
    let kinds: Vec<_> = outlines.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        vec![
            OutlineKind::Exterior,
            OutlineKind::Hole,
            OutlineKind::Exterior
        ]
    );
    assert!(
        outlines
            .iter()
            .all(|o| o.vertices.len() == 5 && o.vertices[0] == o.vertices[4])
    );
    assert!(
        outlines
            .iter()
            .all(|o| o.footprint_id == 7 && o.envelope == EnvelopeClass::Residential)
    );
}

#[test]
fn degenerate_inputs_are_ignored() {
    let two_points = vec![at(0, 0), at(10, 0), at(0, 0)];
    assert!(outlines_of(5.0, building(vec![vec![two_points.clone()]])).is_empty());
    assert!(outlines_of(0.0, building(vec![vec![ring(0, 0, 50)]])).is_empty());
    assert!(outlines_of(3.0, RowGeometry::Wall(vec![at(0, 0), at(0, 0)])).is_empty());
    // Vertices repeated (as quantisation can make them) collapse the ring below three.
    let repeated = vec![at(0, 0), at(0, 0), at(1, 0), at(1, 0), at(0, 0)];
    assert!(outlines_of(5.0, building(vec![vec![repeated]])).is_empty());
    // A part whose exterior collapsed goes with its holes; the next part stays.
    let parts = vec![vec![two_points, ring(0, 0, 10)], vec![ring(500, 0, 20)]];
    let kept = outlines_of(5.0, building(parts));
    assert_eq!((kept.len(), kept[0].vertices[0]), (1, at(480, -20)));
}

#[test]
fn non_finite_heights_are_rejected() {
    for height_m in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(outlines_of(height_m, building(vec![vec![ring(0, 0, 50)]])).is_empty());
        assert!(outlines_of(height_m, RowGeometry::Wall(vec![at(0, 0), at(9, 9)])).is_empty());
    }
}

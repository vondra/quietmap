//! Screening outlines of one structures row, by dev4's formation rules
//! (`obstacle_index/builder.rs`): which rings and walls screen, at what height.

use tiles::geo::GlobalSteps;
use tiles::obstacles::{EnvelopeClass, OutlineKind};

/// The tallest building on Earth (Burj Khalifa, 828 m); a taller mapped value is a tag error.
const BUILDING_HEIGHT_MAX_M: f64 = 828.0;

/// One ring or wall of a row; a ring is closed (its first vertex repeated last).
pub struct ScreeningOutline {
    pub footprint_id: u64,
    pub kind: OutlineKind,
    pub envelope: EnvelopeClass,
    pub height_m: f64,
    pub vertices: Vec<GlobalSteps>,
}

/// A row's geometry on the lattice: a building's parts (exterior ring first, then its holes) or a
/// wall's points.
pub enum RowGeometry {
    Building {
        envelope: EnvelopeClass,
        parts: Vec<Vec<Vec<GlobalSteps>>>,
    },
    Wall(Vec<GlobalSteps>),
}

fn without_repeats(points: Vec<GlobalSteps>) -> Vec<GlobalSteps> {
    let mut kept: Vec<GlobalSteps> = Vec::with_capacity(points.len() + 1);
    for point in points {
        if kept.last() != Some(&point) {
            kept.push(point);
        }
    }
    kept
}

/// The outlines of one row: none below or at 0 m or non-finite; a building clamped at 828 m, each
/// ring without repeated vertices (the closing repeat included, which quantisation can also
/// create) and with three or more, a part dropped with its exterior; a wall with two points or
/// more.
pub fn row_outlines(
    footprint_id: u64,
    height_m: f64,
    geometry: RowGeometry,
    out: &mut Vec<ScreeningOutline>,
) {
    if !height_m.is_finite() || height_m <= 0.0 {
        return;
    }
    let outline = |kind, envelope, height_m, vertices| ScreeningOutline {
        footprint_id,
        kind,
        envelope,
        height_m,
        vertices,
    };
    match geometry {
        RowGeometry::Building { envelope, parts } => {
            let height_m = height_m.min(BUILDING_HEIGHT_MAX_M);
            for part in parts {
                for (index, ring) in part.into_iter().enumerate() {
                    let mut vertices = without_repeats(ring);
                    while vertices.len() > 1 && vertices.first() == vertices.last() {
                        vertices.pop();
                    }
                    if vertices.len() < 3 {
                        if index == 0 {
                            break;
                        }
                        continue;
                    }
                    vertices.push(vertices[0]);
                    let kind = if index == 0 {
                        OutlineKind::Exterior
                    } else {
                        OutlineKind::Hole
                    };
                    out.push(outline(kind, envelope, height_m, vertices));
                }
            }
        }
        RowGeometry::Wall(points) => {
            let vertices = without_repeats(points);
            if vertices.len() >= 2 {
                out.push(outline(
                    OutlineKind::Wall,
                    EnvelopeClass::Outdoor,
                    height_m,
                    vertices,
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "screening_tests.rs"]
mod tests;

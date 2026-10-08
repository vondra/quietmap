//! Noise screens that OpenStreetMap lacks, from national topographic databases
//! (`fetch/barriers.sh`), by the z9 square of their first point: walls for the obstacles beside
//! dev4's, each but those whose middle lies within [`DUPLICATE_M`] of a wall already read (the
//! same screen mapped in OpenStreetMap).

use crate::dev4::{Square, degrees_to_z30};
use crate::screening::{RowGeometry, ScreeningOutline, row_outlines};
use crate::structures::{footprint_id, nearest_copy};
use std::collections::HashMap;
use std::path::Path;
use tiles::geo::{GlobalSteps, Mercator};
use tiles::obstacles::OutlineKind;

/// A screen this near a wall already read is that wall (m; dev4's rule for its walls).
const DUPLICATE_M: f64 = 5.0;
/// The height of a screen the database gives none (m): the walls' default, as measured screens
/// stand (GDDKiA's S14 3.6 m, Germany's 3.7, the Netherlands' 3.8; evidence 2026-10-08, Poland).
const UNMAPPED_HEIGHT_M: f64 = 4.0;
/// Footprint ordinals of the screens, above any of dev4's in the square.
const FIRST_ORDINAL: u32 = 1 << 31;
/// Metres per lattice step at the equator.
const EQUATOR_M_PER_STEP: f64 = 40_075_016.686 / (32_768.0 * 4_096.0);

/// One screen: its height (m, 0 when unknown) and its axis (latitude, longitude).
#[derive(Debug, Clone, PartialEq)]
pub struct Barrier {
    pub height_m: f64,
    pub points: Vec<(f64, f64)>,
}

/// The screens by the z9 square of their first point.
pub struct Barriers {
    by_square: HashMap<(u32, u32), Vec<Barrier>>,
}

impl Barriers {
    /// Reads `barriers.txt`: `source height_m lat lon lat lon ...` per line (tab-separated).
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut by_square: HashMap<(u32, u32), Vec<Barrier>> = HashMap::new();
        for (number, line) in text.lines().enumerate() {
            let bad = |error: &dyn std::fmt::Display| format!("line {}: {error}", number + 1);
            let fields: Vec<&str> = line.split('\t').collect();
            let numbers = fields
                .iter()
                .skip(1)
                .map(|field| field.parse::<f64>().map_err(|error| bad(&error)))
                .collect::<Result<Vec<_>, _>>()?;
            let Some((&height_m, coordinates)) = numbers.split_first() else {
                return Err(bad(&"no height"));
            };
            if coordinates.len() < 4 || coordinates.len() % 2 != 0 {
                return Err(bad(&"expected two points or more"));
            }
            let points: Vec<(f64, f64)> = coordinates.chunks(2).map(|p| (p[0], p[1])).collect();
            if points
                .iter()
                .any(|(lat, lon)| lat.abs() > 90.0 || lon.abs() > 180.0)
            {
                return Err(bad(&"no place on Earth"));
            }
            let (gx, gy) = degrees_to_z30(points[0].0, points[0].1);
            let square = Square::of_z30(gx, gy);
            by_square
                .entry((square.x, square.y))
                .or_default()
                .push(Barrier { height_m, points });
        }
        Ok(Barriers { by_square })
    }

    /// The walls of `square`'s screens on the lattice copy nearest `reference_x`, but those within
    /// [`DUPLICATE_M`] of a wall among `read`.
    pub fn walls(
        &self,
        square: Square,
        reference_x: i64,
        read: &[ScreeningOutline],
    ) -> Vec<ScreeningOutline> {
        let screens = self
            .by_square
            .get(&(square.x, square.y))
            .map_or(&[][..], Vec::as_slice);
        let mut out = Vec::new();
        for (index, screen) in screens.iter().enumerate() {
            let vertices: Vec<GlobalSteps> = screen
                .points
                .iter()
                .map(|&(lat, lon)| {
                    let steps = GlobalSteps::nearest(Mercator::from_degrees(lat, lon));
                    nearest_copy(steps, reference_x)
                })
                .collect();
            let middle = vertices[vertices.len() / 2];
            let (lat, _) = screen.points[screen.points.len() / 2];
            let reach = DUPLICATE_M / (EQUATOR_M_PER_STEP * lat.to_radians().cos());
            let duplicate = read
                .iter()
                .filter(|outline| outline.kind == OutlineKind::Wall)
                .flat_map(|wall| wall.vertices.windows(2))
                .any(|pair| distance_to_segment(middle, pair[0], pair[1]) <= reach);
            if duplicate {
                continue;
            }
            let height_m = if screen.height_m > 0.0 {
                screen.height_m
            } else {
                UNMAPPED_HEIGHT_M
            };
            row_outlines(
                footprint_id(square, FIRST_ORDINAL + index as u32),
                height_m,
                RowGeometry::Wall(vertices),
                &mut out,
            );
        }
        out
    }
}

/// Distance (lattice steps) from `point` to the segment `a`-`b`.
fn distance_to_segment(point: GlobalSteps, a: GlobalSteps, b: GlobalSteps) -> f64 {
    let (px, py) = ((point.x - a.x) as f64, (point.y - a.y) as f64);
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > 0.0 {
        ((px * dx + py * dy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - t * dx).hypot(py - t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiles::obstacles::EnvelopeClass;

    /// A screen 3 m beside a wall already read is that wall; one 20 m away is a new 4 m wall.
    #[test]
    fn a_screen_beside_a_mapped_wall_is_that_wall() {
        let north = |metres: f64| 52.0 + metres / 111_195.0;
        let east = |metres: f64| 19.0 + metres / (111_195.0 * 52f64.to_radians().cos());
        let barriers = Barriers::parse(&format!(
            "bdot10k\t0\t{}\t{}\t{}\t{}\nbdot10k\t0\t{}\t{}\t{}\t{}\n",
            north(3.0),
            east(0.0),
            north(3.0),
            east(100.0),
            north(20.0),
            east(0.0),
            north(20.0),
            east(100.0),
        ))
        .unwrap();
        let steps = |lat: f64, lon: f64| GlobalSteps::nearest(Mercator::from_degrees(lat, lon));
        let wall = ScreeningOutline {
            footprint_id: 1,
            kind: OutlineKind::Wall,
            envelope: EnvelopeClass::Default,
            height_m: 4.0,
            vertices: vec![steps(52.0, east(0.0)), steps(52.0, east(100.0))],
        };
        let (gx, gy) = degrees_to_z30(52.0, 19.0);
        let square = Square::of_z30(gx, gy);
        let walls = barriers.walls(square, wall.vertices[0].x, &[wall]);
        assert_eq!(walls.len(), 1);
        assert_eq!(walls[0].height_m, 4.0);
        assert_eq!(
            walls[0].footprint_id & 0xffff_ffff,
            u64::from(FIRST_ORDINAL + 1)
        );
    }
}

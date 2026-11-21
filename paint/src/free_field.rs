//! The free field of a block's own sources at a point, by layer: the geometry a blend between exact points cannot tell.

use crate::exact::LAYERS;
use crate::paint::metres_at;
use crate::square::Square;
use physics::line::{LinePieceGeometry, POINT_DIVERGENCE_LINEAR};
use popup::answer::RECEIVER_HEIGHT_M;

/// A block's own sources (crossing it or the eight around) with the ground under each one's ends
/// as its rays stand on it (`Candidate::traced_ground_m`: a bridge's deck, else the terrain), read
/// once for all the block's points.
pub(crate) struct OwnSources(Vec<(u32, [f64; 2])>);

impl OwnSources {
    pub fn of(square: &Square, own: &[u32]) -> Result<Self, String> {
        (own.iter())
            .map(|&index| {
                let ground_m = square.candidates[index as usize].traced_ground_m(&square.ground)?;
                Ok((index, ground_m))
            })
            .collect::<Result<_, String>>()
            .map(OwnSources)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// [`OwnSources::free_field`] at the centre of pixel `at` (row-major) of a square of `pixels`
    /// a side, a receiver over its ground; `None` where the ground cannot be read: no blend, a
    /// pass paints the pixel and reports the ground.
    pub fn free_field_at_pixel(
        &self,
        square: &Square,
        pixels: usize,
        at: usize,
    ) -> Option<[f64; LAYERS]> {
        if self.is_empty() {
            return Some([0.0; LAYERS]);
        }
        let centre = ((at % pixels) as f64 + 0.5, (at / pixels) as f64 + 0.5);
        let position = metres_at(square, pixels, centre.0, centre.1);
        let ground = square.ground.at(position).ok()?;
        Some(self.free_field(square, position, ground.height_m + RECEIVER_HEIGHT_M))
    }

    /// The Lden energy the sources would deliver by layer at a receiver at `position` (frame
    /// metres) and `altitude_m` with nothing but distance between, as `popup::evaluate::source_rays`
    /// spreads them (the two must agree, or the excess takes a distance): a point's power over
    /// its distance squared, a line piece's angle over its distance (`physics::line`: what its
    /// rays' weights sum to). Beside a road this falls as 1/r between exact points 25 m apart, which
    /// their blend reads up to a decibel wrong, and where the receiver, 4 m over its own ground,
    /// stands level with a road on an embankment or a bridge it rises by 10 dB within a pixel;
    /// what the ground, the air and the terrain take of it changes slowly there.
    pub fn free_field(
        &self,
        square: &Square,
        position: [f64; 2],
        altitude_m: f64,
    ) -> [f64; LAYERS] {
        let mut free = [0.0; LAYERS];
        for &(index, ground_m) in &self.0 {
            let candidate = &square.candidates[index as usize];
            let source = &square.attributes[candidate.attribute];
            let [a, b] = (candidate.ends_m).map(|end| [end[0] - position[0], end[1] - position[1]]);
            // The emitters' altitude over the receiver's at the piece's ends.
            let [above_a, above_b] =
                ground_m.map(|ground_m| ground_m + source.height_m - altitude_m);
            let spread = if candidate.line {
                LinePieceGeometry::new([a[0], a[1], above_a], [b[0], b[1], above_b])
                    .map_or(0.0, |piece| {
                        piece.subtended_angle_rad() * piece.divergence_factor()
                    })
            } else {
                let distance = (a[0].hypot(a[1]).max(source.exclusion_radius_m))
                    .hypot(above_a)
                    .max(1.0);
                1.0 / (distance * distance * POINT_DIVERGENCE_LINEAR)
            };
            free[candidate.layer as usize] += source.lden_power * spread;
        }
        free
    }
}

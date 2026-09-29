//! The tiles a click has read so far, in the click's metre frame: the ground under any point and
//! the sampled profile of a ray.

use physics::profile::Profile;
use tiles::geo::{LocalFrame, TILES_PER_AXIS, TileId};
use tiles::terrain::{GroundSample, OCEAN, Terrain};

/// Terrain of every read tile within `radius` rings of the clicked tile, by offset.
pub struct Ground<'a> {
    pub frame: LocalFrame,
    centre: TileId,
    radius: i64,
    /// `None`: not read (an error to sample); `Some(None)`: read, absent (ocean).
    tiles: Vec<Option<Option<Terrain<'a>>>>,
}

impl<'a> Ground<'a> {
    pub fn new(frame: LocalFrame, centre: TileId, radius: u32) -> Self {
        let side = 2 * radius as usize + 1;
        Ground {
            frame,
            centre,
            radius: i64::from(radius),
            tiles: (0..side * side).map(|_| None).collect(),
        }
    }

    fn slot(&self, tile: TileId) -> Option<usize> {
        let n = i64::from(TILES_PER_AXIS);
        let dx = (i64::from(tile.x) - i64::from(self.centre.x) + n / 2).rem_euclid(n) - n / 2;
        let dy = i64::from(tile.y) - i64::from(self.centre.y);
        if dx.abs() > self.radius || dy.abs() > self.radius {
            return None;
        }
        Some(((dy + self.radius) * (2 * self.radius + 1) + dx + self.radius) as usize)
    }

    /// Records a read tile's terrain (`None` for an absent file: ocean).
    pub fn insert(&mut self, tile: TileId, terrain: Option<Terrain<'a>>) {
        let slot = self
            .slot(tile)
            .expect("a read tile lies within the scene radius");
        self.tiles[slot] = Some(terrain);
    }

    /// The ground at [east, north] metres; an error outside the read tiles or at a node without
    /// data (the click fails instead of answering quieter).
    pub fn at(&self, metres: [f64; 2]) -> Result<GroundSample, String> {
        let position = self.frame.to_mercator(metres);
        let tile = TileId::containing(position);
        let terrain = self
            .slot(tile)
            .and_then(|slot| self.tiles[slot].as_ref())
            .ok_or_else(|| format!("terrain of tile {tile:?} not read"))?;
        let Some(terrain) = terrain else {
            return Ok(OCEAN);
        };
        terrain.sample(position).ok_or_else(|| {
            let (lat, lon) = position.to_degrees();
            format!("no terrain data at {lat:.6},{lon:.6}")
        })
    }

    /// Samples the ground under the ray from `source` to the receiver at the origin.
    pub fn fill_profile(&self, source: [f64; 2], profile: &mut Profile) -> Result<(), String> {
        profile.reset(source[0].hypot(source[1]).max(1.0));
        for index in 0..profile.t.len() {
            let keep = 1.0 - profile.t[index];
            let sample = self.at([source[0] * keep, source[1] * keep])?;
            profile.ground_m.push(sample.height_m);
            profile.ground_factor.push(sample.ground_factor);
            profile.forest_cover.push(sample.forest_cover);
        }
        Ok(())
    }
}

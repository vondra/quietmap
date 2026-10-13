//! Tall-building coverage of a continuously sliding window on the global step lattice.

use super::Scene;
use super::reflection_coverage::{REFLECTION_LATTICE_STEPS, ReflectionCoverage, raster_coordinate};
use super::tile::SceneTile;
use physics::reflection::{REFLECTION_WINDOW_M, receiver_reflection_db};
use tiles::geo::{LocalFrame, Mercator, STEPS_PER_TILE};
use tiles::obstacles::{CELL_STEPS, CELLS_PER_SIDE};

impl Scene<'_> {
    // A receiver's latitude fixes its window independently of the click or painter frame.
    pub(super) fn reflection_window_half_steps(&self, centre: [f64; 2]) -> [f64; 2] {
        let global = [0, 1].map(|axis| {
            ((self.lattice.origin_cell[axis] * CELL_STEPS) as f64 + centre[axis]) / STEPS_PER_TILE
        });
        let frame = LocalFrame::at(Mercator {
            x: global[0],
            y: global[1],
        });
        [frame.east_m_per_unit, frame.north_m_per_unit]
            .map(|scale| REFLECTION_WINDOW_M / 2.0 * STEPS_PER_TILE / scale)
    }

    fn reflection_tile_span(&self, low: [f64; 2], high: [f64; 2]) -> ([i64; 2], [i64; 2]) {
        let tile_coordinate = |point: [f64; 2]| {
            [0, 1].map(|axis| {
                (point[axis] + (self.origin_cell_in_centre[axis] * CELL_STEPS) as f64)
                    / STEPS_PER_TILE
            })
        };
        (
            tile_coordinate(low).map(|coordinate| coordinate.floor() as i64),
            tile_coordinate(high).map(|coordinate| coordinate.ceil() as i64),
        )
    }

    fn reflection_tile_at(&self, column: i64, row: i64) -> Result<Option<&SceneTile<'_>>, String> {
        let cell = [column, row];
        let cell =
            [0, 1].map(|axis| cell[axis] * CELLS_PER_SIDE - self.origin_cell_in_centre[axis]);
        Ok(self.tile_at(cell)?.map(|(tile, _)| tile))
    }

    fn tile_reflection_coordinate(tile: &SceneTile<'_>, point: [f64; 2]) -> [f64; 2] {
        [0, 1].map(|axis| raster_coordinate(point[axis] - tile.offset[axis] as f64))
    }

    /// Builds only the windows needed by receivers in this metre rectangle, before querying them.
    pub fn prepare_reflection_coverage(&self, low: [f64; 2], high: [f64; 2]) -> Result<(), String> {
        let a = self.lattice.steps(low);
        let b = self.lattice.steps(high);
        let half_a = self.reflection_window_half_steps(a);
        let half_b = self.reflection_window_half_steps(b);
        let half = [0, 1].map(|axis| half_a[axis].max(half_b[axis]));
        let low = [0, 1].map(|axis| a[axis].min(b[axis]) - half[axis]);
        let high = [0, 1].map(|axis| a[axis].max(b[axis]) + half[axis]);
        let (first, last) = self.reflection_tile_span(low, high);
        for row in first[1]..last[1] {
            for column in first[0]..last[0] {
                let Some(tile) = self.reflection_tile_at(column, row)? else {
                    continue;
                };
                let low = Self::tile_reflection_coordinate(tile, low);
                let high = Self::tile_reflection_coordinate(tile, high);
                let coverage = tile
                    .reflection_coverage
                    .get_or_init(|| Box::new(ReflectionCoverage::new(tile.obstacles, low, high)));
                if !coverage.covers(low, high) {
                    return Err(
                        "reflection coverage was prepared for a smaller receiver extent".into(),
                    );
                }
            }
        }
        Ok(())
    }

    /// Tall-building share of the receiver's window; its own footprint alone is excluded (CNOSSOS 2.8).
    pub fn reflection_building_share(
        &self,
        receiver: [f64; 2],
        own_footprint: Option<u64>,
    ) -> Result<f64, String> {
        let centre = self.lattice.steps(receiver);
        let half = self.reflection_window_half_steps(centre);
        let low = [0, 1].map(|axis| centre[axis] - half[axis]);
        let high = [0, 1].map(|axis| centre[axis] + half[axis]);
        let (first, last) = self.reflection_tile_span(low, high);
        let mut covered = 0.0;
        for row in first[1]..last[1] {
            for column in first[0]..last[0] {
                let Some(tile) = self.reflection_tile_at(column, row)? else {
                    continue;
                };
                let low = Self::tile_reflection_coordinate(tile, low);
                let high = Self::tile_reflection_coordinate(tile, high);
                let coverage = tile
                    .reflection_coverage
                    .get()
                    .ok_or("receiver reflection coverage was not prepared")?;
                if !coverage.covers(low, high) {
                    return Err("receiver is outside the prepared reflection extent".into());
                }
                covered += coverage.covered(low, high, own_footprint);
            }
        }
        let area = 4.0 * half[0] * half[1] / (REFLECTION_LATTICE_STEPS as f64).powi(2);
        Ok(covered / area)
    }

    /// Shared by click and painter; fractional edge cells keep the moving window continuous.
    pub fn reflection_db(
        &self,
        receiver: [f64; 2],
        own_footprint: Option<u64>,
    ) -> Result<f64, String> {
        Ok(receiver_reflection_db(
            self.reflection_building_share(receiver, own_footprint)?,
        ))
    }
}

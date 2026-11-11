//! The tiles and kinds a painter can read, including fine terrain read on demand.

use popup::aircraft::{FINE_BOXES_WITHIN_M, boxes::AIRCRAFT_REACH_M, rolls::ROLL_REACH_M};
use popup::candidates::GROUND_REACH_M;
use tiles::Kind;
use tiles::geo::{LocalFrame, TileId};

pub const KINDS: [Kind; 7] = [
    Kind::Terrain,
    Kind::Obstacles,
    Kind::Sources,
    Kind::Aircraft,
    Kind::AircraftFar,
    Kind::AircraftRolls,
    Kind::TerrainFine,
];

pub struct Neighbourhood {
    tile: TileId,
    frame: LocalFrame,
    pub ground_rings: u32,
    aircraft_rings: u32,
}

impl Neighbourhood {
    pub fn new(tile: TileId) -> Self {
        let frame = LocalFrame::at(tile.centre());
        let width = frame.east_m_per_unit.min(frame.north_m_per_unit);
        Self {
            tile,
            frame,
            ground_rings: (GROUND_REACH_M / width).ceil() as u32,
            aircraft_rings: (AIRCRAFT_REACH_M / width).ceil() as u32,
        }
    }

    pub fn tiles(&self) -> impl Iterator<Item = TileId> + '_ {
        (0..=self.aircraft_rings.max(self.ground_rings)).flat_map(|ring| self.tile.ring(ring))
    }

    pub fn contains(&self, other: TileId, kind: Kind) -> bool {
        // Ground and the device upload allocate this complete box; an unread slot is not ocean.
        let ring = other.ring_distance(self.tile);
        let square_gap = || {
            let [dx, dy] = other.offset_from(self.tile);
            let gap = |d: i64| (d.unsigned_abs() as f64 - 1.0).max(0.0);
            (gap(dx) * self.frame.east_m_per_unit).hypot(gap(dy) * self.frame.north_m_per_unit)
        };
        match kind {
            Kind::Terrain | Kind::TerrainFine | Kind::Obstacles | Kind::Sources => {
                ring <= self.ground_rings
            }
            Kind::Aircraft => ring <= self.aircraft_rings && square_gap() <= FINE_BOXES_WITHIN_M,
            Kind::AircraftFar => ring <= self.aircraft_rings,
            Kind::AircraftRolls => {
                ring <= self.aircraft_rings && square_gap() <= ROLL_REACH_M + 1.0
            }
            Kind::AircraftEvents => false,
        }
    }
}

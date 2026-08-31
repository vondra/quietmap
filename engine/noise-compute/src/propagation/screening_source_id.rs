//! Stable identities for physical obstacles used by line-source screening.

/// High-bit namespace tag for a wall microsegment.
const WALL_SOURCE_TAG: u64 = 1_u64 << 63;
/// Extraction leaves 47 payload bits for a non-negative OSM id.
const WALL_OSM_ID_LIMIT: i64 = 1_i64 << 47;

/// Stable identity of one physical screening candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ScreeningSourceId(u64);

/// A screening identity cannot fit its namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreeningSourceIdError {
    ObstacleOrdinalOutOfRange,
    WallOsmIdOutOfRange,
}

impl ScreeningSourceId {
    /// Obstacle edge identity: its ordinal in the packed ordered obstacle set.
    pub fn obstacle(flattened_edge_ordinal: u64) -> Result<Self, ScreeningSourceIdError> {
        if flattened_edge_ordinal >= WALL_SOURCE_TAG {
            return Err(ScreeningSourceIdError::ObstacleOrdinalOutOfRange);
        }
        Ok(Self(flattened_edge_ordinal))
    }

    /// Wall identity: exact `(osm_id, segment_idx)` without hashing.
    pub fn wall(osm_id: i64, segment_idx: i16) -> Result<Self, ScreeningSourceIdError> {
        if !(0..WALL_OSM_ID_LIMIT).contains(&osm_id) {
            return Err(ScreeningSourceIdError::WallOsmIdOutOfRange);
        }
        Ok(Self(
            WALL_SOURCE_TAG | ((osm_id as u64) << 16) | segment_idx as u16 as u64,
        ))
    }

    pub const fn bits(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_and_obstacle_namespaces_do_not_overlap() {
        let obstacle = ScreeningSourceId::obstacle(WALL_SOURCE_TAG - 1).unwrap();
        let wall = ScreeningSourceId::wall(WALL_OSM_ID_LIMIT - 1, i16::MAX).unwrap();
        assert_ne!(obstacle, wall);
        assert!(ScreeningSourceId::obstacle(WALL_SOURCE_TAG).is_err());
        assert!(ScreeningSourceId::wall(WALL_OSM_ID_LIMIT, 0).is_err());
    }
}

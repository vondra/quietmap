//! Prepared class identities and the frozen precedence of a multi-sport OSM tag.

pub use physics::emission::leisure::{
    ARTIFICIAL_TURF_PITCH as LEISURE_AGP, BASKETBALL as LEISURE_BASKETBALL,
    CAR_PARK as LEISURE_CAR_PARK, CAR_PARK_STREET as LEISURE_CAR_PARK_STREET,
    OUTDOOR_SEATING as LEISURE_OUTDOOR_SEATING, PADEL as LEISURE_PADEL, PITCH as LEISURE_PITCH,
    PLAYGROUND as LEISURE_PLAYGROUND, POOL as LEISURE_POOL, STADIUM as LEISURE_STADIUM,
    TENNIS as LEISURE_TENNIS,
};
pub use physics::emission::settlement::{
    FOOD_RETAIL as SETTLEMENT_FOOD_RETAIL, HOSPITALITY as SETTLEMENT_HOSPITALITY,
    HOUSE as SETTLEMENT_HOUSE, SILENT as SETTLEMENT_SILENT,
};

pub const SETTLEMENT_PARKING_STRUCTURE: u8 = 7;
pub(crate) use crate::sources::road::DERESTRICTED_CODE as SPEED_LIMIT_DERESTRICTED;

// Retain the source classifier's ordering; sound powers belong only to physics.
pub fn leisure_sport_precedence(class: u8) -> u8 {
    match class {
        LEISURE_PITCH => 4,
        LEISURE_PADEL => 3,
        LEISURE_POOL => 2,
        LEISURE_TENNIS => 1,
        _ => 0,
    }
}

/// OSM `sport=*` value (lower-cased) to leisure class id.
pub fn leisure_sport_class_id(sport: &str) -> Option<u8> {
    Some(match sport {
        "padel" => LEISURE_PADEL,
        "tennis" => LEISURE_TENNIS,
        "basketball" | "netball" | "handball" => LEISURE_BASKETBALL,
        "soccer" | "football" | "american_football" | "rugby" | "rugby_union" | "rugby_league"
        | "field_hockey" | "hockey" | "baseball" | "cricket" | "multi" => LEISURE_PITCH,
        "swimming" => LEISURE_POOL,
        _ => return None,
    })
}

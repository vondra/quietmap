//! Emission class ids shared with the future `noise-compute` transfer.
//!
//! TEMPORARY duplication: these u8s are owned by
//! `noise-compute/src/emission/{settlement,leisure}.rs`. They live here so
//! `osm-extract` builds standalone on the green field; the `noise-compute`
//! transfer reunites them (this module then becomes `pub use` re-exports).
//! Values verified 2026-09-04 against dev/1: a shipped id never moves, and a new
//! one is minted in `noise-compute` together with its emission profile and only
//! then mirrored here (leisure 8 and 9, the car parks, 2026-09-19).

// settlement.rs ids
/// Garage, carport, multi-storey car park: the profile is a structure's vent fans.
pub const SETTLEMENT_PARKING_STRUCTURE: u8 = 7;
pub const SETTLEMENT_SILENT: u8 = 10;
pub const SETTLEMENT_HOUSE: u8 = 11;
pub const SETTLEMENT_FOOD_RETAIL: u8 = 12;
pub const SETTLEMENT_HOSPITALITY: u8 = 13;
// normalize/mod.rs
pub const SPEED_LIMIT_DERESTRICTED: u8 = 255;
// leisure.rs ids
pub const LEISURE_PITCH: u8 = 0;
pub const LEISURE_PADEL: u8 = 1;
pub const LEISURE_TENNIS: u8 = 2;
pub const LEISURE_BASKETBALL: u8 = 3;
pub const LEISURE_PLAYGROUND: u8 = 4;
pub const LEISURE_POOL: u8 = 5;
pub const LEISURE_OUTDOOR_SEATING: u8 = 6;
pub const LEISURE_STADIUM: u8 = 7;
/// Open car park (`amenity=parking` AREA with no `building` tag): manoeuvring
/// cars, doors and trolleys on open ground — a source, never an obstacle.
pub const LEISURE_CAR_PARK: u8 = 8;
/// Street-side / lane parking: the same movements on a strip with no aisle.
pub const LEISURE_CAR_PARK_STREET: u8 = 9;

/// Loudness anchor at the class reference area, transcribed from the
/// `leisure_profile` comments: a year-average Lden for the sports (pitch 89 …
/// seating 66), the day Lw for the two car parks, which have no annualization.
/// Resolves multi-sport `sport=a;b` to the loudest — the same argmax the old
/// code computed live via `leisure_lw`, with identical last-wins tie semantics.
pub fn leisure_loudness_anchor(class: u8) -> i64 {
    match class {
        // Pitch 89: the Sport England AGP evidence (50.8 dB/m² over 7,000 m²
        // → 89.3 Lden), not the old open-air guess 78 — a pitch now beats a
        // padel court in a multi-sport value.
        LEISURE_PITCH => 89,
        LEISURE_PADEL => 81,
        LEISURE_CAR_PARK => 79,
        LEISURE_STADIUM => 78,
        LEISURE_POOL => 76,
        LEISURE_TENNIS => 74,
        LEISURE_PLAYGROUND => 71,
        LEISURE_BASKETBALL => 68,
        LEISURE_CAR_PARK_STREET => 68,
        LEISURE_OUTDOOR_SEATING => 66,
        // An id outside the table never reaches here: the spill writes only the
        // classes above. The arm keeps the function total, at the pitch anchor.
        _ => 89,
    }
}

/// OSM `sport=*` value (lower-cased) to leisure class id. Transcribed from
/// `leisure::sport_class`.
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

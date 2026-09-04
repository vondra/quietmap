//! Emission class ids shared with the future `noise-compute` transfer.
//!
//! TEMPORARY duplication: these u8s are owned by
//! `noise-compute/src/emission/{settlement,leisure}.rs`. They live here so
//! `osm-extract` builds standalone on the green field; the `noise-compute`
//! transfer reunites them (this module then becomes `pub use` re-exports).
//! Values verified 2026-09-04 against dev/1 — never invent new ids here.

// settlement.rs ids
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

/// Year-average Lden anchor at the class reference area, transcribed from the
/// `leisure_profile` comments (padel 81 … seating 66). Resolves multi-sport
/// `sport=a;b` to the loudest — the same argmax the old code computed live
/// via `leisure_lw`, with identical last-wins tie semantics.
pub fn leisure_loudness_anchor(class: u8) -> i64 {
    match class {
        LEISURE_PADEL => 81,
        LEISURE_STADIUM => 78,
        LEISURE_PITCH => 78,
        LEISURE_POOL => 76,
        LEISURE_TENNIS => 74,
        LEISURE_PLAYGROUND => 71,
        LEISURE_BASKETBALL => 68,
        LEISURE_OUTDOOR_SEATING => 66,
        _ => 78, // unknown → pitch anchor, same fallback as the profile fn
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

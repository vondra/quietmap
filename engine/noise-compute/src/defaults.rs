//! Road traffic priors for rows without a count, and legal speed defaults.
//!
//! ```text
//!   city arm (tier-1 metro)        Bangkok: hand-set section totals
//!   │   ↓
//!   country arm                    TH rural: hand-set section totals
//!   │   ↓
//!   classes 0-4                    measured per carriageway, by direction and built-up
//!   one-way secondary              half of the fitted two-way section, shared
//!   classes 5-12                   WORLD_DEFAULT section totals
//! ```
//!
//! No country or continent factor exists: vehicles per paved km (clamped
//! 0.7-1.3) measured worse than no factor in every scaled class
//! (leave-one-country-out MAE/bias dB with vs without: motorway 3.51/+1.32
//! vs 3.42/+1.28, trunk 5.00/+3.58 vs 4.72/+3.11, primary 4.28/+2.37 vs
//! 4.05/+1.93), and nine alternative country predictors failed to beat a
//! constant.
//!
//! Data format: `Aadt = (light, medium, heavy, moto)`, vehicles/day.

use crate::square_country_city::{Continent, SquareCountryCity};

/// Vehicle-class AADT tuple: (light, medium, heavy, moto), veh/day.
pub type Aadt = (f64, f64, f64, f64);

/// Both-directions section totals. Classes 5-12 use them as the prior;
/// classes 0-4 contribute only their vehicle-class proportions to the
/// measured carriageway prior below, except one-way secondary streets, which
/// share the fitted two-way section (see [`resolve_traffic_default`]).
pub const WORLD_DEFAULT: [Aadt; 13] = [
    (21600.0, 2400.0, 5700.0, 300.0), // 0 motorway — 30k
    (11700.0, 1200.0, 1800.0, 300.0), // 1 trunk — 15k
    (7470.0, 540.0, 810.0, 180.0),    // 2 primary
    (2640.0, 120.0, 180.0, 60.0),     // 3 secondary
    (720.0, 26.0, 38.0, 16.0),        // 4 tertiary
    (480.0, 5.0, 10.0, 5.0),          // 5 residential
    (98.0, 0.0, 1.0, 1.0),            // 6 living_street
    (240.0, 2.0, 5.0, 3.0),           // 7 service: parking aisles, driveways
    (4.0, 0.0, 1.0, 0.0),             // 8 track: tractor + occasional delivery
    (1200.0, 30.0, 80.0, 30.0),       // 9 unclassified: rural connector
    // Ramps — 15 % of the respective mainline (HCM 7 / FEHRL / CERTU
    // lower-range typical, calibrated against Pasito Blanco GC-1 popup).
    (3240.0, 360.0, 855.0, 45.0), // 10 motorway_link — 4500
    (1755.0, 180.0, 270.0, 45.0), // 11 trunk_link    — 2250
    (1120.5, 81.0, 121.5, 27.0),  // 12 primary_link  — 1350
];

/// Bangkok's metro id in the baked `city_id` column (`scripts/square-country-city/geography.json`).
pub const CITY_BANGKOK: u16 = 22;

/// A hand-set or world class total describes the whole two-way section and
/// is shared between its carriageways by the producer; a measured per-lane
/// prior already describes the one stored carriageway and is never divided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrafficDefault {
    SectionBothDirections(Aadt),
    Carriageway(Aadt),
}

/// A measured prior for one stored carriageway: vehicles per lane for a lanes tag of 1-6 (0 where the class
/// has no per-lane fit), else the whole count of a carriageway without such a tag, which sits far below
/// lanes x rate on main roads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarriagewayPrior {
    pub vehicles_per_lane: f64,
    pub untagged: f64,
}

pub use crate::road_traffic_priors_generated::MEASURED_CARRIAGEWAY_PRIORS;

/// The prior for a row without a count: city arm, country arm, then the
/// measured carriageway prior (classes 0-4, by direction and `built_up`) or the
/// world class total. `class` clamps to the WORLD_DEFAULT bounds. AUTHORITATIVE
/// for the native producer (`roads-finalize` allocation); runtime consumers
/// read prepared counts and never call this.
pub fn resolve_traffic_default(
    class: u8,
    square_country_city: SquareCountryCity,
    lanes: u8,
    one_way: bool,
    built_up: u8,
) -> TrafficDefault {
    let hand_set = (square_country_city.city_id != 0)
        .then(|| city_default(square_country_city.city_id, class))
        .flatten()
        .or_else(|| country_default(&square_country_city.country_iso, class));
    if let Some(section) = hand_set {
        return TrafficDefault::SectionBothDirections(section);
    }
    let world = WORLD_DEFAULT[(class as usize).min(WORLD_DEFAULT.len() - 1)];
    let Some(by_direction) = MEASURED_CARRIAGEWAY_PRIORS.get(class as usize) else {
        return TrafficDefault::SectionBothDirections(world);
    };
    // An unknown built-up flag (0) takes the prior fitted on both kinds of place.
    let place = usize::from(built_up.min(BUILT_UP_URBAN));
    // A one-way secondary street takes half the fitted two-way section: the
    // retired one-way arm read +3.0 dB on holdout genuine one-way secondary
    // streets and doubled split-mapped two-way streets (w3-priors, 2026-09-25).
    // The producer shares this section between the carriageways it finds and
    // gives a lone row one half; the table's class-3 one-way cells stay zero.
    if class == 3 && one_way {
        let total = by_direction[1][place].untagged;
        let scale = total / (world.0 + world.1 + world.2 + world.3);
        return TrafficDefault::SectionBothDirections((world.0 * scale, world.1 * scale, world.2 * scale, world.3 * scale));
    }
    let prior = by_direction[usize::from(!one_way)][place];
    let total = if prior.vehicles_per_lane > 0.0 && (1..=6).contains(&lanes) {
        f64::from(lanes) * prior.vehicles_per_lane
    } else {
        prior.untagged
    };
    let scale = total / (world.0 + world.1 + world.2 + world.3);
    TrafficDefault::Carriageway((world.0 * scale, world.1 * scale, world.2 * scale, world.3 * scale))
}

// One arm per (city_id, class). Values reflect each metro's published or
// enricher-coded tier defaults. Missing classes fall through to country.

fn city_default(city_id: u16, class: u8) -> Option<Aadt> {
    match (city_id, class) {
        // ─── Bangkok — TH metro (rural × 1.5) split 60/8/7/25 ─────────────
        // Derivation: the TH rural arm totals × 1.5, split via
        // pipeline/enrich-roads-th.ts thaiClassSplit(isBangkok=true).
        (CITY_BANGKOK, 0) => Some((54000.0, 7200.0, 6300.0, 22500.0)), // 90k motorway
        (CITY_BANGKOK, 1) => Some((27000.0, 3600.0, 3150.0, 11250.0)), // 45k trunk
        (CITY_BANGKOK, 2) => Some((13500.0, 1800.0, 1575.0, 5625.0)),  // 22.5k primary
        (CITY_BANGKOK, 3) => Some((5400.0, 720.0, 630.0, 2250.0)),     // 9k secondary
        (CITY_BANGKOK, 4) => Some((2250.0, 300.0, 263.0, 937.0)),      // 3.75k tertiary
        (CITY_BANGKOK, 5) => Some((1080.0, 144.0, 126.0, 450.0)),      // 1.8k residential

        _ => None,
    }
}

fn country_default(iso: &[u8; 2], class: u8) -> Option<Aadt> {
    // Hand-set arms exist only where an enricher publishes per-class AADT
    // but no per-section measured census is wired into the pipeline.
    match (iso, class) {
        // ─── Thailand rural — split 62/10/13/15 ───────────────────────────
        // Thai-tuned class totals × pipeline/enrich-roads-th.ts
        // thaiClassSplit(isBangkok=false). Rural baseline; a Bangkok square
        // overrides via CITY_BANGKOK above.
        (b"TH", 0) => Some((37200.0, 6000.0, 7800.0, 9000.0)), // 60k motorway
        (b"TH", 1) => Some((18600.0, 3000.0, 3900.0, 4500.0)), // 30k trunk
        (b"TH", 2) => Some((9300.0, 1500.0, 1950.0, 2250.0)),  // 15k primary
        (b"TH", 3) => Some((3720.0, 600.0, 780.0, 900.0)),     // 6k secondary
        (b"TH", 4) => Some((1550.0, 250.0, 325.0, 375.0)),     // 2.5k tertiary
        (b"TH", 5) => Some((744.0, 120.0, 156.0, 180.0)),      // 1.2k residential

        _ => None,
    }
}

// ── Legal default SPEEDS for untagged maxspeed (task #15, 2026-07-03) ──────────────
//
// Scope is deliberately NARROW (/gg consensus Codex+Gemini): only untagged main-network
// classes resolve through the country table — 0 motorway, 1 trunk, 2/3/4/9 by the
// segment's built-up flag. Residential/living/service/track (5-8) and links (10-12)
// KEEP the legacy low defaults: routing them to a national urban limit would raise
// local streets +20-30 km/h (≈ +10 dB) worldwide with no supporting evidence.

use crate::country_speed_defaults_generated::COUNTRY_SPEEDS;

/// `built_up` comes from the roads.arrow column of the same name, classified
/// from vector-footprint density: 0 = unknown (coverage missing — never guess
/// rural), 1 = rural, 2 = urban.
pub const BUILT_UP_UNKNOWN: u8 = 0;
pub const BUILT_UP_RURAL: u8 = 1;
pub const BUILT_UP_URBAN: u8 = 2;

/// The country's LEGAL implicit speed for an untagged road, or None → caller uses the
/// legacy `default_road_speed` world table. The SquareCountryCity is the SEGMENT's own when the
/// M3 baked columns are present (see [`baked_square_country_city`]); on pre-bake data it is the
/// receiver's/region's — the accepted border approximation the AADT cascade above
/// also makes.
pub fn resolve_speed_default(
    class: u8,
    square_country_city: SquareCountryCity,
    built_up: u8,
) -> Option<f64> {
    let row = COUNTRY_SPEEDS
        .binary_search_by(|(iso, _)| iso[..].cmp(&square_country_city.country_iso[..]))
        .ok()
        .map(|i| COUNTRY_SPEEDS[i].1)?;
    let [urban, rural, motorway, motorroad] = row;
    let v = match class {
        0 => motorway,
        // trunk: motorroad where the country defines one (CZ 110), else rural
        1 => {
            if motorroad > 0 {
                motorroad
            } else {
                rural
            }
        }
        2 | 3 | 4 | 9 => match built_up {
            BUILT_UP_URBAN => urban,
            BUILT_UP_RURAL => rural,
            _ => 0, // unknown → legacy table (/gg Codex: absent raster must not mean "rural")
        },
        _ => 0, // 5-8 local + 10-12 links: legacy table by design
    };
    (v > 0).then_some(v as f64)
}

// ── Per-segment SquareCountryCity (plan M4, 2026-07-28) ─────────────────────────────────
//
// The M3 bake (`pipeline/enrich-roads-country.ts`) stamps three all-or-none
// columns into every `roads.arrow` / `railways.arrow`: `country_iso` (UInt16,
// two ASCII bytes packed `iso0 | iso1<<8`, 0 = `\0\0`), `city_id` (UInt16),
// `continent` (UInt8, mirroring `square_country_city.rs::Continent`). When a row carries
// them, its OWN country/city/continent drives the defaults cascade; when the
// `country_iso` COLUMN is absent (pre-bake data) the caller falls back to
// today's receiver/region SquareCountryCity. A PRESENT 0 bakes `SquareCountryCity::UNKNOWN` → WORLD
// defaults with NO receiver fallback.

/// Decode one row's baked SquareCountryCity triplet. The `country_iso` column's PRESENCE
/// is the fallback switch (handled by callers); this only decodes a present
/// row value. Rail keeps an exact copy in `emission::railway` — the two live
/// in separate layer-codever buckets, so neither may import from the other.
pub fn baked_square_country_city(
    country_iso: u16,
    city_id: u16,
    continent: u8,
) -> SquareCountryCity {
    if country_iso == 0 {
        return SquareCountryCity::UNKNOWN;
    }
    SquareCountryCity {
        continent: Continent::from_u8(continent),
        country_iso: country_iso.to_le_bytes(),
        city_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_country_city_for(
        iso: &[u8; 2],
        city: u16,
        continent: Continent,
    ) -> SquareCountryCity {
        SquareCountryCity {
            continent,
            country_iso: *iso,
            city_id: city,
        }
    }

    fn section_total(default: TrafficDefault) -> Aadt {
        match default {
            TrafficDefault::SectionBothDirections(section) => section,
            TrafficDefault::Carriageway(_) => panic!("hand-set arms are section totals"),
        }
    }

    #[test]
    fn main_classes_take_the_measured_carriageway_prior_by_direction_and_built_up() {
        let anywhere = square_country_city_for(b"DE", 0, Continent::Europe);
        let sum = |default| match default {
            TrafficDefault::Carriageway(v) => v.0 + v.1 + v.2 + v.3,
            TrafficDefault::SectionBothDirections(_) => panic!("classes 0-4 are per carriageway"),
        };
        let TrafficDefault::Carriageway(motorway) = resolve_traffic_default(0, anywhere, 3, true, BUILT_UP_URBAN) else {
            panic!("motorway prior is per carriageway");
        };
        let total = motorway.0 + motorway.1 + motorway.2 + motorway.3;
        assert!((total - 3.0 * MEASURED_CARRIAGEWAY_PRIORS[0][0][2].vehicles_per_lane).abs() < 1e-9);
        // WORLD_DEFAULT motorway proportions 72/8/19/1 %.
        assert!((motorway.0 / total - 0.72).abs() < 1e-12 && (motorway.2 / total - 0.19).abs() < 1e-12);
        // No lanes tag (0) or an implausible one: the median of untagged measured carriageways.
        let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} != {b}");
        close(sum(resolve_traffic_default(1, anywhere, 0, true, BUILT_UP_RURAL)), MEASURED_CARRIAGEWAY_PRIORS[1][0][1].untagged);
        close(sum(resolve_traffic_default(1, SquareCountryCity::UNKNOWN, 9, false, BUILT_UP_UNKNOWN)),
            MEASURED_CARRIAGEWAY_PRIORS[1][1][0].untagged);
        // Secondary and tertiary have no per-lane fit: the lanes tag does not scale them.
        close(sum(resolve_traffic_default(4, anywhere, 4, false, BUILT_UP_URBAN)), MEASURED_CARRIAGEWAY_PRIORS[4][1][2].untagged);
        for class in 5..=12 {
            assert_eq!(
                resolve_traffic_default(class, anywhere, 3, true, BUILT_UP_URBAN),
                TrafficDefault::SectionBothDirections(WORLD_DEFAULT[class as usize])
            );
        }
        assert_eq!(section_total(resolve_traffic_default(200, anywhere, 0, false, BUILT_UP_RURAL)), WORLD_DEFAULT[12]);
    }

    #[test]
    fn secondary_one_way_shares_the_two_way_section_instead_of_the_one_way_arm() {
        // Holdout counts on genuine one-way secondary streets (w3-priors, 2026-09-25):
        // the fitted one-way arm read +3.0 dB (n=13 strict-lone, GB), and 90% of
        // paired one-way secondary rows sit within 7 m of their pair (split-mapped
        // two-way streets that took the arm twice). One direction takes half the
        // two-way section in every place; the tertiary arm stays (holdout -1.0 dB).
        let anywhere = square_country_city_for(b"DE", 0, Continent::Europe);
        for built_up in [BUILT_UP_UNKNOWN, BUILT_UP_RURAL, BUILT_UP_URBAN] {
            let TrafficDefault::SectionBothDirections(section) =
                resolve_traffic_default(3, anywhere, 0, true, built_up) else {
                panic!("one-way secondary is a shared section total");
            };
            let total = section.0 + section.1 + section.2 + section.3;
            let expected = MEASURED_CARRIAGEWAY_PRIORS[3][1][usize::from(built_up)].untagged;
            assert!((total - expected).abs() < 1e-9);
        }
        assert!(matches!(resolve_traffic_default(3, anywhere, 0, false, BUILT_UP_URBAN),
            TrafficDefault::Carriageway(_)), "two-way secondary keeps its arm");
        assert!(matches!(resolve_traffic_default(4, anywhere, 0, true, BUILT_UP_URBAN),
            TrafficDefault::Carriageway(_)), "one-way tertiary keeps its arm");
    }

    #[test]
    fn bangkok_motorway_is_90k_with_heavy_moto_share() {
        let a = square_country_city_for(b"TH", CITY_BANGKOK, Continent::Asia);
        let (l, m, h, x) = section_total(resolve_traffic_default(0, a, 3, true, BUILT_UP_URBAN));
        let total = l + m + h + x;
        assert!(
            (total - 90000.0).abs() < 1.0,
            "BKK motorway total should be 90k, got {}",
            total
        );
        // Bangkok split is 60/8/7/25 — motorcycles 25 %.
        assert!(
            x / total > 0.20 && x / total < 0.30,
            "moto share should be ~25% in BKK, got {}",
            x / total
        );
    }

    #[test]
    fn unknown_city_falls_through_to_country() {
        // A Thai square with no metro match (city_id=0) gets the TH country default.
        let thailand = square_country_city_for(b"TH", 0, Continent::Asia);
        assert_eq!(
            section_total(resolve_traffic_default(3, thailand, 0, false, BUILT_UP_RURAL)),
            (3720.0, 600.0, 780.0, 900.0)
        );
    }

    #[test]
    fn speed_default_gb_matrix() {
        // GB legal: urban 48 (30 mph), rural 97 (60 mph), motorway 113 (70 mph).
        let gb = square_country_city_for(b"GB", 0, Continent::Europe);
        assert_eq!(resolve_speed_default(4, gb, BUILT_UP_RURAL), Some(97.0));
        assert_eq!(resolve_speed_default(4, gb, BUILT_UP_URBAN), Some(48.0));
        // unknown built-up → None → caller's legacy table (never guess rural)
        assert_eq!(resolve_speed_default(4, gb, BUILT_UP_UNKNOWN), None);
        assert_eq!(resolve_speed_default(0, gb, BUILT_UP_UNKNOWN), Some(113.0));
        // GB defines no motorroad → trunk falls to rural
        assert_eq!(resolve_speed_default(1, gb, BUILT_UP_UNKNOWN), Some(97.0));
    }

    #[test]
    fn speed_default_scope_and_fallbacks() {
        let cz = square_country_city_for(b"CZ", 0, Continent::Europe);
        // residential/living/service/track + links stay on the legacy table by design
        for class in [5u8, 6, 7, 8, 10, 11, 12] {
            assert_eq!(resolve_speed_default(class, cz, BUILT_UP_URBAN), None);
        }
        assert_eq!(resolve_speed_default(0, cz, BUILT_UP_UNKNOWN), Some(130.0));
        assert_eq!(resolve_speed_default(1, cz, BUILT_UP_UNKNOWN), Some(110.0)); // CZ motorroad
        assert_eq!(resolve_speed_default(3, cz, BUILT_UP_RURAL), Some(90.0));
        // country without a table row → None (legacy behavior everywhere)
        let zz = square_country_city_for(b"ZZ", 0, Continent::Unknown);
        assert_eq!(resolve_speed_default(4, zz, BUILT_UP_RURAL), None);
    }

    #[test]
    fn baked_square_country_city_decodes_m3_triplet() {
        // Schema authority: pipeline/enrich-roads-country.ts — `iso0 | iso1<<8`,
        // continent ids mirror square_country_city.rs::Continent (4 = Asia).
        let th = baked_square_country_city(u16::from_le_bytes(*b"TH"), 0, 4);
        assert_eq!(th.country_code(), Some("TH"));
        assert_eq!(th.continent, Continent::Asia);
        assert_eq!(th.city_id, 0);
        // A present 0 (`\0\0`) is SquareCountryCity::UNKNOWN — WORLD defaults, NO receiver
        // fallback (column absence is the switch).
        assert_eq!(
            baked_square_country_city(0, 0, 0),
            SquareCountryCity::UNKNOWN
        );
        // City id rides along (Bangkok metro, gated by the resolved country).
        let bkk = baked_square_country_city(u16::from_le_bytes(*b"TH"), CITY_BANGKOK, 4);
        assert_eq!(bkk.city_id, CITY_BANGKOK);
    }
}

//! Road traffic priors for rows without a count, and legal speed defaults.
//!
//! ```text
//!   city arm (tier-1 metro)        São Paulo, Rio, Bangkok: hand-set section totals
//!   │   ↓
//!   country arm                    TH, BR rural: hand-set section totals
//!   │   ↓
//!   classes 0-2                    measured world rate per lane, per carriageway
//!   classes 3-12                   WORLD_DEFAULT section totals
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

/// Both-directions section totals. Classes 3-12 use them as the prior;
/// classes 0-2 contribute only their vehicle-class proportions to the
/// measured per-lane prior below.
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

// Re-export the dedicated module so existing callers (`CITY_SAO_PAULO` …)
// keep working unchanged. Edit the JSON, then run
// `node scripts/gen-city-consts-rs.mjs` to refresh.

pub use crate::city_consts_generated::*;

/// A hand-set or world class total describes the whole two-way section and
/// is shared between its carriageways by the producer; a measured per-lane
/// prior already describes the one stored carriageway and is never divided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrafficDefault {
    SectionBothDirections(Aadt),
    Carriageway(Aadt),
}

// Length-weighted medians of stored per-carriageway counts over all measured
// rows of release r260910 outside tunnels (motorway 174,178 km, trunk
// 218,042 km, primary 200,387 km), trained separately for one-way and two-way
// rows; leave-one-country-out MAE 3.2-3.7 dB, |bias| < 0.6 dB. Columns:
// vehicles per lane per day for a lanes tag of 1-6 (one-way, two-way), then
// the median whole count of rows without such a tag (one-way, two-way), which
// sit far below lanes x rate (trunk one-way 1,810 against 2 x 4,533).
// Lone one-way rows that still held a two-way total (18,246 km) are halved and
// ramp-sized mainline rows (3,729 km) dropped before taking the medians.
const MEASURED_CARRIAGEWAY_VEHICLES_PER_DAY: [[f64; 4]; 3] = [
    [6379.0, 3010.0, 5200.0, 6019.0], // 0 motorway
    [4533.0, 2594.0, 1810.0, 3045.0], // 1 trunk
    [4250.0, 2800.0, 5882.0, 3719.0], // 2 primary
];

/// The prior for a row without a count: city arm, country arm, then the
/// measured per-lane carriageway prior (classes 0-2) or the world class total.
/// `class` clamps to the WORLD_DEFAULT bounds. AUTHORITATIVE for the native
/// producer (`roads-finalize` allocation); runtime consumers read prepared
/// counts and never call this.
pub fn resolve_traffic_default(
    class: u8,
    square_country_city: SquareCountryCity,
    lanes: u8,
    one_way: bool,
) -> TrafficDefault {
    let hand_set = (square_country_city.city_id != 0)
        .then(|| city_default(square_country_city.city_id, class))
        .flatten()
        .or_else(|| country_default(&square_country_city.country_iso, class));
    if let Some(section) = hand_set {
        return TrafficDefault::SectionBothDirections(section);
    }
    let world = WORLD_DEFAULT[(class as usize).min(WORLD_DEFAULT.len() - 1)];
    let Some(measured) = MEASURED_CARRIAGEWAY_VEHICLES_PER_DAY.get(class as usize) else {
        return TrafficDefault::SectionBothDirections(world);
    };
    let direction = usize::from(!one_way);
    let total = if (1..=6).contains(&lanes) {
        f64::from(lanes) * measured[direction]
    } else {
        measured[2 + direction]
    };
    let scale = total / (world.0 + world.1 + world.2 + world.3);
    TrafficDefault::Carriageway((world.0 * scale, world.1 * scale, world.2 * scale, world.3 * scale))
}

// One arm per (city_id, class). Values reflect each metro's published or
// enricher-coded tier defaults. Missing classes fall through to country.

fn city_default(city_id: u16, class: u8) -> Option<Aadt> {
    match (city_id, class) {
        // ─── São Paulo + Rio — BR tier-1 (×2.0) split 70/10/15/5 ─────────
        // Source: pipeline/enrich-roads-br.ts CLASS_AADT × tierMultiplier(1)
        // × splitVehicles(tier=1).
        (CITY_SAO_PAULO, 0) | (CITY_RIO, 0) => Some((70000.0, 10000.0, 15000.0, 5000.0)), // 100k motorway
        (CITY_SAO_PAULO, 1) | (CITY_RIO, 1) => Some((35000.0, 5000.0, 7500.0, 2500.0)), // 50k trunk
        (CITY_SAO_PAULO, 2) | (CITY_RIO, 2) => Some((16800.0, 2400.0, 3600.0, 1200.0)), // 24k primary
        (CITY_SAO_PAULO, 3) | (CITY_RIO, 3) => Some((7000.0, 1000.0, 1500.0, 500.0)), // 10k secondary
        (CITY_SAO_PAULO, 4) | (CITY_RIO, 4) => Some((2800.0, 400.0, 600.0, 200.0)),   // 4k tertiary
        (CITY_SAO_PAULO, 5) | (CITY_RIO, 5) => Some((1400.0, 200.0, 300.0, 100.0)), // 2k residential

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

        // ─── Brazil rural (tier 0) — split 60/10/25/5 ────────────────────
        // Source: pipeline/enrich-roads-br.ts CLASS_AADT rural × splitVehicles(tier=0).
        (b"BR", 0) => Some((30000.0, 5000.0, 12500.0, 2500.0)), // 50k motorway
        (b"BR", 1) => Some((15000.0, 2500.0, 6250.0, 1250.0)),  // 25k trunk
        (b"BR", 2) => Some((7200.0, 1200.0, 3000.0, 600.0)),    // 12k primary
        (b"BR", 3) => Some((3000.0, 500.0, 1250.0, 250.0)),     // 5k secondary
        (b"BR", 4) => Some((1200.0, 200.0, 500.0, 100.0)),      // 2k tertiary
        (b"BR", 5) => Some((600.0, 100.0, 250.0, 50.0)),        // 1k residential
        (b"BR", 6) => Some((240.0, 40.0, 100.0, 20.0)),         // 400 living_street

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
    fn main_classes_take_the_measured_rate_per_lane_per_carriageway() {
        let anywhere = square_country_city_for(b"DE", 0, Continent::Europe);
        let TrafficDefault::Carriageway(motorway) = resolve_traffic_default(0, anywhere, 3, true) else {
            panic!("motorway prior is per carriageway");
        };
        let total = motorway.0 + motorway.1 + motorway.2 + motorway.3;
        assert!((total - 3.0 * 6379.0).abs() < 1e-9);
        // WORLD_DEFAULT motorway proportions 72/8/19/1 %.
        assert!((motorway.0 / total - 0.72).abs() < 1e-12 && (motorway.2 / total - 0.19).abs() < 1e-12);
        let sum = |default| match default {
            TrafficDefault::Carriageway(v) => v.0 + v.1 + v.2 + v.3,
            TrafficDefault::SectionBothDirections(_) => panic!("classes 0-2 are per carriageway"),
        };
        assert!((sum(resolve_traffic_default(2, anywhere, 2, false)) - 2.0 * 2800.0).abs() < 1e-9);
        // No lanes tag (0) or an implausible one: the median of untagged measured rows.
        assert!((sum(resolve_traffic_default(1, anywhere, 0, true)) - 1810.0).abs() < 1e-9);
        assert!((sum(resolve_traffic_default(1, SquareCountryCity::UNKNOWN, 9, false)) - 3045.0).abs() < 1e-9);
        for class in 3..=12 {
            assert_eq!(
                resolve_traffic_default(class, anywhere, 3, true),
                TrafficDefault::SectionBothDirections(WORLD_DEFAULT[class as usize])
            );
        }
        assert_eq!(section_total(resolve_traffic_default(200, anywhere, 0, false)), WORLD_DEFAULT[12]);
    }

    #[test]
    fn sao_paulo_tier1_motorway_is_100k() {
        let a = square_country_city_for(b"BR", CITY_SAO_PAULO, Continent::SouthAmerica);
        let (l, m, h, x) = section_total(resolve_traffic_default(0, a, 3, true));
        let total = l + m + h + x;
        assert!(
            (total - 100000.0).abs() < 1.0,
            "SP motorway total should be 100k, got {}",
            total
        );
    }

    #[test]
    fn bangkok_motorway_is_90k_with_heavy_moto_share() {
        let a = square_country_city_for(b"TH", CITY_BANGKOK, Continent::Asia);
        let (l, m, h, x) = section_total(resolve_traffic_default(0, a, 3, true));
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
        // Brazilian square with no metro match (city_id=0) gets BR country default.
        let a = square_country_city_for(b"BR", 0, Continent::SouthAmerica);
        assert_eq!(
            section_total(resolve_traffic_default(0, a, 3, true)),
            (30000.0, 5000.0, 12500.0, 2500.0)
        );
        let thailand = square_country_city_for(b"TH", 0, Continent::Asia);
        assert_eq!(
            section_total(resolve_traffic_default(3, thailand, 0, false)),
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

//! Hierarchical traffic-default cascade.
//!
//! For any H3R4 cell whose segment has no enriched AADT (i.e. no spatial
//! match against a national census + no service-tree heuristic), the engine
//! picks a default via this cascade:
//!
//! ```text
//!   city default (tier-1 metro override)   e.g. São Paulo, Bangkok, NYC, ...
//!   │   ↓ fallback
//!   MEASURED region default                country × class medians from national
//!   │   (region_defaults_generated.rs)     censuses (M6.3 — mechanism parked,
//!   │                                        table empty: the TH DRR band→class
//!   │                                        crosswalk proved invalid /gg M6)
//!   │   ↓ fallback
//!   country default                        e.g. TH/BR rural (hand-tuned), GDP-scaled
//!   │   ↓ fallback
//!   continent default                      e.g. SA/AF coarse averages
//!   │   ↓ fallback
//!   WORLD_DEFAULT                          EU-generic (same as pre-redesign)
//! ```
//!
//! `WORLD_DEFAULT` reproduces the legacy `normalize.rs::default_road_traffic`
//! table bit-for-bit so today's non-admin call sites see no behavior change.
//! The measured region arm is generated from census data only
//! (`scripts/gen-region-defaults-rs.mjs`) and SUPERSEDES a country's
//! hand-tuned arm once its class attribution is proven — the TH DRR attempt
//! was parked (/gg M6 Codex: 1xxx–5xxx sections are dominantly engine class
//! 4, not 3, so band medians biased class-3 roads ~4.6 dB low); it re-lands
//! with class attribution via exact-ref joins.
//! (M6); BR's hand-tuned arm stays until a BR per-section census is wired
//! into the pipeline (the DNIT-derived BR table is corridor-level).
//!
//! Data format: `Aadt = (light, medium, heavy, moto)`, all in vehicles/day
//! both-directions total. Vehicle-class split follows the country's typical
//! fleet composition (e.g. TH has ~25 % motorcycles in Bangkok, BR has
//! higher heavy-vehicle share on rural freight corridors).

use crate::admin::{Admin, Continent};

/// Vehicle-class AADT tuple: (light, medium, heavy, moto), veh/day
/// both-directions total.
pub type Aadt = (f64, f64, f64, f64);

// Exact copy of the pre-redesign `normalize.rs::default_road_traffic` table.
// Must match bit-for-bit so non-admin call sites (the legacy
// `default_road_traffic(class)` wrapper) see zero behavior change.

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

/// Returns the best-known default (light, medium, heavy, moto) AADT for a
/// segment with no spatial / ref / service-tree data. Cascades most-specific
/// → least-specific: city → country → continent → world. `class` clamps to
/// the WORLD_DEFAULT array bounds.
pub fn resolve_traffic_default(class: u8, admin: Admin) -> Aadt {
    if admin.city_id != 0 {
        if let Some(v) = city_default(admin.city_id, class) {
            return v;
        }
    }
    if let Some(v) = country_default(&admin.country_iso, class) {
        return v;
    }
    if let Some(v) = continent_default(admin.continent, class) {
        return v;
    }
    let idx = (class as usize).min(WORLD_DEFAULT.len() - 1);
    WORLD_DEFAULT[idx]
}

/// Materialise the per-class cascade once for a fixed admin so hot loops
/// can index by `road_class` instead of paying for a city/country/
/// continent lookup per segment. Build at the top of any code path that
/// processes many segments under a stable admin (e.g. a per-hex batch
/// in the batch road loader or a popup-time receiver
/// query in `source-reader`).
pub fn build_traffic_default_cache(admin: Admin) -> [Aadt; WORLD_DEFAULT.len()] {
    std::array::from_fn(|c| resolve_traffic_default(c as u8, admin))
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
        // Derivation: the pre-M6 TH rural arm totals × 1.5, split via
        // pipeline/enrich-roads-th.ts thaiClassSplit(isBangkok=true) (live,
        // still used by the DOH tiers there). Deliberately NOT replaced by
        // the M6.3 measured medians: the DRR census is rural roads — it has
        // no measured say inside the Bangkok metro.
        (CITY_BANGKOK, 0) => Some((54000.0, 7200.0, 6300.0, 22500.0)), // 90k motorway
        (CITY_BANGKOK, 1) => Some((27000.0, 3600.0, 3150.0, 11250.0)), // 45k trunk
        (CITY_BANGKOK, 2) => Some((13500.0, 1800.0, 1575.0, 5625.0)),  // 22.5k primary
        (CITY_BANGKOK, 3) => Some((5400.0, 720.0, 630.0, 2250.0)),     // 9k secondary
        (CITY_BANGKOK, 4) => Some((2250.0, 300.0, 263.0, 937.0)),      // 3.75k tertiary
        (CITY_BANGKOK, 5) => Some((1080.0, 144.0, 126.0, 450.0)),      // 1.8k residential

        _ => None,
    }
}

// Three-layer policy:
//   (a) MEASURED region arm from `region_defaults_generated::region_default`
//       — country × class medians computed from national censuses (M6.3).
//       PARKED: the TH DRR band→class crosswalk proved invalid (/gg M6
//       Codex), so the generated table is empty until class attribution via
//       exact-ref joins lands; the arm then supersedes that country's
//       hand-tuned arm below.
//   (b) Hand-tuned arm for a country whose enricher publishes per-class
//       AADT but has NO per-section measured census wired in (BR rural —
//       DNIT-derived corridor table; TH's arm was deleted at M6).
//   (c) Data-driven fallback from `country_defaults_generated::country_scale`
//       — Wikipedia vehicles_per_km ratio vs DE (wiki-sourced, ~160
//       countries), or World Bank pop_density fallback (~80 more).
//       Clamped to [0.7, 1.3], applied to motorway/trunk/primary/link
//       classes. Local roads (class 3-9) stay at WORLD regardless.
//
// Refresh (c):
//   node scripts/fetch-wb-country-data.mjs
//   node scripts/fetch-wiki-roads-fleet.mjs
//   node scripts/gen-country-defaults-rs.mjs
// Refresh (a):
//   node scripts/gen-region-defaults-rs.mjs
// Empirical basis: direct fleet / paved_km is the AADT denominator
// itself — Wikipedia gives both signals for ~160 countries so we sidestep
// the need for a proxy. For the remaining ~80 we still use pop_density
// (weak but positive correlation with motorway fleet-per-km).

/// Classes whose default AADT scales with country traffic intensity
/// (motorway, trunk, primary, and their ramp classes). Local roads (3-9)
/// stay at WORLD — informal transport, cycling, pedestrian share
/// decorrelate local AADT from motorway AADT.
///
/// Encoded as a bitmask (1 << class) for O(1) set membership: each
/// `country_default` call shaves ~6 cmp branches off the hot fallback
/// path that fires for every `source_id == 0` segment.
const GDP_SCALED_MASK: u16 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 10) | (1 << 11) | (1 << 12);

#[inline]
fn is_traffic_scaled(class: u8) -> bool {
    class < 16 && (GDP_SCALED_MASK >> class) & 1 != 0
}

fn country_default(iso: &[u8; 2], class: u8) -> Option<Aadt> {
    // (a) MEASURED region arm — census medians, country × class (M6.3).
    // PARKED: the TH DRR number-band → engine-class crosswalk proved invalid
    // (/gg M6 Codex: band-median defaults biased engine class 3 ~4.6 dB low
    // because 1xxx–5xxx sections are dominantly class 4). The mechanism and
    // generated table stay for censuses with proper class attribution
    // (exact-ref joins); TH's entries are removed until then.
    if let Some(v) = crate::region_defaults_generated::region_default(iso, class) {
        return Some(v);
    }

    // (b) Hand-tuned arm — only where no per-section measured census is
    // wired into the pipeline. TH's arm was deleted at M6 only for classes
    // the DRR census measures (3 secondary, 4 tertiary); the DOH-backed
    // classes stay until a DOH adapter lands (/gg M6: deleting them dropped
    // TH motorway/trunk/primary/residential −1.2 to −5 dB on no measured
    // evidence). BR stays until a BR per-section census exists in the
    // pipeline (the DNIT-derived BR table is corridor-level, not per-section
    // counts).
    match (iso, class) {
        // ─── Thailand rural — split 62/10/13/15 ───────────────────────────
        // Thai-tuned class totals × pipeline/enrich-roads-th.ts
        // thaiClassSplit(isBangkok=false). Rural baseline, Bangkok hex
        // overrides via CITY_BANGKOK above. The M6.3 measured DRR arm is
        // PARKED (see country_default): the DRR number-band → engine-class
        // crosswalk proved invalid (/gg M6 Codex: 1xxx–5xxx sections are
        // dominantly engine class 4, not 3 — band-median defaults biased
        // class-3 roads ~4.6 dB low). Re-land only after class attribution
        // via exact-ref joins.
        (b"TH", 0) => return Some((37200.0, 6000.0, 7800.0, 9000.0)), // 60k motorway
        (b"TH", 1) => return Some((18600.0, 3000.0, 3900.0, 4500.0)), // 30k trunk
        (b"TH", 2) => return Some((9300.0, 1500.0, 1950.0, 2250.0)),  // 15k primary
        (b"TH", 3) => return Some((3720.0, 600.0, 780.0, 900.0)),     // 6k secondary
        (b"TH", 4) => return Some((1550.0, 250.0, 325.0, 375.0)),     // 2.5k tertiary
        (b"TH", 5) => return Some((744.0, 120.0, 156.0, 180.0)),      // 1.2k residential

        // ─── Brazil rural (tier 0) — split 60/10/25/5 ────────────────────
        // Source: pipeline/enrich-roads-br.ts CLASS_AADT rural × splitVehicles(tier=0).
        (b"BR", 0) => return Some((30000.0, 5000.0, 12500.0, 2500.0)), // 50k motorway
        (b"BR", 1) => return Some((15000.0, 2500.0, 6250.0, 1250.0)),  // 25k trunk
        (b"BR", 2) => return Some((7200.0, 1200.0, 3000.0, 600.0)),    // 12k primary
        (b"BR", 3) => return Some((3000.0, 500.0, 1250.0, 250.0)),     // 5k secondary
        (b"BR", 4) => return Some((1200.0, 200.0, 500.0, 100.0)),      // 2k tertiary
        (b"BR", 5) => return Some((600.0, 100.0, 250.0, 50.0)),        // 1k residential
        (b"BR", 6) => return Some((240.0, 40.0, 100.0, 20.0)),         // 400 living_street

        _ => {}
    }

    // (c) WB GDP-scaled fallback from WORLD_DEFAULT.
    if !is_traffic_scaled(class) {
        // Local roads: don't scale with GDP.
        return None;
    }
    let scale = crate::country_defaults_generated::country_scale(iso)?;
    let class_idx = (class as usize).min(WORLD_DEFAULT.len() - 1);
    let base = WORLD_DEFAULT[class_idx];
    Some((
        base.0 * scale,
        base.1 * scale,
        base.2 * scale,
        base.3 * scale,
    ))
}

// Sparse — only where a continent-wide skew is known to diverge from the EU
// baseline (e.g. Africa has a sparser motorway network, so the class-0
// default is lower).

fn continent_default(continent: Continent, class: u8) -> Option<Aadt> {
    // Reached only when country arm returned None (usually because the
    // ISO is not in the WB dataset or the class isn't GDP-scaled).
    // We still apply continent scaling for GDP-scaled classes because
    // those are the ones where the signal matters — local roads fall
    // through to WORLD_DEFAULT.
    if !is_traffic_scaled(class) {
        return None;
    }
    let scale = crate::country_defaults_generated::continent_scale(continent)?;
    let class_idx = (class as usize).min(WORLD_DEFAULT.len() - 1);
    let base = WORLD_DEFAULT[class_idx];
    Some((
        base.0 * scale,
        base.1 * scale,
        base.2 * scale,
        base.3 * scale,
    ))
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
/// legacy `default_road_speed` world table. The admin is the SEGMENT's own when the
/// M3 baked columns are present (see [`baked_admin`]); on pre-bake data it is the
/// receiver's/region's — the accepted border approximation the AADT cascade above
/// also makes.
pub fn resolve_speed_default(class: u8, admin: Admin, built_up: u8) -> Option<f64> {
    let row = COUNTRY_SPEEDS
        .binary_search_by(|(iso, _)| iso[..].cmp(&admin.country_iso[..]))
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

// ── Per-segment admin (plan M4, 2026-07-28) ─────────────────────────────────
//
// The M3 bake (`pipeline/enrich-roads-country.ts`) stamps three all-or-none
// columns into every `roads.arrow` / `railways.arrow`: `country_iso` (UInt16,
// two ASCII bytes packed `iso0 | iso1<<8`, 0 = `\0\0`), `city_id` (UInt16),
// `continent` (UInt8, mirroring `admin.rs::Continent`). When a row carries
// them, its OWN country/city/continent drives the defaults cascade; when the
// `country_iso` COLUMN is absent (pre-bake data) the caller falls back to
// today's receiver/region admin. A PRESENT 0 bakes `Admin::UNKNOWN` → WORLD
// defaults with NO receiver fallback.

/// Decode one row's baked admin triplet. The `country_iso` column's PRESENCE
/// is the fallback switch (handled by callers); this only decodes a present
/// row value. Rail keeps an exact copy in `emission::railway` — the two live
/// in separate layer-codever buckets, so neither may import from the other.
pub fn baked_admin(country_iso: u16, city_id: u16, continent: u8) -> Admin {
    if country_iso == 0 {
        return Admin::UNKNOWN;
    }
    Admin {
        continent: Continent::from_u8(continent),
        country_iso: country_iso.to_le_bytes(),
        city_id,
    }
}

thread_local! {
    /// Per-row road admins for the popup kernel, aligned by index with the
    /// `&[RoadSegment]` slice handed to `compute_at_point*`. `RoadSegment`
    /// (`types/inputs.rs`) is codever-SHARED and cannot grow a field, so the
    /// admins ride this thread-local: source-reader installs them right
    /// before the compute call and clears them right after; every other
    /// caller (parity bins, tests) leaves the channel unset and gets today's
    /// receiver-admin behaviour bit-for-bit. `None` entries mark rows whose
    /// batch carried no baked columns (receiver fallback); `Some(Admin::
    /// UNKNOWN)` is a baked `\0\0` — WORLD defaults, no fallback.
    static ROAD_ROW_ADMINS: std::cell::RefCell<Option<Vec<Option<Admin>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install (`Some`) or clear (`None`) the per-row road-admin channel for the
/// next `compute_roads` call on THIS thread. Popup-only — see above.
pub fn set_road_row_admins(admins: Option<Vec<Option<Admin>>>) {
    ROAD_ROW_ADMINS.with(|c| *c.borrow_mut() = admins);
}

/// Row `i`'s baked admin, or `None` for the receiver-admin fallback. Also
/// `None` when the channel is unset or its length disagrees with `len`
/// (defensive: a mis-aligned channel must not mis-assign countries — the
/// tolerant rollout falls back, never guesses).
pub(crate) fn road_row_admin(i: usize, len: usize) -> Option<Admin> {
    ROAD_ROW_ADMINS.with(|c| {
        let guard = c.borrow();
        let v = guard.as_ref()?;
        if v.len() != len {
            return None;
        }
        v[i]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin_for(iso: &[u8; 2], city: u16, continent: Continent) -> Admin {
        Admin {
            continent,
            country_iso: *iso,
            city_id: city,
        }
    }

    #[test]
    fn world_default_matches_legacy_motorway() {
        // Baseline check: cascade with Admin::UNKNOWN = WORLD_DEFAULT.
        assert_eq!(resolve_traffic_default(0, Admin::UNKNOWN), WORLD_DEFAULT[0]);
    }

    #[test]
    fn us_scales_close_to_world_via_wiki_vpk() {
        // US vehicles_per_km ≈ 60.2 (Wikipedia 2024) ≈ DE's 63.5 → scale ≈ 0.98
        // → motorway ≈ 29k. (I.3 refined, wiki-sourced.)
        let a = admin_for(b"US", 0, Continent::NorthAmerica);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 28_000.0 && total < 30_000.0,
            "US motorway ≈ 29k, got {}",
            total
        );
    }

    #[test]
    fn brazil_rural_motorway_is_50k() {
        let a = admin_for(b"BR", 0, Continent::SouthAmerica);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            (total - 50000.0).abs() < 1.0,
            "BR rural motorway total should be 50k, got {}",
            total
        );
    }

    #[test]
    fn sao_paulo_tier1_motorway_is_100k() {
        let a = admin_for(b"BR", CITY_SAO_PAULO, Continent::SouthAmerica);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            (total - 100000.0).abs() < 1.0,
            "SP motorway total should be 100k, got {}",
            total
        );
    }

    #[test]
    fn bangkok_motorway_is_90k_with_heavy_moto_share() {
        let a = admin_for(b"TH", CITY_BANGKOK, Continent::Asia);
        let (l, m, h, x) = resolve_traffic_default(0, a);
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
    fn thailand_classes_use_the_hand_tuned_arm_while_the_measured_arm_is_parked() {
        // M6.3 follow-up (/gg M6 Codex): the DRR band→class crosswalk proved
        // invalid (1xxx–5xxx sections are dominantly engine class 4, so band
        // medians biased class-3 roads low), so the measured arm is parked
        // (REGION_DEFAULTS is empty) and TH falls back to the hand-tuned
        // rural arm ×62/10/13/15 for every class until class attribution
        // lands via exact-ref joins.
        let a = admin_for(b"TH", 0, Continent::Asia);
        assert_eq!(resolve_traffic_default(3, a), (3720.0, 600.0, 780.0, 900.0));
        assert_eq!(resolve_traffic_default(4, a), (1550.0, 250.0, 325.0, 375.0));
        assert_eq!(
            resolve_traffic_default(0, a),
            (37200.0, 6000.0, 7800.0, 9000.0)
        );
        assert_eq!(resolve_traffic_default(5, a), (744.0, 120.0, 156.0, 180.0));
    }

    #[test]
    fn city_overrides_country() {
        // If admin.city_id matches, city wins over country default.
        let sp = admin_for(b"BR", CITY_SAO_PAULO, Continent::SouthAmerica);
        let br_rural = admin_for(b"BR", 0, Continent::SouthAmerica);
        let sp_motorway = resolve_traffic_default(0, sp).0;
        let br_motorway = resolve_traffic_default(0, br_rural).0;
        assert!(sp_motorway > br_motorway, "SP tier-1 > BR rural");
    }

    #[test]
    fn unknown_city_falls_through_to_country() {
        // Brazilian hex with no metro match (city_id=0) gets BR country default.
        let a = admin_for(b"BR", 0, Continent::SouthAmerica);
        assert_eq!(
            resolve_traffic_default(0, a),
            (30000.0, 5000.0, 12500.0, 2500.0)
        );
    }

    #[test]
    fn dz_algeria_scales_from_wiki_vpk() {
        // Algeria vehicles_per_km ≈ 66.1 (Wikipedia) ≈ DE → scale ≈ 1.02
        // → motorway ≈ 30.5k. (I.3 refined, wiki-sourced — replaces
        // the old density-only heuristic which under-ranked Algeria.)
        let a = admin_for(b"DZ", 0, Continent::Africa);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 29_000.0 && total < 32_000.0,
            "DZ motorway ≈ 30.5k, got {}",
            total
        );
    }

    #[test]
    fn class_out_of_range_clamps() {
        // Classes ≥ 13 clamp to primary_link (class 12) as deterministic fallback.
        let v = resolve_traffic_default(200, Admin::UNKNOWN);
        assert_eq!(v, WORLD_DEFAULT[12]);
    }

    // ── Wiki-vpk country scaling tests (plan v5 §I.3 refined) ───────────
    #[test]
    fn de_reference_country_at_world_default() {
        // DE vehicles_per_km = 63.5 = reference → scale = 1.0 → motorway ≈ 30k.
        // (I.3 refined: DE is the calibration reference for wiki scale.)
        let a = admin_for(b"DE", 0, Continent::Europe);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            (total - 30_000.0).abs() < 100.0,
            "DE motorway ≈ 30k, got {}",
            total
        );
    }

    #[test]
    fn sg_hits_upper_clamp() {
        // SG vehicles_per_km ≈ 285 (Wikipedia) → tanh hits 1.3 clamp
        // → motorway ≈ 39k. City-state with short dense network.
        let a = admin_for(b"SG", 0, Continent::Asia);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 38_000.0 && total < 40_000.0,
            "SG motorway ≈ 39k, got {}",
            total
        );
    }

    #[test]
    fn no_norway_scales_down_via_wiki_vpk() {
        // NO vehicles_per_km ≈ 35.5 (Wikipedia) → log2(35.5/63.5) ≈ -0.84
        // → scale ≈ 0.79 → motorway ≈ 24k. Sparse network, moderate fleet.
        let a = admin_for(b"NO", 0, Continent::Europe);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 22_000.0 && total < 25_000.0,
            "NO motorway ≈ 24k, got {}",
            total
        );
    }

    #[test]
    fn et_hits_lower_clamp_via_wiki_vpk() {
        // ET (Ethiopia) vehicles_per_km ≈ 10 (Wikipedia) → hits 0.7 clamp
        // → motorway ≈ 21k. One of the lowest-motorization countries.
        let a = admin_for(b"ET", 0, Continent::Africa);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 20_000.0 && total < 22_000.0,
            "ET motorway ≈ 21k, got {}",
            total
        );
    }

    #[test]
    fn ng_hits_upper_clamp_via_wiki_vpk() {
        // NG vehicles_per_km ≈ 225 (Wikipedia: 13.5M vehicles, 60k paved)
        // → scale 1.285 → motorway ≈ 38.5k.
        let a = admin_for(b"NG", 0, Continent::Africa);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 37_000.0 && total < 40_000.0,
            "NG motorway ≈ 38.5k, got {}",
            total
        );
    }

    #[test]
    fn density_fallback_still_active_for_missing_wiki() {
        // Kuwait is in wiki, Angola isn't (vehicles_per_km=null). Angola
        // should fall back to density-based scale from WB.
        // AO density ≈ 28.6/km² → scale ≈ 0.76 → motorway ≈ 23k.
        let a = admin_for(b"AO", 0, Continent::Africa);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 21_000.0 && total < 25_000.0,
            "AO motorway ≈ 23k (density fallback), got {}",
            total
        );
    }

    #[test]
    fn country_local_roads_are_not_scaled() {
        // Classes ≥ 3 never scale by GDP — they go straight to WORLD.
        for iso in [b"NG", b"IN", b"LU", b"DE"] {
            for class in [3u8, 4, 5, 6, 7, 8, 9] {
                let a = admin_for(iso, 0, Continent::Unknown);
                assert_eq!(
                    resolve_traffic_default(class, a),
                    WORLD_DEFAULT[class as usize],
                    "{:?} class={} should be WORLD",
                    std::str::from_utf8(iso).unwrap(),
                    class
                );
            }
        }
    }

    #[test]
    fn unknown_country_falls_through_to_continent_or_world() {
        // ZZ is an invalid ISO — the country_scale table returns None.
        // Continent::Unknown → cascade lands on WORLD.
        let a = admin_for(b"ZZ", 0, Continent::Unknown);
        assert_eq!(resolve_traffic_default(0, a), WORLD_DEFAULT[0]);
    }

    #[test]
    fn continent_arm_applies_scale() {
        // Unknown ISO in Africa continent → Africa pop-weighted mean scale.
        // Africa mean ≈ 1.057 (wiki+density blend) → motorway ≈ 31.7k.
        let a = admin_for(b"ZZ", 0, Continent::Africa);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        let total = l + m + h + x;
        assert!(
            total > 30_000.0 && total < 33_000.0,
            "Africa continent mtw ≈ 31.7k, got {}",
            total
        );
    }

    #[test]
    fn speed_default_gb_matrix() {
        // GB legal: urban 48 (30 mph), rural 97 (60 mph), motorway 113 (70 mph).
        let gb = admin_for(b"GB", 0, Continent::Europe);
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
        let cz = admin_for(b"CZ", 0, Continent::Europe);
        // residential/living/service/track + links stay on the legacy table by design
        for class in [5u8, 6, 7, 8, 10, 11, 12] {
            assert_eq!(resolve_speed_default(class, cz, BUILT_UP_URBAN), None);
        }
        assert_eq!(resolve_speed_default(0, cz, BUILT_UP_UNKNOWN), Some(130.0));
        assert_eq!(resolve_speed_default(1, cz, BUILT_UP_UNKNOWN), Some(110.0)); // CZ motorroad
        assert_eq!(resolve_speed_default(3, cz, BUILT_UP_RURAL), Some(90.0));
        // country without a table row → None (legacy behavior everywhere)
        let zz = admin_for(b"ZZ", 0, Continent::Unknown);
        assert_eq!(resolve_speed_default(4, zz, BUILT_UP_RURAL), None);
    }

    #[test]
    fn explicit_br_takes_priority_over_gdp_scale() {
        // BR is in WB dataset (would give ~15k motorway from sqrt(17k/69k) × 30k ≈ 15k),
        // but the explicit arm is 50k — that must win.
        let a = admin_for(b"BR", 0, Continent::SouthAmerica);
        let (l, m, h, x) = resolve_traffic_default(0, a);
        assert!((l + m + h + x - 50000.0).abs() < 1.0);
    }

    #[test]
    fn baked_admin_decodes_m3_triplet() {
        // Schema authority: pipeline/enrich-roads-country.ts — `iso0 | iso1<<8`,
        // continent ids mirror admin.rs::Continent (4 = Asia).
        let th = baked_admin(u16::from_le_bytes(*b"TH"), 0, 4);
        assert_eq!(th.country_code(), Some("TH"));
        assert_eq!(th.continent, Continent::Asia);
        assert_eq!(th.city_id, 0);
        // A present 0 (`\0\0`) is Admin::UNKNOWN — WORLD defaults, NO receiver
        // fallback (column absence is the switch).
        assert_eq!(baked_admin(0, 0, 0), Admin::UNKNOWN);
        // City id rides along (Bangkok metro, gated by the resolved country).
        let bkk = baked_admin(u16::from_le_bytes(*b"TH"), CITY_BANGKOK, 4);
        assert_eq!(bkk.city_id, CITY_BANGKOK);
    }

    #[test]
    fn road_row_admin_channel_alignment_and_fallback() {
        // Unset channel → every row falls back (receiver admin at the caller).
        assert_eq!(road_row_admin(0, 1), None);
        set_road_row_admins(Some(vec![None, Some(Admin::UNKNOWN)]));
        assert_eq!(road_row_admin(0, 2), None, "no baked columns → fallback");
        assert_eq!(
            road_row_admin(1, 2),
            Some(Admin::UNKNOWN),
            "baked \\0\\0 → UNKNOWN, no fallback"
        );
        // A mis-aligned channel must not mis-assign countries — fall back.
        assert_eq!(road_row_admin(0, 3), None, "length mismatch → fallback");
        set_road_row_admins(None);
        assert_eq!(road_row_admin(1, 2), None, "cleared channel → fallback");
    }
}

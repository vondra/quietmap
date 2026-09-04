//! CNOSSOS-EU Annex IV railway emission — calibrated simplification.
//!
//! Per-band: rolling + traction. Speed-dependent rolling.
//! L_roll(f) = A_rolling(f) + 30 × log₁₀(v / v_ref)
//! L_traction(f) = A_traction(f)  [constant]
//! L_total(f) = 10×log₁₀(10^(L_roll/10) + 10^(L_traction/10))
//!
//! Line source density (CNOSSOS Annex IV, NoiseModelling-compatible):
//!   L_W'/m = L_total + 10·log₁₀(Q / (T_h × 1000 × v))
//! where Q = trains in period, T_h = period hours, v = km/h.
//!
//! Prior revision used `10·log₁₀(Q_per_day)` and treated SRM II `a_r`
//! coefficients as sound-power levels. The coefficients peaked at 4 kHz —
//! a band with 22 dB/km atmospheric absorption — so rail emission was
//! systematically destroyed at range (the atmosphere ate the signal before
//! the receiver could). Coefficients below are entire-train A-weighted
//! L_W values peaked at 500-1000 Hz (physical rail spectrum per ISO 3095
//! / CNOSSOS), scaled so a typical mainline corridor matches EU END
//! reference levels in the 0-5 km range.

use crate::admin::Admin;
use crate::types::NUM_BANDS;

const B_ROLLING: f64 = 30.0;

/// END day/evening/night period lengths [h] (12 / 4 / 8). The ONE definition
/// of the period split — every rail period loop (popup `compute_railways`,
/// heatmap `NormalizedRail::period_emissions`, the reach solver
/// `free_field_lden_at`) iterates [`RailTimeDist::periods`] over these so the
/// share model can never fork into a second copy.
pub const RAIL_PERIOD_HOURS: [f64; 3] = [12.0, 4.0, 8.0];

/// Per-region, per-category day/evening/night traffic split for rail.
///
/// Replaces the flat 65/20/15 that was applied to passenger AND freight alike —
/// the cause of rail `L_night` always being exactly `Lden − 7.91 dB`.
/// `pax` and `frt` each sum to 1.0; freight shares
/// only ever differ from `pax` for [`RailType::Rail`] (other types carry no
/// freight, so the resolver hands them `frt = pax` — belt and suspenders).
#[derive(Debug, Clone, Copy)]
pub struct RailTimeDist {
    pub pax: [f64; 3],
    pub frt: [f64; 3],
}

impl RailTimeDist {
    /// `(pax_share, frt_share, period_hours)` per END period — the single
    /// iterator every rail period loop consumes. Keeping the zip here (not
    /// re-spelled at each call site) is what makes the popup kernel, the heatmap
    /// loader, and the reach solver share one split.
    #[inline]
    pub fn periods(&self) -> [(f64, f64, f64); 3] {
        [
            (self.pax[0], self.frt[0], RAIL_PERIOD_HOURS[0]),
            (self.pax[1], self.frt[1], RAIL_PERIOD_HOURS[1]),
            (self.pax[2], self.frt[2], RAIL_PERIOD_HOURS[2]),
        ]
    }
}

/// EU-derived freight night split: **measured-derived** from EP IPOL-TRAN
/// ET(2012)474533 Table 22 (Rheintalbahn 129 day-trains / 155 night-trains ⇒
/// 54.6 % at night). The 16 h END "day" block (06–18 day + 18–22 evening)
/// carries the 129 daytime trains, split 12:4 by hour ⇒ 129·12/16 = 96.75 day,
/// 129·4/16 = 32.25 evening; night = 155. Total 96.75+32.25+155 = 284 ⇒
/// **0.3407 / 0.1136 / 0.5458** (shipping the exact fractions, not the rounded
/// 0.33/0.13/0.54). Corroboration: EBA Lärm-Monitoring
/// Jahresbericht 2023 (night Lm freight-dominated at ~all 19 stations); UBA.
const EU_FREIGHT: [f64; 3] = [96.75 / 284.0, 32.25 / 284.0, 155.0 / 284.0];

/// EU heavy-rail passenger split — `derived`: service span ~05–24 h ⇒ ~1.5–2 h
/// inside the 23–07 night window; EBA station night counts are passenger-minor.
const EU_PAX: [f64; 3] = [0.70, 0.20, 0.10];

/// Urban tram / light-rail / narrow-gauge / funicular passenger split —
/// `derived`: service ends ~00:30, starts ~04:30, so night is small. Freight
/// never applies to these types.
const TRAM_PAX: [f64; 3] = [0.70, 0.25, 0.05];

/// Non-EU freight (US Class I, RU, CN…) — `derived/uniform`: continuous 24/7
/// operation, no published time-of-day ⇒ flat 12/4/8 h split = 0.50 / 0.1667 /
/// 0.3333.
const WORLD_FREIGHT: [f64; 3] = [12.0 / 24.0, 4.0 / 24.0, 8.0 / 24.0];

/// Non-EU passenger — `derived`, same reasoning as [`EU_PAX`].
const WORLD_PAX: [f64; 3] = [0.70, 0.20, 0.10];

const TD_EU_RAIL: RailTimeDist = RailTimeDist {
    pax: EU_PAX,
    frt: EU_FREIGHT,
};
const TD_EU_TRAM: RailTimeDist = RailTimeDist {
    pax: TRAM_PAX,
    frt: TRAM_PAX,
};
const TD_WORLD_RAIL: RailTimeDist = RailTimeDist {
    pax: WORLD_PAX,
    frt: WORLD_FREIGHT,
};
const TD_WORLD_TRAM: RailTimeDist = RailTimeDist {
    pax: TRAM_PAX,
    frt: TRAM_PAX,
};

/// ISO-3166 alpha-2 whitelist for the EU-derived freight table: EU27 plus CH,
/// NO, UK. Keyed on the country code, NOT [`crate::admin::Continent::Europe`] —
/// that label is *geographic* Europe (it includes RU-west / UA / BY), and the
/// EP/EBA freight curve is only sourced for the central/western EU corridor
/// network. Geographic-Europe countries outside this list fall
/// through to the world/uniform table.
const EU_ISO_WHITELIST: [&[u8; 2]; 30] = [
    b"AT", b"BE", b"BG", b"HR", b"CY", b"CZ", b"DK", b"EE", b"FI", b"FR", b"DE", b"GR", b"HU",
    b"IE", b"IT", b"LV", b"LT", b"LU", b"MT", b"NL", b"PL", b"PT", b"RO", b"SK", b"SI", b"ES",
    b"SE", // EU27
    b"CH", b"NO", b"GB", // EFTA-adjacent + UK on the same network
];

#[inline]
fn is_eu_rail_region(admin: Admin) -> bool {
    EU_ISO_WHITELIST.contains(&&admin.country_iso)
}

/// Resolve the day/evening/night split for a rail segment from its admin region
/// and vehicle type. Trams / light-rail / narrow-gauge / funicular always take
/// the urban passenger curve (no freight). [`RailType::Rail`] takes the EU vs
/// world freight+passenger table on the [`EU_ISO_WHITELIST`]. `Admin::UNKNOWN`
/// (oceanic / pre-build z9 squares / tests) is deterministically non-EU.
///
/// Structured for per-country overrides (match `admin.country_code()` first,
/// then the EU/world fork), but only the cited rows ship today: refining
/// DE/CH/NL from EBA Lärmkartierung / BAV Emissionsplan / ProRail geluidregister
/// per-section counts is the R2 follow-up (those feeds fix counts AND shares).
pub fn rail_time_dist(admin: Admin, rail_type: RailType) -> &'static RailTimeDist {
    let eu = is_eu_rail_region(admin);
    match rail_type {
        RailType::Rail => {
            if eu {
                &TD_EU_RAIL
            } else {
                &TD_WORLD_RAIL
            }
        }
        _ => {
            if eu {
                &TD_EU_TRAM
            } else {
                &TD_WORLD_TRAM
            }
        }
    }
}

// Per-segment admin
//
// The M3 bake (`pipeline/enrich-roads-country.ts`) stamps three all-or-none
// columns into every `railways.arrow`: `country_iso` (UInt16, two ASCII bytes
// packed `iso0 | iso1<<8`, 0 = `\0\0`), `city_id` (UInt16), `continent`
// (UInt8, mirroring `admin.rs::Continent`). When a row carries them, its OWN
// ISO drives the EU/world split (and reach); when the `country_iso` COLUMN is
// absent (pre-bake data) the caller falls back to today's receiver/region
// admin. A PRESENT 0 bakes `Admin::UNKNOWN` → the world split with NO
// receiver fallback.

/// Decode one row's baked admin triplet — exact copy of
/// `crate::defaults::baked_admin`. The two live in separate layer-codever
/// buckets (road vs rail), so neither may import from the other.
pub fn baked_admin(country_iso: u16, city_id: u16, continent: u8) -> Admin {
    if country_iso == 0 {
        return Admin::UNKNOWN;
    }
    Admin {
        continent: crate::admin::Continent::from_u8(continent),
        country_iso: country_iso.to_le_bytes(),
        city_id,
    }
}

thread_local! {
    /// Per-row rail admins for the popup kernel, aligned by index with the
    /// `&[RailSegment]` slice handed to `compute_at_point*`. `RailSegment`
    /// (`types/inputs.rs`) is codever-SHARED and cannot grow a field, so the
    /// admins ride this thread-local: source-reader installs them right
    /// before the compute call and clears them right after; every other
    /// caller (parity bins, tests) leaves the channel unset and gets today's
    /// receiver-admin behaviour bit-for-bit. Entry semantics mirror the road
    /// channel (`defaults::ROAD_ROW_ADMINS`).
    static RAIL_ROW_ADMINS: std::cell::RefCell<Option<Vec<Option<Admin>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install (`Some`) or clear (`None`) the per-row rail-admin channel for the
/// next `compute_railways` call on THIS thread. Popup-only — see above.
pub fn set_rail_row_admins(admins: Option<Vec<Option<Admin>>>) {
    RAIL_ROW_ADMINS.with(|c| *c.borrow_mut() = admins);
}

/// Row `i`'s baked admin, or `None` for the receiver-admin fallback. Also
/// `None` when the channel is unset or its length disagrees with `len`
/// (defensive: a mis-aligned channel must not mis-assign countries — the
/// tolerant rollout falls back, never guesses).
pub(crate) fn rail_row_admin(i: usize, len: usize) -> Option<Admin> {
    RAIL_ROW_ADMINS.with(|c| {
        let guard = c.borrow();
        let v = guard.as_ref()?;
        if v.len() != len {
            return None;
        }
        v[i]
    })
}

struct RailVehicleCoeffs {
    a_rolling: [f64; NUM_BANDS],
    a_traction: [f64; NUM_BANDS],
    v_ref: f64,
    v_max: f64,
}

const FREIGHT: RailVehicleCoeffs = RailVehicleCoeffs {
    a_rolling: [110.0, 118.0, 126.0, 130.0, 131.0, 128.0, 120.0, 110.0],
    a_traction: [115.0, 113.0, 110.0, 105.0, 100.0, 95.0, 90.0, 85.0],
    v_ref: 80.0,
    v_max: 120.0,
};

const PASSENGER: RailVehicleCoeffs = RailVehicleCoeffs {
    a_rolling: [105.0, 112.0, 118.0, 122.0, 125.0, 122.0, 115.0, 105.0],
    a_traction: [100.0, 98.0, 95.0, 92.0, 88.0, 84.0, 78.0, 70.0],
    v_ref: 100.0,
    // 300 km/h high-speed: rolling scales via 30·log10(v/v_ref) — not a
    // dedicated aerodynamic model, but avoids the old silent clamp at 200.
    v_max: 300.0,
};

const TRAM: RailVehicleCoeffs = RailVehicleCoeffs {
    a_rolling: [98.0, 105.0, 110.0, 114.0, 117.0, 114.0, 107.0, 97.0],
    a_traction: [105.0, 103.0, 100.0, 97.0, 93.0, 89.0, 83.0, 75.0],
    v_ref: 50.0,
    v_max: 70.0,
};

const LIGHT_RAIL: RailVehicleCoeffs = RailVehicleCoeffs {
    a_rolling: [100.0, 107.0, 112.0, 116.0, 119.0, 116.0, 109.0, 99.0],
    a_traction: [108.0, 106.0, 103.0, 100.0, 96.0, 92.0, 86.0, 78.0],
    v_ref: 80.0,
    v_max: 120.0,
};

/// Rail vehicle type (matches rail_type field in Arrow IPC).
#[derive(Debug, Clone, Copy)]
pub enum RailType {
    Rail,        // 0 — mixed passenger/freight
    Tram,        // 1
    LightRail,   // 2
    NarrowGauge, // 3
    Funicular,   // 4
}

impl RailType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Tram,
            2 => Self::LightRail,
            3 => Self::NarrowGauge,
            4 => Self::Funicular,
            _ => Self::Rail,
        }
    }
}

/// Compute emission bands for one vehicle type at given speed [dB/vehicle].
fn vehicle_emission(coeffs: &RailVehicleCoeffs, speed_kmh: f64) -> [f64; NUM_BANDS] {
    let v = speed_kmh.clamp(20.0, coeffs.v_max);
    let speed_corr = B_ROLLING * (v / coeffs.v_ref).log10();

    let mut bands = [0.0f64; NUM_BANDS];
    let c = std::f64::consts::LN_10 * 0.1;
    // Per-band emission: `i` indexes the parallel rolling/traction coefficient
    // arrays and writes `bands[i]`; index loop kept, exp sum order is byte-parity.
    #[allow(clippy::needless_range_loop)]
    for i in 0..NUM_BANDS {
        let l_roll = coeffs.a_rolling[i] + speed_corr;
        let l_tract = coeffs.a_traction[i];
        bands[i] = 10.0 * ((l_roll * c).exp() + (l_tract * c).exp()).log10();
    }
    bands
}

/// Compute railway line source emission per meter [dB/m] as Leq over `period_hours`.
///
/// CNOSSOS Annex IV density: `L_W/m = L_W_per_train + 10·log₁₀(Q / (T × 1000 × v))`
/// where Q = trains in the period, T = period hours (12 day / 4 evening / 8 night),
/// v = km/h. Callers pass the per-period train subset and the period length.
pub fn railway_emission(
    rail_type: RailType,
    speed_kmh: f64,
    trains_passenger: f64,
    trains_freight: f64,
    period_hours: f64,
) -> [f64; NUM_BANDS] {
    let v = speed_kmh.max(20.0);
    let flow_denom = (period_hours.max(0.1) * 1000.0 * v).max(1.0);
    let mut total_energy = [0.0f64; NUM_BANDS];

    if trains_passenger > 0.0 {
        let coeffs = match rail_type {
            RailType::Tram => &TRAM,
            RailType::LightRail | RailType::NarrowGauge => &LIGHT_RAIL,
            _ => &PASSENGER,
        };
        let per_train = vehicle_emission(coeffs, v);
        let q_corr = 10.0 * (trains_passenger / flow_denom).log10();
        for i in 0..NUM_BANDS {
            total_energy[i] += ((per_train[i] + q_corr) * std::f64::consts::LN_10 * 0.1).exp();
        }
    }

    if trains_freight > 0.0 {
        let per_train = vehicle_emission(&FREIGHT, v.min(FREIGHT.v_max));
        let q_corr = 10.0 * (trains_freight / flow_denom).log10();
        for i in 0..NUM_BANDS {
            total_energy[i] += ((per_train[i] + q_corr) * std::f64::consts::LN_10 * 0.1).exp();
        }
    }

    let mut result = [f64::NEG_INFINITY; NUM_BANDS];
    for i in 0..NUM_BANDS {
        result[i] = if total_energy[i] > 0.0 {
            10.0 * total_energy[i].log10()
        } else {
            f64::NEG_INFINITY
        };
    }
    result
}

/// Default train counts when enrichment data is not available.
/// Returns (passenger_per_day, freight_per_day).
pub fn default_traffic(rail_type: RailType, usage: u8) -> (f64, f64) {
    match rail_type {
        RailType::Tram => (120.0, 0.0),       // urban tram: ~120 services/day
        RailType::LightRail => (80.0, 0.0),   // light rail: ~80/day
        RailType::NarrowGauge => (10.0, 0.0), // narrow gauge: tourist/local
        RailType::Funicular => (40.0, 0.0),   // funicular: frequent but short
        RailType::Rail => match usage {
            0 => (80.0, 20.0), // main line: 80 passenger + 20 freight
            1 => (30.0, 5.0),  // branch: 30 passenger + 5 freight
            2 => (0.0, 15.0),  // industrial siding: freight only
            _ => (40.0, 10.0), // unknown: moderate
        },
    }
}

/// Default speed when maxspeed tag is missing.
///
/// Tram 25 km/h (was 40 until 2026-07-11): OSM tram ways almost never carry
/// maxspeed, so the default IS the fleet's modelled speed. European street
/// trams average ~18-19 km/h commercial speed incl. stops (TRAM Barcelona
/// publishes 18.6; Prague DPP ~19), with 20-35 km/h between stops — 25 is
/// the between-stops street-running middle. At 40 the rolling term
/// (30·log10(v/50)) made a single modelled tram line exceed a street NMT's
/// measured TOTAL ambient (Barcelona station 9907, finding
/// 2026-07-10-bcn-tram-emission-hot; −3.9 dB/line A-weighted at 25 incl.
/// traction + the +10·log10(1/v) density term). A Europe-first
/// street-running prior, not a measured global constant — Reserved-track trams running 45-55 are now
/// under-defaulted — accepted until tram speeds are enriched from GTFS
/// stop-to-stop times (finding's follow-up), which fixes both directions.
pub fn default_speed(rail_type: RailType) -> f64 {
    match rail_type {
        RailType::Tram => 25.0,
        RailType::LightRail => 60.0,
        RailType::NarrowGauge => 40.0,
        RailType::Funicular => 20.0,
        RailType::Rail => 80.0,
    }
}

/// Free-field Lden [dB(A)] of one rail row at horizontal distance `d` metres.
///
/// **Reference propagation** (the per-row reach solver's spine, which
/// reproduces the default mainline boundary at 7 km):
/// ISO 9613-2 cylindrical line spreading `10·log10(2π·d)` + atmospheric
/// absorption `α_atm·d/1000`, **best-case ground** (`G = 0`, hard reflective
/// ground — the loudest the receiver can ever hear, so reach never under-shoots
/// a soft-ground site), **no** terrain / screening / vegetation / finite-line
/// (a blanket reach can't know the per-receiver geometry; the kernel still
/// applies all of those per pixel inside the reach). Per-period emission uses
/// the SAME per-region, per-category day/evening/night split as the kernel —
/// resolved via [`rail_time_dist`] on `admin` and `rail_type`, so a freight-heavy
/// EU corridor reaches farther at night exactly as `compute_railways` hears it.
/// The shares feed [`railway_emission`], then fold to Lden with the END +5/+10 dB
/// penalties via [`crate::periods::compute_lden`].
///
/// `q_pax` / `q_frt` are the *effective* whole-day counts (post service /
/// parallel-divisor scaling — i.e. `NormalizedRail::scaled_*_per_day`), so a
/// divided or service track shrinks its own reach.
fn free_field_lden_at(
    admin: Admin,
    rail_type: RailType,
    speed_kmh: f64,
    q_pax: f64,
    q_frt: f64,
    d: f64,
) -> f64 {
    use crate::constants::ALPHA_ATM;
    use crate::propagation::iso9613::{a_weighted_total, legacy_ground_atten_db};

    let d = d.max(1.0);
    let geo = 10.0 * (2.0 * std::f64::consts::PI * d).log10();
    let d_over_1000 = d / 1000.0;
    let received = |pax_pct: f64, frt_pct: f64, period_hours: f64| -> f64 {
        let em = railway_emission(
            rail_type,
            speed_kmh,
            q_pax * pax_pct,
            q_frt * frt_pct,
            period_hours,
        );
        let mut bands = [0.0f64; NUM_BANDS];
        for i in 0..NUM_BANDS {
            // G = 0 is the LOUDEST ground the path could have (A_ground is
            // monotone increasing in G), so the reach this solves stays an
            // upper bound on audibility; kept explicit, and routed through the
            // shared term, so the boundary matches the kernel's free-field
            // limit exactly. Post hard-ground fix that term is −3 dB, not 0.
            bands[i] = em[i] - geo - ALPHA_ATM[i] * d_over_1000 - legacy_ground_atten_db(i, 0.0);
        }
        a_weighted_total(&bands)
    };
    let [(pd, fd, hd), (pe, fe, he), (pn, fn_, hn)] = rail_time_dist(admin, rail_type).periods();
    let ld = received(pd, fd, hd);
    let le = received(pe, fe, he);
    let ln = received(pn, fn_, hn);
    crate::periods::compute_lden(ld, le, ln)
}

/// Per-row rail audibility reach [m]: the distance at which this segment's own
/// free-field Lden falls to [`crate::constants::RAILWAY_REACH_TARGET_LDEN_DB`]
/// (~25 dB), clamped to `[RAILWAY_REACH_CLAMP_MIN, RAILWAY_REACH_CLAMP_MAX]`.
/// Replaces the retired blanket `RAILWAY_MAX_RADIUS`; the heatmap loader and the
/// popup distance gate BOTH call this, so their cutoff is identical by
/// construction (no magic-number drift). Runs once per row at load — cost is
/// irrelevant.
///
/// Solved by bisection over **log-distance** (`free_field_lden_at` is
/// monotonically decreasing in `d`, dominated by the `10·log10(2π·d)` term, so
/// the root is unique). 40 log-steps over `[100 m, 50 km]` converge to < 1 m —
/// far tighter than the 30 m raster cadence the reach feeds. If the row is so
/// loud it never crosses 25 dB inside 50 km, or so quiet it is already below at
/// 100 m, the clamp catches it.
///
/// `q_pax` / `q_frt` = effective whole-day counts (post service / divisor
/// scaling). `admin` selects the per-region period split so the reach the loader
/// bakes and the cutoff the popup gates on share ONE share model (the same model
/// the kernel computes) — see [`free_field_lden_at`] for the propagation
/// reference.
pub fn rail_reach_m(
    admin: Admin,
    rail_type: RailType,
    speed_kmh: f64,
    q_pax: f64,
    q_frt: f64,
) -> f64 {
    use crate::constants::{
        RAILWAY_REACH_CLAMP_MAX, RAILWAY_REACH_CLAMP_MIN, RAILWAY_REACH_TARGET_LDEN_DB,
    };
    let target = RAILWAY_REACH_TARGET_LDEN_DB;
    let mut lo = 100.0_f64; // below floor; bisection bracket, clamp finalises
    let mut hi = 50_000.0_f64; // above ceiling; widest bracket we ever need
                               // 40 log-halvings: (ln(50000)-ln(100))/2^40 → sub-millimetre, ample margin.
    for _ in 0..40 {
        let mid = ((lo.ln() + hi.ln()) * 0.5).exp();
        if free_field_lden_at(admin, rail_type, speed_kmh, q_pax, q_frt, mid) > target {
            lo = mid; // still loud → push the crossing outward
        } else {
            hi = mid;
        }
    }
    let reach = ((lo.ln() + hi.ln()) * 0.5).exp();
    reach.clamp(RAILWAY_REACH_CLAMP_MIN, RAILWAY_REACH_CLAMP_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::iso9613::a_weighted_total;

    // 24h is used as "day-equivalent total" so old tests remain comparable.
    const DAY_H: f64 = 24.0;

    #[test]
    fn test_passenger_100kmh() {
        // 50 passenger trains/day at 100 km/h — Leq over 24 h
        let bands = railway_emission(RailType::Rail, 100.0, 50.0, 0.0, DAY_H);
        let aw = a_weighted_total(&bands);
        // Expected: 50-85 dB(A)/m for suburban rail.
        assert!(
            aw > 50.0 && aw < 85.0,
            "passenger 100km/h 50 trains: {:.1}",
            aw
        );
    }

    #[test]
    fn test_freight_louder() {
        // Freight should be louder than passenger at same Q and speed
        let pax = railway_emission(RailType::Rail, 80.0, 20.0, 0.0, DAY_H);
        let frt = railway_emission(RailType::Rail, 80.0, 0.0, 20.0, DAY_H);
        let pax_aw = a_weighted_total(&pax);
        let frt_aw = a_weighted_total(&frt);
        assert!(
            frt_aw > pax_aw,
            "freight ({:.1}) should be louder than passenger ({:.1})",
            frt_aw,
            pax_aw
        );
    }

    #[test]
    fn test_tram_lower_speed() {
        // 100 trams/day at 40 km/h
        let bands = railway_emission(RailType::Tram, 40.0, 100.0, 0.0, DAY_H);
        let aw = a_weighted_total(&bands);
        assert!(aw > 50.0 && aw < 85.0, "tram 40km/h 100 trams: {:.1}", aw);
    }

    #[test]
    fn test_leq_day_vs_night_same_count() {
        // Same trains passed per period — shorter period means higher hourly flow,
        // so Leq over 4 h (evening) is louder than Leq over 12 h (day).
        let day = a_weighted_total(&railway_emission(RailType::Rail, 100.0, 100.0, 10.0, 12.0));
        let eve = a_weighted_total(&railway_emission(RailType::Rail, 100.0, 100.0, 10.0, 4.0));
        let diff = eve - day;
        // +10·log10(12/4) ≈ 4.77 dB expected
        assert!(
            (diff - 4.77).abs() < 0.1,
            "expected +4.77 dB, got {:.2}",
            diff
        );
    }

    /// The reach solver must put the free-field Lden of each representative row
    /// exactly at the 25 dB target *at the distance it returns* — the defining
    /// property. Verified by re-evaluating `free_field_lden_at` at the solved
    /// reach (skipped when the clamp fired, since then the crossing is outside
    /// `[min,max]` and the returned value is the clamp, not the root).
    /// Uses `Admin::UNKNOWN` (world split) — the property holds under any split.
    #[test]
    fn reach_lands_on_25_db_target() {
        let admin = Admin::UNKNOWN;
        let mut unclamped = 0;
        for (rt, sp, qp, qf) in [
            (RailType::Rail, 80.0, 80.0, 20.0),
            (RailType::Rail, 300.0, 80.0, 0.0),
            (RailType::Tram, 40.0, 120.0, 0.0),
        ] {
            let r = rail_reach_m(admin, rt, sp, qp, qf);
            let lden = free_field_lden_at(admin, rt, sp, qp, qf, r);
            if r >= 10_000.0 {
                // Clamped: the crossing lies OUTSIDE the band, so the defining
                // property cannot hold at `r`. What must hold is that the clamp
                // is the reason — the row is still above target at the ceiling.
                // (The 300 km/h corridor moved here when the CNOSSOS
                // hard-ground floor lifted every row's free-field limit 3 dB.)
                assert!(
                    lden > 25.0,
                    "{rt:?} clamped at {r} but Lden there is {lden:.3} ≤ 25 — not a clamp"
                );
                continue;
            }
            assert!(r > 2_000.0, "{rt:?} reach {r} hit the floor clamp");
            assert!(
                (lden - 25.0).abs() < 0.05,
                "{:?} Lden@reach = {lden:.3}, want 25",
                rt
            );
            unclamped += 1;
        }
        assert!(unclamped >= 2, "the 25 dB property was never exercised");
    }

    /// POST-C1 ANCHOR: a default mainline (80 pax + 20 freight @ 80 km/h) under
    /// the WORLD split (`Admin::UNKNOWN`, freight 0.50/0.167/0.333) reaches
    /// ≈9.2 km — PAST the retired blanket `RAILWAY_MAX_RADIUS = 7000` because
    /// even the uniform world split lifts the freight night share 0.15→0.333 vs
    /// the old flat split, whose crossing was 25.3 dB at 7 km. The dominant
    /// mainline class is no longer perfectly
    /// value-neutral — that is the intended C1 effect (the night-heavy
    /// redistribution reaches the fringe ring), bounded by the 10 km clamp.
    ///
    /// WAS ≈7.7 km until the CNOSSOS hard-ground floor landed (2026-08-05).
    /// `free_field_lden_at` solves at G = 0, the loudest ground a path can
    /// have, and that limit is `A_ground = −3 dB` (not 0 dB), so every row is
    /// 3 dB louder at every distance and its 25 dB crossing moves outward. The
    /// old figure was the missing term, not a calibration; recomputed, not
    /// re-fitted.
    #[test]
    fn default_mainline_reach_post_c1() {
        let r = rail_reach_m(Admin::UNKNOWN, RailType::Rail, 80.0, 80.0, 20.0);
        assert!(
            (8_900.0..=9_400.0).contains(&r),
            "world mainline reach {r:.0} m, want ≈9.2 km"
        );
    }

    /// C1: the SAME default mainline under an EU region (CZ) reaches FARTHER than
    /// off-corridor — EU freight runs 54.6 % at night (vs 33 % world), so the
    /// night-penalised Lden rises and the 25 dB crossing moves outward. Direction
    /// is the whole point of C1; magnitude is bounded by the 10 km clamp.
    #[test]
    fn eu_mainline_reach_exceeds_world() {
        let cz = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let eu = rail_reach_m(cz, RailType::Rail, 80.0, 80.0, 20.0);
        let world = rail_reach_m(Admin::UNKNOWN, RailType::Rail, 80.0, 80.0, 20.0);
        assert!(
            eu > world,
            "EU mainline reach {eu:.0} must exceed world {world:.0}"
        );
    }

    /// HONESTY FIX: a 300 km/h high-speed passenger corridor is 30.8 dB at 7 km,
    /// 5.8 dB louder than the boundary. Pax-only, so the EU
    /// vs world freight split is irrelevant (pax night 0.10 both).
    ///
    /// Its unclamped crossing is 10,866.8 m (measured 2026-09-03): the old 10 km
    /// ceiling clipped it, the decided 11 km ceiling lets the class end where its
    /// own 25 dB crossing is. The assertion worth pinning is that the class is
    /// solved acoustically again, between the old cap and the new ceiling.
    #[test]
    fn highspeed_reach_is_solved_below_the_ceiling() {
        let r = rail_reach_m(Admin::UNKNOWN, RailType::Rail, 300.0, 80.0, 0.0);
        assert!(
            r > 10_000.0 && r < crate::constants::RAILWAY_REACH_CLAMP_MAX,
            "HS reach {r:.0} m, want (10 km, 11 km ceiling)"
        );
        // …and the old 10 km cap really clipped it: the free-field Lden there is
        // still above the 25 dB target, while at the solved reach it has fallen
        // to the target.
        let at_old_cap =
            free_field_lden_at(Admin::UNKNOWN, RailType::Rail, 300.0, 80.0, 0.0, 10_000.0);
        assert!(
            at_old_cap > crate::constants::RAILWAY_REACH_TARGET_LDEN_DB,
            "HS Lden at the old 10 km cap is {at_old_cap:.2} dB, must still exceed the 25 dB target"
        );
        let at_reach = free_field_lden_at(Admin::UNKNOWN, RailType::Rail, 300.0, 80.0, 0.0, r);
        assert!(
            (at_reach - crate::constants::RAILWAY_REACH_TARGET_LDEN_DB).abs() < 0.1,
            "HS Lden at its solved reach is {at_reach:.2} dB, want the 25 dB target"
        );
    }

    /// PERF WIN: tram (120 services/day @ 40 km/h) is only 16.8 dB @ 7 km —
    /// far below the boundary, so it shrinks. Calibrated reach ≈4.3-4.7 km
    /// (continuous form; the 3.5 km bucket was the rounded light-rail figure,
    /// while the busier 120-train tram default lands a touch
    /// higher). Lighter rail classes shrink further still. Was ≈3.6 km before
    /// the CNOSSOS hard-ground floor made the G = 0 free-field limit −3 dB
    /// instead of 0 dB; recomputed, not re-fitted.
    #[test]
    fn tram_reach_shrinks_below_mainline() {
        let admin = Admin::UNKNOWN;
        let tram = rail_reach_m(admin, RailType::Tram, 40.0, 120.0, 0.0);
        assert!(
            (4_300.0..=4_700.0).contains(&tram),
            "tram reach {tram:.0} m, want ≈4.3-4.7 km"
        );
        let light = rail_reach_m(admin, RailType::LightRail, 60.0, 80.0, 0.0);
        assert!(
            light < tram,
            "light-rail {light:.0} should be < tram {tram:.0}"
        );
        assert!(
            light < 7_000.0,
            "light-rail {light:.0} must be well under the old 7 km"
        );
    }

    /// Clamp floor: a near-silent stub (one passenger train/day @ 80 km/h
    /// solves to ~900 m) must still clamp UP to the 2 km floor so its near
    /// field stays drawn. Clamp ceiling: a very loud, fast, freight-heavy
    /// corridor solves past 11 km and must clamp DOWN to the halo budget.
    #[test]
    fn reach_clamps_at_floor_and_ceiling() {
        let admin = Admin::UNKNOWN;
        let stub = rail_reach_m(admin, RailType::Rail, 80.0, 1.0, 0.0);
        assert_eq!(
            stub, 2_000.0,
            "degenerate-quiet row must clamp to the 2 km floor"
        );
        let loud = rail_reach_m(admin, RailType::Rail, 250.0, 200.0, 80.0);
        assert_eq!(
            loud,
            crate::constants::RAILWAY_REACH_CLAMP_MAX,
            "loud HS-freight corridor must clamp to the 11 km ceiling"
        );
    }

    // ── C1: per-region, per-category period shares ──────────────────────────

    /// Every shipped table row's pax AND frt shares must sum to 1.0; energy is
    /// only redistributed across periods, never created or destroyed.
    #[test]
    fn time_dist_shares_sum_to_one() {
        for td in [&TD_EU_RAIL, &TD_EU_TRAM, &TD_WORLD_RAIL, &TD_WORLD_TRAM] {
            let ps: f64 = td.pax.iter().sum();
            let fs: f64 = td.frt.iter().sum();
            assert!((ps - 1.0).abs() < 1e-9, "pax shares sum {ps}");
            assert!((fs - 1.0).abs() < 1e-9, "frt shares sum {fs}");
        }
    }

    /// Plausibility bands: EU freight night ∈ [0.45, 0.60]; EU pax
    /// night ∈ [0.05, 0.15]; tram night ≤ 0.08; non-EU freight night = 8/24.
    // assertions_on_constants: the tram bound asserts a single const ≤ literal;
    // kept as a runtime guard (with its message) alongside the range checks it sits with.
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn time_dist_plausibility_bands() {
        assert!(
            (0.45..=0.60).contains(&TD_EU_RAIL.frt[2]),
            "EU frt night {}",
            TD_EU_RAIL.frt[2]
        );
        assert!(
            (0.05..=0.15).contains(&TD_EU_RAIL.pax[2]),
            "EU pax night {}",
            TD_EU_RAIL.pax[2]
        );
        assert!(
            TD_EU_TRAM.pax[2] <= 0.08,
            "tram night {}",
            TD_EU_TRAM.pax[2]
        );
        assert!(
            (TD_WORLD_RAIL.frt[2] - 8.0 / 24.0).abs() < 1e-9,
            "non-EU frt night {} != 8/24",
            TD_WORLD_RAIL.frt[2]
        );
    }

    /// The exact derived EU freight fractions — 96.75/32.25/155
    /// of 284 — must ship, not the rounded-then-drifted 0.33/0.13/0.54.
    #[test]
    fn eu_freight_exact_derived_fractions() {
        assert!(
            (TD_EU_RAIL.frt[0] - 0.340_67).abs() < 1e-4,
            "{}",
            TD_EU_RAIL.frt[0]
        );
        assert!(
            (TD_EU_RAIL.frt[1] - 0.113_56).abs() < 1e-4,
            "{}",
            TD_EU_RAIL.frt[1]
        );
        assert!(
            (TD_EU_RAIL.frt[2] - 0.545_77).abs() < 1e-4,
            "{}",
            TD_EU_RAIL.frt[2]
        );
    }

    /// The resolver hands trams/light-rail/etc the urban PAX curve in both slots;
    /// no freight ever applies to rail_type 1-4.
    #[test]
    fn freight_shares_never_apply_to_non_rail_types() {
        for rt in [
            RailType::Tram,
            RailType::LightRail,
            RailType::NarrowGauge,
            RailType::Funicular,
        ] {
            for admin in [
                Admin::UNKNOWN,
                Admin {
                    continent: crate::admin::Continent::Europe,
                    country_iso: *b"DE",
                    city_id: 0,
                },
            ] {
                let td = rail_time_dist(admin, rt);
                assert_eq!(td.pax, td.frt, "{rt:?} must have frt == pax (no freight)");
            }
        }
    }

    /// Geographic Europe outside the EU whitelist (RU/UA/BY) must take the WORLD
    /// table, not the EU freight curve; `Continent::Europe` is geographic,
    /// not the EU.
    #[test]
    fn geographic_europe_outside_whitelist_is_world() {
        for iso in [*b"RU", *b"UA", *b"BY"] {
            let admin = Admin {
                continent: crate::admin::Continent::Europe,
                country_iso: iso,
                city_id: 0,
            };
            let td = rail_time_dist(admin, RailType::Rail);
            assert_eq!(
                td.frt,
                TD_WORLD_RAIL.frt,
                "{:?} must take the world freight split",
                std::str::from_utf8(&iso)
            );
        }
        // …while a whitelisted EU country (FR) takes the EU split.
        let fr = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"FR",
            city_id: 0,
        };
        assert_eq!(rail_time_dist(fr, RailType::Rail).frt, TD_EU_RAIL.frt);
    }

    /// SOLVER-VS-KERNEL CONSISTENCY (task mandate): the reach solver and the
    /// kernel must compute the same period Lden for the same row+admin. Since the
    /// solver IS `free_field_lden_at` (which now consumes `rail_time_dist`), this
    /// pins that no second copy of the split exists — recompute the kernel's
    /// free-field Lden independently from `railway_emission` + the shared shares
    /// and require an exact match to `free_field_lden_at`.
    #[test]
    fn solver_period_model_matches_kernel_split() {
        let cz = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let (rt, sp, qp, qf, d) = (RailType::Rail, 80.0, 80.0, 20.0, 3_500.0);
        // Independent re-derivation using the public shared helper.
        let td = rail_time_dist(cz, rt);
        let geo = 10.0 * (2.0 * std::f64::consts::PI * d).log10();
        let recv = |pax_pct: f64, frt_pct: f64, h: f64| {
            let em = railway_emission(rt, sp, qp * pax_pct, qf * frt_pct, h);
            let mut bands = [0.0f64; NUM_BANDS];
            for i in 0..NUM_BANDS {
                // Same G = 0 free-field limit the solver takes — through the
                // shared ground term, so this stays an independent check of
                // the PERIOD SPLIT and not a second copy of the ground formula
                // (it silently was one while `A_ground(0)` happened to be 0).
                bands[i] = em[i]
                    - geo
                    - crate::constants::ALPHA_ATM[i] * (d / 1000.0)
                    - crate::propagation::iso9613::legacy_ground_atten_db(i, 0.0);
            }
            a_weighted_total(&bands)
        };
        let [(pd, fd, hd), (pe, fe, he), (pn, fn_, hn)] = td.periods();
        let want =
            crate::periods::compute_lden(recv(pd, fd, hd), recv(pe, fe, he), recv(pn, fn_, hn));
        let got = free_field_lden_at(cz, rt, sp, qp, qf, d);
        assert!(
            (want - got).abs() < 1e-9,
            "kernel split {want} != solver {got}"
        );
    }

    /// C1 CORE INVARIANT: a mixed EU line's `Ln − Lden` must NOT equal the old
    /// −7.91 dB identity (the flat-split artifact this milestone kills). A
    /// freight-heavy EU corridor must also have night hourly
    /// energy exceed day — the physical point of the freight night split.
    #[test]
    fn eu_split_breaks_minus_7_91_identity_and_night_exceeds_day() {
        let cz = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let td = rail_time_dist(cz, RailType::Rail);
        // Mixed line 80 pax + 60 freight: per-period A-weighted received-equivalent
        // (use the emission Leq directly — period geometry is common).
        let aw = |pax_pct: f64, frt_pct: f64, h: f64| {
            a_weighted_total(&railway_emission(
                RailType::Rail,
                80.0,
                80.0 * pax_pct,
                60.0 * frt_pct,
                h,
            ))
        };
        let [(pd, fd, hd), (pe, fe, he), (pn, fn_, hn)] = td.periods();
        let (ld, le, ln) = (aw(pd, fd, hd), aw(pe, fe, he), aw(pn, fn_, hn));
        let lden = crate::periods::compute_lden(ld, le, ln);
        assert!(
            (ln - lden - (-7.91)).abs() > 0.5,
            "Ln-Lden = {:.2} must break the −7.91 flat-split identity",
            ln - lden
        );
        // Freight-heavy: night hourly Leq exceeds day hourly Leq.
        assert!(
            ln > ld,
            "freight-heavy EU night Leq {ln:.1} must exceed day {ld:.1}"
        );
    }

    /// GATE UPPER-BOUND REGRESSION: the popup early-exit in
    /// `compute_railways` must screen on the LOUDEST period, not day. For a quiet,
    /// slow EU freight row the night block (freight 0.5458 over 8 h) is louder than
    /// day (freight 0.3407 over 12 h), so a day-only gate would prune a segment the
    /// heatmap (all-period Lden) keeps — a parity break. Pin: at a distance where
    /// the DAY band drops below the free-field threshold, the max-over-periods band
    /// stays above it, so the segment survives the gate.
    #[test]
    fn early_gate_screens_on_loudest_period_not_day() {
        let cz = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let td = rail_time_dist(cz, RailType::Rail);
        // Quiet slow EU freight (a near-silent service/branch stub: effective
        // 0.02 freight/day @ 30 km/h). Loud rows never expose the window — the
        // gate only matters near the threshold, which is exactly where a quiet
        // night-freight row sits.
        let (sp, qp, qf) = (30.0, 0.0, 0.02);
        let max_band = |pax_pct: f64, frt_pct: f64, h: f64| {
            railway_emission(RailType::Rail, sp, qp * pax_pct, qf * frt_pct, h)
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
        };
        let day = max_band(td.pax[0], td.frt[0], 12.0);
        let loudest = td
            .periods()
            .iter()
            .map(|&(p, f, h)| max_band(p, f, h))
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            loudest > day,
            "night must be the loudest period for EU freight"
        );
        // 1500 m sits inside the day-prunes / loudest-keeps window (measured
        // 1200–1900 m for this row).
        let d = 1_500.0;
        let day_pruned = grid::geo::below_free_field_threshold_line(day, d, 0.0);
        let loudest_kept = !grid::geo::below_free_field_threshold_line(loudest, d, 0.0);
        assert!(
            day_pruned && loudest_kept,
            "at {d} m: day-gate prunes ({day_pruned}) but max-over-periods keeps ({loudest_kept}) — the bug the gate fix closes",
        );
    }

    /// A pax-only line's Lden shifts only modestly vs the old flat split: its
    /// night fraction drops 0.15→0.10, so Lden falls −0.8 ± 0.2 dB.
    /// Computed against the retired flat 0.65/0.20/0.15 split.
    #[test]
    fn pax_only_lden_shift_vs_old_flat_split() {
        let cz = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let aw = |pct: f64, h: f64| {
            a_weighted_total(&railway_emission(RailType::Rail, 100.0, 80.0 * pct, 0.0, h))
        };
        // New (EU pax 0.70/0.20/0.10):
        let td = rail_time_dist(cz, RailType::Rail);
        let new = crate::periods::compute_lden(
            aw(td.pax[0], 12.0),
            aw(td.pax[1], 4.0),
            aw(td.pax[2], 8.0),
        );
        // Old flat 0.65/0.20/0.15:
        let old = crate::periods::compute_lden(aw(0.65, 12.0), aw(0.20, 4.0), aw(0.15, 8.0));
        let shift = new - old;
        assert!(
            (shift - (-0.8)).abs() <= 0.2,
            "pax-only Lden shift {shift:.2} dB, want -0.8±0.2"
        );
    }
}

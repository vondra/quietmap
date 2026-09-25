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

use crate::square_country_city::SquareCountryCity;
use crate::types::NUM_BANDS;

const B_ROLLING: f64 = 30.0;

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
/// NO, UK. Keyed on the country code, NOT [`crate::square_country_city::Continent::Europe`] —
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
fn is_eu_rail_region(square_country_city: SquareCountryCity) -> bool {
    EU_ISO_WHITELIST.contains(&&square_country_city.country_iso)
}

/// Resolve the day/evening/night split for a rail segment from its SquareCountryCity
/// and vehicle type. Trams / light-rail / narrow-gauge / funicular always take
/// the urban passenger curve (no freight). [`RailType::Rail`] takes the EU vs
/// world freight+passenger table on the [`EU_ISO_WHITELIST`]. `SquareCountryCity::UNKNOWN`
/// (oceanic / pre-build z9 squares / tests) is deterministically non-EU.
///
/// Structured for per-country overrides (match `square_country_city.country_code()` first,
/// then the EU/world fork), but only the cited rows ship today: refining
/// DE/CH/NL from EBA Lärmkartierung / BAV Emissionsplan / ProRail geluidregister
/// per-section counts is the R2 follow-up (those feeds fix counts AND shares).
pub fn rail_time_dist(
    square_country_city: SquareCountryCity,
    rail_type: RailType,
) -> &'static RailTimeDist {
    let eu = is_eu_rail_region(square_country_city);
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

// Per-segment SquareCountryCity
//
// The M3 bake (`pipeline/enrich-roads-country.ts`) stamps three all-or-none
// columns into every `railways.arrow`: `country_iso` (UInt16, two ASCII bytes
// packed `iso0 | iso1<<8`, 0 = `\0\0`), `city_id` (UInt16), `continent`
// (UInt8, mirroring `square_country_city.rs::Continent`). When a row carries them, its OWN
// ISO drives the EU/world split (and reach); when the `country_iso` COLUMN is
// absent (pre-bake data) the caller falls back to today's receiver/region
// square_country_city. A PRESENT 0 bakes `SquareCountryCity::UNKNOWN` → the world split with NO
// receiver fallback.

/// Decode one row's baked SquareCountryCity triplet — exact copy of
/// `crate::defaults::baked_square_country_city`. The two live in separate layer-codever
/// buckets (road vs rail), so neither may import from the other.
pub fn baked_square_country_city(
    country_iso: u16,
    city_id: u16,
    continent: u8,
) -> SquareCountryCity {
    if country_iso == 0 {
        return SquareCountryCity::UNKNOWN;
    }
    SquareCountryCity {
        continent: crate::square_country_city::Continent::from_u8(continent),
        country_iso: country_iso.to_le_bytes(),
        city_id,
    }
}

struct RailVehicleCoeffs {
    a_rolling: [f64; NUM_BANDS],
    a_traction: [f64; NUM_BANDS],
    v_ref: f64,
    /// The category's representative speed never exceeds this, in its level and its density.
    v_max: f64,
}

const FREIGHT: RailVehicleCoeffs = RailVehicleCoeffs {
    a_rolling: [110.0, 118.0, 126.0, 130.0, 131.0, 128.0, 120.0, 110.0],
    a_traction: [115.0, 113.0, 110.0, 105.0, 100.0, 95.0, 90.0, 85.0],
    v_ref: 80.0,
    // Freight runs below the posted line speed: EBA Laerm-Monitoring 2023 (Table 11) measured
    // freight pass-bys at a train-weighted mean of 88.9 km/h over its 14 training-square main-line
    // stations (station means 78-96 km/h; holdout rule v1, 2026-09-24).
    v_max: 88.9,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailType {
    Rail,        // 0 — mixed passenger/freight
    Tram,        // 1
    LightRail,   // 2
    NarrowGauge, // 3
    Funicular,   // 4
    Preserved,   // 5 — heritage model not yet assessed
}

impl RailType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Tram,
            2 => Self::LightRail,
            3 => Self::NarrowGauge,
            4 => Self::Funicular,
            5 => Self::Preserved,
            _ => Self::Rail,
        }
    }
}

/// One representative speed per category on a line: the line speed within the category's range.
fn category_speed_kmh(coeffs: &RailVehicleCoeffs, line_speed_kmh: f64) -> f64 {
    line_speed_kmh.clamp(20.0, coeffs.v_max)
}

/// Compute emission bands for one vehicle type at its representative speed [dB/vehicle].
fn vehicle_emission(coeffs: &RailVehicleCoeffs, category_speed_kmh: f64) -> [f64; NUM_BANDS] {
    let speed_corr = B_ROLLING * (category_speed_kmh / coeffs.v_ref).log10();

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
/// v = the category's representative speed in km/h, the same speed that sets its per-train
/// level. Callers pass the line speed, the per-period train subset and the period length.
pub fn railway_emission(
    rail_type: RailType,
    speed_kmh: f64,
    trains_passenger: f64,
    trains_freight: f64,
    period_hours: f64,
) -> [f64; NUM_BANDS] {
    if matches!(rail_type, RailType::Preserved) {
        return [f64::NEG_INFINITY; NUM_BANDS];
    }
    let passenger_coeffs = match rail_type {
        RailType::Tram => &TRAM,
        RailType::LightRail | RailType::NarrowGauge => &LIGHT_RAIL,
        _ => &PASSENGER,
    };
    let mut total_energy = [0.0f64; NUM_BANDS];
    for (coeffs, trains) in [
        (passenger_coeffs, trains_passenger),
        (&FREIGHT, trains_freight),
    ] {
        if trains <= 0.0 {
            continue;
        }
        let v = category_speed_kmh(coeffs, speed_kmh);
        let per_train = vehicle_emission(coeffs, v);
        let q_corr = 10.0 * (trains / (period_hours.max(0.1) * 1000.0 * v)).log10();
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

/// Main-line freight prior per day: the flat rate that, together with measured lines,
/// other usages and passenger-only zeros, conserves the official 2023 national goods
/// train-km (Eurostat `rail_tf_trainmv`, goods trains, thousand train-km: DE 247,471,
/// FR 52,507, PL 74,090, CZ 29,898, AT 41,584, CH 27,365; fetched 2026-09-25).
/// Solved as (official − fixed) over the prior-carrying line-km measured under these
/// rules in a world refinalize; served-build weights would undercount the sharing on
/// tokenless networks (CZ carries no ref on corridor twins). Planet-260831
/// `railway:traffic_mode` zeroes passenger-only rows in the same solve. The EBA 2023
/// median of 85 it replaces was measured on freight corridors and overcounted DE
/// mains 3.2× (FR 10×); EBA acoustic levels stayed validation-only throughout.
fn mainline_freight_prior(country_iso: [u8; 2]) -> f64 {
    match &country_iso {
        b"DE" => 24.5,
        b"FR" => 5.5,
        b"PL" => 12.0,
        b"CZ" => 13.5,
        b"AT" => 27.1,
        b"CH" => 31.4,
        _ => 20.0, // unevidenced fallback (pre-D2 value; no national total to solve from)
    }
}

/// Default train counts when enrichment data is not available.
/// Returns (passenger_per_day, freight_per_day). `traffic_mode` is the OSM
/// `railway:traffic_mode` enum (0 unknown, 1 passenger, 2 freight, 3 mixed):
/// a passenger-only line gets no freight prior, a freight-only line no passenger
/// prior; measured evidence still wins over the tag downstream.
pub fn default_traffic(
    rail_type: RailType,
    usage: u8,
    country_iso: [u8; 2],
    traffic_mode: u8,
) -> (f64, f64) {
    let (passenger, freight) = match rail_type {
        RailType::Tram => (120.0, 0.0),       // urban tram: ~120 services/day
        RailType::LightRail => (80.0, 0.0),   // light rail: ~80/day
        RailType::NarrowGauge => (10.0, 0.0), // narrow gauge: tourist/local
        RailType::Funicular => (40.0, 0.0),   // funicular: frequent but short
        RailType::Preserved => (0.0, 0.0),
        RailType::Rail => match usage {
            0 => (80.0, mainline_freight_prior(country_iso)),
            1 => (30.0, 5.0),  // branch: 30 passenger + 5 freight
            2 => (0.0, 15.0),  // industrial siding: freight only
            _ => (40.0, 10.0), // unknown: moderate
        },
    };
    match traffic_mode {
        1 => (passenger, 0.0),
        2 => (0.0, freight),
        _ => (passenger, freight),
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
        RailType::Preserved => 0.0,
    }
}

/// The prepared passenger and freight counts of each period as band emissions `L_W′`.
pub fn rail_period_emissions(
    rail_type: RailType,
    speed_kmh: f64,
    traffic: crate::normalize::RailTraffic,
) -> [[f64; NUM_BANDS]; 3] {
    traffic
        .periods()
        .map(|(passenger, freight, hours)| railway_emission(rail_type, speed_kmh, passenger, freight, hours))
}

/// Reach of a rail row: where the surface relevance bound's Lden falls to the reach edge.
pub fn rail_reach_m(
    rail_type: RailType,
    speed_kmh: f64,
    traffic: crate::normalize::RailTraffic,
    weather: &crate::propagation::meteorology::Meteorology,
) -> f64 {
    use crate::propagation::relevance_bound::{
        surface_relevance_bound, SourceSpread, LINE_REACH_CEILING_M, REACH_EDGE_LDEN_DB,
    };
    surface_relevance_bound(weather).reach_m(
        &rail_period_emissions(rail_type, speed_kmh, traffic),
        SourceSpread::Line,
        REACH_EDGE_LDEN_DB,
        LINE_REACH_CEILING_M,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::periods::END_PERIOD_HOURS;
    use crate::propagation::iso9613::a_weighted_total;

    fn prepared_traffic(
        country: SquareCountryCity,
        kind: RailType,
        passenger: f64,
        freight: f64,
    ) -> crate::normalize::RailTraffic {
        use crate::normalize::{RailCategoryTraffic, RailTraffic};
        let split = rail_time_dist(country, kind);
        RailTraffic {
            passenger: RailCategoryTraffic {
                periods: split.pax.map(|share| passenger * share),
                status: 2,
                ..Default::default()
            },
            freight: RailCategoryTraffic {
                periods: split.frt.map(|share| freight * share),
                status: 2,
                ..Default::default()
            },
        }
    }

    fn rail_reach_m(
        country: SquareCountryCity,
        kind: RailType,
        speed: f64,
        passenger: f64,
        freight: f64,
    ) -> f64 {
        super::rail_reach_m(
            kind,
            speed,
            prepared_traffic(country, kind, passenger, freight),
            &crate::propagation::meteorology::Meteorology::defaults(),
        )
    }

    /// The relevance bound's Lden of the row at `distance` — what the reach solves.
    fn bound_lden_at(
        country: SquareCountryCity,
        kind: RailType,
        speed: f64,
        passenger: f64,
        freight: f64,
        distance: f64,
    ) -> f64 {
        use crate::propagation::relevance_bound::{surface_relevance_bound, SourceSpread};
        surface_relevance_bound(&crate::propagation::meteorology::Meteorology::defaults()).lden_db(
            &rail_period_emissions(kind, speed, prepared_traffic(country, kind, passenger, freight)),
            SourceSpread::Line,
            distance,
        )
    }

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

    /// #34: freight at 160 km/h posted used a 120 km/h level with a 160 km/h density (−1.25 dB).
    #[test]
    fn one_representative_speed_sets_both_level_and_density_of_a_category() {
        for line_speed in [120.0, 160.0, 300.0] {
            assert_eq!(
                railway_emission(RailType::Rail, line_speed, 0.0, 20.0, DAY_H),
                railway_emission(RailType::Rail, FREIGHT.v_max, 0.0, 20.0, DAY_H)
            );
        }
        assert_eq!(
            railway_emission(RailType::Tram, 90.0, 100.0, 0.0, DAY_H),
            railway_emission(RailType::Tram, TRAM.v_max, 100.0, 0.0, DAY_H)
        );
        assert_ne!(
            railway_emission(RailType::Rail, 60.0, 0.0, 20.0, DAY_H),
            railway_emission(RailType::Rail, FREIGHT.v_max, 0.0, 20.0, DAY_H)
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

    /// The reach puts the bound's Lden of each representative row exactly at the 30 dB edge
    /// at the distance it returns, unless the profile ceiling cut it — then the row is still
    /// above the edge there.
    #[test]
    fn reach_lands_on_the_edge_or_the_ceiling() {
        use crate::propagation::relevance_bound::{LINE_REACH_CEILING_M, REACH_EDGE_LDEN_DB};
        let square_country_city = SquareCountryCity::UNKNOWN;
        let mut on_edge = 0;
        for (rt, sp, qp, qf) in [
            (RailType::Rail, 80.0, 2.0, 0.0),
            (RailType::Rail, 80.0, 80.0, 20.0),
            (RailType::Tram, 25.0, 120.0, 0.0),
        ] {
            let r = rail_reach_m(square_country_city, rt, sp, qp, qf);
            let lden = bound_lden_at(square_country_city, rt, sp, qp, qf, r);
            if r >= LINE_REACH_CEILING_M {
                assert!(lden > REACH_EDGE_LDEN_DB, "{rt:?} at the ceiling {r} but Lden there is {lden:.3}");
                continue;
            }
            assert!((lden - REACH_EDGE_LDEN_DB).abs() < 1e-3, "{rt:?} Lden@reach = {lden:.3}");
            on_edge += 1;
        }
        assert!(on_edge >= 2, "the edge property was never exercised");
    }

    /// C1: the SAME quiet mainline under an EU region (CZ) reaches FARTHER than
    /// off-corridor — EU freight runs 54.6 % at night (vs 33 % world), so the
    /// night-penalised Lden rises and the edge crossing moves outward.
    #[test]
    fn eu_mainline_reach_exceeds_world() {
        let cz = SquareCountryCity {
            continent: crate::square_country_city::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let eu = rail_reach_m(cz, RailType::Rail, 60.0, 2.0, 2.0);
        let world = rail_reach_m(SquareCountryCity::UNKNOWN, RailType::Rail, 60.0, 2.0, 2.0);
        assert!(
            eu > world,
            "EU mainline reach {eu:.0} must exceed world {world:.0}"
        );
    }

    /// A tram line reaches less far than a default mainline, a light-rail line less still.
    #[test]
    fn tram_reach_shrinks_below_mainline() {
        let square_country_city = SquareCountryCity::UNKNOWN;
        let mainline = rail_reach_m(square_country_city, RailType::Rail, 80.0, 4.0, 0.0);
        let tram = rail_reach_m(square_country_city, RailType::Tram, 25.0, 20.0, 0.0);
        let light = rail_reach_m(square_country_city, RailType::LightRail, 60.0, 4.0, 0.0);
        assert!(tram < mainline, "tram {tram:.0} should be < mainline {mainline:.0}");
        assert!(light < mainline, "light-rail {light:.0} should be < mainline {mainline:.0}");
    }

    /// A loud corridor stops at the profile ceiling; a near-silent stub reaches only metres.
    #[test]
    fn reach_stops_at_the_profile_ceiling() {
        use crate::propagation::relevance_bound::LINE_REACH_CEILING_M;
        let square_country_city = SquareCountryCity::UNKNOWN;
        let loud = rail_reach_m(square_country_city, RailType::Rail, 250.0, 200.0, 80.0);
        assert_eq!(loud, LINE_REACH_CEILING_M);
        let stub = rail_reach_m(square_country_city, RailType::Rail, 30.0, 0.001, 0.0);
        assert!(stub < 200.0, "stub reach {stub}");
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
            for square_country_city in [
                SquareCountryCity::UNKNOWN,
                SquareCountryCity {
                    continent: crate::square_country_city::Continent::Europe,
                    country_iso: *b"DE",
                    city_id: 0,
                },
            ] {
                let td = rail_time_dist(square_country_city, rt);
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
            let square_country_city = SquareCountryCity {
                continent: crate::square_country_city::Continent::Europe,
                country_iso: iso,
                city_id: 0,
            };
            let td = rail_time_dist(square_country_city, RailType::Rail);
            assert_eq!(
                td.frt,
                TD_WORLD_RAIL.frt,
                "{:?} must take the world freight split",
                std::str::from_utf8(&iso)
            );
        }
        // …while a whitelisted EU country (FR) takes the EU split.
        let fr = SquareCountryCity {
            continent: crate::square_country_city::Continent::Europe,
            country_iso: *b"FR",
            city_id: 0,
        };
        assert_eq!(rail_time_dist(fr, RailType::Rail).frt, TD_EU_RAIL.frt);
    }

    /// The reach and the kernel read one period split: the emissions the reach bounds are the
    /// kernel's own per-period emissions of the row.
    #[test]
    fn reach_emissions_are_the_kernel_period_split() {
        let cz = SquareCountryCity {
            continent: crate::square_country_city::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let (rt, sp, qp, qf) = (RailType::Rail, 80.0, 80.0, 20.0);
        let td = rail_time_dist(cz, rt);
        let got = rail_period_emissions(rt, sp, prepared_traffic(cz, rt, qp, qf));
        for (period, hours) in END_PERIOD_HOURS.into_iter().enumerate() {
            let (pax, frt) = (td.pax[period], td.frt[period]);
            assert_eq!(got[period], railway_emission(rt, sp, qp * pax, qf * frt, hours));
        }
    }

    /// C1 CORE INVARIANT: a mixed EU line's `Ln − Lden` must NOT equal the old
    /// −7.91 dB identity (the flat-split artifact this milestone kills). A
    /// freight-heavy EU corridor must also have night hourly
    /// energy exceed day — the physical point of the freight night split.
    #[test]
    fn eu_split_breaks_minus_7_91_identity_and_night_exceeds_day() {
        let cz = SquareCountryCity {
            continent: crate::square_country_city::Continent::Europe,
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
        let ([pd, pe, pn], [fd, fe, fn_], [hd, he, hn]) = (td.pax, td.frt, END_PERIOD_HOURS);
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

    /// A pax-only line's Lden shifts only modestly vs the old flat split: its
    /// night fraction drops 0.15→0.10, so Lden falls −0.8 ± 0.2 dB.
    /// Computed against the retired flat 0.65/0.20/0.15 split.
    #[test]
    fn pax_only_lden_shift_vs_old_flat_split() {
        let cz = SquareCountryCity {
            continent: crate::square_country_city::Continent::Europe,
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

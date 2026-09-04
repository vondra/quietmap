//! Rail source normalization: raw OSM railway inputs → per-band
//! emission-ready values (`NormalizedRail`), threading the admin region so
//! the period split + audibility reach match the popup kernel and reach
//! solver by construction.

use crate::admin::Admin;
use crate::constants::SOURCE_HEIGHT_RAIL;
use crate::types::{RailSegment, NUM_BANDS};

use super::bands_to_f32;

#[derive(Debug, Clone, Copy)]
pub struct RawRailInput {
    pub rail_type: u8,
    pub usage: u8,
    /// km/h; u16 so 300+ km/h high-speed lines survive (u8 saturated to 255).
    pub maxspeed: u16,
    pub service: u8,
    pub highspeed: bool,
    pub trains_passenger: i32,
    pub trains_freight: i32,
    pub parallel_divisor: u8,
}

#[derive(Debug, Clone)]
pub struct NormalizedRail {
    /// Admin region of the segment, threaded so `period_emissions` and the reach
    /// solver pick the SAME per-region day/evening/night split as the popup
    /// kernel (C1, plan delta 4). The heatmap loader resolves it once per region;
    /// `Admin::UNKNOWN` (tests / un-init) deterministically takes the world split.
    pub admin: Admin,
    pub rail_type: crate::emission::railway::RailType,
    pub source_height_m: f64,
    pub speed_kmh: f64,
    pub scaled_passenger_per_day: f64,
    pub scaled_freight_per_day: f64,
}

impl NormalizedRail {
    pub fn period_emission(
        &self,
        passenger_pct: f64,
        freight_pct: f64,
        period_hours: f64,
    ) -> [f32; NUM_BANDS] {
        bands_to_f32(crate::emission::railway::railway_emission(
            self.rail_type,
            self.speed_kmh,
            self.scaled_passenger_per_day * passenger_pct,
            self.scaled_freight_per_day * freight_pct,
            period_hours,
        ))
    }

    /// Per-period emission using the C1 per-region, per-category split resolved
    /// from `self.admin` + `self.rail_type` (shared with the popup kernel + the
    /// reach solver via [`crate::emission::railway::rail_time_dist`]).
    pub fn period_emissions(&self) -> ([f32; NUM_BANDS], [f32; NUM_BANDS], [f32; NUM_BANDS]) {
        let [(pd, fd, hd), (pe, fe, he), (pn, fn_, hn)] =
            crate::emission::railway::rail_time_dist(self.admin, self.rail_type).periods();
        (
            self.period_emission(pd, fd, hd),
            self.period_emission(pe, fe, he),
            self.period_emission(pn, fn_, hn),
        )
    }

    /// Per-row audibility reach [m] — the distance at which THIS segment's own
    /// free-field Lden falls to the ~25 dB boundary, clamped to `[2 km, 10 km]`
    /// (`emission::railway::rail_reach_m`). Replaces the retired blanket
    /// `RAILWAY_MAX_RADIUS`; the popup gate (`compute_railways`) calls the same
    /// solver on its `RailSegment` with the same `admin`, so the heatmap loader
    /// and popup cull at an identical distance by construction. Uses the *scaled*
    /// (post service / divisor) counts, so a divided or service track shrinks its
    /// own reach.
    pub fn max_distance_m(&self) -> f64 {
        crate::emission::railway::rail_reach_m(
            self.admin,
            self.rail_type,
            self.speed_kmh,
            self.scaled_passenger_per_day,
            self.scaled_freight_per_day,
        )
    }
}

pub fn normalize_rail(input: RawRailInput, admin: Admin) -> NormalizedRail {
    let rail_type = crate::emission::railway::RailType::from_u8(input.rail_type);
    let (def_pax, def_frt) = crate::emission::railway::default_traffic(rail_type, input.usage);
    let speed_kmh = if input.maxspeed > 0 {
        input.maxspeed as f64
    } else if input.highspeed {
        300.0
    } else {
        crate::emission::railway::default_speed(rail_type)
    };

    let q_pax = if input.trains_passenger > 0 {
        input.trains_passenger as f64
    } else {
        def_pax
    };
    let q_frt = if input.trains_freight > 0 {
        input.trains_freight as f64
    } else {
        def_frt
    };
    let service_factor = if input.service > 0 { 0.02 } else { 1.0 };
    let divisor = input.parallel_divisor.max(1) as f64;
    let scale_factor = service_factor / divisor;

    NormalizedRail {
        admin,
        rail_type,
        source_height_m: SOURCE_HEIGHT_RAIL,
        speed_kmh,
        scaled_passenger_per_day: q_pax * scale_factor,
        scaled_freight_per_day: q_frt * scale_factor,
    }
}

pub fn normalize_rail_segment(seg: &RailSegment, admin: Admin) -> NormalizedRail {
    NormalizedRail {
        admin,
        rail_type: crate::emission::railway::RailType::from_u8(seg.rail_type),
        source_height_m: SOURCE_HEIGHT_RAIL,
        speed_kmh: if seg.speed_kmh > 0.0 {
            seg.speed_kmh
        } else {
            // SSOT fallback — a hardcoded 80 here silently diverged from
            // default_speed for non-Rail types (Codex /gg 2026-07-11).
            crate::emission::railway::default_speed(crate::emission::railway::RailType::from_u8(
                seg.rail_type,
            ))
        },
        scaled_passenger_per_day: seg.trains_passenger.max(0.0),
        scaled_freight_per_day: seg.trains_freight.max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rail `maxspeed` is u16 since 2026-06: a posted 300 km/h must reach
    /// the emission as 300, not u8-saturated.
    #[test]
    fn rail_maxspeed_300_survives_u16() {
        let rail = normalize_rail(
            RawRailInput {
                rail_type: 0,
                usage: 0,
                maxspeed: 300,
                service: 0,
                highspeed: false,
                trains_passenger: 50,
                trains_freight: 0,
                parallel_divisor: 1,
            },
            Admin::UNKNOWN,
        );
        assert_eq!(rail.speed_kmh, 300.0);
    }

    #[test]
    fn rail_defaults_keep_freight_when_only_passenger_enriched() {
        let rail = normalize_rail(
            RawRailInput {
                rail_type: 0,
                usage: 0,
                maxspeed: 125,
                service: 0,
                highspeed: false,
                trains_passenger: 42,
                trains_freight: 0,
                parallel_divisor: 3,
            },
            Admin::UNKNOWN,
        );
        assert!((rail.scaled_passenger_per_day - 14.0).abs() < 1e-9);
        assert!((rail.scaled_freight_per_day - (20.0 / 3.0)).abs() < 1e-9);
    }

    /// High-speed rail with no posted maxspeed resolves to 300 km/h. Regression
    /// guard for the `RailSegment.speed_kmh` u8 truncation (300 → 255) that made
    /// the popup ~1.4 dB too quiet vs the surface heatmap (which baked emission
    /// from the full f64 speed). See `types::RailSegment::speed_kmh`.
    #[test]
    fn highspeed_default_resolves_to_300_not_u8_truncated() {
        let rail = normalize_rail(
            RawRailInput {
                rail_type: 0,
                usage: 0,
                maxspeed: 0,
                service: 0,
                highspeed: true,
                trains_passenger: 50,
                trains_freight: 0,
                parallel_divisor: 1,
            },
            Admin::UNKNOWN,
        );
        assert_eq!(
            rail.speed_kmh, 300.0,
            "highspeed no-maxspeed → 300, not 255"
        );
    }

    /// C1 delta 4 — POPUP-vs-LOADER rail emission parity: the heatmap loader's
    /// `NormalizedRail::period_emissions()` must equal an independent application
    /// of the SAME shared `rail_time_dist` shares through `railway_emission` (the
    /// exact chain the popup `compute_railways` loop runs). Bit-identical proves
    /// there is no second copy of the period split (mirrors the aircraft
    /// hoisted-vs-popup parity pattern). Run on a CZ (EU) row so the freight
    /// night share is non-trivial.
    #[test]
    fn loader_period_emissions_match_shared_split() {
        use crate::emission::railway::{rail_time_dist, railway_emission, RailType};
        let cz = Admin {
            continent: crate::admin::Continent::Europe,
            country_iso: *b"CZ",
            city_id: 0,
        };
        let norm = normalize_rail(
            RawRailInput {
                rail_type: 0,
                usage: 0,
                maxspeed: 120,
                service: 0,
                highspeed: false,
                trains_passenger: 100,
                trains_freight: 40,
                parallel_divisor: 1,
            },
            cz,
        );
        let (day, eve, night) = norm.period_emissions();
        let td = rail_time_dist(cz, RailType::Rail);
        let want = |pct_pax: f64, pct_frt: f64, h: f64| -> [f32; NUM_BANDS] {
            bands_to_f32(railway_emission(
                RailType::Rail,
                norm.speed_kmh,
                norm.scaled_passenger_per_day * pct_pax,
                norm.scaled_freight_per_day * pct_frt,
                h,
            ))
        };
        assert_eq!(day, want(td.pax[0], td.frt[0], 12.0), "day period parity");
        assert_eq!(
            eve,
            want(td.pax[1], td.frt[1], 4.0),
            "evening period parity"
        );
        assert_eq!(
            night,
            want(td.pax[2], td.frt[2], 8.0),
            "night period parity"
        );
    }
}

#[cfg(test)]
mod tram_default_speed_tests {
    use super::*;

    /// Locks the 2026-07-11 tram street-running prior (finding
    /// bcn-tram-emission-hot): missing maxspeed on a tram row resolves to
    /// 25 km/h through normalize_rail, and the segment path shares the SSOT.
    #[test]
    fn missing_maxspeed_tram_resolves_to_25() {
        let raw = RawRailInput {
            rail_type: 1, // Tram
            usage: 0,
            maxspeed: 0,
            highspeed: false,
            trains_passenger: 0,
            trains_freight: 0,
            service: 0,
            parallel_divisor: 1,
        };
        let norm = normalize_rail(raw, crate::admin::Admin::UNKNOWN);
        assert_eq!(norm.speed_kmh, 25.0);
    }
}

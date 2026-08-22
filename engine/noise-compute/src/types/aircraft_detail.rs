//! Aircraft popup-detail types — per-band event stats and top-flight rows
//! (typecode, callsign, ICAO hex, geometry) shown in the aircraft popup.
use super::*;

#[derive(Debug, Clone, Default, Serialize)]
pub struct AircraftEventBandStats {
    pub observed_events_per_day: f64,
    pub avg_altitude_m: f64,
    pub top_aircraft: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AircraftTopFlight {
    pub lmax_db: f64,
    pub cpa_distance_m: f64,
    pub altitude_m: f64,
    pub period: u8,   // 0=day, 1=evening, 2=night
    pub date: String, // ISO date "2024-03-15"
    pub profile: String,
    pub energy_pct: f64,         // % of total airborne Lden energy
    pub geometry: [[f64; 2]; 2], // [[start_lon, start_lat], [end_lon, end_lat]]
    /// ICAO typecode (e.g. "B738", "A320") as carried by Stage 0 from
    /// adsb.lol metadata. Empty when typecode was unknown at extract
    /// time. Distinct from `profile`, which is the Doc 29 profile-anchor
    /// name we matched the typecode to.
    pub aircraft_type: String,
    /// ATC callsign / flight number (e.g. "TVS100P"). Empty when the
    /// trace had no callsign metadata.
    pub callsign: String,
    /// ICAO 24-bit transponder address as 6-char lowercase hex (e.g.
    /// "4b1805"). Empty string for synthetic / surface segments. Frontend
    /// uses this for hexdb.io lookup + globe.adsb.lol deep-link.
    pub icao_hex: String,
    /// Unix timestamp (seconds) of flight start. `None` for synthetic
    /// rows where no real timestamp exists.
    pub start_unix: Option<u32>,
    /// True for surface model / cruise bucket synthesised flight IDs;
    /// frontend hides ICAO/time UI in this case.
    pub synthetic: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AircraftAirborneDetail {
    pub periods: NoisePeriods,
    pub observed_flights_per_day: f64,
    pub helicopter_flights_per_day: f64,
    /// Distinct cruise-phase transits (real `flight_id`s) visible at
    /// this receiver per day. Cruise transits contribute to the `faint
    /// / audible / disruptive` band counts (when their `peak_lmax`
    /// crosses 30/45/60 dB) but not to `observed_flights_per_day`
    /// (which is airborne-only).
    ///
    /// v14 caveat: derived from `cruise_flight_stats.len()` which is
    /// populated from each cruise row's top-K `top_candidates`
    /// (K=`CRUISE_TOP_K`=50). Fids that rank outside the top-K cut
    /// in every R7 bucket they crossed are silently UNDERCOUNTED
    /// here — same regression as band_stats (plan §4.4 +
    /// `cruise-ground-top-n-v5.md` §9). At busy LKPR-style hubs the
    /// regression on this number is small (<5% per quick mental
    /// model: tail fids are quiet and rare); at quieter R7s the
    /// undercount approaches zero since per-bucket fid counts are
    /// already below K.
    ///
    /// Notes on overlap: a single real `flight_id` can appear in
    /// both `flights` (airborne sub-segment encounter) and
    /// `cruise_flight_stats` (cruise bucket overhead) at the same
    /// receiver — counted in both `observed_flights_per_day` and
    /// `cruise_transits_per_day` by design, since this counts
    /// per-phase encounters, not distinct flights.
    pub cruise_transits_per_day: f64,
    pub lmax_peak: Option<f64>,
    pub faint: AircraftEventBandStats,
    pub audible: AircraftEventBandStats,
    pub disruptive: AircraftEventBandStats,
    /// Sampling-fragility transparency: the estimator uses finite per-class
    /// archive windows, so one sampled day or flight can dominate at a
    /// receiver. These shares let the UI flag that case (display thresholds
    /// 0.5 / 0.3 live in the frontend).
    /// Shares are of TOTAL aircraft energy (airborne + cruise); 0.0 when
    /// no dated airborne flights contribute.
    pub top_day_energy_share: f64,
    /// ISO date of that loudest sample day ("" when none).
    pub top_day_date: String,
    pub top_flight_energy_share: f64,
    /// Number of archive days behind the Lden average for AIRLINE classes
    /// (`n_days`, the 12-day TTM window).
    pub sample_days: u32,
    /// Number of archive days behind GA + helicopter classes — the
    /// GA window in a hybrid extract, equal to `sample_days` when
    /// non-hybrid. The popup's
    /// "Data" row renders the actual two counts so the sample basis is honest
    /// per class.
    pub ga_sample_days: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_flights: Vec<AircraftTopFlight>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AircraftGroundOpsClassDetail {
    pub periods: NoisePeriods,
    pub observed_movements_per_day: f64,
    pub modeled_movements_per_day: f64,
}

/// One entry of the ground-ops profile-mix display: a noise class,
/// its share in [0, 1] of the airport's linear received energy, and
/// the ICAO typecode of the class's anchor profile (e.g. "B738" for
/// narrowbody jets, "C172" for piston-prop GA).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProfileMixEntry {
    pub class: u8,
    pub share: f64,
    pub rep_typecode: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AircraftGroundOpsDetail {
    pub periods: NoisePeriods,
    pub periods_free: NoisePeriods,
    pub observed_movements_per_day: f64,
    pub modeled_movements_per_day: f64,
    pub distance_m: f64,
    pub emission_db: f64,
    pub received_bands: [f64; NUM_BANDS],
    pub runway_roll: AircraftGroundOpsClassDetail,
    pub taxi: AircraftGroundOpsClassDetail,
    pub apron_movement: AircraftGroundOpsClassDetail,
    /// Top-3 noise classes by linear received energy at this airport
    /// (or globally for the aggregate). Empty when no row carried a
    /// populated `profile_mix`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub profile_mix: Vec<ProfileMixEntry>,
    pub baseline: PropagationBaseline,
    pub terrain: TerrainBreakdown,
    pub screening: ScreeningBreakdown,
    pub vegetation: VegetationBreakdown,
    pub terrain_impact_db: f64,     // A-weighted ΔL_A (≤ 0)
    pub screening_impact_db: f64,   // A-weighted ΔL_A (≤ 0)
    pub vegetation_impact_db: f64,  // A-weighted ΔL_A (≤ 0)
    pub atmospheric_impact_db: f64, // A-weighted ΔL_A (≤ 0)
    pub ground_impact_db: f64,      // A-weighted ΔL_A (SIGNED)
    /// Unique runway-roll arrival ROTATIONS per day. Set union of
    /// `flight_ids` across all rows where `ops_kind = RUNWAY` and
    /// `is_departure = 0`, divided by `n_days`. **Unit is
    /// `flight_id`, which corresponds to ONE rotation (split per
    /// long telemetry gap)** — when a single ICAO24 does ≥2
    /// turnarounds in a cache day, each becomes its own flight_id.
    /// So this count over-states true ICAO Annex-14 movements when
    /// an airframe rotates inside one cache day.
    /// For LKPR/14-day windows the over-count is ~30% (~418/day
    /// observed vs ~280/day published).
    pub arrivals_per_day: f64,
    /// Unique runway-roll departure rotations per day. Same unit and
    /// caveat as [`arrivals_per_day`].
    pub departures_per_day: f64,
    /// GSE (ground support equipment) ROTATIONS per day, indexed by
    /// `noise_compute::emission::gse` class id:
    /// `[LIGHT, MEDIUM, HEAVY]`. Per-class union of `flight_ids`
    /// across rows where `veh_kind = 1`, divided by `n_days`. Always
    /// 0 if the cache has no GSE ADS-B transmitters (most non-
    /// Czech/German airports today — adsb.lol GSE coverage is
    /// concentrated around LKPR / EDDF / EDDM).
    pub gse_per_day: [f64; 3],
}

/// Aircraft detail payload for frontend popup.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AircraftBandData {
    pub airborne: AircraftAirborneDetail,
    pub ground_ops: AircraftGroundOpsDetail,
}

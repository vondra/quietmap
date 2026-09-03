//! Per-flight / per-band accumulators reused across the airborne and
//! cruise scatter passes. Shared so `compute_aircraft_v6` can join them
//! into one `AircraftBandData` without needing to re-walk the row sets.

use crate::emission::aircraft;

/// Per-flight scatter state. `period_energy` carries screened Doc 29 SEL ×
/// count_weight per period; the popup divides by `n_days × period_seconds`
/// at write time. The three companion sums retain the pre-screen and
/// one-effect-removed energies for aircraft's popup variants. Peak fields
/// track the loudest single segment so the popup can render `top_flights` with
/// realistic `Lmax` and CPA distance.
pub struct FlightAccum {
    pub period_energy: [f64; 3],
    pub free_period_energy: [f64; 3],
    pub no_terrain_period_energy: [f64; 3],
    pub no_screening_period_energy: [f64; 3],
    pub peak_lmax: f64,
    pub peak_sel: f64,
    pub min_dist_m: f64,
    pub peak_altitude_m: f64,
    pub peak_period: u8,
    pub peak_date_id: i16,
    pub peak_seg_start: [f64; 2], // [lon, lat]
    pub peak_seg_end: [f64; 2],
    pub profile_idx: u8,
    pub flight_weight: f64,
    /// 4-byte FixedSizeBinary ICAO typecode (ASCII, '\0'-padded). Empty
    /// `[0; 4]` for synth fids without per-flight metadata.
    pub aircraft_type: [u8; 4],
    /// ATC callsign; empty when the trace had no metadata.
    pub callsign: String,
    /// Set when this entry was created from a cruise bucket. `flights_per_day`
    /// and the band counters route cruise via `CruiseFlightStats` instead so
    /// one transit crossing N R7 cells doesn't multi-count.
    pub is_cruise: bool,
}

impl FlightAccum {
    pub fn new(
        profile_idx: u8,
        weight: f64,
        is_cruise: bool,
        aircraft_type: [u8; 4],
        callsign: String,
    ) -> Self {
        Self {
            period_energy: [0.0; 3],
            free_period_energy: [0.0; 3],
            no_terrain_period_energy: [0.0; 3],
            no_screening_period_energy: [0.0; 3],
            peak_lmax: -999.0,
            peak_sel: -999.0,
            min_dist_m: f64::MAX,
            peak_altitude_m: 0.0,
            peak_period: 0,
            peak_date_id: 0,
            peak_seg_start: [0.0; 2],
            peak_seg_end: [0.0; 2],
            profile_idx,
            flight_weight: weight,
            aircraft_type,
            callsign,
            is_cruise,
        }
    }
}

/// Per-band event count + altitude average + per-class histogram. Used
/// for the popup's `faint` / `audible` / `disruptive` band counters
/// (>30 / >45 / >60 dB peak Lmax thresholds).
pub struct BandStats {
    pub count: f64,
    pub alt_sum: f64,
    pub class_counts: [u32; aircraft::NUM_CLASSES],
}

impl Default for BandStats {
    fn default() -> Self {
        Self::new()
    }
}

impl BandStats {
    pub fn new() -> Self {
        Self {
            count: 0.0,
            alt_sum: 0.0,
            class_counts: [0; aircraft::NUM_CLASSES],
        }
    }
    pub fn add_event(&mut self, count_w: f64, alt_w_sum: f64, cls: usize, class_w: u32) {
        self.count += count_w;
        self.alt_sum += alt_w_sum;
        self.class_counts[cls] += class_w;
    }
    pub fn top_type(&self) -> &'static str {
        let idx = self
            .class_counts
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| *c)
            .map(|(i, _)| i)
            .unwrap_or(aircraft::FALLBACK_NOISE_CLASS as usize);
        aircraft::CLASS_NAMES[idx]
    }
}

/// Cruise dedup state — keyed by *real* fid (not the bucket synth fid).
/// One transit crossing many R7 cells writes to many `FlightAccum`s
/// (per-bucket synth fids), but the popup band counters need to count
/// the real flight once. `peak_lmax` tracks the loudest cell encounter.
pub struct CruiseFlightStats {
    pub peak_lmax: f64,
    pub alt_at_peak: f64,
    pub class_at_peak: usize,
}

/// Cruise-side per-real-fid candidate for the unified popup
/// `top_flights` table. Cruise scatter keys energy on the bucket synth
/// fid (see mod.rs), so we can't piggy-back on `FlightAccum` and need
/// a real-fid-keyed mirror.
pub struct TopFlightCandidate {
    pub peak_lmax: f64,
    pub peak_altitude_m: f64,
    pub peak_period: u8,
    pub peak_seg_start: [f64; 2],
    pub peak_seg_end: [f64; 2],
    pub min_dist_m: f64,
    pub profile_idx: u8,
    pub aircraft_type: [u8; 4],
    pub callsign: String,
}

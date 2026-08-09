//! Core in-memory types crossing the stage boundaries.
//!
//! `Flight` (Stage 0 output) carries the raw point list per aircraft.
//! `FlightSegment` (Stage 1 output) carries one classified segment.
//! `AirborneEvent` / `CruiseBucket` are the per-R4 aggregates produced
//! by Stage 2A / 2B and serialised into the per-R4 Arrow files. Stage
//! 2C's own aggregate lives in `stage_2c::airport_traffic` — it writes
//! per-microsegment airport ground ops, not a per-flight path.

use crate::trace::TracePoint;

/// Flight phase classification — from `classify`. Stored as `u8` in
/// segments.arrow (`phase` column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ground = 0,
    Airborne = 1,
    Cruise = 2,
}

impl Phase {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Phase::Ground,
            2 => Phase::Cruise,
            _ => Phase::Airborne,
        }
    }
}

/// Per-segment bitfield flags. Bit 0 = is_departure, bit 1 = on_ground
/// (composite, post-ground-inference), bit 2 = synthetic identity.
pub mod segment_flags {
    pub const IS_DEPARTURE: u8 = 1 << 0;
    pub const ON_GROUND: u8 = 1 << 1;
    pub const SYNTHETIC: u8 = 1 << 2;
}

/// Pack a variable-width ICAO typecode (`"A320"`, `"B738"`, `"PC12"`,
/// `""`) into the canonical 4-byte FixedSizeBinary representation —
/// truncated to 4 chars and zero-padded. The Arrow column type is
/// `FixedSizeBinary(4)` so consumers can read it as `&[u8; 4]` without
/// allocating. Empty / shorter codes round-trip as trailing `\0`.
pub fn typecode_bytes(s: &str) -> [u8; 4] {
    let mut out = [0u8; 4];
    let bytes = s.as_bytes();
    let n = bytes.len().min(4);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// Stage 0 in-memory record. One per rotation — a single
/// `trace_full_<icao>.json` typically yields N `Flight`s, one per
/// sustained on-ground rest (≥ `MIN_TURNAROUND_S`) detected in the
/// trace. `flight_id` is per-leg so popup dedup counts movements
/// rather than aircraft-days; airborne signal dropouts of any
/// duration preserve flight identity, so a transoceanic crossing with
/// a 4 h coverage hole stays one `Flight`.
///
/// `veh_kind` partitions aircraft (`0`) from ground-support equipment
/// (`1`) so Stage 0 routing can fork the two pipelines: aircraft run
/// through the NPD path, GSE through the CNOSSOS-derived emission
/// table in `noise_compute::emission::gse`. Wired by phase:
/// 2.3a adds the struct fields (defaults `(0, 0)` everywhere),
/// 2.3b extends `flights.arrow` / `segments.arrow` schemas so the
/// flag survives Stage 0 → Stage 1 and Stage 1 → Stage 2 disk
/// round-trips, 2.3c activates GND → GSE routing in trace_to_flight.
#[derive(Clone)]
pub struct Flight {
    pub flight_id: u64,
    /// Callsign active at the rotation's first point. Empty when no
    /// point in the trace carried `flight` metadata.
    pub callsign: String,
    pub aircraft_type: String,
    pub profile_idx: u8,
    pub source_id: u8,
    pub origin: u8,
    /// 0 = aircraft (default), 1 = GSE.
    pub veh_kind: u8,
    /// Valid when `veh_kind == 1`: GSE noise class index
    /// (`GSE_CLASS_LIGHT`/`MEDIUM`/`HEAVY` from `noise_compute::emission::gse`).
    /// Ignored otherwise.
    pub gse_class: u8,
    pub points: Vec<TracePoint>,
}

/// Stage 1 in-memory record. One per classified flight segment.
///
/// `veh_kind`/`gse_class` mirror the `Flight` fields and propagate
/// down so Stage 2A/2B/2C writers can route GSE segments off the
/// aircraft NPD path. See `Flight` for the contract.
///
/// `start_elev_m` / `end_elev_m` are sampled at the segment endpoints
/// during Stage 1 (Opt A v15): the per-point AGL loop already calls
/// `rasters.elevation` so emitting `elev = alt_msl - agl` adds zero
/// extra raster I/O. Stage 2A propagates these into airborne
/// sub-segments so the popup kernel can skip `SegmentTerrain::sample`
/// on the hot path. For ground-flagged endpoints the popup ignores
/// the elev value (it gates on the `ON_GROUND` flag earlier).
#[derive(Clone)]
pub struct FlightSegment {
    pub flight_id: u64,
    pub callsign: String,
    pub aircraft_type: [u8; 4],
    pub profile_idx: u8,
    pub source_id: u8,
    pub origin: u8,
    pub veh_kind: u8,
    pub gse_class: u8,
    pub period: u8,
    pub date_id: i16,
    pub phase: Phase,
    pub flags: u8,
    pub start_lat: f32,
    pub start_lon: f32,
    pub start_alt_m: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    pub end_alt_m: f32,
    pub speed_kt: f32,
    pub length_m: f32,
    pub agl_avg_m: f32,
    pub start_elev_m: f32,
    pub end_elev_m: f32,
}

impl FlightSegment {
    pub fn is_airborne(&self) -> bool {
        self.phase != Phase::Ground
    }
    /// Phase-aware departure signal. For Airborne segments it's
    /// Doc 29 §A.3.2 climb classification (positive ROCD, or shallow
    /// descent at FL100+). For Ground segments it's smoothed speed
    /// acceleration above `GROUND_DEPARTURE_ACCEL_KT_PER_MIN` — a
    /// candidate takeoff-roll signal that DOES NOT mean "takeoff
    /// thrust": consumers applying a takeoff-specific acoustic bonus
    /// must additionally check `ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL`
    /// (airport_traffic_writer enforces this for the +2 dB bonus
    /// gate). Bit is `false` for arrivals, steady taxi, and landing
    /// rollouts.
    pub fn is_departure(&self) -> bool {
        self.flags & segment_flags::IS_DEPARTURE != 0
    }
    pub fn is_on_ground(&self) -> bool {
        self.flags & segment_flags::ON_GROUND != 0
    }
}

/// Stage 2A row — one per (flight, R4) crossing. The vector of sub
/// segments is what gets serialised as a `List<Struct>` in the airborne
/// arrow file.
#[derive(Clone)]
pub struct AirborneEvent {
    pub flight_id: u64,
    pub callsign: String,
    pub aircraft_type: [u8; 4],
    pub profile_idx: u8,
    pub source_id: u8,
    pub origin: u8,
    pub sub_segments: Vec<AirborneSubSegment>,
    pub total_length_m: f32,
    pub bbox_min_lat: f32,
    pub bbox_max_lat: f32,
    pub bbox_min_lon: f32,
    pub bbox_max_lon: f32,
}

/// One sub-segment inside an [`AirborneEvent`]. Period and date are
/// stored per sub-segment so a long airborne crossing that spans the
/// 19:00 evening boundary still gets the correct Lden weighting.
///
/// `terrain_*_elev_m` are pre-sampled at extract time (Opt A v15):
/// start/end propagate from Stage 1's per-point elevation; q1, mid,
/// q3 are sampled at Stage 2A from the sub-segment's 0.25 / 0.5 / 0.75
/// points. The popup terrain gates (`is_ground_stale_with_terrain`,
/// `is_valid_airborne_with_terrain`, `segment_sel_with_terrain`) read
/// these directly instead of
/// calling `SegmentTerrain::sample` (5 raster lookups per sub-segment).
/// At LKPR popup this saves ~1 M raster mutex acquisitions.
///
/// All three intermediate elevations are stored: real DEM isn't
/// linearly interpolated between endpoints, so in mountain terrain
/// (LOWI / SEQM / SLLP / KASE) a sharp peak between two ADS-B samples
/// can sit tens of meters above any linear estimate. /gg rev 2 (3 of
/// 4 reviewers) caught that storing only mid lets a narrow spike at
/// frac=0.25 or 0.75 sneak past the AGL gate.
#[derive(Clone)]
pub struct AirborneSubSegment {
    pub start_lat: f32,
    pub start_lon: f32,
    pub start_alt_m: f32,
    pub end_lat: f32,
    pub end_lon: f32,
    pub end_alt_m: f32,
    pub speed_kt: f32,
    pub length_m: f32,
    pub period: u8,
    pub date_id: i16,
    pub flags: u8,
    pub terrain_start_elev_m: f32,
    pub terrain_end_elev_m: f32,
}

/// One entry in a cruise row's `top_candidates` list (v14).
/// Identity + ranking dimension only — row-constant fields
/// (period / date_id) are NOT duplicated per candidate per
/// Codex W4 + Claude C3.
#[derive(Clone, Debug, PartialEq)]
pub struct CruiseTopCandidate {
    pub flight_id: u64,
    pub callsign: String,
    pub aircraft_type: [u8; 4],
    /// Source-side NPD `lookup_lmax` at the 25 m anchor for the
    /// loudest segment this fid contributed to the bucket. Ranking
    /// dimension matches popup's peak Lmax display ranking.
    pub peak_lmax_25m_db: f32,
    /// Altitude at the peak segment (needed by popup's CPA + slant
    /// recompute against the receiver).
    pub altitude_m: f32,
}

/// Stage 2B row — per (R7, fl_bin, class, period) bucket
/// (v14: bounded top-K candidates + scalar unique_count replace the
/// per-fid lists from v13. v16 drops `flags`, which was tautologically
/// always IS_DEPARTURE per Doc 29 §A.3.2.)
#[derive(Clone)]
pub struct CruiseBucket {
    pub r7_hex: u64,
    pub class: u8,
    pub rep_profile_idx: u8,
    pub fl_bin: u8,
    pub period: u8,
    pub sum_length_m: f32,
    pub rep_len_m: f32,
    pub rep_alt_m: f32,
    pub rep_speed_kt: f32,
    /// Distinct real fids that touched this bucket. Display-only.
    pub unique_count: u32,
    /// Bounded top-K (K=`arrow_schemas::CRUISE_TOP_K`) ranked by
    /// `peak_lmax_25m_db` descending. Tail fids below the cut drop
    /// out of band counters; documented regression vs v13.
    pub top_candidates: Vec<CruiseTopCandidate>,
    pub source_id: u8,
    pub origin: u8,
}

/// FL bins — five buckets covering the cruise altitude range. Tracks
/// the plan's documented FL250 / FL280 / FL310 / FL350 / FL390+ split.
pub fn fl_bin_of(alt_m: f32) -> u8 {
    let alt_ft = alt_m / 0.3048;
    match alt_ft {
        f if f < 26_500.0 => 0, // FL250
        f if f < 29_500.0 => 1, // FL280
        f if f < 32_500.0 => 2, // FL310
        f if f < 37_000.0 => 3, // FL350
        _ => 4,                 // FL390+
    }
}

/// Source registry positions. Adding a driver here keeps `source_id`
/// values stable across past extracts.
pub mod source_id {
    pub const ADSB_LOL_TAR: u8 = 0;
    pub const SCHEDULE_SYNTH: u8 = 1;
    /// adsbexchange free-sample feed. Identical readsb `trace_full` format to
    /// adsb.lol — only the provenance differs (denser receiver network).
    pub const ADSB_EXCHANGE: u8 = 2;
}

/// Origin tag — always 0 for now. Reserved for the schedule-synth and
/// interpolation drivers documented in the plan.
pub mod origin {
    pub const OBSERVED: u8 = 0;
    pub const INTERPOLATED: u8 = 1;
    pub const SCHEDULE_MODELED: u8 = 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fl_bin_boundaries() {
        assert_eq!(fl_bin_of(7_620.0), 0); // FL250
        assert_eq!(fl_bin_of(8_535.0), 1); // FL280
        assert_eq!(fl_bin_of(9_450.0), 2); // FL310
        assert_eq!(fl_bin_of(10_670.0), 3); // FL350
        assert_eq!(fl_bin_of(12_500.0), 4); // FL410
    }

    #[test]
    fn phase_round_trip() {
        for phase in [Phase::Ground, Phase::Airborne, Phase::Cruise] {
            assert_eq!(Phase::from_u8(phase.as_u8()), phase);
        }
    }
}

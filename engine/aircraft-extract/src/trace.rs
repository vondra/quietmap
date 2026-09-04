//! ADS-B trace parser and point types; archive integrity and type probing are separate modules.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::io::Read;

mod archive;
mod typecode_probe;
pub use archive::{read_day_traces, read_day_traces_filtered, TypecodeProbeStats};

/// Trace-point bit 0 — `on_ground` set by the adsb.lol bitfield.
pub const FLAG_ON_GROUND_RAW: u8 = 1 << 0;
/// Trace-point bit 1 — altitude column was the literal string `"ground"`.
pub const FLAG_ALT_IS_GROUND: u8 = 1 << 1;

/// One ADS-B point. `flags` packs the two ground-related raw signals so
/// the v6 Arrow schema can carry them as a single byte without losing
/// the distinction the composite ground inference relies on.
#[derive(Clone, Debug)]
pub struct TracePoint {
    pub timestamp: f64,
    pub lat: f32,
    pub lon: f32,
    /// Barometric altitude in feet. `NaN` when [`FLAG_ALT_IS_GROUND`] is
    /// set; read via [`TracePoint::airborne_alt_ft`] to keep ground
    /// sentinels out of arithmetic.
    pub alt_ft: f32,
    pub speed_kt: f32,
    pub track_deg: f32,
    pub baro_rate_fpm: f32,
    pub flags: u8,
}

/// Inline callsign transition: at trace `point_idx`, the callsign became
/// `value`. Most traces have 1–4 transitions per day (single flight or
/// rotation through 2–3 schedules). Stored on the trace, not per-point,
/// to avoid the 24-byte Option<String> on the hot 1.6M-point Vec.
#[derive(Clone, Debug)]
pub struct CallsignChange {
    pub point_idx: usize,
    pub value: String,
}

impl TracePoint {
    pub fn alt_is_ground(&self) -> bool {
        self.flags & FLAG_ALT_IS_GROUND != 0
    }
    pub fn on_ground_raw(&self) -> bool {
        self.flags & FLAG_ON_GROUND_RAW != 0
    }
    /// `Some(alt_ft)` for airborne points, `None` for `alt_is_ground`
    /// sentinel rows whose `alt_ft` is `NaN`. Funnels every alt-arithmetic
    /// site through one flag-aware accessor so a missed branch can't
    /// silently propagate NaN into AGL / ROCD / teleport arithmetic.
    pub fn airborne_alt_ft(&self) -> Option<f32> {
        if self.alt_is_ground() {
            None
        } else {
            Some(self.alt_ft)
        }
    }
}

/// All trace points for one aircraft on one day.
pub struct AircraftTrace {
    pub icao24: String,
    pub aircraft_type: String,
    pub points: Vec<TracePoint>,
    /// Callsign transitions in raw-trace `point_idx` order. The Stage 0
    /// driver (`source_adsb_tar::trace_to_flight`) rebases these onto
    /// post-`point_is_sane` indices and reduces them to one scalar
    /// callsign per emitted [`Flight`].
    pub callsigns: Vec<CallsignChange>,
}

/// Parse one gzipped `trace_full_*.json` from a TAR entry. Returns
/// `None` only for a valid trace with fewer than two points. Malformed input fails.
pub fn parse_trace<R: Read>(reader: R) -> Result<Option<AircraftTrace>> {
    let mut json_bytes = Vec::new();
    GzDecoder::new(reader).read_to_end(&mut json_bytes)?;
    let val: serde_json::Value = serde_json::from_slice(&json_bytes)?;

    let icao24 = val
        .get("icao")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let aircraft_type = val
        .get("t")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let base_timestamp = val
        .get("timestamp")
        .and_then(|v| v.as_f64())
        .context("trace is missing its base timestamp")?;

    let trace_arr = match val.get("trace").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => anyhow::bail!("trace JSON is missing the trace array"),
    };

    let mut points = Vec::with_capacity(trace_arr.len());
    let mut callsigns: Vec<CallsignChange> = Vec::new();
    for entry in trace_arr {
        let arr = match entry.as_array() {
            Some(a) if a.len() >= 7 => a,
            _ => continue,
        };
        let ts_offset = arr[0].as_f64().unwrap_or(f64::NAN);
        let lat = arr[1].as_f64().unwrap_or(f64::NAN) as f32;
        let lon = arr[2].as_f64().unwrap_or(f64::NAN) as f32;
        let (alt_ft, alt_is_ground) = parse_altitude_ft(&arr[3]);
        let speed_kt = arr[4].as_f64().unwrap_or(0.0) as f32;
        let track_deg = arr[5].as_f64().unwrap_or(0.0) as f32;
        let on_ground_bit = arr[6].as_i64().unwrap_or(0);
        let baro_rate_fpm = arr.get(7).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

        // Compare on `&str` before allocating — adsb.lol re-emits the
        // meta block on every position, so most points produce a
        // duplicate that would otherwise allocate a String just to be
        // dropped.
        if let Some(raw) = arr
            .get(8)
            .and_then(|v| v.get("flight"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if callsigns.last().map(|c| c.value.as_str()) != Some(raw) {
                callsigns.push(CallsignChange {
                    point_idx: points.len(),
                    value: raw.to_string(),
                });
            }
        }

        let mut flags = 0u8;
        if on_ground_bit & 1 != 0 {
            flags |= FLAG_ON_GROUND_RAW;
        }
        if alt_is_ground {
            flags |= FLAG_ALT_IS_GROUND;
            // adsb.lol semantics: "alt is ground" implies on_ground.
            flags |= FLAG_ON_GROUND_RAW;
        }
        points.push(TracePoint {
            timestamp: base_timestamp + ts_offset,
            lat,
            lon,
            alt_ft,
            speed_kt,
            track_deg,
            baro_rate_fpm,
            flags,
        });
    }
    if points.len() < 2 {
        return Ok(None);
    }
    Ok(Some(AircraftTrace {
        icao24,
        aircraft_type,
        points,
        callsigns,
    }))
}

/// `"ground"` string maps to `(NaN, true)` — not `(0.0, true)` — so a
/// sub-sea-level aerodrome (Schiphol −3 m, Atyrau −22 m) can't collide
/// with the on-surface marker, and a missed flag check downstream
/// surfaces as NaN rather than as a silent underground truncation.
fn parse_altitude_ft(value: &serde_json::Value) -> (f32, bool) {
    if let Some(alt_ft) = value.as_f64() {
        return (alt_ft as f32, false);
    }
    if value
        .as_str()
        .map(|s| s.eq_ignore_ascii_case("ground"))
        .unwrap_or(false)
    {
        return (f32::NAN, true);
    }
    (f32::NAN, false)
}

#[cfg(test)]
mod tests;

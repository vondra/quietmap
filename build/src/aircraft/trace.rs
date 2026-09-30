//! One aircraft-day of ADS-B: its samples, callsign changes, type and address identity.

/// A surface report: the altitude field was the string "ground", readsb's only on-ground signal.
/// readsb's flag bit 0 marks a gap of 20 s or more without positions before a point (PLAN-z13:
/// 906,924 airborne points had been read as ground from it), so it is never read as ground.
pub const SURFACE_REPORT: u8 = 1 << 0;
/// A secondary-provider sample kept by the provider union because the primary missed that instant.
pub const SECONDARY_PROVIDER: u8 = 1 << 1;

/// One ADS-B sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracePoint {
    pub timestamp: f64,
    pub lat: f32,
    pub lon: f32,
    /// Barometric (pressure) altitude in feet; NaN on a surface report or when not reported.
    pub altitude_ft: f32,
    /// Geometric (GNSS) altitude in feet as readsb reports it; NaN when absent or on the ground.
    pub geometric_altitude_ft: f32,
    pub ground_speed_kt: f32,
    pub track_deg: f32,
    pub vertical_rate_fpm: f32,
    pub flags: u8,
}

impl TracePoint {
    pub fn is_surface_report(&self) -> bool {
        self.flags & SURFACE_REPORT != 0
    }

    pub fn is_secondary(&self) -> bool {
        self.flags & SECONDARY_PROVIDER != 0
    }

    /// The barometric altitude of an airborne sample; `None` on a surface report, so no NaN
    /// sentinel reaches altitude arithmetic.
    pub fn airborne_altitude_ft(&self) -> Option<f32> {
        (!self.is_surface_report()).then_some(self.altitude_ft)
    }
}

/// At `point_index` the callsign became `callsign`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallsignChange {
    pub point_index: usize,
    pub callsign: String,
}

/// All samples of one aircraft address on one day.
#[derive(Clone, Debug)]
pub struct AircraftTrace {
    /// As in the archive: six hex digits, `~` first for a non-ICAO (TIS-B, anonymous) address.
    pub address: String,
    pub aircraft_type: String,
    /// ADS-B emitter category as a hex-like byte (`A3` is 0xA3); 0 when never reported.
    pub emitter_category: u8,
    pub points: Vec<TracePoint>,
    pub callsigns: Vec<CallsignChange>,
}

impl AircraftTrace {
    /// Keep the points `keep` accepts. A callsign change on a dropped point moves to the next kept
    /// point (the last one wins) and equal neighbours collapse.
    pub fn retain_points(&mut self, mut keep: impl FnMut(usize, &TracePoint) -> bool) {
        let mut surviving = Vec::with_capacity(self.points.len());
        let mut points = Vec::with_capacity(self.points.len());
        for (index, point) in std::mem::take(&mut self.points).into_iter().enumerate() {
            if keep(index, &point) {
                surviving.push(index);
                points.push(point);
            }
        }
        let mut callsigns: Vec<CallsignChange> = Vec::with_capacity(self.callsigns.len());
        for change in std::mem::take(&mut self.callsigns) {
            let point_index = surviving.partition_point(|&index| index < change.point_index);
            if point_index >= surviving.len() {
                continue;
            }
            match callsigns
                .last_mut()
                .filter(|c| c.point_index == point_index)
            {
                Some(last) => last.callsign = change.callsign,
                None => callsigns.push(CallsignChange {
                    point_index,
                    callsign: change.callsign,
                }),
            }
        }
        callsigns.dedup_by(|later, earlier| later.callsign == earlier.callsign);
        self.points = points;
        self.callsigns = callsigns;
    }
}

/// The identity of an address for grouping: (non-ICAO `~`, 24-bit value). Empty, invalid and the
/// reserved ICAO values 0 and 0xFFFFFF identify no single aircraft.
pub fn address_identity(address: &str) -> Option<(bool, u32)> {
    let anonymous = address.strip_prefix('~');
    let value = parse_address_hex(anonymous.unwrap_or(address))?;
    if anonymous.is_none() && matches!(value, 0 | 0xff_ffff) {
        return None;
    }
    Some((anonymous.is_some(), value))
}

/// 1-6 hex digits as a 24-bit address.
pub fn parse_address_hex(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 6 {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

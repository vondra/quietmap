//! Airport ground operations (dev4 `ground_ops.rs`, `airport_traffic.rs` and `gse.rs`): the sound
//! energy one aircraft or ground vehicle leaves per metre of the runway or taxiway it moves along,
//! per octave band. The builder sums it over a window's movements into sound power per metre, which
//! the popup propagates as any other line source.

use crate::bands::BANDS;
use crate::bound::POINT_DIVERGENCE_OFFSET_DB;
use crate::doc29::profiles_generated::NUM_CLASSES;

/// What a movement does on an aeroway line: dev4 takes runway, stopway and airstrip lines as
/// runway roll and taxiway lines as taxiing (aprons are areas, which its ground traffic never met).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundOperation {
    RunwayRoll,
    Taxi,
}

/// Runway roll per noise class as dev4's 1 km event SEL anchor (dB): flyover NPDs overstate the
/// roll by 6-10 dB, so the bands are set per anchor type: narrowbody jet 104, widebody 108,
/// regional jet 100, business jet 99, turboprop 97, helicopter 94, piston single 92.
const RUNWAY_ROLL_EVENT_SEL_DB: [(&str, f64); NUM_CLASSES] = [
    ("WING_FALLBACK", 104.0),
    ("WING_A320", 104.0),
    ("WING_B738", 104.0),
    ("PROP_C172", 92.0),
    ("WING_B38M", 104.0),
    ("WING_B789", 108.0),
    ("WING_A21N", 104.0),
    ("WING_A321", 104.0),
    ("WING_A20N", 104.0),
    ("WING_A319", 104.0),
    ("FUSE_CRJ9", 100.0),
    ("WING_B748", 108.0),
    ("HELICOPTER", 94.0),
    ("PROP_DH8D", 97.0),
    ("FUSE_C56X", 99.0),
    ("PROP_AT72", 97.0),
];
/// dev4's per-metre level of an anchor, 10 lg(25/pi): its receiver formula LW' + 10 lg(theta/d)
/// then reads the anchor - 0.14 dB at 25 m from the middle of a 1 km roll.
const EVENT_SEL_TO_PER_METRE_DB: f64 = 9.01;
/// Taxiing sits 12 dB below the runway roll.
const TAXI_BELOW_RUNWAY_DB: f64 = 12.0;
/// A departure's roll runs at take-off power.
const RUNWAY_DEPARTURE_BONUS_DB: f64 = 2.0;
/// The dwell correction -10 lg(v / v_ref) (Doc 29 Eq. 4-14: energy per metre goes as 1/v) stays
/// within +-3 dB, so outlying ADS-B speeds do not dominate; dev4 applies it above 1 kt.
const SPEED_CORRECTION_LIMIT_DB: f64 = 3.0;
const SPEED_CORRECTION_MIN_KT: f64 = 1.0;
const METRES_PER_SECOND_PER_KT: f64 = 1852.0 / 3600.0;

impl GroundOperation {
    pub const fn name(self) -> &'static str {
        match self {
            GroundOperation::RunwayRoll => "runway_roll",
            GroundOperation::Taxi => "taxi",
        }
    }

    /// The speed the anchors hold (kt).
    const fn reference_speed_kt(self) -> f64 {
        match self {
            GroundOperation::RunwayRoll => 70.0,
            GroundOperation::Taxi => 18.0,
        }
    }

    /// Relative band levels (dB, Z-weighted).
    const fn spectrum_db(self) -> [f64; BANDS] {
        match self {
            GroundOperation::RunwayRoll => [17.0, 14.0, 11.0, 8.0, 5.0, 2.0, -1.0, -5.0],
            GroundOperation::Taxi => [14.0, 11.0, 8.0, 5.0, 2.0, 0.0, -3.0, -7.0],
        }
    }
}

/// Band levels of `spectrum_db` whose Z-weighted sum is `total_db`.
fn bands_of_total(total_db: f64, spectrum_db: &[f64; BANDS]) -> [f64; BANDS] {
    let shape_db = 10.0
        * spectrum_db
            .iter()
            .map(|level| 10f64.powf(level / 10.0))
            .sum::<f64>()
            .log10();
    spectrum_db.map(|relative| total_db - shape_db + relative)
}

/// Sound energy per metre (dB re 1 pW s per metre, per band, Z-weighted) of one aircraft of noise
/// class `class` on `operation` at `speed_kt`; `None` when it does not move. dev4's per-metre
/// level lacks the 10 lg 4 pi of point divergence its receiver formula leaves out and the CNOSSOS
/// point sum divides by (10^1.1): the same level at the receiver needs 11 dB more here.
pub fn aircraft_pass_energy_db(
    class: u8,
    operation: GroundOperation,
    departure: bool,
    speed_kt: f64,
) -> Option<[f64; BANDS]> {
    if speed_kt.is_nan() || speed_kt <= 0.0 {
        return None;
    }
    let (_, runway_sel_db) = RUNWAY_ROLL_EVENT_SEL_DB[usize::from(class)];
    let mut total = runway_sel_db + EVENT_SEL_TO_PER_METRE_DB + POINT_DIVERGENCE_OFFSET_DB;
    match operation {
        GroundOperation::RunwayRoll if departure => total += RUNWAY_DEPARTURE_BONUS_DB,
        GroundOperation::RunwayRoll => {}
        GroundOperation::Taxi => total -= TAXI_BELOW_RUNWAY_DB,
    }
    if speed_kt > SPEED_CORRECTION_MIN_KT {
        total += (-10.0 * (speed_kt / operation.reference_speed_kt()).log10())
            .clamp(-SPEED_CORRECTION_LIMIT_DB, SPEED_CORRECTION_LIMIT_DB);
    }
    Some(bands_of_total(total, &operation.spectrum_db()))
}

/// Airport ground vehicles by dev4's classes (`gse_class` 0-2 of the segments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundVehicle {
    /// Follow-me cars, meteorology, bird control.
    Light,
    /// Vans, fuel carts, maintenance.
    Medium,
    /// Pushback tractors, fire trucks, sweepers.
    Heavy,
}

impl GroundVehicle {
    pub fn from_code(code: u8) -> Option<Self> {
        [
            GroundVehicle::Light,
            GroundVehicle::Medium,
            GroundVehicle::Heavy,
        ]
        .get(usize::from(code))
        .copied()
    }

    /// Sound power per band (dB): CNOSSOS-EU categories 1, 2 and 3 at 20 km/h, the law's lowest
    /// validated speed (89, 100 and 103 dB(A)).
    const fn sound_power_db(self) -> [f64; BANDS] {
        match self {
            GroundVehicle::Light => [98.9, 87.6, 85.4, 83.4, 84.1, 83.5, 78.9, 71.4],
            GroundVehicle::Medium => [106.9, 96.9, 96.0, 95.0, 96.7, 93.3, 86.6, 80.4],
            GroundVehicle::Heavy => [108.8, 102.1, 100.2, 99.9, 99.4, 95.1, 90.4, 84.1],
        }
    }
}

/// Sound energy per metre (dB re 1 pW s per metre, per band) of one ground vehicle passing at
/// `speed_kt`: sound power W at speed v leaves W / v on every metre (dev4's moving-point integral
/// at 25 m is this line read through the point divergence); `None` when it does not move.
pub fn vehicle_pass_energy_db(vehicle: GroundVehicle, speed_kt: f64) -> Option<[f64; BANDS]> {
    if speed_kt.is_nan() || speed_kt <= 0.0 {
        return None;
    }
    let dwell_db = -10.0 * (speed_kt * METRES_PER_SECOND_PER_KT).log10();
    Some(vehicle.sound_power_db().map(|level| level + dwell_db))
}

#[cfg(test)]
#[path = "airport_tests.rs"]
mod tests;

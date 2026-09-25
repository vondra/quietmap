//! Ground-operations surface constants: per-kind reference speeds, spectrum shapes and per-class runway bands for the `airport_traffic` kernel.

use crate::emission::profiles_generated::{CLASS_NAMES, NUM_CLASSES};
use crate::types::NUM_BANDS;

pub(crate) const SURFACE_RUNWAY_SPEED_KT: f32 = 70.0;
pub(crate) const SURFACE_TAXIWAY_SPEED_KT: f32 = 18.0;
pub(crate) const SURFACE_APRON_SPEED_KT: f32 = 12.0;
pub(crate) const GROUND_OPS_REF_OFFSET_M: f64 = 25.0;
pub(crate) const GROUND_OPS_SPEED_CLAMP_DB: f64 = 3.0;
pub(crate) const GROUND_OPS_RUNWAY_DEPARTURE_BONUS_DB: f64 = 2.0;
pub(crate) const GROUND_OPS_RUNWAY_SPECTRUM_SHAPE: [f64; NUM_BANDS] =
    [17.0, 14.0, 11.0, 8.0, 5.0, 2.0, -1.0, -5.0];
pub(crate) const GROUND_OPS_TAXI_SPECTRUM_SHAPE: [f64; NUM_BANDS] =
    [14.0, 11.0, 8.0, 5.0, 2.0, 0.0, -3.0, -7.0];
pub(crate) const GROUND_OPS_APRON_SPECTRUM_SHAPE: [f64; NUM_BANDS] =
    [12.0, 9.0, 6.0, 3.0, 1.0, -1.0, -4.0, -8.0];


/// Runway-roll level per noise class as the legacy 1 km event SEL anchor (dB),
/// keyed by class name so that a class without its own band fails the build
/// instead of inheriting a flyover NPD value. Flyover `dep@200ft` overstates
/// runway roll by 6–10 dB (no ground absorption, no engine baffling), so the
/// bands are set per anchor type: narrowbody jet 104, widebody 108, regional
/// jet 100, business jet 99, turboprop 97, helicopter 94, piston single 92.
/// WING_A321 and WING_A20N are narrowbody jets like WING_A21N.
const RUNWAY_ROLL_1KM_EVENT_SEL_DB_BY_CLASS: [(&str, f64); NUM_CLASSES] = [
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
];

/// `10·log10(25/π)`: a 1 km event SEL anchor becomes a per-metre `LW'` whose
/// 1 km / 25 m midpoint receiver sees the anchor − 0.14 dB (CNOSSOS-EU §2.5.5).
const EVENT_SEL_TO_LW_PER_METER_DB: f64 = 9.01;
/// Taxi and apron sit 12 dB and 18 dB below runway roll.
const TAXI_BELOW_RUNWAY_DB: f64 = 12.0;
const APRON_BELOW_RUNWAY_DB: f64 = 18.0;

/// Runway-roll, taxi, apron per-metre `LW'` (dB re 1 pW/m) per noise class.
pub(crate) static GROUND_OPS_REFERENCE_LW_PER_METER_DB: [[f64; 3]; NUM_CLASSES] =
    ground_ops_reference_lw_per_meter_db();

const fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn ground_ops_reference_lw_per_meter_db() -> [[f64; 3]; NUM_CLASSES] {
    let mut table = [[0.0; 3]; NUM_CLASSES];
    let mut class = 0;
    while class < NUM_CLASSES {
        let (name, runway_sel_db) = RUNWAY_ROLL_1KM_EVENT_SEL_DB_BY_CLASS[class];
        assert!(
            same_name(name, CLASS_NAMES[class]),
            "runway band table must list every noise class in CLASS_NAMES order"
        );
        let runway = runway_sel_db + EVENT_SEL_TO_LW_PER_METER_DB;
        table[class] = [
            runway,
            runway - TAXI_BELOW_RUNWAY_DB,
            runway - APRON_BELOW_RUNWAY_DB,
        ];
        class += 1;
    }
    table
}

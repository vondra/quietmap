//! EASA-certified helicopter levels (PLAN-z13 KEEP) read per type designator (its Stage 1 FIX):
//! every helicopter shares the helicopter class's NPD shape, shifted so it reads its designator's
//! certified SEL at 150 m in level flight, climb and descent.
//!
//! Source: EASA Certification Noise Levels - Helicopters, Issue 52 (26 June 2026; reproduction
//! authorised provided the source is acknowledged), by the method of `HeliLevels` in dev4's
//! `scripts/build-aircraft-thrust.py` (6c6114ea): the level is the Chapter 11 SEL energy mean of
//! the designator's representative records, else their Chapter 8 overflight EPNL energy mean less
//! 2.65 dB (the median EPNL - SEL of same-model, same-engine pairs); the climb and descent uplifts
//! are the medians of their Chapter 8 takeoff and approach minus overflight levels, else the
//! database medians 1.3 and 4.1 dB. The first twenty rows are dev4's. The FIX adds every
//! designator dev4 read at the EC135 level that has its own EASA record, the H-series names of
//! ICAO designators, and mass classes for the rest; its rows were computed by a replica of that
//! method on the same workbook, and the generator port (a later step) must reproduce them.

use std::sync::LazyLock;

use super::npd::{is_helicopter_class, read_npd};
use super::profiles_generated::NUM_CLASSES;
use super::thrust::PowerBracket;

/// Certified levels of one helicopter type at 150 m.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelicopterLevels {
    /// Level-flight SEL at 150 m (dB).
    pub level_sel_150m_db: f64,
    /// Climb (Chapter 8 takeoff) minus level (dB).
    pub climb_uplift_db: f64,
    /// Descent (Chapter 8 approach, blade-vortex interaction) minus level (dB).
    pub descent_uplift_db: f64,
}

const fn certified(level: f64, climb: f64, descent: f64) -> HelicopterLevels {
    HelicopterLevels {
        level_sel_150m_db: level,
        climb_uplift_db: climb,
        descent_uplift_db: descent,
    }
}

/// Designators with their own EASA levels. Comments: the representative EASA models, the basis
/// (Ch11 SEL, or Ch8 overflight EPNL less 2.65 dB), the records averaged, the largest MTOM (kg).
const CERTIFIED: [(&[&str], HelicopterLevels); 36] = [
    (&["A109"], certified(87.63837754510874, 0.4, 2.0)), // A109E A109S AW109SP, Ch8, 5, 3,175 kg
    (&["AS50", "H125"], certified(84.57132811597987, 2.3, 3.9)), // AS 350 B2 B3, Ch11, 6, 2,370
    (&["AS55"], certified(84.5704054541693, 2.0, 6.1)),  // AS 355 F2 N NP, Ch8, 5, 2,600
    (&["AS65"], certified(88.14367145095889, 2.5, 5.6)), // AS 365 N2 N3, Ch8, 5, 4,300
    (&["B06"], certified(84.0005116840498, 3.2, 5.2)),   // 206B 206L-4, Ch11, 2, 2,018
    (&["B407"], certified(85.07327207584672, 1.3, 4.1)), // 407, Ch11, 6, 2,381
    (&["B412"], certified(90.71729974737926, -0.6, 2.2)), // 412 412EP, Ch8, 30, 5,534
    (&["B505"], certified(81.65, 2.6, 5.4)),             // 505, Ch8, 1, 1,669
    (&["BK17"], certified(88.16546137569284, -0.4, 5.0)), // BK117 B-2 C-1, Ch8, 5, 3,350
    (&["EC20"], certified(78.7, 1.3, 4.1)),              // EC 120 B, Ch11, 3, 1,715
    (&["EC30", "H130"], certified(81.1, 1.3, 6.3)),      // EC 130 B4 T2, Ch11, 1, 2,500
    (&["EC35", "H135"], certified(80.57797082533364, 3.4, 8.7)), // EC135 T/P 1-3, Ch11, 7, 3,100
    (&["EC45", "H145"], certified(83.19804640828247, 1.8, 5.6)), // BK117 C-2 D-2 D-3, Ch8, 6, 3,800
    (&["EC55", "H155"], certified(86.25, 3.3, 6.8)),     // EC 155 B1, Ch8, 2, 4,920
    (&["H500"], certified(83.70617696799351, 1.35, 2.95)), // 369D 369E, Ch8, 2, 1,360
    (&["MD52"], certified(77.78788730291393, 5.4, 7.7)), // 500N, Ch8, 3, 1,520
    (&["R22"], certified(77.4, 1.3, 4.1)),               // R22 Beta, Ch11, 1, 621
    (&["R44"], certified(80.20574748607798, 1.3, 4.1)),  // R44 R44 II, Ch11, 8, 1,134
    (&["R66"], certified(81.88036651050578, 2.75, 3.55)), // R66, Ch8, 4, 1,225
    (&["S76"], certified(88.96823808273533, 2.2, 4.4)),  // S-76C S-76D, Ch8, 4, 5,386
    (&["A119"], certified(86.5, 2.6, 2.8)), // A119 (Ch11, 1) AW119 MKII (uplifts), 2,850
    (&["A139"], certified(87.77022310324253, -0.4, 3.4)), // AW139, Ch8, 6, 7,000
    (&["A169"], certified(85.94678698842503, 1.6, 8.1)), // AW169, Ch8, 7, 4,800
    (&["A189"], certified(92.45228461396019, -3.9, 3.9)), // AW189, Ch8, 3, 8,600
    (&["AS32", "H215"], certified(90.82579396560632, 0.95, 2.65)), // AS 332 L2, Ch8, 4, 9,300
    (&["B427"], certified(86.38358982687998, -0.5, 2.2)), // 427, Ch8, 3, 2,971
    (&["B429"], certified(86.95, -0.7, 1.8)), // 429, Ch8, 2, 3,175
    (&["B430"], certified(88.95, 0.8, 2.2)), // 430, Ch8, 1, 4,218
    (&["B47G"], certified(82.0, 1.3, 4.1)), // 47G-2 47G-4, Ch11, 2, 1,338
    (&["EC25", "H225"], certified(90.85, 2.1, 5.4)), // EC 225 LP, Ch8, 3, 11,160
    (&["EC75", "H175"], certified(88.35, -1.0, 4.1)), // EC 175 B, Ch8, 2, 7,800
    (&["G2CA"], certified(75.7, 1.3, 4.1)), // Cabri G2, Ch11, 1, 700
    (&["H160"], certified(85.95, 1.3, 2.4)), // H160-B, Ch8, 1, 6,050
    (&["H269"], certified(79.5121013790426, 1.3, 4.1)), // 269C-1, Ch11, 5, 794
    (&["S92"], certified(94.55, -2.6, 0.3)), // S-92A, Ch8, 4, 12,565
    (&["W3"], certified(89.75, 1.4, 3.4)),  // PZL W-3A W-3AS, Ch8, 2, 6,400
];

/// Maximum take-off mass classes of helicopters without their own EASA levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelicopterMassClass {
    /// Up to 3,200 kg (Chapter 11 certifies helicopters up to 3,175 kg).
    Light,
    /// Up to 5,000 kg.
    Medium,
    /// Above 5,000 kg.
    Heavy,
}

impl HelicopterMassClass {
    /// dev4 `HeliLevels` over the class's designators among its first twenty rows: the level is
    /// their traffic-weighted energy mean (SPEC: 83.1, 84.4, 89.7 dB), the uplifts their
    /// traffic-weighted means. The weights are dev4's pinned counts per profile, in which EC135's
    /// count includes the designators that fell back to it.
    pub fn levels(self) -> HelicopterLevels {
        match self {
            HelicopterMassClass::Light => {
                certified(83.08758754946382, 2.0812335558225183, 4.901424189457327)
            }
            HelicopterMassClass::Medium => {
                certified(84.38262908834118, 1.8160237628938907, 5.600049117965887)
            }
            HelicopterMassClass::Heavy => {
                certified(89.73021006336194, 1.117162637847947, 3.549199215451954)
            }
        }
    }
}

/// Designators the mapping routes to the helicopter class without EASA noise levels.
const WITHOUT_LEVELS: [(&str, HelicopterMassClass); 9] = [
    ("AS3B", HelicopterMassClass::Heavy), // AS 532 Cougar, the military AS 332: 9 t
    ("B212", HelicopterMassClass::Heavy), // Bell 212: EASA lists 5,080 kg without levels
    ("EC65", HelicopterMassClass::Heavy), // EC 665 Tiger: 6 t
    ("GYRO", HelicopterMassClass::Light), // gyroplanes
    ("H60", HelicopterMassClass::Heavy),  // UH-60 family: 10 t
    ("MI8", HelicopterMassClass::Heavy),  // Mi-8 and Mi-17: 12-13 t
    ("MM16", HelicopterMassClass::Light), // Magni M-16 gyroplane
    ("S70", HelicopterMassClass::Heavy),  // S-70 family: 10 t
    ("UHEL", HelicopterMassClass::Light), // ultralight helicopters
];

/// Certified levels of a designator of the helicopter class: its own EASA record, else its mass
/// class; any other rotorcraft the mapping routes there reads the light class (the gyroplane
/// prior of dev4).
pub fn helicopter_levels(designator: &str) -> HelicopterLevels {
    if let Some((_, levels)) = CERTIFIED
        .iter()
        .find(|(designators, _)| designators.contains(&designator))
    {
        return *levels;
    }
    WITHOUT_LEVELS
        .iter()
        .find(|(listed, _)| *listed == designator)
        .map_or(HelicopterMassClass::Light, |&(_, class)| class)
        .levels()
}

/// The helicopter class curve at 150 m: approach (level and descent) and departure (climb) SEL.
static CLASS_SEL_AT_150_M_DB: LazyLock<[f64; 2]> = LazyLock::new(|| {
    let class = (0..NUM_CLASSES)
        .find(|&class| is_helicopter_class(class))
        .expect("the generated classes include the helicopter class");
    [false, true].map(|departure| read_npd(class, departure, PowerBracket::FIRST_ROW, 150.0).sel_db)
});

impl HelicopterLevels {
    /// The correction (dB) added to the helicopter class's NPD SEL and LAmax at every distance:
    /// the certified SEL of the segment's state minus the class curve at 150 m. A departure
    /// (climbing) segment reads the climb level, a descending one the descent level (Stage 1's
    /// whole-chord descent state), any other the level-flight level.
    pub fn correction_db(&self, departure: bool, descent: bool) -> f64 {
        let [approach_curve, departure_curve] = *CLASS_SEL_AT_150_M_DB;
        if departure {
            self.level_sel_150m_db + self.climb_uplift_db - departure_curve
        } else if descent {
            self.level_sel_150m_db + self.descent_uplift_db - approach_curve
        } else {
            self.level_sel_150m_db - approach_curve
        }
    }
}

#[cfg(test)]
#[path = "helicopters_tests.rs"]
mod tests;

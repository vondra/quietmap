//! NPD curves (Doc 29 4th ed. Vol 2 4.2 and 4.5.6): SEL and LAmax at the ten NPD distances,
//! linear in log distance between them, dev4's energy tail past 25,000 ft, the scaled distance of
//! the finite-segment correction, and the power rows each noise class reads.

use std::f64::consts::PI;
use std::sync::LazyLock;

use super::atmosphere::class_increments_db;
use super::profiles_generated::{CLASS_NAMES, CLASS_REP_PROFILE_IDX, NUM_CLASSES, PROFILES};
use super::thrust::PowerBracket;
use super::thrust_generated::THRUST;

/// Metres per international foot.
pub const METRES_PER_FOOT: f64 = 0.3048;
/// Number of NPD distances.
pub const NPD_DISTANCES: usize = 10;
/// The NPD distances (ft) of Doc 29 Vol 2 4.2.
pub const NPD_DISTANCES_FT: [f64; NPD_DISTANCES] = [
    200.0, 400.0, 630.0, 1_000.0, 2_000.0, 4_000.0, 6_300.0, 10_000.0, 16_000.0, 25_000.0,
];
/// Nearer slants read the curve here (dev4): Eq. 4-5a's extrapolation below 200 ft would grow
/// without bound toward zero distance.
pub const NPD_NEAREST_SLANT_M: f64 = 100.0 * METRES_PER_FOOT;
/// The last NPD distance, where dev4's tail takes over.
pub const NPD_LAST_DISTANCE_M: f64 = NPD_DISTANCES_FT[NPD_DISTANCES - 1] * METRES_PER_FOOT;
/// Eq. 4-11 reference duration t0 (s).
const SEL_REFERENCE_DURATION_S: f64 = 1.0;
/// The profile generator fills a missing ANP LAmax curve with SEL - 12 dB on every distance; such
/// a curve carries no event duration, so d_lambda takes the dipole limit, the slant itself (Doc 29
/// Appendix E).
const PLACEHOLDER_SEL_MINUS_LAMAX_DB: f64 = 12.0;

static LOG10_NPD_DISTANCES_FT: LazyLock<[f64; NPD_DISTANCES]> =
    LazyLock::new(|| NPD_DISTANCES_FT.map(f64::log10));

/// Engine installation: the coefficients of the installation correction Delta_I (Eq. 4-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installation {
    /// Wing-mounted jet engines.
    Wing,
    /// Fuselage- or tail-mounted jet engines.
    Fuselage,
    /// Propellers, rotors included: no installation correction.
    Propeller,
}

/// One profile of the generated table: SEL and LAmax curves per operation at the ten NPD
/// distances, the NPD reference speed and the installation.
#[derive(Debug)]
pub struct NpdProfile {
    pub name: &'static str,
    pub approach_sel: [f64; NPD_DISTANCES],
    pub departure_sel: [f64; NPD_DISTANCES],
    pub approach_lmax: [f64; NPD_DISTANCES],
    pub departure_lmax: [f64; NPD_DISTANCES],
    pub v_ref_kt: f64,
    pub installation: Installation,
}

impl NpdProfile {
    pub const fn new(
        name: &'static str,
        approach_sel: [f64; NPD_DISTANCES],
        departure_sel: [f64; NPD_DISTANCES],
        approach_lmax: [f64; NPD_DISTANCES],
        departure_lmax: [f64; NPD_DISTANCES],
        v_ref_kt: f64,
        installation: Installation,
    ) -> Self {
        NpdProfile {
            name,
            approach_sel,
            departure_sel,
            approach_lmax,
            departure_lmax,
            v_ref_kt,
            installation,
        }
    }
}

/// The profile whose curves, reference speed and installation a noise class reads.
pub fn class_anchor(class: usize) -> &'static NpdProfile {
    &PROFILES[usize::from(CLASS_REP_PROFILE_IDX[class])]
}

/// Whether a noise class is the helicopter class (its index follows the generated order).
pub fn is_helicopter_class(class: usize) -> bool {
    CLASS_NAMES[class] == "HELICOPTER"
}

/// Where a slant falls among the NPD distances: the interval (0..=8) and the fraction along it in
/// log distance, below 0 before 200 ft and above 1 past 25,000 ft.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NpdPosition {
    pub(crate) interval: usize,
    pub(crate) fraction: f64,
    pub(crate) slant_m: f64,
}

impl NpdPosition {
    pub(crate) fn at(slant_m: f64) -> Self {
        let slant_m = slant_m.max(NPD_NEAREST_SLANT_M);
        let log_d = (slant_m / METRES_PER_FOOT).log10();
        let logs = &*LOG10_NPD_DISTANCES_FT;
        let interval = (1..NPD_DISTANCES - 1)
            .rev()
            .find(|&k| log_d > logs[k])
            .unwrap_or(0);
        let fraction = (log_d - logs[interval]) / (logs[interval + 1] - logs[interval]);
        NpdPosition {
            interval,
            fraction,
            slant_m,
        }
    }

    /// Linear in log distance, extrapolated with the end intervals (Eq. 4-4, 4-5).
    pub(crate) fn linear(&self, curve: &[f64; NPD_DISTANCES]) -> f64 {
        let lower = curve[self.interval];
        lower + self.fraction * (curve[self.interval + 1] - lower)
    }

    /// A level curve: linear in log distance up to 25,000 ft, then dev4's tail.
    fn level(&self, curve: &[f64; NPD_DISTANCES], tail_absorption_db_per_m: f64) -> f64 {
        if self.slant_m < NPD_LAST_DISTANCE_M {
            return self.linear(curve);
        }
        curve[NPD_DISTANCES - 1]
            - 20.0 * (self.slant_m / NPD_LAST_DISTANCE_M).log10()
            - tail_absorption_db_per_m * (self.slant_m - NPD_LAST_DISTANCE_M)
    }
}

/// Level (dB) of an NPD curve at `slant_m`: linear in log distance between the ten distances,
/// below 200 ft with the first interval's slope down to 100 ft, and past 25,000 ft dev4's tail
/// `L(25,000 ft) - 20 lg(d / 7,620 m) - alpha (d - 7,620 m)` with `tail_absorption_db_per_m`.
pub fn curve_level_db(
    curve: &[f64; NPD_DISTANCES],
    tail_absorption_db_per_m: f64,
    slant_m: f64,
) -> f64 {
    NpdPosition::at(slant_m).level(curve, tail_absorption_db_per_m)
}

/// The tail's absorption alpha (dB/m) of a SEL curve (dev4): after removing spherical divergence
/// from its last three distances, the least-squares slope of the residual in metres, floored at
/// 0. It reproduces the curve's own tail and is not an ISO 9613-1 coefficient; LAmax shares the
/// SEL curve's value.
pub fn tail_absorption_db_per_m(sel_db: &[f64; NPD_DISTANCES]) -> f64 {
    let fit: [(f64, f64); 3] = std::array::from_fn(|i| {
        let k = NPD_DISTANCES - 3 + i;
        let distance_m = NPD_DISTANCES_FT[k] * METRES_PER_FOOT;
        (
            distance_m,
            sel_db[k] + 20.0 * (distance_m / NPD_LAST_DISTANCE_M).log10(),
        )
    });
    let n = fit.len() as f64;
    let sx: f64 = fit.iter().map(|p| p.0).sum();
    let sy: f64 = fit.iter().map(|p| p.1).sum();
    let sxx: f64 = fit.iter().map(|p| p.0 * p.0).sum();
    let sxy: f64 = fit.iter().map(|p| p.0 * p.1).sum();
    (-(n * sxy - sx * sy) / (n * sxx - sx * sx)).max(0.0)
}

/// NPD values of one noise class, operation and power bracket at one slant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpdReading {
    pub sel_db: f64,
    pub lamax_db: f64,
    /// Scaled distance d_lambda (m) of the finite-segment correction.
    pub scaled_distance_m: f64,
}

/// NPD values of noise class `class` at `slant_m`, the power rows interpolated linearly in power
/// (Eq. 4-3): SEL, LAmax and d_lambda alike, d_lambda in metres (SEL - LAmax is nearly flat
/// across rows). Slants below 100 ft read the curve at 100 ft.
pub fn read_npd(class: usize, departure: bool, power: PowerBracket, slant_m: f64) -> NpdReading {
    let rows = &POWER_ROWS[class][usize::from(departure)];
    let position = NpdPosition::at(slant_m);
    let low = rows[power.row].read(&position);
    if power.weight == 0.0 {
        return low;
    }
    let high = rows[power.row + 1].read(&position);
    let lerp = |a: f64, b: f64| a + power.weight * (b - a);
    NpdReading {
        sel_db: lerp(low.sel_db, high.sel_db),
        lamax_db: lerp(low.lamax_db, high.lamax_db),
        scaled_distance_m: lerp(low.scaled_distance_m, high.scaled_distance_m),
    }
}

/// One tabulated power setting of a class and operation.
struct PowerRow {
    sel_db: [f64; NPD_DISTANCES],
    lamax_db: [f64; NPD_DISTANCES],
    tail_absorption_db_per_m: f64,
    scaled_distance: ScaledDistance,
}

/// How a row's scaled distance follows from its curves (Doc 29 Eq. 4-11).
enum ScaledDistance {
    /// `d_lambda = d0 10^((SEL - LAmax) / 10)`, `d0 = (2 / pi) V_ref t0`; SEL - LAmax linear in
    /// log distance and extrapolated with the end intervals on both sides (not the energy tail).
    EventDuration {
        d0_m: f64,
        sel_minus_lamax_db: [f64; NPD_DISTANCES],
    },
    /// Placeholder LAmax: the dipole limit `d_lambda = slant`.
    Dipole,
}

impl PowerRow {
    fn new(sel_db: &[f64; NPD_DISTANCES], lamax_db: &[f64; NPD_DISTANCES], v_ref_kt: f64) -> Self {
        let sel_minus_lamax_db: [f64; NPD_DISTANCES] =
            std::array::from_fn(|k| sel_db[k] - lamax_db[k]);
        let placeholder = sel_minus_lamax_db
            .iter()
            .all(|delta| (delta - PLACEHOLDER_SEL_MINUS_LAMAX_DB).abs() < 1e-9);
        let scaled_distance = if placeholder {
            ScaledDistance::Dipole
        } else {
            let speed_m_per_s = v_ref_kt * 1852.0 / 3600.0;
            ScaledDistance::EventDuration {
                d0_m: 2.0 / PI * speed_m_per_s * SEL_REFERENCE_DURATION_S,
                sel_minus_lamax_db,
            }
        };
        PowerRow {
            sel_db: *sel_db,
            lamax_db: *lamax_db,
            tail_absorption_db_per_m: tail_absorption_db_per_m(sel_db),
            scaled_distance,
        }
    }

    fn read(&self, position: &NpdPosition) -> NpdReading {
        let alpha = self.tail_absorption_db_per_m;
        let scaled_distance_m = match &self.scaled_distance {
            ScaledDistance::EventDuration {
                d0_m,
                sel_minus_lamax_db,
            } => d0_m * 10f64.powf(position.linear(sel_minus_lamax_db) / 10.0),
            ScaledDistance::Dipole => position.slant_m,
        };
        NpdReading {
            sel_db: position.level(&self.sel_db, alpha),
            lamax_db: position.level(&self.lamax_db, alpha),
            scaled_distance_m,
        }
    }
}

/// `[class][departure]`: every tabulated power row of a thrust class, the anchor curve alone for
/// a pinned class; SEL and LAmax alike moved from the AIR-1845 atmosphere of the ANP curves to the
/// model's (`atmosphere`, Doc 29 Appendix D).
static POWER_ROWS: LazyLock<Vec<[Vec<PowerRow>; 2]>> = LazyLock::new(|| {
    (0..NUM_CLASSES)
        .map(|class| {
            let anchor = class_anchor(class);
            let model = &THRUST[class];
            let rows = |sel: &[[f64; NPD_DISTANCES]],
                        lamax: &[[f64; NPD_DISTANCES]],
                        count: u8,
                        departure: bool| {
                let increments = class_increments_db(class, departure);
                let adjusted = |curve: &[f64; NPD_DISTANCES]| -> [f64; NPD_DISTANCES] {
                    std::array::from_fn(|k| curve[k] + increments[k])
                };
                (0..usize::from(count))
                    .map(|row| {
                        PowerRow::new(
                            &adjusted(&sel[row]),
                            &adjusted(&lamax[row]),
                            anchor.v_ref_kt,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            if model.has_thrust {
                [
                    rows(&model.app_sel, &model.app_lmax, model.app_rows, false),
                    rows(&model.dep_sel, &model.dep_lmax, model.dep_rows, true),
                ]
            } else {
                [
                    rows(&[anchor.approach_sel], &[anchor.approach_lmax], 1, false),
                    rows(&[anchor.departure_sel], &[anchor.departure_lmax], 1, true),
                ]
            }
        })
        .collect()
});

/// The steepest fall (dB per decade of distance) of any row's SEL curve in each interval between
/// NPD distances: the first one is the most any level can gain below 200 ft, where each row
/// extrapolates its own first interval; all of them size the aircraft boxes.
pub(crate) static STEEPEST_INTERVAL_DB_PER_DECADE: LazyLock<[f64; NPD_DISTANCES - 1]> =
    LazyLock::new(|| {
        let logs = &*LOG10_NPD_DISTANCES_FT;
        std::array::from_fn(|k| {
            POWER_ROWS
                .iter()
                .flatten()
                .flatten()
                .map(|row| (row.sel_db[k] - row.sel_db[k + 1]) / (logs[k + 1] - logs[k]))
                .fold(f64::NEG_INFINITY, f64::max)
        })
    });

/// The first interval's value of [`STEEPEST_INTERVAL_DB_PER_DECADE`].
pub(crate) static STEEPEST_FIRST_INTERVAL_DB_PER_DECADE: LazyLock<f64> =
    LazyLock::new(|| STEEPEST_INTERVAL_DB_PER_DECADE[0]);

/// The largest tail absorption (dB/m) of any row: past 25,000 ft no SEL falls faster than
/// spherical divergence plus this.
pub(crate) static STEEPEST_TAIL_ABSORPTION_DB_PER_M: LazyLock<f64> = LazyLock::new(|| {
    POWER_ROWS
        .iter()
        .flatten()
        .flatten()
        .map(|row| row.tail_absorption_db_per_m)
        .fold(0.0, f64::max)
});

/// The steepest fall of any NPD SEL curve at `slant_m` (dB per metre): its interval's steepest
/// decade slope, below 200 ft the first interval's, past 25,000 ft spherical divergence plus the
/// largest tail absorption.
pub fn steepest_sel_slope_db_per_m(slant_m: f64) -> f64 {
    let slant_m = slant_m.max(NPD_NEAREST_SLANT_M);
    if slant_m >= NPD_LAST_DISTANCE_M {
        return 20.0 / (slant_m * std::f64::consts::LN_10) + *STEEPEST_TAIL_ABSORPTION_DB_PER_M;
    }
    let log_d = (slant_m / METRES_PER_FOOT).log10();
    let logs = &*LOG10_NPD_DISTANCES_FT;
    let interval = (1..NPD_DISTANCES - 1)
        .rev()
        .find(|&k| log_d > logs[k])
        .unwrap_or(0);
    STEEPEST_INTERVAL_DB_PER_DECADE[interval] / (slant_m * std::f64::consts::LN_10)
}

/// The far anchor of an aircraft box's curves (m): past 25,000 ft every row falls with its own
/// absorption, so a mix of rows falls slower than any one fitted curve; a box also sums its
/// pieces here, at the aircraft reach, and its tail runs through both ends.
pub const TAIL_ANCHOR_M: f64 = 16_000.0;

/// The slant (m) at which an aircraft box states its loudest piece's LAmax: 1,000 ft.
pub const LAMAX_REFERENCE_SLANT_M: f64 = 1_000.0 * METRES_PER_FOOT;
/// The rise table starts at the nearest slant a curve is read at and has this many entries per
/// decade of slant, out to 1,000 km.
const RISE_TABLE_STEPS_PER_DECADE: f64 = 50.0;
const RISE_TABLE_LAST_M: f64 = 1.0e6;

/// Per slant of the rise table, the most any row's LAmax curve rises from the reference slant.
static LAMAX_RISE_TABLE: LazyLock<Vec<f64>> = LazyLock::new(|| {
    let decades = (RISE_TABLE_LAST_M / NPD_NEAREST_SLANT_M).log10();
    let entries = (decades * RISE_TABLE_STEPS_PER_DECADE).ceil() as usize + 1;
    let reference = NpdPosition::at(LAMAX_REFERENCE_SLANT_M);
    (0..entries)
        .map(|index| {
            let slant_m =
                NPD_NEAREST_SLANT_M * 10f64.powf(index as f64 / RISE_TABLE_STEPS_PER_DECADE);
            let position = NpdPosition::at(slant_m);
            POWER_ROWS
                .iter()
                .flatten()
                .flatten()
                .map(|row| row.read(&position).lamax_db - row.read(&reference).lamax_db)
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .collect()
});

/// The most any LAmax curve rises from [`LAMAX_REFERENCE_SLANT_M`] to `slant_m` (dB; negative
/// where every curve falls): a piece whose LAmax at the reference is L is at most L plus this at
/// `slant_m`. Every curve falls with slant, so the table entry at or below `slant_m` bounds it;
/// power rows interpolate linearly in dB and stay within their rows.
pub fn lamax_rise_bound_db(slant_m: f64) -> f64 {
    let table = &*LAMAX_RISE_TABLE;
    let steps = (slant_m.max(NPD_NEAREST_SLANT_M) / NPD_NEAREST_SLANT_M).log10()
        * RISE_TABLE_STEPS_PER_DECADE;
    table[(steps.floor() as usize).min(table.len() - 1)]
}

#[cfg(test)]
#[path = "npd_tests.rs"]
mod tests;

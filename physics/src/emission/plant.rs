//! The plant of shops, offices, restaurants, cafés and bars, heard in its hours (research
//! 2026-10-18, building-plant-hours): a cooling plant sized by its floor and its climate, running
//! its climate's share of the open hours in the share of buildings that cool, and a restaurant's
//! kitchen extract sized by its footprint, running in its kitchen's hours. Each is heard in use: at
//! its running level for the share of each period it runs, silent the rest. dev4 gave every shop
//! and office 70 dB(A) and every bar, café and restaurant 68, steady and at night too: judgments
//! (Guyer's machine-room chillers; no kitchen data).

use super::spectrum::SoundPower;
use crate::bands::{BANDS, PERIOD_HOURS, PERIODS, WeekHours, period_of_hour};

/// Shops' and offices' open hours, a judgment: Monday to Saturday 09-20, CBECS 2018's mean 66 h a
/// week of retail other than malls (Table B1; offices 54, all buildings 62); the DOE prototype
/// buildings run their plant longer (small offices 06-19 on weekdays, retail 06-21).
pub const BUSINESS_HOURS: WeekHours = business_hours();

const fn business_hours() -> WeekHours {
    let mut week = [[false; 24]; 7];
    let mut day = 0;
    while day < 6 {
        let mut hour = 9;
        while hour < 20 {
            week[day][hour] = true;
            hour += 1;
        }
        day += 1;
    }
    week
}

/// Installed cooling a cooled square metre (W) by the cooling degree days (base 18 C): Heat Roadmap
/// Europe D3.2's service-sector fit 159 + 0.142 CDD, at most Cyprus's 360.
const COOLING_W_PER_M2: f64 = 159.0;
const COOLING_W_PER_M2_PER_DEGREE_DAY: f64 = 0.142;
const COOLING_W_PER_M2_MAX: f64 = 360.0;
/// Cooling delivered a cooled square metre a year (kWh): HRE D3.2's 40.1 + 0.142 CDD (41 in
/// Ireland, 121 in Spain, 241 in Cyprus).
const COOLING_KWH_PER_M2: f64 = 40.1;
const COOLING_KWH_PER_M2_PER_DEGREE_DAY: f64 = 0.142;
/// An outdoor cooling unit's sound power by its capacity, L_WA = 56.6 + 14.6 lg(Q / kW): the least
/// squares over 11 declared units from a 2.4 kW split to 523 kW chillers (rms 2.9 dB). A larger
/// plant is units of the EU15's mean chiller (Pezzutto 2017). Its full-load hours count as hours
/// running at that power, as an on/off unit runs (a judgment: an inverter runs longer at part
/// speed, more quietly).
const UNIT_DBA_AT_ONE_KW: f64 = 56.6;
const UNIT_DB_PER_DECADE: f64 = 14.6;
const UNIT_KW_MAX: f64 = 135.7;
/// dev4's shape of commercial plant (its air handlers and chillers after Guyer).
const COOLING_SPECTRUM: [f64; BANDS] = [-1.0, 1.0, 1.0, 1.0, 0.0, -1.0, -3.0, -6.0];

/// A kitchen extract's sound power while running over a footprint of A m2, 47 + 10 lg A dB(A): the
/// fan law at 6 l/s a square metre of floor (the DOE prototype restaurants' 5.0-6.7; DEFRA 2005's
/// 15-20 a square metre of kitchen), 375 Pa (within 250-500) on the outlet side, 53 + 10 lg A in
/// the duct and 6 dB less outside, as the 72 dB(A) measured at the exhaust of a 316 m2 fast-food
/// restaurant gives (Aurich, IEL 2641-21-L3, 2020).
const EXTRACT_DBA_AT_ONE_M2: f64 = 47.0;
/// A footprint larger than the DOE's full-service prototype restaurant (511 m2) holds more than a
/// restaurant (a hotel's or a hut's rooms, the shops of a mixed building), its kitchen that one's.
const EXTRACT_FOOTPRINT_MAX_M2: f64 = 511.0;
/// dev4's shape of a kitchen extract.
const EXTRACT_SPECTRUM: [f64; BANDS] = [-1.0, 0.0, 1.0, 1.0, 1.0, 0.0, -3.0, -6.0];

/// The share of each period's hours of the week that `hours` holds.
pub fn period_shares(hours: &WeekHours) -> [f64; PERIODS] {
    let mut held = [0.0; PERIODS];
    for day in hours {
        for (hour, &on) in day.iter().enumerate() {
            if on {
                held[period_of_hour(hour)] += 1.0;
            }
        }
    }
    std::array::from_fn(|p| held[p] / (7.0 * PERIOD_HOURS[p]))
}

/// A kitchen's hours, a judgment: its place's open hours with the hour before it opens and the hour
/// after it closes (preparation and cleaning: extracts run "early in the morning and until late at
/// night", DEFRA 2005; the DOE prototype restaurants' fans 05:00-01:00).
pub fn kitchen_hours(open: &WeekHours) -> WeekHours {
    let at = |hour_of_week: usize| open[hour_of_week / 24 % 7][hour_of_week % 24];
    std::array::from_fn(|day| {
        std::array::from_fn(|hour| {
            let now = day * 24 + hour + 7 * 24;
            at(now - 1) || at(now) || at(now + 1)
        })
    })
}

/// A plant heard in use: its sound power while it runs, the share of each period it runs, the shape
/// of its bands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Running {
    pub running_dba: f64,
    pub duty: [f64; PERIODS],
    pub spectrum_db: [f64; BANDS],
}

impl Running {
    /// Each period's mean over the hours it runs and the hours it is silent; `None` when it never
    /// runs.
    pub fn sound_power(&self) -> Option<SoundPower> {
        let levels = self
            .duty
            .map(|share| self.running_dba + 10.0 * share.log10());
        SoundPower::of_period_means(levels, self.spectrum_db)
    }
}

/// A plant at `running_dba` for the share `duty` of each period; `None` when it never runs.
fn running(running_dba: f64, duty: [f64; PERIODS], spectrum_db: [f64; BANDS]) -> Option<Running> {
    duty.iter().any(|&share| share > 0.0).then_some(Running {
        running_dba,
        duty,
        spectrum_db,
    })
}

/// A building's cooling plant over `gross_floor_area_m2` under `cooling_degree_days`, in
/// `cooled_share` of such buildings, running while the building is `open`: heard in use at its
/// units' level for the open hours its climate's full-load hours fill. The share cooled counts as
/// the share of buildings that cool their floor (a judgment: one cooling a part of its floor runs
/// smaller units longer).
pub fn cooling_plant(
    gross_floor_area_m2: f64,
    cooling_degree_days: f64,
    cooled_share: f64,
    open: &WeekHours,
) -> Option<Running> {
    let w_per_m2 = (COOLING_W_PER_M2 + COOLING_W_PER_M2_PER_DEGREE_DAY * cooling_degree_days)
        .min(COOLING_W_PER_M2_MAX);
    let capacity_kw = w_per_m2 * gross_floor_area_m2 / 1_000.0;
    let running_dba = UNIT_DBA_AT_ONE_KW
        + UNIT_DB_PER_DECADE * capacity_kw.min(UNIT_KW_MAX).log10()
        + 10.0 * (capacity_kw / UNIT_KW_MAX).max(1.0).log10();
    let full_load_hours = 1_000.0
        * (COOLING_KWH_PER_M2 + COOLING_KWH_PER_M2_PER_DEGREE_DAY * cooling_degree_days)
        / w_per_m2;
    let open_shares = period_shares(open);
    let open_hours_a_year = 52.0
        * (0..PERIODS)
            .map(|p| open_shares[p] * 7.0 * PERIOD_HOURS[p])
            .sum::<f64>();
    let running_share = (full_load_hours / open_hours_a_year).min(1.0);
    running(
        running_dba,
        open_shares.map(|share| cooled_share * running_share * share),
        COOLING_SPECTRUM,
    )
}

/// A restaurant's kitchen extract over `footprint_m2`, running in its `kitchen` hours.
pub fn kitchen_extract(footprint_m2: f64, kitchen: &WeekHours) -> Option<Running> {
    running(
        EXTRACT_DBA_AT_ONE_M2 + 10.0 * footprint_m2.min(EXTRACT_FOOTPRINT_MAX_M2).log10(),
        period_shares(kitchen),
        EXTRACT_SPECTRUM,
    )
}

#[cfg(test)]
#[path = "plant_tests.rs"]
mod tests;

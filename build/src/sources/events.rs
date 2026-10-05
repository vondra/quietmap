//! Sources that sound in events (church bells, calls to prayer): how many a day and how many
//! seconds they sound in each period (day, evening, night), the energy they carry, and the point
//! source whose display fields let the popup's time levels sound it only in its share of each
//! period.

use super::Converted;
use super::cells::{Site, Z30Ring, push_site_points, site_points};
use super::group_key;
use physics::bands::{BANDS, PERIOD_HOURS};
use physics::emission::spectrum::SoundPower;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

pub const DAY: usize = 0;
pub const EVENING: usize = 1;
pub const NIGHT: usize = 2;

/// Events a day, seconds sounding a day and the energy of a day (seconds times 10^(LW/10)) per
/// period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventSchedule {
    pub events_per_day: [f64; 3],
    pub seconds: [f64; 3],
    pub energy: [f64; 3],
}

impl EventSchedule {
    /// `events` a day in `period`, each sounding `seconds` at `lw_dba`.
    pub fn add(&mut self, period: usize, events: f64, seconds: f64, lw_dba: f64) {
        self.events_per_day[period] += events;
        self.seconds[period] += events * seconds;
        self.energy[period] += events * seconds * 10f64.powf(lw_dba / 10.0);
    }
}

fn level(energy: f64) -> f64 {
    if energy > 0.0 {
        10.0 * energy.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// The events' mean sound power per period (dB(A); day level, evening and night offsets, silence
/// minus infinity), their share of each period sounding and their sound power while sounding.
pub fn event_emission(
    schedule: &EventSchedule,
    spectrum_db: [f64; BANDS],
) -> (SoundPower, [f64; 3], f64) {
    let mean =
        std::array::from_fn::<f64, 3, _>(|p| schedule.energy[p] / (PERIOD_HOURS[p] * 3600.0));
    let duty = std::array::from_fn(|p| schedule.seconds[p] / (PERIOD_HOURS[p] * 3600.0));
    let day = level(mean[0]);
    let sounding: f64 = schedule.seconds.iter().sum();
    let while_sounding = level(schedule.energy.iter().sum::<f64>() / sounding);
    (
        SoundPower {
            day_dba: day,
            spectrum_db,
            evening_offset_db: level(mean[1]) - day,
            night_offset_db: level(mean[2]) - day,
        },
        duty,
        while_sounding,
    )
}

/// The events as one point source `height_m` above the ground at `point` (lat, lon), screened by
/// everything but `footprint_id`, listed as `label` under `name`.
pub fn push_event_source(
    (point, height_m, footprint_id): ((f64, f64), f64, u64),
    (label, name): (&str, &str),
    (schedule, spectrum_db): (&EventSchedule, [f64; BANDS]),
    key: &str,
    out: &mut Vec<Converted>,
) {
    let (sound, duty, while_sounding) = event_emission(schedule, spectrum_db);
    let decibels = |level: f64| (level * 10.0).round() / 10.0;
    let ring: Z30Ring = Vec::new();
    let site = Site {
        centroid: point,
        ring: &ring,
        area_m2: 1.0,
        single_point_up_to_m2: 1.0,
        cell_m: 1.0,
    };
    let attribute = Attribute {
        layer: Layer::Building,
        height_m,
        ground_percent: GROUND_FROM_TERRAIN,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id,
        group_key: group_key(&[label, key]),
        emission: sound.band_levels_db(),
        display: json!([
            name,
            label,
            decibels(height_m),
            0,
            0,
            "",
            decibels(while_sounding),
            null,
            schedule.events_per_day.map(|e| (e * 100.0).round() / 100.0),
            duty.map(|d| (d * 1e5).round() / 1e5),
        ])
        .to_string(),
    };
    push_site_points(&site_points(&site), 1.0, &attribute, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two events of 180 s a day at 114 dB(A) and one of 450 s a week: 424.3 s of the 12 hours, a
    /// mean of 114 + 10 lg(424.3 / 43,200) = 93.9 dB(A); an empty evening is silent.
    #[test]
    fn events_average_over_their_period() {
        let mut schedule = EventSchedule::default();
        schedule.add(DAY, 2.0, 180.0, 114.0);
        schedule.add(DAY, 1.0 / 7.0, 450.0, 114.0);
        schedule.add(NIGHT, 1.0, 180.0, 114.0);
        let (sound, duty, sounding) = event_emission(&schedule, [0.0; BANDS]);
        assert!((sound.day_dba - 93.92).abs() < 0.01, "{}", sound.day_dba);
        assert!((duty[0] - (360.0 + 450.0 / 7.0) / 43_200.0).abs() < 1e-12);
        assert_eq!(sound.evening_offset_db, f64::NEG_INFINITY);
        assert!(
            (sound.night_offset_db - (10.0 * (180.0f64 / 28_800.0).log10() + 114.0 - 93.92)).abs()
                < 0.01
        );
        assert!((sounding - 114.0).abs() < 1e-9);
        assert_eq!(schedule.events_per_day, [2.0 + 1.0 / 7.0, 0.0, 1.0]);
    }
}

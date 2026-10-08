//! The aircraft events table (PLAN section 6, B): at a receiver every flight counts once, at its
//! loudest moment (Doc 29 Eq. 4-8a, outdoors in the open); per band of that maximum level the
//! flights of an average day by the period of the moment, their mean height above the ground
//! there and the type flying most of them, and the helicopters of the lowest band. The weights are
//! the boxes': a flight whose primary segments pass a band counts on baseline days, one that
//! passes it only through what the secondary provider saw counts on increment days (the energy's
//! P/B + S/I estimator, a count in place of the energy).

use physics::bands::PERIODS;
use std::collections::HashMap;

/// The bands: maximum level at or above (dB; the owner's 50, 60 and 70).
pub const EVENT_BANDS_DB: [f64; 3] = [50.0, 60.0, 70.0];
pub const BANDS: usize = EVENT_BANDS_DB.len();

/// A flight's moment at a receiver: its maximum level, the period it falls in and the aircraft's
/// height above the ground under the receiver (m; negative below it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    pub lmax_db: f64,
    pub period: u8,
    pub height_m: f64,
}

/// A flight's loudest moments of one day at one receiver: over its primary segments and over all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightPeaks {
    pub designator: [u8; 4],
    pub helicopter: bool,
    primary: Option<Peak>,
    any: Option<Peak>,
}

fn louder(a: Option<Peak>, b: Option<Peak>) -> Option<Peak> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.lmax_db > a.lmax_db { b } else { a }),
        (a, b) => a.or(b),
    }
}

impl FlightPeaks {
    pub fn new(designator: [u8; 4], helicopter: bool) -> Self {
        FlightPeaks {
            designator,
            helicopter,
            primary: None,
            any: None,
        }
    }

    /// Adds a segment's moment; `secondary` when only the secondary provider saw the segment.
    pub fn add(&mut self, peak: Peak, secondary: bool) {
        if !secondary {
            self.primary = louder(self.primary, Some(peak));
        }
        self.any = louder(self.any, Some(peak));
    }

    pub fn merge(&mut self, other: &FlightPeaks) {
        self.primary = louder(self.primary, other.primary);
        self.any = louder(self.any, other.any);
    }

    /// The moment that counts the flight at or above `threshold_db` and its weight: the primary
    /// segments' at the day's weight for them, else all segments' at the secondary's weight;
    /// none where that weight is 0 (the day lacks the role).
    fn counted(&self, threshold_db: f64, (primary, secondary): (f64, f64)) -> Option<(Peak, f64)> {
        let passes = |peak: Option<Peak>| peak.filter(|peak| peak.lmax_db >= threshold_db);
        match passes(self.primary) {
            Some(peak) => Some((peak, primary)),
            None => passes(self.any).map(|peak| (peak, secondary)),
        }
        .filter(|(_, weight)| *weight > 0.0)
    }
}

/// The table at one receiver, summed over days.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventCounts {
    /// Per band, flights an average day by period, and their weights times height (m).
    pub per_day: [[f64; PERIODS]; BANDS],
    pub height_sum: [f64; BANDS],
    pub designators: [HashMap<[u8; 4], f64>; BANDS],
    /// Helicopters an average day in the lowest band.
    pub helicopters: f64,
}

impl EventCounts {
    /// Counts one flight of a day whose primary and secondary segments weigh `weights`.
    pub fn add(&mut self, flight: &FlightPeaks, weights: (f64, f64)) {
        for (band, &threshold) in EVENT_BANDS_DB.iter().enumerate() {
            let Some((peak, weight)) = flight.counted(threshold, weights) else {
                continue;
            };
            self.per_day[band][usize::from(peak.period).min(PERIODS - 1)] += weight;
            self.height_sum[band] += weight * peak.height_m;
            *self.designators[band].entry(flight.designator).or_default() += weight;
            if band == 0 && flight.helicopter {
                self.helicopters += weight;
            }
        }
    }

    pub fn merge(&mut self, other: EventCounts) {
        for band in 0..BANDS {
            for period in 0..PERIODS {
                self.per_day[band][period] += other.per_day[band][period];
            }
            self.height_sum[band] += other.height_sum[band];
            for (designator, weight) in &other.designators[band] {
                *self.designators[band].entry(*designator).or_default() += weight;
            }
        }
        self.helicopters += other.helicopters;
    }

    /// Flights an average day in `band`.
    pub fn flights(&self, band: usize) -> f64 {
        self.per_day[band].iter().sum()
    }

    /// The mean height (m) of `band`'s flights, NaN without any.
    pub fn mean_height_m(&self, band: usize) -> f64 {
        self.height_sum[band] / self.flights(band)
    }

    /// The designator flying most of `band`'s flights (equal weights by the designator).
    pub fn top_designator(&self, band: usize) -> Option<[u8; 4]> {
        self.designators[band]
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(designator, _)| *designator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B738: [u8; 4] = *b"B738";
    const DAY: (f64, f64) = (1.0 / 354.0, 1.0 / 11.0);

    fn peak(lmax_db: f64, period: u8, height_m: f64) -> Peak {
        Peak {
            lmax_db,
            period,
            height_m,
        }
    }

    /// A flight passing a receiver twice counts once per band, at its louder moment's period and
    /// height.
    #[test]
    fn a_flight_counts_once_at_its_loudest_moment() {
        let mut flight = FlightPeaks::new(B738, false);
        flight.add(peak(63.0, 0, 900.0), false);
        flight.add(peak(71.0, 1, 400.0), false);
        let mut counts = EventCounts::default();
        counts.add(&flight, DAY);
        for band in 0..BANDS {
            assert_eq!(counts.per_day[band], [0.0, 1.0 / 354.0, 0.0]);
            assert!((counts.mean_height_m(band) - 400.0).abs() < 1e-9);
            assert_eq!(counts.top_designator(band), Some(B738));
        }
    }

    /// Primary segments to 55 dB with a secondary gap fill to 65 dB: the primary counts the 50 dB
    /// band at a baseline day's weight, the gap fill the 60 dB band at an increment day's; on a
    /// baseline day that is no increment day the gap fill adds nothing.
    #[test]
    fn the_secondary_adds_only_what_the_primary_misses() {
        let mut flight = FlightPeaks::new(B738, false);
        flight.add(peak(55.0, 0, 1_000.0), false);
        flight.add(peak(65.0, 2, 700.0), true);
        let mut both = EventCounts::default();
        both.add(&flight, DAY);
        assert_eq!(both.per_day[0], [1.0 / 354.0, 0.0, 0.0]);
        assert_eq!(both.per_day[1], [0.0, 0.0, 1.0 / 11.0]);
        assert_eq!(both.flights(2), 0.0);
        let mut baseline_only = EventCounts::default();
        baseline_only.add(&flight, (1.0 / 354.0, 0.0));
        assert_eq!(baseline_only.flights(0), 1.0 / 354.0);
        assert_eq!(baseline_only.flights(1), 0.0);
    }

    /// A flight only the secondary provider saw weighs an increment day's 1/11, and nothing on a
    /// day that is no increment day; a helicopter counts in the lowest band's helicopters only.
    #[test]
    fn a_secondary_flight_counts_on_increment_days() {
        let mut flight = FlightPeaks::new(*b"EC35", true);
        flight.add(peak(66.0, 0, 150.0), true);
        let mut counts = EventCounts::default();
        counts.add(&flight, DAY);
        assert_eq!(counts.flights(0), 1.0 / 11.0);
        assert_eq!(counts.flights(1), 1.0 / 11.0);
        assert_eq!(counts.helicopters, 1.0 / 11.0);
        let mut none = EventCounts::default();
        none.add(&flight, (1.0 / 354.0, 0.0));
        assert_eq!(none, EventCounts::default());
    }
}

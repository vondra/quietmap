//! Prepared railway period traffic and speed normalization shared by popup and surface loaders.

use crate::constants::SOURCE_HEIGHT_RAIL;
use crate::emission::railway::{self, RailType};
use crate::periods::END_PERIOD_HOURS;
use crate::types::NUM_BANDS;
use serde::Serialize;

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq)]
pub struct RailCategoryTraffic {
    pub periods: [f64; 3],
    pub status: u8,
    pub source_id: u16,
    pub matching: u8,
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq)]
pub struct RailTraffic {
    pub passenger: RailCategoryTraffic,
    pub freight: RailCategoryTraffic,
}

impl RailTraffic {
    pub fn periods(self) -> [(f64, f64, f64); 3] {
        std::array::from_fn(|i| {
            (
                self.passenger.periods[i],
                self.freight.periods[i],
                END_PERIOD_HOURS[i],
            )
        })
    }

    pub fn is_silent(self) -> bool {
        self.passenger
            .periods
            .iter()
            .chain(&self.freight.periods)
            .all(|value| *value == 0.0)
    }
}

/// Half-width of a single-track rail platform (METHOD.md §2.2 proposal, 2.5 m per track, until
/// W4 delivers track counts and formation widths; no measured provenance).
pub const RAIL_PLATFORM_HALF_WIDTH_M: f64 = 2.5;

/// How a rail row radiates around the track: omnidirectional until W4's D1 emission, fitted with
/// the CNOSSOS-EU track dipole and two source heights, lands; the kernels take
/// `LineDirectivity::TrackDipole` (CUDA `SOURCE_FLAG_TRACK_DIPOLE`) then.
pub const RAIL_SOURCE_DIRECTIVITY: crate::propagation::line_quadrature::LineDirectivity =
    crate::propagation::line_quadrature::LineDirectivity::Omnidirectional;

/// Gs of (2.5.14) under a rail source: ballast is porous (1); embedded tram track and a bridge
/// deck are hard (0) (#26: the deck is Gs only). Track form defaults by rail type until W4
/// supplies it.
pub fn rail_source_ground_factor(rail_type: RailType, on_bridge: bool) -> f64 {
    if on_bridge || rail_type == RailType::Tram {
        0.0
    } else {
        1.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RawRailInput {
    pub rail_type: u8,
    pub maxspeed: u16,
    pub highspeed: bool,
    pub traffic: RailTraffic,
}

#[derive(Debug, Clone)]
pub struct NormalizedRail {
    pub rail_type: RailType,
    pub source_height_m: f64,
    pub speed_kmh: f64,
    pub traffic: RailTraffic,
}

impl NormalizedRail {
    pub fn period_emissions(&self) -> ([f32; NUM_BANDS], [f32; NUM_BANDS], [f32; NUM_BANDS]) {
        let bands = self.traffic.periods().map(|(passenger, freight, hours)| {
            super::bands_to_f32(railway::railway_emission(
                self.rail_type,
                self.speed_kmh,
                passenger,
                freight,
                hours,
            ))
        });
        (bands[0], bands[1], bands[2])
    }

    /// How far the row reaches: where the surface relevance bound's Lden falls to the reach edge.
    pub fn reach_m(&self, weather: &crate::propagation::meteorology::Meteorology) -> f64 {
        railway::rail_reach_m(self.rail_type, self.speed_kmh, self.traffic, weather)
    }
}

pub fn normalize_rail(input: RawRailInput) -> NormalizedRail {
    let rail_type = RailType::from_u8(input.rail_type);
    NormalizedRail {
        rail_type,
        source_height_m: SOURCE_HEIGHT_RAIL,
        speed_kmh: if input.maxspeed > 0 {
            f64::from(input.maxspeed)
        } else if input.highspeed && !matches!(rail_type, RailType::Preserved) {
            300.0
        } else {
            railway::default_speed(rail_type)
        },
        traffic: input.traffic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heritage_has_no_ordinary_traffic_speed_or_emission_prior() {
        let kind = RailType::from_u8(5);
        for usage in [0, 1, 2, 3, 4] {
            for (iso, mode) in [(*b"DE", 0), (*b"FR", 1), (*b"XX", 3)] {
                assert_eq!(railway::default_traffic(kind, usage, iso, mode), (0.0, 0.0));
            }
        }
        assert_eq!(railway::default_speed(kind), 0.0);
        for highspeed in [false, true] {
            let rail = normalize_rail(RawRailInput {
                rail_type: 5,
                maxspeed: 0,
                highspeed,
                traffic: RailTraffic::default(),
            });
            assert_eq!(rail.speed_kmh, 0.0);
        }
        assert!(railway::railway_emission(kind, 40.0, 2.0, 0.0, 12.0)
            .iter()
            .all(|band| *band == f64::NEG_INFINITY));
        assert_eq!(crate::source_names::rail_type_name(5), "heritage");
    }

    #[test]
    fn prepared_zero_and_fractional_periods_are_never_defaulted_or_resplit() {
        let traffic = RailTraffic {
            passenger: RailCategoryTraffic {
                periods: [0.0, 0.125, 0.0],
                status: 2,
                source_id: 7,
                matching: 1,
            },
            freight: RailCategoryTraffic {
                periods: [0.0, 0.0, 0.25],
                status: 1,
                source_id: 8,
                matching: 0,
            },
        };
        let rail = normalize_rail(RawRailInput {
            rail_type: 0,
            maxspeed: 300,
            highspeed: false,
            traffic,
        });
        assert_eq!(rail.speed_kmh, 300.0);
        assert_eq!(rail.traffic, traffic);
        let emissions = rail.period_emissions();
        for (i, actual) in [emissions.0, emissions.1, emissions.2].iter().enumerate() {
            let (passenger, freight, hours) = traffic.periods()[i];
            assert_eq!(
                *actual,
                super::super::bands_to_f32(railway::railway_emission(
                    RailType::Rail,
                    300.0,
                    passenger,
                    freight,
                    hours
                ))
            );
        }
        assert!(RailTraffic::default().is_silent());
        assert_eq!(
            normalize_rail(RawRailInput {
                rail_type: 1,
                maxspeed: 0,
                highspeed: false,
                traffic: RailTraffic::default()
            })
            .speed_kmh,
            25.0
        );
        assert_eq!(
            normalize_rail(RawRailInput {
                rail_type: 0,
                maxspeed: 0,
                highspeed: true,
                traffic: RailTraffic::default()
            })
            .speed_kmh,
            300.0
        );
    }
}

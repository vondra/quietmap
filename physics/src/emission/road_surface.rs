//! The road surface correction of CNOSSOS-EU (Directive 2015/996 Annex II 2.2.6, Table F-4 of
//! Delegated Directive 2021/1226, the table that goes with the vehicle coefficients of
//! [`super::road`]): rolling noise of category m in octave band i takes alpha_i,m + beta_m lg(v / 70),
//! propulsion noise min(alpha_i,m; 0) (an absorbing surface quietens the engine, a hard one does not
//! raise it). The speed term holds at the edges of the speeds the row is valid for. Motorcycles
//! have no correction.

use super::road::{
    CategoryFlow, REFERENCE_AIR_TEMPERATURE_C, REFERENCE_SPEED_KMH, VehicleCategory,
    line_emission_db,
};
use crate::bands::{BANDS, a_weighted_energy, level_db};

/// A surface: per category (light, medium, heavy) the spectral correction of rolling noise at the
/// reference speed (dB per octave band, 63 Hz to 8 kHz) and its speed term (dB per decade), and the
/// speeds the table gives them for (km/h).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoadSurface {
    pub alpha: [[f64; BANDS]; 3],
    pub beta: [f64; 3],
    pub speeds_kmh: (f64, f64),
}

impl RoadSurface {
    /// The rolling and the propulsion correction (dB) of `category` at `speed_kmh` in `band`.
    pub fn corrections(
        &self,
        category: VehicleCategory,
        speed_kmh: f64,
        band: usize,
    ) -> (f64, f64) {
        let m = match category {
            VehicleCategory::Light => 0,
            VehicleCategory::Medium => 1,
            VehicleCategory::Heavy => 2,
            VehicleCategory::Motorcycle => return (0.0, 0.0),
        };
        let speed = speed_kmh.clamp(self.speeds_kmh.0, self.speeds_kmh.1);
        let alpha = self.alpha[m][band];
        (
            alpha + self.beta[m] * (speed / REFERENCE_SPEED_KMH).log10(),
            alpha.min(0.0),
        )
    }
}

/// The reference surface: dense asphalt concrete 0/11 and stone mastic asphalt 0/11, averaged.
pub const REFERENCE_SURFACE: RoadSurface = RoadSurface {
    alpha: [[0.0; BANDS]; 3],
    beta: [0.0; 3],
    speeds_kmh: (0.0, f64::INFINITY),
};

/// Brushed down concrete (NL06), 70-120 km/h.
pub const BRUSHED_CONCRETE: RoadSurface = RoadSurface {
    alpha: [
        [8.2, -0.4, 2.8, 2.7, 2.5, 0.8, -0.3, -0.1],
        [0.3, 4.5, 2.5, -0.2, -0.1, -0.5, -0.9, -0.8],
        [0.2, 5.3, 2.5, -0.2, -0.1, -0.6, -1.0, -0.9],
    ],
    beta: [1.4, 5.0, 5.5],
    speeds_kmh: (70.0, 120.0),
};

/// Hard elements in herring-bone (NL10): block pavers, 30-60 km/h.
pub const HARD_ELEMENTS_HERRINGBONE: RoadSurface = RoadSurface {
    alpha: [
        [27.0, 16.2, 14.7, 6.1, 3.0, -1.0, 1.2, 4.5],
        [29.5, 20.0, 17.6, 8.0, 6.2, -1.0, 3.1, 5.2],
        [29.4, 21.2, 18.2, 8.4, 5.6, -1.0, 3.0, 5.8],
    ],
    beta: [2.5, 2.5, 2.5],
    speeds_kmh: (30.0, 60.0),
};

/// Hard elements not in herring-bone (NL11), 30-60 km/h.
pub const HARD_ELEMENTS: RoadSurface = RoadSurface {
    alpha: [
        [31.4, 19.7, 16.8, 8.4, 7.2, 3.3, 7.8, 9.1],
        [34.0, 23.6, 19.8, 10.5, 11.7, 8.2, 12.2, 10.0],
        [33.8, 24.7, 20.4, 10.9, 10.9, 6.8, 12.0, 10.8],
    ],
    beta: [2.9, 2.9, 2.9],
    speeds_kmh: (30.0, 60.0),
};

/// What `surface` does to a car's A-weighted sound power at `speed_kmh` (dB): the number a road
/// shows for its surface.
pub fn car_effect_db(surface: &RoadSurface, speed_kmh: f64) -> f64 {
    let car = [CategoryFlow {
        vehicles_per_hour: 1.0,
        speed_kmh,
        category: VehicleCategory::Light,
        slope_percent: 0.0,
        junction: None,
        electric_share: 0.0,
    }];
    let level = |surface: &RoadSurface| {
        level_db(a_weighted_energy(&line_emission_db(
            &car,
            (surface, REFERENCE_AIR_TEMPERATURE_C),
        )))
    };
    level(surface) - level(&REFERENCE_SURFACE)
}

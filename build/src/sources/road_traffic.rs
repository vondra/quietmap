//! The daily traffic of the road rows nobody counted, from the buildings ([`crate::traffic`]): a
//! local street carries the trip ends routed down it to the nearest main road, no background and
//! no street more than the one it drains into; a main road of the classes the counts cover takes
//! the counted roads' relation between their traffic and the trip ends generated around them
//! (fitted on the counted rows of 13 European countries, evidence 2026-10-01, building traffic).
//! Squares whose buildings are not mapped keep dev4's guesses.

use crate::dev4::Square;
use crate::traffic::{SquareTraffic, Surroundings};
use std::path::Path;

/// Trip ends per kilometre of local street below which a square's buildings count as unmapped.
const MAPPED_TRIP_ENDS_PER_KM: f64 = 30.0;

/// Per main class (dev4 code): the radius of the trip ends that predict it (m), their cap, the
/// intercepts for built-up unknown, rural and urban, the slope on the log of the trip ends, and
/// the one-way shift (natural logs of vehicles a day). Secondary and tertiary roads weigh the 13
/// countries alike (country-balanced least squares, held out by square: 2.6 and 5.2 dB mean
/// absolute error against 3.4 and 5.3 for a class median); unclassified roads are fitted on
/// Great Britain's, France's and Sweden's counts (4.1 dB against 6.1). Primary roads keep
/// dev4's priors: the counted ones are national roads outside the cities, and their fit put
/// Barcelona's arterials at a third of their traffic.
///
/// The trip ends around are capped at the 90th percentile of the counted rows' (dense city cores
/// beyond it drive less per dwelling than the fit would extrapolate: Barcelona's and Paris's
/// stations read +4 dB too loud without the cap).
struct MainModel {
    class: u8,
    radius_m: f64,
    most_around: f64,
    intercept: [f64; 3],
    slope: f64,
    oneway: f64,
}

const MAIN_MODELS: [MainModel; 3] = [
    MainModel {
        class: 3,
        radius_m: 5_000.0,
        most_around: 617_000.0,
        intercept: [0.0, 4.358, 4.551],
        slope: 0.337,
        oneway: -0.073,
    },
    MainModel {
        class: 4,
        radius_m: 5_000.0,
        most_around: 446_000.0,
        intercept: [0.0, 2.936, 3.796],
        slope: 0.388,
        oneway: -0.170,
    },
    MainModel {
        class: 9,
        radius_m: 2_000.0,
        most_around: 242_000.0,
        intercept: [0.0, 2.160, 2.920],
        slope: 0.403,
        oneway: 0.408,
    },
];

/// The countries whose counts cover a whole class network (Sweden's NVDB every state road,
/// Czechia's census its I-III class roads, Great Britain's DfT every A and B road and a random
/// sample of the minor ones, Finland's every public road) shift the pooled model by their own
/// median residual (dB; secondary, tertiary; outside and inside built-up areas): Swedish tertiary
/// roads carry 6.7 dB less than the 13 countries' at the same trip ends. Czechia's are its
/// census rows across the country (254,359 secondary and 150,413 tertiary): a III-class road
/// through a village carries 1,807 a day in the median, where the pooled model put 2,750.
const COUNTRY_SHIFTS_DB: [([u8; 2], [[f64; 2]; 2]); 4] = [
    (*b"CZ", [[-1.8, -1.4], [0.0, -2.6]]),
    (*b"FI", [[-1.6, -1.6], [-4.4, -4.4]]),
    (*b"GB", [[0.9, 0.9], [-1.4, -1.4]]),
    (*b"SE", [[-0.8, -0.8], [-6.7, -6.7]]),
];

/// dev4 classes the trees run down (residential, living street, service) and unclassified,
/// which in a town also takes its class's relation to the trip ends around it.
const TREE_CLASSES: [u8; 3] = [5, 6, 7];
const UNCLASSIFIED: u8 = 9;
/// dev4's built-up code of a row inside a settlement's built-up area.
const URBAN: u8 = 2;

/// A square's building traffic as the conversion reads it.
pub struct BuildingTraffic {
    traffic: SquareTraffic,
    surroundings: Surroundings,
}

impl BuildingTraffic {
    /// The traffic of `square`'s rows, `None` where the square has no traffic file or its
    /// buildings are not mapped (`local_km`: the length of its local streets).
    pub fn load(dir: &Path, square: Square, local_km: f64) -> Result<Option<Self>, String> {
        let Some(traffic) = crate::traffic::read(dir, square)? else {
            return Ok(None);
        };
        let generated: f64 = traffic.grid.iter().map(|&v| f64::from(v)).sum();
        if local_km > 0.0 && generated / local_km < MAPPED_TRIP_ENDS_PER_KM {
            return Ok(None);
        }
        Ok(Some(BuildingTraffic {
            traffic,
            surroundings: Surroundings::load(dir, square)?,
        }))
    }

    /// The modelled daily total of a row whose traffic dev4 guessed, `None` to keep the guess:
    /// `class` (dev4 code), `built_up` (0 unknown, 1 rural, 2 urban), `middle` (z30).
    /// `prior` tells a class prior (no dataset) from a country default or classification
    /// fallback, whose totals stand on a main road (their splits do not).
    pub fn total(
        &self,
        row: usize,
        (class, built_up, oneway): (u8, u8, bool),
        (middle, country_iso, prior): ((i32, i32), u16, bool),
    ) -> Option<f64> {
        let flow = self
            .traffic
            .flows
            .get(row)
            .copied()
            .filter(|f| f.is_finite());
        let iso = country_iso.to_le_bytes();
        let shift_db = |class: u8| {
            let Some(shifts) = COUNTRY_SHIFTS_DB
                .iter()
                .find(|(code, _)| *code == iso)
                .and_then(|(_, shifts)| shifts.get(usize::from(class.checked_sub(3)?)))
            else {
                return 0.0;
            };
            match built_up {
                1 => shifts[0],
                URBAN => shifts[1],
                _ => 0.5 * (shifts[0] + shifts[1]),
            }
        };
        let main = |model: &MainModel| {
            let around = self
                .surroundings
                .around(middle, model.radius_m)
                .min(model.most_around);
            (around > 0.0).then(|| {
                (model.intercept[usize::from(built_up.min(2))]
                    + model.slope * around.ln()
                    + if oneway { model.oneway } else { 0.0 }
                    + shift_db(model.class) / 10.0 * std::f64::consts::LN_10)
                    .exp()
            })
        };
        let model = MAIN_MODELS.iter().find(|model| model.class == class);
        match (class, flow, model) {
            (c, Some(flow), _) if TREE_CLASSES.contains(&c) => Some(f64::from(flow)),
            // In a town an unclassified street also collects what passes through; outside one it
            // is a village's lane (the owner: the lane by the hotel in Bosen carries its 33 cars a
            // day, where the fit on British and French minor roads put 301).
            (UNCLASSIFIED, flow, Some(model)) => {
                let routed = flow.map_or(0.0, f64::from);
                if built_up == URBAN {
                    Some(main(model).map_or(routed, |related| related.max(routed)))
                } else {
                    Some(routed)
                }
            }
            (_, _, Some(model)) if prior => main(model),
            _ => None,
        }
    }
}

/// The trip ends within 2 km at which a bus line runs a city's service, the exponent and the
/// least share: a village among 4,000 trip ends runs 28 % of it (17 departures a direction).
const CITY_TRIP_ENDS: f64 = 100_000.0;
const SERVICE_EXPONENT: f64 = 0.4;
const LEAST_SERVICE: f64 = 0.15;

impl BuildingTraffic {
    /// The share of a city bus line's service a line runs at `middle` (z30).
    pub fn bus_service(&self, middle: (i32, i32)) -> f64 {
        let around = self.surroundings.around(middle, 2_000.0);
        (around / CITY_TRIP_ENDS)
            .powf(SERVICE_EXPONENT)
            .clamp(LEAST_SERVICE, 1.0)
    }
}

/// The share of a city bus line's service where the buildings are not known: by the row's
/// built-up flag (unknown, rural, urban).
pub const BUS_SERVICE_BY_BUILT_UP: [f64; 3] = [0.5, 0.2, 1.0];

/// The length of a square's open local streets (km), for [`BuildingTraffic::load`].
pub fn local_km(lengths: impl Iterator<Item = (u8, f64)>) -> f64 {
    lengths
        .filter(|(class, _)| TREE_CLASSES.contains(class) || *class == UNCLASSIFIED)
        .map(|(_, metres)| metres)
        .sum::<f64>()
        / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_classes_scale_with_the_trip_ends_around_them() {
        let tertiary = MAIN_MODELS.iter().find(|m| m.class == 4).unwrap();
        // Twice the trip ends around: 2^0.388 = 1.31 times the traffic.
        let at = |around: f64| (tertiary.intercept[2] + tertiary.slope * f64::ln(around)).exp();
        assert!((at(2e6) / at(1e6) - 2f64.powf(0.388)).abs() < 1e-9);
        // An urban tertiary road among a million trip ends: 9,500 vehicles a day.
        assert!((at(1e6) - 9_478.0).abs() < 5.0, "{}", at(1e6));
    }
}

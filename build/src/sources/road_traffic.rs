//! The daily traffic of the road rows nobody counted, from the buildings ([`crate::traffic`]): a
//! local street carries the trip ends routed down it to the nearest main road, no background and
//! no street more than the one it drains into, and in a town an unclassified street, or a
//! residential one that carries a bus line, at least what its trip ends around predict for an
//! unclassified street; a secondary, tertiary or unclassified road takes the counted roads'
//! relation between their traffic and the trip ends generated around them, and a motorway, trunk,
//! primary road or link that of the major-road model, in the countries whose counts it predicts
//! better than their class constants (the year run's building-road-models table,
//! [`super::road_model_fit`]). Class estimates are replaced, a country's own defaults stay, and a
//! count copied from a main road (`road_copies`) takes the model in every country. Squares whose
//! buildings are not mapped keep dev4's guesses, and a copy carries none.

use crate::dev4::Square;
use crate::source_tables::{BusModel, MainModel, RoadModels};
use crate::traffic::{SquareTraffic, Surroundings};
use std::path::Path;

const MAPPED_TRIP_ENDS_PER_KM: f64 = 30.0;
/// Trip ends added before the major-road model takes their logarithm, so a road among none has a
/// finite expectation (the 2026 fit's choice, kept for both years).
pub(crate) const MAJOR_TRIP_END_OFFSET: f64 = 100.0;

fn by_built_up([rural, urban]: [f64; 2], built_up: u8) -> f64 {
    match built_up {
        1 => rural,
        URBAN => urban,
        _ => 0.5 * (rural + urban),
    }
}

fn major_model_replaces(models: &RoadModels, guess: Guess, iso: [u8; 2]) -> bool {
    match guess {
        Guess::ClassPrior => models.major_countries.contains(&iso),
        Guess::Fallback => false,
        Guess::Copy => true,
    }
}

/// The major-road model's group of a class: motorway, trunk, primary, any link.
pub(crate) fn major_group(class: u8) -> Option<usize> {
    match class {
        0 => Some(0),
        1 => Some(1),
        2 => Some(2),
        10..=12 => Some(3),
        _ => None,
    }
}

/// A main-class model's daily traffic among `around` trip ends (capped at the counted rows' bound),
/// `None` among none.
pub(crate) fn main_prediction(
    model: &MainModel,
    around: f64,
    (built_up, oneway): (u8, bool),
) -> Option<f64> {
    let around = around.min(model.most_around);
    (around > 0.0).then(|| {
        (by_built_up(model.intercept, built_up)
            + model.slope * around.ln()
            + if oneway { model.oneway } else { 0.0 })
        .exp()
    })
}

/// The major-road model's daily traffic of a row of `group` among the trip ends `around` its
/// radii (`models.major_radii_m`).
pub(crate) fn major_group_prediction(
    models: &RoadModels,
    group: usize,
    around: [f64; 3],
    (built_up, oneway, lanes): (u8, bool, u8),
) -> f64 {
    let model = &models.major[group];
    let density: f64 = around
        .iter()
        .zip(model.slopes)
        .map(|(&trip_ends, slope)| slope * (trip_ends + MAJOR_TRIP_END_OFFSET).ln())
        .sum();
    let lanes_term = if lanes > 0 {
        models.major_ln_lanes * f64::from(lanes).ln()
    } else {
        models.major_lanes_unknown
    };
    let oneway_term = if oneway { models.major_oneway } else { 0.0 };
    (by_built_up(model.intercept, built_up) + density + lanes_term + oneway_term).exp()
}

pub fn thai_rural_road_ref(reference: &str) -> bool {
    reference.split(';').any(|part| {
        let Some((province, number)) = part.trim().split_once('.') else {
            return false;
        };
        let letters = province.chars().count();
        let number = number.trim();
        (1..=2).contains(&letters)
            && province
                .chars()
                .all(|c| ('\u{0E01}'..='\u{0E2E}').contains(&c))
            && number.len() == 4
            && number.bytes().all(|b| b.is_ascii_digit())
    })
}

const TREE_CLASSES: [u8; 3] = [5, 6, 7];
const RESIDENTIAL: u8 = 5;
const UNCLASSIFIED: u8 = 9;
const URBAN: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Guess {
    ClassPrior,
    Fallback,
    Copy,
}

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

    /// The trip ends within `radius_m` of the z30 point `middle`.
    pub fn around(&self, middle: (i32, i32), radius_m: f64) -> f64 {
        self.surroundings.around(middle, radius_m)
    }

    /// The world major-road model's daily traffic of a row: a motorway, trunk, primary road or
    /// link by its group, any other class as a primary road.
    pub fn major_prediction(
        &self,
        models: &RoadModels,
        middle: (i32, i32),
        (class, built_up, oneway, lanes): (u8, u8, bool, u8),
    ) -> f64 {
        let around = models
            .major_radii_m
            .map(|radius| self.surroundings.around(middle, radius));
        major_group_prediction(
            models,
            major_group(class).unwrap_or(2),
            around,
            (built_up, oneway, lanes),
        )
    }

    /// The modelled daily total of a row whose traffic dev4 guessed or whose count is a copy,
    /// `None` where no model replaces it (the conversion keeps a guess and drops a copy): `class`
    /// (dev4 code), `built_up` (0 unknown, 1 rural, 2 urban), `bus_line` (its way carries a bus,
    /// trolleybus or coach line), `middle` (z30). A fallback's total stands on a main road (its
    /// split does not); a copy takes the model of its class wherever there is one.
    pub fn total(
        &self,
        models: &RoadModels,
        row: usize,
        (class, built_up, oneway, bus_line): (u8, u8, bool, bool),
        (middle, country_iso, guess): ((i32, i32), u16, Guess),
        (thai_rural_road, lanes): (bool, u8),
    ) -> Option<f64> {
        let flow = self
            .traffic
            .flows
            .get(row)
            .copied()
            .filter(|f| f.is_finite());
        // A residential street that carries a bus line in a town is a through street its class
        // missed: in Great Britain's random sample of counted minor roads (DfT), 592 such streets
        // carry 1,820 vehicles a day in the median, which the trips routed down them read 8.2 dB
        // low and the town's unclassified streets' rule 0.5 dB (mean absolute error 8.8 -> 2.6
        // dB); its 3,368 residential streets without a line would read 4.1 dB high by that rule.
        // It takes a two-way street's traffic: those counted streets are all two-way, and the
        // other classes' one-way roads carry 0.83-0.93 of their two-way ones, where the
        // unclassified fit's one-way term (1.5 times) put 4,200 vehicles a day on Barcelona's
        // one-way back streets with a bus line.
        let (class, oneway) = if class == RESIDENTIAL && bus_line && built_up == URBAN {
            (UNCLASSIFIED, false)
        } else {
            (class, oneway)
        };
        let iso = country_iso.to_le_bytes();
        let main = |model: &MainModel| {
            main_prediction(
                model,
                self.around(middle, model.radius_m),
                (built_up, oneway),
            )
        };
        if major_group(class).is_some() {
            if !major_model_replaces(models, guess, iso) {
                return None;
            }
            return Some(self.major_prediction(models, middle, (class, built_up, oneway, lanes)));
        }
        let regional = models.regional_main.iter().find(|row| row.country == iso);
        let regional = regional.filter(|_| thai_rural_road);
        let model = regional
            .and_then(|row| row.models.iter().find(|model| model.class == class))
            .or_else(|| models.main.iter().find(|model| model.class == class));
        match (class, flow, model) {
            (c, Some(flow), _) if TREE_CLASSES.contains(&c) => Some(f64::from(flow)),
            // In a town an unclassified street also collects what passes through; outside one it
            // is a village's lane (one by a village hotel carries its 33 cars a day, where the fit
            // on British and French minor roads put 301).
            (UNCLASSIFIED, flow, Some(model)) => {
                let routed = flow.map_or(0.0, f64::from);
                if built_up == URBAN {
                    Some(main(model).map_or(routed, |related| related.max(routed)))
                } else {
                    Some(routed)
                }
            }
            (_, _, Some(model)) if guess == Guess::ClassPrior => main(model),
            _ => None,
        }
    }
}

impl BuildingTraffic {
    /// The share of [`super::bus`]'s buses a street's lines run at `middle` (z30).
    pub fn bus_service(&self, model: &BusModel, middle: (i32, i32)) -> f64 {
        bus_service_share(model, self.bus_trip_ends(middle))
    }

    /// The trip ends within 2 km of `middle` (z30) that set a street's bus service.
    pub fn bus_trip_ends(&self, middle: (i32, i32)) -> f64 {
        self.surroundings.around(middle, 2_000.0)
    }
}

pub(crate) fn bus_service_share(model: &BusModel, trip_ends: f64) -> f64 {
    (trip_ends.min(model.most_trip_ends) / model.city_trip_ends)
        .powf(model.service_exponent)
        .max(model.least_service)
}

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
    use crate::source_tables::SourceTables;

    #[test]
    fn a_streets_bus_service_grows_with_its_trip_ends_between_its_floor_and_its_bound() {
        let model = &SourceTables::test_fixture().bus_model;
        assert_eq!(bus_service_share(model, 100_000.0), 1.0);
        assert!((bus_service_share(model, 4_000.0) - 0.217).abs() < 0.001);
        assert_eq!(bus_service_share(model, 0.0), model.least_service);
        assert!((bus_service_share(model, 600_000.0) - 2.338).abs() < 0.001);
        assert_eq!(
            bus_service_share(model, 5e6),
            bus_service_share(model, 600_000.0)
        );
    }

    #[test]
    fn major_roads_grow_with_the_buildings_and_lanes_around() {
        let models = &SourceTables::test_fixture().road_models;
        let model = &models.major[2];
        let at = |boxes: [f64; 3], lanes: f64| {
            (model.intercept[1]
                + model
                    .slopes
                    .iter()
                    .zip(boxes)
                    .map(|(s, trip_ends)| s * (trip_ends + 100.0).ln())
                    .sum::<f64>()
                + models.major_ln_lanes * lanes.ln())
            .exp()
        };
        // A two-lane urban primary road in a city (50,000 trip ends within 1 km, 500,000 within
        // 5 km, 2 M within 15 km): about 9,600 a day, like the counted ones of dense European
        // areas (12,000-14,000 across both directions); four lanes 1.5 times that.
        let city = [5e4, 5e5, 2e6];
        assert!(
            (8_000.0..12_000.0).contains(&at(city, 2.0)),
            "{}",
            at(city, 2.0)
        );
        assert!((at(city, 4.0) / at(city, 2.0) - 2f64.powf(models.major_ln_lanes)).abs() < 1e-9);
        let village = [500.0, 5e3, 5e4];
        assert!(
            at(village, 2.0) < at(city, 2.0) / 2.0,
            "{}",
            at(village, 2.0)
        );
        assert!(models.major_countries.windows(2).all(|w| w[0] < w[1]));
        assert!(!models.major_countries.contains(b"TH") && models.major_countries.contains(b"MX"));
        assert_eq!(major_group(11), Some(3));
        assert_eq!(major_group(3), None);
    }

    /// A ramp whose count is its main road's takes the model, in Thailand too; a class prior only
    /// where the model beat the priors, a country default's total never.
    #[test]
    fn a_copied_count_takes_the_major_model_everywhere() {
        let models = &SourceTables::test_fixture().road_models;
        assert!(major_model_replaces(models, Guess::Copy, *b"CZ"));
        assert!(major_model_replaces(models, Guess::Copy, *b"TH"));
        assert!(major_model_replaces(models, Guess::ClassPrior, *b"CZ"));
        assert!(!major_model_replaces(models, Guess::ClassPrior, *b"TH"));
        assert!(!major_model_replaces(models, Guess::Fallback, *b"CZ"));
    }

    #[test]
    fn thai_rural_road_refs_are_told_from_other_refs() {
        let models = &SourceTables::test_fixture().road_models;
        for reference in ["สฎ.6038", "กบ. 4038", "นธ.6037;3060", "อ.1234"] {
            assert!(thai_rural_road_ref(reference), "{reference}");
        }
        for reference in ["4169", "สฎ.603", "Sukhumvit", "", "สฎ6038", "AH2"] {
            assert!(!thai_rural_road_ref(reference), "{reference}");
        }
        // The network's tertiary roads carry about 1,900 a day at the counted rows' median
        // 33,000 trip ends within 5 km in the country, 2,600 in towns at 50,000; its secondary
        // roads 3,700 at their median 53,000.
        let at = |model: &MainModel, built_up: u8, around: f64| {
            (by_built_up(model.intercept, built_up) + model.slope * around.ln()).exp()
        };
        let regional = &models.regional_main[0].models;
        let [secondary, tertiary] = [&regional[0], &regional[1]];
        for (value, range) in [
            (at(tertiary, 1, 32_700.0), 1_700.0..2_000.0),
            (at(tertiary, 2, 50_000.0), 2_400.0..2_800.0),
            (at(secondary, 1, 53_400.0), 3_400.0..4_000.0),
        ] {
            assert!(range.contains(&value), "{value}");
        }
    }

    /// A residential street that carries a bus line in a town collects what passes through, as the
    /// town's unclassified streets do, and keeps its routed trips where they are more; without a
    /// line, outside a town or where the built-up area is unknown, and as another local class, its
    /// routed trips alone.
    #[test]
    fn a_residential_bus_street_in_a_town_takes_the_unclassified_streets_traffic() {
        let models = &SourceTables::test_fixture().road_models;
        let square = Square { x: 276, y: 173 };
        let grids = std::collections::HashMap::from([(
            square,
            vec![10.0f32; crate::traffic::GRID_SIDE * crate::traffic::GRID_SIDE],
        )]);
        let traffic = BuildingTraffic {
            traffic: SquareTraffic {
                flows: vec![59.0, 4_000.0],
                grid: Vec::new(),
            },
            surroundings: Surroundings::from_grids(square, &grids),
        };
        // The square's middle: z30 eastings from the west, northings from the south.
        let middle = ((276 << 21) + (1 << 20), ((512 - 174) << 21) + (1 << 20));
        let czechia = u16::from_le_bytes(*b"CZ");
        let total = |row, class, built_up, oneway, bus_line| {
            traffic
                .total(
                    models,
                    row,
                    (class, built_up, oneway, bus_line),
                    (middle, czechia, Guess::ClassPrior),
                    (false, 0),
                )
                .unwrap()
        };
        let unclassified = total(0, UNCLASSIFIED, URBAN, false, false);
        assert!((300.0..4_000.0).contains(&unclassified), "{unclassified}");
        assert_eq!(total(0, RESIDENTIAL, URBAN, false, true), unclassified);
        assert_eq!(total(1, RESIDENTIAL, URBAN, false, true), 4_000.0);
        // One way, it takes the two-way street's traffic, not the one-way unclassified street's.
        assert_eq!(total(0, RESIDENTIAL, URBAN, true, true), unclassified);
        assert!(total(0, UNCLASSIFIED, URBAN, true, false) > unclassified);
        assert_eq!(total(0, RESIDENTIAL, URBAN, false, false), 59.0);
        for built_up in [0, 1] {
            assert_eq!(total(0, RESIDENTIAL, built_up, false, true), 59.0);
        }
        for local_class in [6, 7] {
            assert_eq!(total(0, local_class, URBAN, false, true), 59.0);
        }
    }

    #[test]
    fn main_classes_scale_with_the_trip_ends_around_them() {
        let models = &SourceTables::test_fixture().road_models;
        let tertiary = models.main.iter().find(|m| m.class == 4).unwrap();
        // Twice the trip ends around: 2^0.388 = 1.31 times the traffic.
        let at = |around: f64| (tertiary.intercept[1] + tertiary.slope * f64::ln(around)).exp();
        assert!((at(2e6) / at(1e6) - 2f64.powf(0.388)).abs() < 1e-9);
        // An urban tertiary road among a million trip ends: 9,500 vehicles a day.
        assert!((at(1e6) - 9_478.0).abs() < 5.0, "{}", at(1e6));
    }
}

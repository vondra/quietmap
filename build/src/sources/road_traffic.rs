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

/// The countries whose counts cover a class network widely (Sweden's NVDB every state road,
/// Great Britain's DfT every A and B road and a random sample of the minor ones, Finland's every
/// public road; Czechia's census its I and II class roads, but only the busier fifth of its III
/// class roads by length) shift the pooled model by their own median residual (dB; secondary,
/// tertiary; outside and inside built-up areas): Swedish tertiary roads carry 6.7 dB less than
/// the 13 countries' at the same trip ends. Czechia's are its census rows across the country
/// (254,359 secondary and 150,413 tertiary): a counted III-class road through a village carries
/// 1,807 a day in the median, where the pooled model put 2,750.
const COUNTRY_SHIFTS_DB: [([u8; 2], [[f64; 2]; 2]); 4] = [
    (*b"CZ", [[-1.8, -1.4], [0.0, -2.6]]),
    (*b"FI", [[-1.6, -1.6], [-4.4, -4.4]]),
    (*b"GB", [[0.9, 0.9], [-1.4, -1.4]]),
    (*b"SE", [[-0.8, -0.8], [-6.7, -6.7]]),
];

/// The world's counted major roads (motorway, trunk, primary and their links): 3.36 M rows in
/// 933 squares of 20 countries (US, Europe, Japan, New Zealand, Colombia, Chile, Mexico), every
/// country weighed alike. The daily traffic of a row follows the trip ends of the buildings
/// within 1, 5 and 15 km (square boxes; every building, the footprints OSM lacks among them) by
/// road group, the class and built-up area, its lanes and a one-way carriageway: held out a whole
/// country at a time, 2.27 dB mean absolute error against 3.00 for the class priors, every one of
/// the 19 fitted countries better (Germany 2.61 -> 1.64, the US 3.36 -> 2.33, the Netherlands
/// 3.88 -> 2.17, Colombia 2.85 -> 2.08). Mexico, left out of the fit, reads 2.86 dB against its
/// priors' 3.09 (+1.3 dB low; 4.77 when only OSM's buildings made trip ends, a fifth of Mexico
/// City's), so the model replaces the class priors there too; Thailand's department roads read
/// 3.12 against the priors' 2.75 (5.07 with OSM's buildings alone) and keep theirs.
struct MajorModel {
    /// Natural-log intercepts, rural and urban (unknown built-up the mean).
    intercept: [f64; 2],
    /// Slopes on ln(trip ends + 100) within `MAJOR_RADII_M`.
    slopes: [f64; 3],
}

const MAJOR_RADII_M: [f64; 3] = [1_000.0, 5_000.0, 15_000.0];

/// Motorways, trunks, primary roads, their links.
const MAJOR_MODELS: [MajorModel; 4] = [
    MajorModel {
        intercept: [2.9621, 2.8953],
        slopes: [0.0072, 0.0176, 0.4403],
    },
    MajorModel {
        intercept: [3.7332, 3.6682],
        slopes: [0.0410, 0.0836, 0.2731],
    },
    MajorModel {
        intercept: [4.3867, 4.4465],
        slopes: [0.0284, 0.0984, 0.1884],
    },
    MajorModel {
        intercept: [-0.7808, -0.8148],
        slopes: [-0.0303, 0.2955, 0.3932],
    },
];
/// The model's one-way term, its elasticity to the lanes and its term for unknown lanes.
const MAJOR_ONEWAY: f64 = -0.1874;
const MAJOR_LN_LANES: f64 = 0.5401;
const MAJOR_LANES_UNKNOWN: f64 = -0.0209;
/// The countries where the model replaces the class priors: those where it beat them with the
/// country held out, and the rest of Europe (UN M49), where every one of the 13 European
/// countries held out did (Austria, outside the fit: 1.53 dB against the priors' 12.2).
const MAJOR_MODEL_COUNTRIES: [[u8; 2]; 58] = [
    *b"AD", *b"AL", *b"AT", *b"AX", *b"BA", *b"BE", *b"BG", *b"BY", *b"CH", *b"CL", *b"CO", *b"CZ",
    *b"DE", *b"DK", *b"EE", *b"ES", *b"FI", *b"FO", *b"FR", *b"GB", *b"GG", *b"GI", *b"GR", *b"HR",
    *b"HU", *b"IE", *b"IM", *b"IS", *b"IT", *b"JE", *b"JP", *b"LI", *b"LT", *b"LU", *b"LV", *b"MC",
    *b"MD", *b"ME", *b"MK", *b"MT", *b"MX", *b"NL", *b"NO", *b"NZ", *b"PL", *b"PT", *b"RO", *b"RS",
    *b"RU", *b"SE", *b"SI", *b"SJ", *b"SK", *b"SM", *b"UA", *b"US", *b"VA", *b"XK",
];

/// The model group of a dev4 class: motorway, trunk, primary, a link of any of them.
fn major_group(class: u8) -> Option<usize> {
    match class {
        0 => Some(0),
        1 => Some(1),
        2 => Some(2),
        10..=12 => Some(3),
        _ => None,
    }
}

/// Thailand's Department of Rural Roads network (refs such as "สฎ.6038": a province's
/// abbreviation and four digits): the network's counted roads (DRR AADT 2024 on 480,221 tertiary
/// and 32,824 secondary rows in 122 and 75 squares) against the trip ends of every building, the
/// footprints OSM lacks among them. Fitted with every square weighed alike, the trip ends capped at
/// the counted rows' 90th percentile; held out by square, 3.0 dB mean absolute error (secondary
/// 2.7). Secondary roads follow the buildings as Europe's do (slope 0.39, Europe's 0.34; 3,700 a
/// day at the median 53,000 trip ends within 5 km, Europe's model 3,100), tertiary ones carry 2.4
/// dB more than Europe's at the same trip ends (1,900 at 33,000). With OSM's buildings alone (a
/// tenth to a half of Thailand's) the roads seemed to carry their traffic whatever the buildings
/// (slopes 0.13 and 0.05). An uncounted road of the network takes this fit; Thailand's other
/// uncounted roads (municipal streets without a ref) keep the pooled model.
const THAI_RURAL_ROAD_MODELS: [MainModel; 2] = [
    MainModel {
        class: 3,
        radius_m: 5_000.0,
        most_around: 628_000.0,
        intercept: [3.982, 3.921, 4.043],
        slope: 0.394,
        oneway: -0.357,
    },
    MainModel {
        class: 4,
        radius_m: 5_000.0,
        most_around: 145_000.0,
        intercept: [4.734, 4.627, 4.841],
        slope: 0.279,
        oneway: 0.212,
    },
];

/// Whether an OSM ref names a road of Thailand's rural road network: one or two Thai letters, a
/// full stop and four digits ("สฎ.6038", "กบ. 4038"), alone or among several refs.
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

/// Counted tertiary roads are the busier ones wherever counting selects them (Czechia's census
/// counts 21 % of its III-class roads by length around Bosen, Germany's and Poland's 14 and 20 %
/// of their tertiary roads in the sampled squares, reading 5 dB over the pooled model), and the
/// pooled model is fitted on them. Where whole networks or random samples are counted, tertiary
/// roads carry less at the same trip ends, most where buildings are few (dB from the pooled
/// model, median of Sweden, Finland and Great Britain by the trip ends within 5 km: -5.8 under
/// 20,000, -4.9, -3.9, -2.8 and -1.9 over 300,000; evidence 2026-10-02, routes). An uncounted
/// tertiary road in a European country with selective counts takes that shift, interpolated in
/// the log of its trip ends; elsewhere the pooled model is not fitted on the country's own
/// counted roads (Bangkok's sois carry more than it says, Thailand's stations).
const UNSELECTED_TERTIARY_SHIFT_DB: [(f64, f64); 5] = [
    (10_000.0, -5.8),
    (32_000.0, -4.9),
    (77_000.0, -3.9),
    (190_000.0, -2.8),
    (550_000.0, -1.9),
];
/// European countries (UN M49) whose tertiary roads the pooled model's counts come from or
/// resemble; Finland, Great Britain and Sweden count theirs whole or at random (their own shift).
const SELECTIVE_TERTIARY_COUNTS: [[u8; 2]; 41] = [
    *b"AD", *b"AL", *b"AT", *b"BA", *b"BE", *b"BG", *b"BY", *b"CH", *b"CZ", *b"DE", *b"DK", *b"EE",
    *b"ES", *b"FR", *b"GR", *b"HR", *b"HU", *b"IE", *b"IS", *b"IT", *b"LI", *b"LT", *b"LU", *b"LV",
    *b"MC", *b"MD", *b"ME", *b"MK", *b"MT", *b"NL", *b"NO", *b"PL", *b"PT", *b"RO", *b"RS", *b"RU",
    *b"SI", *b"SK", *b"SM", *b"UA", *b"VA",
];

/// The shift of an uncounted tertiary road among `around` trip ends within 5 km (dB).
fn unselected_tertiary_shift_db(around: f64) -> f64 {
    let points = UNSELECTED_TERTIARY_SHIFT_DB;
    if around <= points[0].0 {
        return points[0].1;
    }
    for pair in points.windows(2) {
        let [(x0, y0), (x1, y1)] = [pair[0], pair[1]];
        if around <= x1 {
            return y0 + (y1 - y0) * (around / x0).ln() / (x1 / x0).ln();
        }
    }
    points[points.len() - 1].1
}

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

    /// The world major-road model's daily traffic of a row: a motorway, trunk, primary road or
    /// link by its group, any other class as a primary road.
    pub fn major_prediction(
        &self,
        middle: (i32, i32),
        (class, built_up, oneway, lanes): (u8, u8, bool, u8),
    ) -> f64 {
        let model = &MAJOR_MODELS[major_group(class).unwrap_or(2)];
        let intercept = match built_up {
            1 => model.intercept[0],
            URBAN => model.intercept[1],
            _ => 0.5 * (model.intercept[0] + model.intercept[1]),
        };
        let density: f64 = MAJOR_RADII_M
            .iter()
            .zip(model.slopes)
            .map(|(&radius, slope)| slope * (self.surroundings.around(middle, radius) + 100.0).ln())
            .sum();
        let lanes_term = if lanes > 0 {
            MAJOR_LN_LANES * f64::from(lanes).ln()
        } else {
            MAJOR_LANES_UNKNOWN
        };
        let oneway_term = if oneway { MAJOR_ONEWAY } else { 0.0 };
        (intercept + density + lanes_term + oneway_term).exp()
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
        (thai_rural_road, lanes): (bool, u8),
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
                let selection_db = if model.class == 4 && SELECTIVE_TERTIARY_COUNTS.contains(&iso) {
                    unselected_tertiary_shift_db(around)
                } else {
                    0.0
                };
                (model.intercept[usize::from(built_up.min(2))]
                    + model.slope * around.ln()
                    + if oneway { model.oneway } else { 0.0 }
                    + (shift_db(model.class) + selection_db) / 10.0 * std::f64::consts::LN_10)
                    .exp()
            })
        };
        if major_group(class).is_some() {
            if !(prior && MAJOR_MODEL_COUNTRIES.contains(&iso)) {
                return None;
            }
            return Some(self.major_prediction(middle, (class, built_up, oneway, lanes)));
        }
        let models: &[MainModel] = if thai_rural_road && iso == *b"TH" {
            &THAI_RURAL_ROAD_MODELS
        } else {
            &MAIN_MODELS
        };
        let model = models
            .iter()
            .find(|model| model.class == class)
            .or_else(|| MAIN_MODELS.iter().find(|model| model.class == class));
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
    fn major_roads_grow_with_the_buildings_and_lanes_around() {
        let model = &MAJOR_MODELS[2];
        let at = |boxes: [f64; 3], lanes: f64| {
            (model.intercept[1]
                + model
                    .slopes
                    .iter()
                    .zip(boxes)
                    .map(|(s, trip_ends)| s * (trip_ends + 100.0).ln())
                    .sum::<f64>()
                + MAJOR_LN_LANES * lanes.ln())
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
        assert!((at(city, 4.0) / at(city, 2.0) - 2f64.powf(MAJOR_LN_LANES)).abs() < 1e-9);
        let village = [500.0, 5e3, 5e4];
        assert!(
            at(village, 2.0) < at(city, 2.0) / 2.0,
            "{}",
            at(village, 2.0)
        );
        assert!(MAJOR_MODEL_COUNTRIES.windows(2).all(|w| w[0] < w[1]));
        assert!(!MAJOR_MODEL_COUNTRIES.contains(b"TH") && MAJOR_MODEL_COUNTRIES.contains(b"MX"));
        assert_eq!(major_group(11), Some(3));
        assert_eq!(major_group(3), None);
    }

    #[test]
    fn thai_rural_road_refs_are_told_from_other_refs() {
        for reference in ["สฎ.6038", "กบ. 4038", "นธ.6037;3060", "อ.1234"] {
            assert!(thai_rural_road_ref(reference), "{reference}");
        }
        for reference in ["4169", "สฎ.603", "Sukhumvit", "", "สฎ6038", "AH2"] {
            assert!(!thai_rural_road_ref(reference), "{reference}");
        }
        // The network's tertiary roads carry about 1,900 a day at the counted rows' median
        // 33,000 trip ends within 5 km in the country, 2,600 in towns at 50,000; its secondary
        // roads 3,700 at their median 53,000.
        let at = |model: &MainModel, b: usize, around: f64| {
            (model.intercept[b] + model.slope * around.ln()).exp()
        };
        let [secondary, tertiary] = &THAI_RURAL_ROAD_MODELS;
        for (value, range) in [
            (at(tertiary, 1, 32_700.0), 1_700.0..2_000.0),
            (at(tertiary, 2, 50_000.0), 2_400.0..2_800.0),
            (at(secondary, 1, 53_400.0), 3_400.0..4_000.0),
        ] {
            assert!(range.contains(&value), "{value}");
        }
    }

    #[test]
    fn uncounted_tertiary_roads_shift_most_where_buildings_are_few() {
        assert_eq!(unselected_tertiary_shift_db(5_000.0), -5.8);
        assert!((unselected_tertiary_shift_db(77_000.0) + 3.9).abs() < 1e-9);
        let between = unselected_tertiary_shift_db(120_000.0);
        assert!(between < -2.8 && between > -3.9, "{between}");
        assert_eq!(unselected_tertiary_shift_db(2e6), -1.9);
        assert!(SELECTIVE_TERTIARY_COUNTS.windows(2).all(|w| w[0] < w[1]));
        assert!(!SELECTIVE_TERTIARY_COUNTS.contains(b"SE"));
        assert!(SELECTIVE_TERTIARY_COUNTS.contains(b"CZ"));
    }

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

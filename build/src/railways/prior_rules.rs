//! Evaluate manifest-selected corridor priors against source attributes and prepared railway geometry.
use super::network::{distance, distance_to_line};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "test", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Predicate {
    All {
        conditions: Vec<Predicate>,
    },
    Any {
        conditions: Vec<Predicate>,
    },
    Not {
        condition: Box<Predicate>,
    },
    Types {
        values: Vec<u8>,
    },
    Usage {
        value: u8,
    },
    Box {
        bounds: [f64; 4],
    },
    NearLine {
        coordinates: Vec<[f64; 2]>,
        metres: f64,
    },
    NearPoint {
        coordinates: [f64; 2],
        metres: f64,
    },
    CloserLine {
        first: Vec<[f64; 2]>,
        second: Vec<[f64; 2]>,
    },
    Name {
        contains: Vec<String>,
    },
    Attribute {
        name: String,
        values: Vec<String>,
    },
    Contains {
        name: String,
        values: Vec<String>,
    },
    AtLeast {
        name: String,
        value: f64,
    },
    Above {
        name: String,
        value: f64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub when: Predicate,
    pub passenger: f64,
    pub freight: f64,
}

pub struct Observation<'a> {
    pub point: [f64; 2],
    pub rail_type: u8,
    pub usage: u8,
    pub name: &'a str,
    pub attributes: &'a BTreeMap<String, String>,
}

impl Predicate {
    pub fn valid(&self) -> bool {
        let valid_line = |coordinates: &[[f64; 2]]| {
            coordinates.len() >= 2 && coordinates.iter().all(|point| valid_point(*point))
        };
        let valid_radius = |metres: f64| metres.is_finite() && metres >= 0.0;
        match self {
            Self::All { conditions } | Self::Any { conditions } => {
                conditions.iter().all(Self::valid)
            }
            Self::Not { condition } => condition.valid(),
            Self::Types { values } => !values.is_empty() && values.iter().all(|value| *value <= 5),
            Self::Usage { value } => *value <= 3,
            Self::Box { bounds } => valid_bounds(*bounds),
            Self::NearLine {
                coordinates,
                metres,
            } => valid_line(coordinates) && valid_radius(*metres),
            Self::NearPoint {
                coordinates,
                metres,
            } => valid_point(*coordinates) && valid_radius(*metres),
            Self::CloserLine { first, second } => valid_line(first) && valid_line(second),
            Self::Name { contains } => !contains.is_empty(),
            Self::Attribute { name, values } | Self::Contains { name, values } => {
                !name.is_empty() && !values.is_empty()
            }
            Self::AtLeast { name, value } | Self::Above { name, value } => {
                !name.is_empty() && value.is_finite()
            }
        }
    }

    pub fn matches(&self, row: &Observation) -> bool {
        match self {
            Self::All { conditions } => conditions.iter().all(|condition| condition.matches(row)),
            Self::Any { conditions } => conditions.iter().any(|condition| condition.matches(row)),
            Self::Not { condition } => !condition.matches(row),
            Self::Types { values } => values.contains(&row.rail_type),
            Self::Usage { value } => row.usage == *value,
            Self::Box { bounds } => {
                (bounds[0]..=bounds[2]).contains(&row.point[0])
                    && (bounds[1]..=bounds[3]).contains(&row.point[1])
            }
            Self::NearLine {
                coordinates,
                metres,
            } => distance_to_line(row.point, coordinates) <= *metres,
            Self::NearPoint {
                coordinates,
                metres,
            } => distance(row.point, *coordinates) <= *metres,
            Self::CloserLine { first, second } => {
                distance_to_line(row.point, first) <= distance_to_line(row.point, second)
            }
            Self::Name { contains } => contains
                .iter()
                .any(|part| row.name.to_lowercase().contains(part)),
            Self::Attribute { name, values } => values
                .iter()
                .any(|value| value == row.attributes.get(name).map(String::as_str).unwrap_or("")),
            Self::Contains { name, values } => values.iter().any(|value| {
                row.attributes
                    .get(name)
                    .is_some_and(|text| text.to_lowercase().contains(value))
            }),
            Self::AtLeast { name, value } => {
                row.attributes
                    .get(name)
                    .and_then(|text| text.parse::<f64>().ok())
                    .unwrap_or(0.0)
                    >= *value
            }
            Self::Above { name, value } => {
                row.attributes
                    .get(name)
                    .and_then(|text| text.parse::<f64>().ok())
                    .unwrap_or(0.0)
                    > *value
            }
        }
    }
}

fn valid_point(point: [f64; 2]) -> bool {
    (-90.0..=90.0).contains(&point[0]) && (-180.0..=180.0).contains(&point[1])
}

pub fn valid_bounds(bounds: [f64; 4]) -> bool {
    valid_point([bounds[0], bounds[1]])
        && valid_point([bounds[2], bounds[3]])
        && bounds[0] <= bounds[2]
        && bounds[1] <= bounds[3]
}

pub fn classify(rules: &[Rule], row: &Observation) -> Option<[f64; 2]> {
    rules
        .iter()
        .find(|rule| rule.when.matches(row))
        .map(|rule| [rule.passenger, rule.freight])
}

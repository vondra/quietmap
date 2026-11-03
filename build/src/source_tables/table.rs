//! One JSON row format for every table; country keys are readable ISO alpha-2 codes.

use crate::year::YearInputs;
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table<Row> {
    rows: Vec<Row>,
}

pub fn read_rows<Row: DeserializeOwned>(
    inputs: &YearInputs,
    kind: crate::year::Kind,
) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for path in inputs.paths(kind)? {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let table: Table<Row> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if table.rows.is_empty() {
            return Err(format!("{kind}: {}: empty required table", path.display()));
        }
        rows.extend(table.rows);
    }
    Ok(rows)
}

pub fn single_row<Row: DeserializeOwned>(
    inputs: &YearInputs,
    kind: crate::year::Kind,
) -> Result<Row, String> {
    let mut rows = read_rows(inputs, kind)?;
    if rows.len() != 1 {
        return Err(format!("{kind}: expected exactly one row"));
    }
    Ok(rows.pop().expect("one required row"))
}

pub fn country_code(code: &str) -> Result<[u8; 2], String> {
    let bytes = code.as_bytes();
    if bytes.len() == 2 && bytes.iter().all(u8::is_ascii_uppercase) {
        Ok([bytes[0], bytes[1]])
    } else {
        Err(format!(
            "invalid country code {code:?}; expected ISO alpha-2"
        ))
    }
}

pub struct CountryTable<Value> {
    countries: BTreeMap<[u8; 2], Value>,
    regions: Vec<(Vec<[u8; 2]>, Value)>,
}

impl<Value> Default for CountryTable<Value> {
    fn default() -> Self {
        Self {
            countries: BTreeMap::new(),
            regions: Vec::new(),
        }
    }
}

impl<Value: DeserializeOwned> CountryTable<Value> {
    pub fn read(inputs: &YearInputs, kind: crate::year::Kind) -> Result<Self, String> {
        let rows = read_rows::<CountryRow<Value>>(inputs, kind)?;
        Self::from_rows(rows).map_err(|error| format!("{kind}: {error}"))
    }

    fn from_rows(rows: Vec<CountryRow<Value>>) -> Result<Self, String> {
        let mut table = Self::default();
        let mut regional_members = std::collections::BTreeSet::new();
        let mut world = false;
        for row in rows {
            match (row.country, row.countries) {
                (Some(country), None) => {
                    if table
                        .countries
                        .insert(country_code(&country)?, row.value)
                        .is_some()
                    {
                        return Err(format!("repeated country {country}"));
                    }
                }
                (None, Some(countries)) => {
                    let members = countries
                        .iter()
                        .map(|c| country_code(c))
                        .collect::<Result<Vec<_>, _>>()?;
                    if members.iter().any(|c| !regional_members.insert(*c))
                        || (members.is_empty() && world)
                    {
                        return Err("overlapping regional defaults".into());
                    }
                    world |= members.is_empty();
                    table.regions.push((members, row.value));
                }
                _ => return Err("row needs country or countries, exclusively".into()),
            }
        }
        Ok(table)
    }

    #[cfg(test)]
    pub fn fixture(json: &str) -> Self {
        Self::from_rows(serde_json::from_str(json).unwrap()).unwrap()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CountryRow<Value> {
    country: Option<String>,
    countries: Option<Vec<String>>,
    value: Value,
}

impl<Value> CountryTable<Value> {
    pub fn get(&self, country: [u8; 2]) -> Option<&Value> {
        self.countries
            .get(&country)
            .or_else(|| {
                self.regions
                    .iter()
                    .find(|(members, _)| members.contains(&country))
                    .map(|(_, v)| v)
            })
            .or_else(|| {
                self.regions
                    .iter()
                    .find(|(members, _)| members.is_empty())
                    .map(|(_, v)| v)
            })
    }
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.countries
            .values()
            .chain(self.regions.iter().map(|(_, value)| value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_service_regions_are_medians_of_the_measured_members() {
        let table = CountryTable::<f64>::read(
            &YearInputs::product_tables(),
            crate::year::Kind::ServiceCooling,
        )
        .unwrap();
        assert_eq!(table.countries.len(), 28);
        for (members, median) in &table.regions {
            let mut measured = members
                .iter()
                .filter_map(|code| table.countries.get(code))
                .copied()
                .collect::<Vec<_>>();
            measured.sort_by(f64::total_cmp);
            assert!(!measured.is_empty());
            let middle = measured.len() / 2;
            let expected = if measured.len() % 2 == 1 {
                measured[middle]
            } else {
                (measured[middle - 1] + measured[middle]) / 2.0
            };
            assert_eq!((median * 1e4).round(), (expected * 1e4).round());
        }
    }
}

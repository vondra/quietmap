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
    kind: &str,
) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for path in inputs.paths(kind)? {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let table: Table<Row> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if table.rows.is_empty() {
            return Err(format!(
                "{}: empty table; declare the missing input in the manifest",
                path.display()
            ));
        }
        rows.extend(table.rows);
    }
    Ok(rows)
}

pub fn single_row<Row: DeserializeOwned>(
    inputs: &YearInputs,
    kind: &str,
) -> Result<Option<Row>, String> {
    let mut rows = read_rows(inputs, kind)?;
    if rows.len() > 1 {
        return Err(format!("{kind}: expected at most one row"));
    }
    Ok(rows.pop())
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

pub struct CountryTable<Value>(BTreeMap<[u8; 2], Value>);

impl<Value> Default for CountryTable<Value> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<Value: DeserializeOwned> CountryTable<Value> {
    pub fn read(inputs: &YearInputs, kind: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CountryRow<Value> {
            country: String,
            value: Value,
        }
        let mut table = BTreeMap::new();
        for row in read_rows::<CountryRow<Value>>(inputs, kind)? {
            if table
                .insert(country_code(&row.country)?, row.value)
                .is_some()
            {
                return Err(format!("{kind}: repeated country {}", row.country));
            }
        }
        Ok(Self(table))
    }
}

impl<Value> CountryTable<Value> {
    pub fn get(&self, country: [u8; 2]) -> Option<&Value> {
        self.0.get(&country)
    }
    #[cfg(test)]
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.0.values()
    }
}

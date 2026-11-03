//! Road-reference counts and vehicle mixes joined through the year's administrative subdivision grids.

use super::table::{country_code, read_rows};
use crate::year::YearInputs;
use serde::Deserialize;
use std::collections::BTreeMap;

type RoadKey = ([u8; 2], u16, u8);
type VehicleKey = ([u8; 2], u8, usize);
pub type ReferenceCount = (u16, u8, f64, [f64; 4]);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubdivisionGrid {
    country: String,
    north: f64,
    west: f64,
    step: f64,
    row_starts: Vec<u16>,
    runs: Vec<(u16, u8)>,
}

impl SubdivisionGrid {
    fn at(&self, lat: f64, lon: f64) -> u8 {
        let row = ((self.north - lat) / self.step).floor();
        let column = ((lon - self.west) / self.step).floor();
        if !(0.0..(self.row_starts.len() - 1) as f64).contains(&row) || column < 0.0 {
            return 0;
        }
        let row = row as usize;
        let runs =
            &self.runs[usize::from(self.row_starts[row])..usize::from(self.row_starts[row + 1])];
        let index = runs.partition_point(|&(end, _)| usize::from(end) <= column as usize);
        runs.get(index).map_or(0, |&(_, subdivision)| subdivision)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.step.is_finite()
            || self.step <= 0.0
            || !self.north.is_finite()
            || !self.west.is_finite()
            || self.row_starts.len() < 2
            || self.row_starts.first() != Some(&0)
            || self.row_starts.last().copied().map(usize::from) != Some(self.runs.len())
            || !self.row_starts.windows(2).all(|pair| pair[0] <= pair[1])
        {
            return Err(format!("{}: invalid subdivision grid", self.country));
        }
        for rows in self.row_starts.windows(2) {
            let runs = &self.runs[usize::from(rows[0])..usize::from(rows[1])];
            if !runs.windows(2).all(|pair| pair[0].0 < pair[1].0) {
                return Err(format!(
                    "{}: subdivision runs must be ordered",
                    self.country
                ));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct RegionalRoads {
    grids: BTreeMap<[u8; 2], SubdivisionGrid>,
    counts: BTreeMap<RoadKey, ReferenceCount>,
    vehicles: BTreeMap<VehicleKey, [[f64; 4]; 2]>,
}

impl RegionalRoads {
    pub fn read(inputs: &YearInputs) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Count {
            country: String,
            number: u16,
            subdivision: u8,
            daily: f64,
            shares: [f64; 4],
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Vehicles {
            country: String,
            subdivision: u8,
            classes: Vec<usize>,
            shares: [[f64; 4]; 2],
        }
        let mut table = Self::default();
        for grid in read_rows::<SubdivisionGrid>(inputs, "subdivision-grids")? {
            grid.validate()?;
            if table
                .grids
                .insert(country_code(&grid.country)?, grid)
                .is_some()
            {
                return Err("repeated subdivision grid".into());
            }
        }
        for row in read_rows::<Count>(inputs, "road-reference-counts")? {
            let country = country_code(&row.country)?;
            let key = (country, row.number, row.subdivision);
            if row.daily <= 0.0
                || row.shares.iter().any(|share| !(0.0..=1.0).contains(share))
                || (row.shares.iter().sum::<f64>() - 1.0).abs() >= 0.003
            {
                return Err(format!("invalid road-reference count {key:?}"));
            }
            if table
                .counts
                .insert(key, (row.number, row.subdivision, row.daily, row.shares))
                .is_some()
            {
                return Err(format!("duplicate road-reference count {key:?}"));
            }
        }
        for row in read_rows::<Vehicles>(inputs, "regional-vehicle-shares")? {
            let country = country_code(&row.country)?;
            for shares in row.shares {
                if shares.iter().any(|share| !(0.0..=1.0).contains(share))
                    || (shares.iter().sum::<f64>() - 1.0).abs() > 1e-6
                {
                    return Err(format!("invalid vehicle shares in {}", row.country));
                }
            }
            for class in row.classes {
                if class > 12
                    || table
                        .vehicles
                        .insert((country, row.subdivision, class), row.shares)
                        .is_some()
                {
                    return Err(format!("invalid or repeated road class in {}", row.country));
                }
            }
        }
        Ok(table)
    }

    pub fn subdivision(&self, country: [u8; 2], lat: f64, lon: f64) -> u8 {
        self.grids.get(&country).map_or(0, |grid| grid.at(lat, lon))
    }

    pub fn reference_count(
        &self,
        country: [u8; 2],
        reference: &str,
        lat: f64,
        lon: f64,
    ) -> Option<&ReferenceCount> {
        let number = reference
            .split(';')
            .map(str::trim)
            .find(|part| {
                (1..=4).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
            })?
            .parse()
            .ok()?;
        self.counts
            .get(&(country, number, self.subdivision(country, lat, lon)))
    }

    pub fn vehicle_shares(
        &self,
        country: [u8; 2],
        lat: f64,
        lon: f64,
        class: usize,
        built_up: u8,
    ) -> Option<[f64; 4]> {
        let [rural, urban] =
            *self
                .vehicles
                .get(&(country, self.subdivision(country, lat, lon), class))?;
        Some(match built_up {
            1 => rural,
            2 => urban,
            _ => std::array::from_fn(|k| 0.5 * (rural[k] + urban[k])),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_tables::SourceTables;

    #[test]
    fn the_admitted_grid_joins_the_right_counts_and_vehicle_mixes() {
        let tables = &SourceTables::test_fixture().regional_roads;
        for (country, latitude, longitude, subdivision) in [
            (*b"TH", 13.75, 100.50, 10),
            (*b"TH", 9.47, 99.95, 84),
            (*b"TH", 18.79, 98.98, 50),
            (*b"TH", 11.0, 101.5, 0),
            (*b"US", 41.14, -104.82, 56),
            (*b"US", 34.05, -118.24, 6),
            (*b"US", 33.52, -86.80, 1),
            (*b"US", 19.43, -99.13, 0),
        ] {
            assert_eq!(
                tables.subdivision(country, latitude, longitude),
                subdivision
            );
        }
        assert!(
            tables
                .reference_count(*b"TH", "4169", 9.47, 99.95)
                .unwrap()
                .3[3]
                > 0.3
        );
        assert_eq!(
            tables
                .reference_count(*b"TH", "AH2;4", 13.08, 99.95)
                .unwrap()
                .0,
            4
        );
        assert!(
            tables
                .reference_count(*b"TH", "336", 13.79, 100.59)
                .is_none()
        );
        let wyoming = tables.vehicle_shares(*b"US", 41.14, -104.82, 0, 1).unwrap();
        let california = tables.vehicle_shares(*b"US", 34.05, -118.24, 0, 2).unwrap();
        assert!(wyoming[1] + wyoming[2] > 0.4 && california[1] + california[2] < 0.15);
        assert!(tables.vehicle_shares(*b"US", 35.0, -50.0, 0, 1).is_none());
        assert!(
            RegionalRoads::default()
                .reference_count(*b"TH", "4169", 9.47, 99.95)
                .is_none()
        );
    }
}

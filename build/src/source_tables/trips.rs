//! Building trip observations used before the road traffic models can be fitted.
use super::{CountryTable, table};
use crate::year::{Kind, YearInputs};

pub struct TripRates {
    pub dwelling_trips: CountryTable<f64>,
    pub hotel_trip_rate: f64,
}

impl TripRates {
    pub fn read(inputs: &YearInputs) -> Result<Self, String> {
        Ok(Self {
            dwelling_trips: CountryTable::read(inputs, Kind::DwellingTrips)?,
            hotel_trip_rate: table::single_row(inputs, Kind::HotelTripRate)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    #[test]
    fn routing_can_precede_the_models_fitted_from_its_output() {
        let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(sources.join("model-year.json")).unwrap()).unwrap();
        manifest["inputs"].as_array_mut().unwrap().retain(|input| {
            matches!(
                input["kind"].as_str(),
                Some("dwelling-trips" | "hotel-trip-rate")
            )
        });
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("year.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let inputs = YearInputs::read(&path, &sources).unwrap();
        assert!(TripRates::read(&inputs).is_ok());
        assert!(super::super::SourceTables::read(&inputs).is_err());
    }
}

//! Year-specific facts loaded once; converters borrow them and keep their ordinary missing-data defaults.

mod activities;
mod models;
use activities::{ChurchBells, VenueSeating, WorshipCalls};
mod regional_roads;
pub use models::{BusModel, MainModel, RoadModels};
mod table;
use crate::year::YearInputs;
pub use regional_roads::RegionalRoads;
use serde::Deserialize;
pub use table::CountryTable;
use table::read_rows;

pub type PeriodShares = [[[f64; 3]; 2]; 3];
pub type ClassShares = [[[f64; 2]; 2]; 2];

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainScales {
    pub scales: [f64; 2],
    pub tier_weights: [f64; 3],
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plant {
    pub heating_units: f64,
    pub cooling_units: f64,
    pub central_share: f64,
    pub cooling_hours: Option<f64>,
}

impl Default for Plant {
    fn default() -> Self {
        Self {
            heating_units: 0.0,
            cooling_units: 0.16,
            central_share: 0.0,
            cooling_hours: None,
        }
    }
}

#[derive(Default)]
pub struct SourceTables {
    pub church_bells: CountryTable<ChurchBells>,
    pub venue_customs: CountryTable<crate::sources::venues::Customs>,
    pub venue_seating: Option<VenueSeating>,
    pub worship_calls: CountryTable<WorshipCalls>,
    pub solar_days: Vec<i64>,
    pub dwelling_trips: CountryTable<f64>,
    pub hotel_trip_rate: Option<f64>,
    pub road_models: Option<RoadModels>,
    pub bus_model: Option<BusModel>,
    pub electric_percent: CountryTable<f64>,
    pub european_freight_fleet: CountryTable<bool>,
    pub train_scales: CountryTable<TrainScales>,
    pub motorcycle_shares: CountryTable<[f64; 2]>,
    pub period_shares: CountryTable<PeriodShares>,
    pub tagged_speeds: CountryTable<[[u8; 2]; 8]>,
    pub legal_speeds: CountryTable<[u8; 4]>,
    pub heavy_speeds: CountryTable<f64>,
    pub class_priors: CountryTable<[[f64; 3]; 2]>,
    pub class_shares: CountryTable<ClassShares>,
    pub counted_class_shares: Vec<[[f64; 2]; 2]>,
    pub rural_free_flow: Option<f64>,
    pub freight_period_shares: Option<[f64; 3]>,
    pub plants: CountryTable<Plant>,
    pub service_cooling: CountryTable<f64>,
    pub regional_roads: RegionalRoads,
}

impl SourceTables {
    pub fn read(inputs: &YearInputs) -> Result<Self, String> {
        let tables = Self {
            church_bells: CountryTable::read(inputs, "church-bells")?,
            venue_customs: CountryTable::read(inputs, "venue-customs")?,
            venue_seating: table::single_row(inputs, "venue-seating")?,
            worship_calls: CountryTable::read(inputs, "worship-calls")?,
            solar_days: inputs.manifest.solar_days()?,
            dwelling_trips: CountryTable::read(inputs, "dwelling-trips")?,
            hotel_trip_rate: table::single_row(inputs, "hotel-trip-rate")?,
            road_models: table::single_row(inputs, "building-road-models")?,
            bus_model: table::single_row(inputs, "bus-models")?,
            electric_percent: CountryTable::read(inputs, "electric-car-share")?,
            european_freight_fleet: CountryTable::read(inputs, "rail-fleet-regions")?,
            train_scales: CountryTable::read(inputs, "train-scales")?,
            motorcycle_shares: CountryTable::read(inputs, "motorcycle-share")?,
            period_shares: CountryTable::read(inputs, "road-period-shares")?,
            tagged_speeds: CountryTable::read(inputs, "tagged-speeds")?,
            legal_speeds: CountryTable::read(inputs, "legal-speeds")?,
            heavy_speeds: CountryTable::read(inputs, "heavy-speeds")?,
            class_priors: CountryTable::read(inputs, "road-class-priors")?,
            class_shares: CountryTable::read(inputs, "road-class-shares")?,
            counted_class_shares: read_rows(inputs, "counted-class-shares")?,
            rural_free_flow: table::single_row(inputs, "rural-free-flow")?,
            freight_period_shares: table::single_row(inputs, "freight-period-shares")?,
            plants: CountryTable::read(inputs, "building-plant")?,
            service_cooling: CountryTable::read(inputs, "service-cooling")?,
            regional_roads: RegionalRoads::read(inputs)?,
        };
        if !tables.counted_class_shares.is_empty() && tables.counted_class_shares.len() != 5 {
            return Err("counted-class-shares needs five road classes".into());
        }
        Ok(tables)
    }

    #[cfg(test)]
    pub fn test_fixture() -> &'static Self {
        use std::path::Path;
        use std::sync::OnceLock;
        static TABLES: OnceLock<SourceTables> = OnceLock::new();
        TABLES.get_or_init(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../years");
            // Historical regression values must not change when another year is added.
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/model-year.json");
            Self::read(&YearInputs::read(&path, &root).unwrap()).unwrap()
        })
    }
}

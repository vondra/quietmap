//! Year-specific facts loaded once; every required model kind must be admitted and present.

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

pub const MODEL_KINDS: &[crate::year::Kind] = &[
    crate::year::Kind::BuildingPlant,
    crate::year::Kind::BuildingRoadModels,
    crate::year::Kind::BusModels,
    crate::year::Kind::ChurchBells,
    crate::year::Kind::CountedClassShares,
    crate::year::Kind::DwellingTrips,
    crate::year::Kind::ElectricCarShare,
    crate::year::Kind::FreightPeriodShares,
    crate::year::Kind::HeavySpeeds,
    crate::year::Kind::HotelTripRate,
    crate::year::Kind::LegalSpeeds,
    crate::year::Kind::MotorcycleShare,
    crate::year::Kind::RailFleetRegions,
    crate::year::Kind::RegionalVehicleShares,
    crate::year::Kind::RoadClassPriors,
    crate::year::Kind::RoadClassShares,
    crate::year::Kind::RoadPeriodShares,
    crate::year::Kind::RoadPreparationModel,
    crate::year::Kind::RoadReferenceCounts,
    crate::year::Kind::RuralFreeFlow,
    crate::year::Kind::ServiceCooling,
    crate::year::Kind::SubdivisionGrids,
    crate::year::Kind::TaggedSpeeds,
    crate::year::Kind::TrainScales,
    crate::year::Kind::VenueCustoms,
    crate::year::Kind::VenueSeating,
    crate::year::Kind::WorshipCalls,
];

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
            // Brazil, Mexico and India household surveys: the world default where no region is known.
            cooling_units: 0.16,
            central_share: 0.0,
            cooling_hours: None,
        }
    }
}

pub struct SourceTables {
    pub church_bells: CountryTable<ChurchBells>,
    pub venue_customs: CountryTable<crate::sources::venues::Customs>,
    pub venue_seating: VenueSeating,
    pub worship_calls: CountryTable<WorshipCalls>,
    pub solar_days: Vec<i64>,
    pub dwelling_trips: CountryTable<f64>,
    pub hotel_trip_rate: f64,
    pub road_models: RoadModels,
    pub bus_model: BusModel,
    pub electric_percent: CountryTable<f64>,
    pub european_freight_fleet: CountryTable<bool>,
    pub train_scales: CountryTable<TrainScales>,
    pub motorcycle_shares: CountryTable<[f64; 2]>,
    pub period_shares: CountryTable<PeriodShares>,
    pub tagged_speeds: CountryTable<[[u8; 2]; 8]>,
    pub legal_speeds: CountryTable<[u8; 4]>,
    pub heavy_speeds: CountryTable<f64>,
    pub class_priors: CountryTable<[[f64; 3]; 2]>,
    pub road_preparation_priors: [[f64; 3]; 2],
    pub class_shares: CountryTable<ClassShares>,
    pub counted_class_shares: Vec<[[f64; 2]; 2]>,
    pub rural_free_flow: f64,
    pub freight_period_shares: [f64; 3],
    pub plants: CountryTable<Plant>,
    pub service_cooling: CountryTable<f64>,
    pub regional_roads: RegionalRoads,
}

impl SourceTables {
    pub fn read(inputs: &YearInputs) -> Result<Self, String> {
        let tables = Self {
            church_bells: CountryTable::read(inputs, crate::year::Kind::ChurchBells)?,
            venue_customs: CountryTable::read(inputs, crate::year::Kind::VenueCustoms)?,
            venue_seating: table::single_row(inputs, crate::year::Kind::VenueSeating)?,
            worship_calls: CountryTable::read(inputs, crate::year::Kind::WorshipCalls)?,
            solar_days: inputs.manifest.solar_days()?,
            dwelling_trips: CountryTable::read(inputs, crate::year::Kind::DwellingTrips)?,
            hotel_trip_rate: table::single_row(inputs, crate::year::Kind::HotelTripRate)?,
            road_models: table::single_row(inputs, crate::year::Kind::BuildingRoadModels)?,
            bus_model: table::single_row(inputs, crate::year::Kind::BusModels)?,
            electric_percent: CountryTable::read(inputs, crate::year::Kind::ElectricCarShare)?,
            european_freight_fleet: CountryTable::read(
                inputs,
                crate::year::Kind::RailFleetRegions,
            )?,
            train_scales: CountryTable::read(inputs, crate::year::Kind::TrainScales)?,
            motorcycle_shares: CountryTable::read(inputs, crate::year::Kind::MotorcycleShare)?,
            period_shares: CountryTable::read(inputs, crate::year::Kind::RoadPeriodShares)?,
            tagged_speeds: CountryTable::read(inputs, crate::year::Kind::TaggedSpeeds)?,
            legal_speeds: CountryTable::read(inputs, crate::year::Kind::LegalSpeeds)?,
            heavy_speeds: CountryTable::read(inputs, crate::year::Kind::HeavySpeeds)?,
            class_priors: CountryTable::read(inputs, crate::year::Kind::RoadClassPriors)?,
            road_preparation_priors: crate::roads::model::Model::read(inputs)
                .map_err(|error| format!("road-preparation-model: {error}"))?
                .native_class_priors(),
            class_shares: CountryTable::read(inputs, crate::year::Kind::RoadClassShares)?,
            counted_class_shares: read_rows(inputs, crate::year::Kind::CountedClassShares)?,
            rural_free_flow: table::single_row(inputs, crate::year::Kind::RuralFreeFlow)?,
            freight_period_shares: table::single_row(
                inputs,
                crate::year::Kind::FreightPeriodShares,
            )?,
            plants: CountryTable::read(inputs, crate::year::Kind::BuildingPlant)?,
            service_cooling: CountryTable::read(inputs, crate::year::Kind::ServiceCooling)?,
            regional_roads: RegionalRoads::read(inputs)?,
        };
        if tables.counted_class_shares.len() != 5 {
            return Err("counted-class-shares needs five road classes".into());
        }
        for groups in tables.period_shares.values() {
            for shares in groups.iter().flatten() {
                if shares.iter().any(|v| !v.is_finite() || *v < 0.0)
                    || (shares.iter().sum::<f64>() - 1.0).abs() >= 0.002
                {
                    return Err(
                        "road-period-shares: shares must be non-negative and sum to one".into(),
                    );
                }
            }
        }
        Ok(tables)
    }

    #[cfg(test)]
    pub fn test_fixture() -> &'static Self {
        use std::sync::OnceLock;
        static TABLES: OnceLock<SourceTables> = OnceLock::new();
        TABLES.get_or_init(Self::fixture_owned)
    }

    #[cfg(test)]
    pub fn fixture_owned() -> Self {
        use std::path::Path;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
        Self::read(&YearInputs::read(&root.join("model-year.json"), &root).unwrap()).unwrap()
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn every_committed_model_table_parses_and_contains_the_expected_branch_inputs() {
        let tables = SourceTables::read(&YearInputs::product_tables()).unwrap();
        assert_eq!(tables.counted_class_shares.len(), 5);
        assert_eq!(
            tables.venue_seating.seats[1][1].to_bits(),
            (0.56_f64 * 40.0).to_bits()
        );
        assert!(tables.church_bells.get(*b"CZ").is_some());
        assert!(tables.worship_calls.get(*b"CN").unwrap().silent);
    }
}

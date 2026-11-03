//! Dated trip-generation and street-background parameters used while preparing uncounted road pieces.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemandModel {
    pub world: [f64; 2],
    pub countries: BTreeMap<String, [f64; 2]>,
    pub medium_share: f64,
    pub heavy_share: f64,
    pub default_footprint_m2: f64,
    pub trip_rates: Vec<TripRate>,
    pub street: StreetParameters,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreetParameters {
    pub residential_urban: f64,
    pub unclassified_urban: f64,
    pub rural: f64,
    pub through_factor: f64,
    pub demand_scale: f64,
    pub single_track_factor: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TripRate {
    #[serde(rename_all = "camelCase")]
    Dwellings {
        gfa_per_dwelling: f64,
        cap_dwellings: f64,
    },
    #[serde(rename_all = "camelCase")]
    Trips {
        basis: String,
        per100m2: f64,
        min_trips: f64,
        cap_trips: f64,
    },
    FixedTrips {
        trips: f64,
    },
    None,
}

impl DemandModel {
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        ensure!(
            self.trip_rates.len() == 14,
            "expected one trip rate per building class"
        );
        ensure!(
            self.default_footprint_m2.is_finite() && self.default_footprint_m2 > 0.0,
            "invalid footprint fallback"
        );
        let positive = |values: &[f64]| values.iter().all(|x| x.is_finite() && *x > 0.0);
        for &[moto, trips] in std::iter::once(&self.world).chain(self.countries.values()) {
            ensure!(
                positive(&[trips]) && moto.is_finite() && (0.0..=1.0).contains(&moto),
                "invalid country fleet"
            );
            ensure!(
                [self.medium_share, self.heavy_share]
                    .iter()
                    .all(|x| x.is_finite() && *x >= 0.0)
                    && self.medium_share + self.heavy_share + moto <= 1.0,
                "invalid local vehicle shares"
            );
        }
        for rate in &self.trip_rates {
            match rate {
                TripRate::Dwellings {
                    gfa_per_dwelling,
                    cap_dwellings,
                } => ensure!(
                    positive(&[*gfa_per_dwelling, *cap_dwellings]) && *cap_dwellings >= 1.0,
                    "invalid dwelling rate"
                ),
                TripRate::Trips {
                    basis,
                    per100m2,
                    min_trips,
                    cap_trips,
                } => ensure!(
                    ["gfa", "footprint"].contains(&basis.as_str())
                        && positive(&[*per100m2, *cap_trips])
                        && min_trips.is_finite()
                        && *min_trips >= 0.0
                        && min_trips <= cap_trips,
                    "invalid trip rate"
                ),
                TripRate::FixedTrips { trips } => ensure!(
                    trips.is_finite() && *trips >= 0.0,
                    "invalid fixed trip rate"
                ),
                TripRate::None => (),
            }
        }
        let p = &self.street;
        ensure!(
            positive(&[
                p.residential_urban,
                p.unclassified_urban,
                p.rural,
                p.through_factor,
                p.demand_scale,
                p.single_track_factor
            ]),
            "invalid street demand parameters"
        );
        Ok(())
    }
    pub fn fleet(&self, country: u16) -> [f64; 2] {
        let bytes = country.to_le_bytes();
        std::str::from_utf8(&bytes)
            .ok()
            .and_then(|c| self.countries.get(c))
            .copied()
            .unwrap_or(self.world)
    }

    pub fn building(&self, kind: u8, storeys: u8, area: Option<f64>) -> [f64; 2] {
        let footprint = area.unwrap_or(self.default_footprint_m2);
        let gfa = footprint * f64::from(storeys.max(1));
        match self
            .trip_rates
            .get(usize::from(kind))
            .unwrap_or(&self.trip_rates[0])
        {
            TripRate::Dwellings {
                gfa_per_dwelling,
                cap_dwellings,
            } => [
                (gfa / gfa_per_dwelling).floor().clamp(1.0, *cap_dwellings),
                0.0,
            ],
            TripRate::Trips {
                basis,
                per100m2,
                min_trips,
                cap_trips,
            } => {
                let area = if basis == "gfa" { gfa } else { footprint };
                [0.0, (area / 100.0 * per100m2).clamp(*min_trips, *cap_trips)]
            }
            TripRate::FixedTrips { trips } => [0.0, *trips],
            TripRate::None => [0.0, 0.0],
        }
    }
}

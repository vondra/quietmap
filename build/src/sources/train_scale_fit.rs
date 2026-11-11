//! Fit each country's factors on guessed train counts so its mapped heavy rail runs the national train-km.
use super::national_train_km::{NationalTrainKm, from_eurostat};
use super::rail_traffic::{Trains, column, run_trains};
use super::tent::{FreightNetwork, Tier};
use crate::dev4::{Dev4, Square, z30_corner_mercator_m};
use crate::source_tables::TrainScales;
use crate::year::{Kind, YearInputs};
use anyhow::{Context, Result, anyhow, ensure};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, Int32Type, UInt8Type, UInt16Type};
use chrono::Datelike;
use physics::emission::rail::RailType;
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tiles::sources::GUESSED_TRAIN_SOURCES;

/// Guessed freight per km of line off the TEN-T network, on its other freight lines and on a core
/// network corridor, relative: dev4 spread a country's freight evenly over every line, 24 trains a
/// day on the German corridors where the 19 Eisenbahn-Bundesamt noise monitors on them counted
/// 30-164 freight trains a day in 2023 (mean 107); weighed so, Germany's corridors carry 62 % of its
/// guessed freight train-km. A judgment on those counts, not a fit to them.
pub const TEN_T_FREIGHT_WEIGHTS: [f64; 3] = [0.5, 2.0, 4.0];
/// Countries whose timetabled rows run at least this share of their mapped passenger train-km
/// measure how far timetabled rows stray from the national train-km. On the 2026 network the
/// countries are either timetabled (defaults at most 0.30 of their passenger train-km) or run on
/// defaults (0.82 and more); any cut of the defaults' share from 0.20 to 0.30 gives a spread of
/// 0.082 (13 to 15 countries), 0.35 gives 0.076.
const MINIMUM_TIMETABLED_SHARE: f64 = 0.75;

/// A country's mapped heavy-rail train-km a day as the sources stage reads them: passenger
/// [observed, guessed], observed freight, and guessed freight by TEN-T tier.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MappedTrainKm {
    pub passenger: [f64; 2],
    pub freight_observed: f64,
    pub freight_guessed: [f64; 3],
}

impl MappedTrainKm {
    /// One row of `length_km` with its run's trains; a guessed count is one of [`GUESSED_TRAIN_SOURCES`].
    pub fn add(&mut self, length_km: f64, trains: &Trains, tier: Tier) {
        let guessed = |source: u16| GUESSED_TRAIN_SOURCES.contains(&source);
        let [passenger_source, freight_source] = trains.source;
        self.passenger[usize::from(guessed(passenger_source))] +=
            trains.passenger.iter().sum::<f64>() * length_km;
        let freight = trains.freight.iter().sum::<f64>() * length_km;
        if guessed(freight_source) {
            self.freight_guessed[tier as usize] += freight;
        } else {
            self.freight_observed += freight;
        }
    }

    fn merge(mut self, other: &Self) -> Self {
        for (sum, value) in self.passenger.iter_mut().zip(other.passenger) {
            *sum += value;
        }
        self.freight_observed += other.freight_observed;
        for (sum, value) in self.freight_guessed.iter_mut().zip(other.freight_guessed) {
            *sum += value;
        }
        self
    }
}

/// The median of |timetabled / national - 1| passenger train-km over the countries the timetables
/// cover: the part of a national residual left after the timetables that their own error explains.
pub(super) fn timetable_spread<'a>(
    countries: impl Iterator<Item = (&'a MappedTrainKm, &'a NationalTrainKm)>,
) -> Option<f64> {
    let mut deviations: Vec<f64> = countries
        .filter_map(|(mapped, national)| {
            let [observed, guessed] = mapped.passenger;
            let (_, national) = national.passenger?;
            (observed > 0.0
                && national > 0.0
                && observed >= MINIMUM_TIMETABLED_SHARE * (observed + guessed))
                .then(|| (observed / national - 1.0).abs())
        })
        .collect();
    deviations.sort_by(f64::total_cmp);
    let middle = deviations.len() / 2;
    match deviations.len() {
        0 => None,
        count if count % 2 == 1 => Some(deviations[middle]),
        _ => Some(0.5 * (deviations[middle - 1] + deviations[middle])),
    }
}

/// The factor on guessed passenger trains: the national passenger train-km the timetables leave,
/// over the guessed, never raised, trusted in proportion to how far that residual exceeds the
/// timetables' own `spread` (fully from one spread up). Where the timetables alone reach the
/// national total, it says nothing about the other lines and their defaults stand; without
/// timetables it is the national total over the guessed.
fn passenger_factor(mapped: &MappedTrainKm, national: Option<f64>, spread: Option<f64>) -> f64 {
    let [observed, guessed] = mapped.passenger;
    let Some(national) = national.filter(|_| guessed > 0.0) else {
        return 1.0;
    };
    let residual = national - observed;
    let fit = (residual / guessed).clamp(0.0, 1.0);
    let trust = match spread {
        _ if observed <= 0.0 => 1.0,
        Some(spread) if spread > 0.0 => (residual / (spread * observed)).clamp(0.0, 1.0),
        _ => 0.0,
    };
    1.0 - trust * (1.0 - fit)
}

/// The factors that bring a country's guessed trains to its national train-km: freight to the
/// goods train-km less the observed freight, weighted by tier (without a published total the mapped
/// guessed total stays and the weights only move it between tiers); passengers by
/// [`passenger_factor`]. `weights` are the TEN-T ones where the map covers the country, else one.
pub(super) fn fitted(
    mapped: &MappedTrainKm,
    national: &NationalTrainKm,
    weights: [f64; 3],
    spread: Option<f64>,
) -> TrainScales {
    let guessed_freight: f64 = mapped.freight_guessed.iter().sum();
    let weighted_freight: f64 = mapped
        .freight_guessed
        .iter()
        .zip(weights)
        .map(|(train_km, weight)| train_km * weight)
        .sum();
    let freight_total = national.freight.map_or(guessed_freight, |(_, train_km)| {
        (train_km - mapped.freight_observed).max(0.0)
    });
    let freight = if weighted_freight > 0.0 {
        freight_total / weighted_freight
    } else {
        1.0
    };
    let passenger = passenger_factor(
        mapped,
        national.passenger.map(|(_, train_km)| train_km),
        spread,
    );
    TrainScales {
        scales: [freight, passenger],
        tier_weights: weights,
    }
}

/// One prepared square's heavy rail: each row's run trains on its length, guessed freight at its tier.
fn square_train_km(
    dev4: &Dev4,
    network: &FreightNetwork,
    square: Square,
) -> Result<BTreeMap<[u8; 2], MappedTrainKm>> {
    let mut countries = BTreeMap::<[u8; 2], MappedTrainKm>::new();
    let Some(table) = dev4
        .table(square, "railways.arrow")
        .map_err(anyhow::Error::msg)?
    else {
        return Ok(countries);
    };
    super::rail::require_rail_stamps(&table).map_err(anyhow::Error::msg)?;
    let run = run_trains(&table).map_err(anyhow::Error::msg)?;
    let mut index = 0;
    for batch in &table.batches {
        let read = |name| column(batch, name).map_err(anyhow::Error::msg);
        let i32s = |name| read(name).map(|column| column.as_primitive::<Int32Type>());
        let (kind, country, length) = (
            read("rail_type")?.as_primitive::<UInt8Type>(),
            read("country_iso")?.as_primitive::<UInt16Type>(),
            read("length_m")?.as_primitive::<Float32Type>(),
        );
        let (start_x, start_y, end_x, end_y) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        for row in 0..batch.num_rows() {
            let trains = &run[index];
            index += 1;
            let iso = country.value(row).to_le_bytes();
            if RailType::from_code(kind.value(row)) != RailType::Rail || iso == [0, 0] {
                continue;
            }
            let tier = if GUESSED_TRAIN_SOURCES.contains(&trains.source[1]) {
                network.tier(
                    iso,
                    z30_corner_mercator_m(start_x.value(row), start_y.value(row)),
                    z30_corner_mercator_m(end_x.value(row), end_y.value(row)),
                )
            } else {
                Tier::Off
            };
            countries.entry(iso).or_default().add(
                f64::from(length.value(row)) / 1000.0,
                trains,
                tier,
            );
        }
    }
    Ok(countries)
}

/// `qm-build train-scale-table`: the year's `train-scales` table from its prepared railways, the
/// TEN-T network and the national train-km its manifest entry depends on; the support beside it.
pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "train-scale fit needs a new output path");
    let manifest = &inputs.manifest;
    let declared: Vec<_> = manifest
        .inputs
        .iter()
        .filter(|input| input.kind == Kind::TrainScales)
        .collect();
    ensure!(declared.len() == 1, "train-scales needs one declaration");
    let dependencies: Vec<_> = declared[0]
        .depends_on
        .iter()
        .map(|id| {
            manifest
                .inputs
                .iter()
                .find(|input| input.id == *id)
                .ok_or_else(|| anyhow!("train-scales depends on undeclared {id}"))
        })
        .collect::<Result<_>>()?;
    for kind in [Kind::Prepared, Kind::Tent, Kind::ModelObservations] {
        ensure!(
            dependencies
                .iter()
                .filter(|input| input.kind == kind)
                .count()
                == 1,
            "train-scales must depend on one {kind} input"
        );
    }
    let observations = dependencies
        .iter()
        .find(|input| input.kind == Kind::ModelObservations)
        .expect("checked above");
    let (first, last) = observations.observed.bounds().map_err(anyhow::Error::msg)?;
    let dataset: Value = serde_json::from_slice(&std::fs::read(
        inputs.by_id(&observations.id).map_err(anyhow::Error::msg)?,
    )?)?;
    let national = from_eurostat(&dataset, first.year()..=last.year())?;
    let network = FreightNetwork::load(inputs.required(Kind::Tent).map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    let dev4 = Dev4 {
        prepared: inputs
            .required(Kind::Prepared)
            .map_err(anyhow::Error::msg)?
            .into(),
        rasters: PathBuf::new(),
    };
    let squares = crate::railways::network::squares(&dev4.prepared).map_err(anyhow::Error::msg)?;
    // Summed in square order, so that a refit of the same state gives the same bytes.
    let by_square = squares
        .par_iter()
        .map(|&[x, y]| {
            let square = Square {
                x: u32::from(x),
                y: u32::from(y),
            };
            square_train_km(&dev4, &network, square)
                .with_context(|| format!("train-scale fit: square {x}/{y}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut mapped = BTreeMap::<[u8; 2], MappedTrainKm>::new();
    for (country, train_km) in by_square.iter().flatten() {
        let sum = mapped.entry(*country).or_default();
        *sum = sum.merge(train_km);
    }
    let spread = timetable_spread(
        mapped
            .iter()
            .filter_map(|(country, train_km)| Some((train_km, national.get(country)?))),
    );
    let mut rows = Vec::new();
    let mut support = Vec::new();
    for (country, train_km) in &mapped {
        let published = national.get(country).copied().unwrap_or_default();
        let covered = network.covers(*country);
        let guessed = train_km.passenger[1] + train_km.freight_guessed.iter().sum::<f64>();
        let code = String::from_utf8_lossy(country).into_owned();
        support.push(json!({"country": code, "mapped": {
            "passenger_observed": train_km.passenger[0], "passenger_guessed": train_km.passenger[1],
            "freight_observed": train_km.freight_observed, "freight_guessed_by_tier": train_km.freight_guessed},
            "national": {"passenger": published.passenger, "freight": published.freight}}));
        if guessed <= 0.0 || (published == NationalTrainKm::default() && !covered) {
            continue;
        }
        let weights = if covered {
            TEN_T_FREIGHT_WEIGHTS
        } else {
            [1.0; 3]
        };
        let value = fitted(train_km, &published, weights, spread);
        rows.push(json!({"country": code, "value": {"scales": value.scales, "tier_weights": value.tier_weights}}));
    }
    // Every other country keeps its guessed counts; the world row says so and keeps a table of a
    // selection without heavy rail in a published country readable.
    rows.push(
        json!({"countries": [], "value": {"scales": [1.0, 1.0], "tier_weights": [1.0, 1.0, 1.0]}}),
    );
    std::fs::write(output, serde_json::to_vec_pretty(&json!({"rows": rows}))?)?;
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&json!({
            "unit": "train-km a day", "squares": squares.len(),
            "ten_t_freight_weights": TEN_T_FREIGHT_WEIGHTS,
            "minimum_timetabled_share": MINIMUM_TIMETABLED_SHARE, "timetable_spread": spread,
            "national_years": [first.year(), last.year()], "countries": support}))?,
    )?;
    eprintln!(
        "train-scale-fit: {} squares, {} countries, {} rows",
        squares.len(),
        mapped.len(),
        rows.len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "train_scale_fit_tests.rs"]
mod tests;

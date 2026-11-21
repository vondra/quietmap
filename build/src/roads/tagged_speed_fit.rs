//! Fit signed-speed medians by the same country, road class and building-density cells used by the reader.
use crate::year::{Kind, YearInputs};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

// The retained tagged-speed estimator requires thirty mapped pieces per class/environment cell.
const MINIMUM_CELL_ROWS: u64 = 30;
type Histogram = BTreeMap<(u16, u8, u8, u8), u64>;

fn medians(histograms: &Histogram) -> BTreeMap<u16, [[u8; 2]; 8]> {
    let mut groups = BTreeMap::<(u16, u8, u8), Vec<(u8, u64)>>::new();
    for (&(country, class, area, speed), &count) in histograms {
        groups
            .entry((country, class, area))
            .or_default()
            .push((speed, count));
    }
    let mut countries = BTreeMap::new();
    for ((country, class, area), values) in groups {
        let count = values.iter().map(|(_, n)| n).sum::<u64>();
        if count < MINIMUM_CELL_ROWS {
            continue;
        }
        let mut seen = 0;
        for (speed, count_at_speed) in values {
            seen += count_at_speed;
            if seen > (count - 1) / 2 {
                countries.entry(country).or_insert([[0; 2]; 8])[usize::from(class - 2)]
                    [usize::from(area - 1)] = speed;
                break;
            }
        }
    }
    countries
}

/// Whether a road's posted speed counts in its cell: a main or local class the reader takes a median
/// for, a sign ([`super::signs::signed_speed`], derestricted roads out), in a known country.
fn counts_as_signed(road: &super::input::Road, fastest_legal_kmh: u8) -> bool {
    (2..=9).contains(&road.class)
        && super::signs::signed_speed(usize::from(road.class), road.speed_limit, fastest_legal_kmh)
            .is_some_and(|speed| speed != super::signs::DERESTRICTED_CODE)
        && road.country.0 != 0
}

pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(
        !output.exists(),
        "speed calibration needs a new output path"
    );
    let mapped = inputs
        .required(Kind::OsmGeography)
        .map_err(anyhow::Error::msg)?;
    let structures = inputs
        .required(Kind::Structures)
        .map_err(anyhow::Error::msg)?;
    let fastest_legal = super::signs::fastest_legal_kmh(
        &crate::source_tables::CountryTable::read(inputs, Kind::LegalSpeeds)
            .map_err(anyhow::Error::msg)?,
    );
    let squares = super::topology::squares(mapped)?
        .into_iter()
        .collect::<Vec<_>>();
    let classified = AtomicUsize::new(0);
    let per_square = squares
        .par_iter()
        .map(|(_, path)| {
            let mut roads = super::input::load(path)?
                .iter()
                .map(super::input::roads)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .filter(|road| counts_as_signed(road, fastest_legal))
                .collect::<Vec<_>>();
            super::settlement::classify(structures, &mut roads)?;
            let mut histogram = Histogram::new();
            for road in roads
                .into_iter()
                .filter(|road| (1..=2).contains(&road.built_up))
            {
                *histogram
                    .entry((road.country.0, road.class, road.built_up, road.speed_limit))
                    .or_default() += 1;
            }
            let done = classified.fetch_add(1, Ordering::Relaxed) + 1;
            if done.is_multiple_of(4096) {
                eprintln!("tagged-speed-fit: {done} squares");
            }
            Ok(histogram)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut histograms = Histogram::new();
    for (cell, count) in per_square.into_iter().flatten() {
        *histograms.entry(cell).or_default() += count;
    }
    let squares = squares
        .iter()
        .map(|(square, _)| [square.x, square.y])
        .collect::<Vec<_>>();
    let rows = medians(&histograms).into_iter().map(|(country, value)|
        json!({"country": String::from_utf8_lossy(&country.to_le_bytes()), "value": value}))
        .collect::<Vec<_>>();
    ensure!(
        !rows.is_empty(),
        "no tagged-speed cell has enough mapped pieces"
    );
    let counts = histograms
        .into_iter()
        .map(|((country, class, area, speed), count)| {
            json!([
                String::from_utf8_lossy(&country.to_le_bytes()),
                class,
                area,
                speed,
                count
            ])
        })
        .collect::<Vec<_>>();
    std::fs::write(output, serde_json::to_vec_pretty(&json!({"rows":rows}))?)?;
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(
            &json!({"minimum_cell_rows":MINIMUM_CELL_ROWS,"squares":squares,"counts":counts}),
        )?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_sparse_cell_is_unknown_and_the_mapped_built_up_cell_selects_the_median() {
        let country = u16::from_le_bytes(*b"CZ");
        let values = Histogram::from([
            ((country, 9, 1, 50), 20),
            ((country, 9, 1, 90), 10),
            ((country, 9, 2, 30), 29),
        ]);
        assert_eq!(medians(&values)[&country][7], [50, 0]);
    }

    /// A primary posted 5 km/h is no sign and counts in no cell (Comoros' rural tertiary roads
    /// took their 6 km/h median from such rows in 2024); posted 50 it counts, derestricted it does not.
    #[test]
    fn a_posted_value_that_is_no_sign_counts_in_no_cell() {
        let mut road = crate::roads::tests::road(1);
        road.country.0 = u16::from_le_bytes(*b"KM");
        road.class = 2;
        let mut counts = |speed: u8| {
            road.speed_limit = speed;
            counts_as_signed(&road, 140)
        };
        assert!(counts(50));
        assert!(!counts(5));
        assert!(!counts(super::super::signs::DERESTRICTED_CODE));
        assert!(!counts(0));
    }
}

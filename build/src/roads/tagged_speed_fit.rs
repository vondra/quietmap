//! Fit signed-speed medians by the same country, road class and building-density cells used by the reader.
use crate::year::{Kind, YearInputs};
use anyhow::{Result, ensure};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

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
    let mut histograms = Histogram::new();
    let mut squares = Vec::new();
    for (square, path) in super::topology::squares(mapped)? {
        let mut roads = super::input::load(&path)?
            .iter()
            .map(super::input::roads)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .filter(|road| {
                (2..=9).contains(&road.class)
                    && road.speed_limit > 0
                    && road.speed_limit < 255
                    && road.country.0 != 0
            })
            .collect::<Vec<_>>();
        super::settlement::classify(structures, &mut roads)?;
        for road in roads {
            if !(1..=2).contains(&road.built_up) {
                continue;
            }
            *histograms
                .entry((road.country.0, road.class, road.built_up, road.speed_limit))
                .or_default() += 1;
        }
        squares.push([square.x, square.y]);
        if squares.len() % 4096 == 0 {
            eprintln!("tagged-speed-fit: {} squares", squares.len());
        }
    }
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
}

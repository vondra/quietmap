//! Supplement missing monthly runway operations using the airport's observed directed rolls.

use super::Traffic;
use super::lines::Aeroways;
use super::operations::{Class, Operations, month_days};
use super::runways::{RollKey, Runways};
use physics::bands::PERIODS;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub type Rolls = BTreeMap<RollKey, [f64; PERIODS]>;

pub type ProviderRolls = [Rolls; 2];

#[path = "frequency_months.rs"]
mod monthly;
pub use monthly::Months;

/// The complete airport census, separately from the observed counts supplying taxi energy.
pub fn write_totals(
    aeroways: &Aeroways,
    observed: &[[f64; 3]],
    modelled: &[[f64; 3]],
    out: &Path,
) -> Result<(), String> {
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(out.join("airport-movements.jsonl")).map_err(|e| e.to_string())?,
    );
    for (index, airport) in aeroways.airports.iter().enumerate() {
        if modelled[index].iter().all(|value| *value == 0.0) {
            continue;
        }
        writeln!(
            file,
            "{}",
            json!({"airport": airport.key, "name": airport.name,
            "observed": observed[index], "modelled": modelled[index]})
        )
        .map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())
}

/// A total alone cannot reveal an arrival/departure sampling bias. Allocate its deficit to
/// restore flow balance (every aircraft arriving must eventually depart), adding only the missing
/// count and keeping every observed operation. Published splits constrain each side directly.
fn deficits(total: f64, arrivals: Option<u64>, observed: [f64; 2]) -> [f64; 2] {
    if let Some(arrivals) = arrivals {
        let targets = [arrivals as f64, total - arrivals as f64];
        return std::array::from_fn(|i| (targets[i] - observed[i]).max(0.0));
    }
    let missing = (total - observed.iter().sum::<f64>()).max(0.0);
    let sides = observed.map(|seen| (total / 2.0 - seen).max(0.0));
    let sum: f64 = sides.iter().sum();
    sides.map(|side| if sum > 0.0 { side * missing / sum } else { 0.0 })
}

impl Months {
    /// Keep the measured class, direction and local-period mixture. A month unseen at this
    /// airport uses only this same window's observed mixture, explicitly reported as such.
    /// Monthly deficits treat official counts as lower bounds; the annual estimate retains its
    /// original sampling weights. A narrower reporting scope cannot erase observed movements. An entirely unseen class remains unallocated; no runway or fleet is invented.
    pub fn supplement(
        &self,
        input: &Operations,
        traffic: &mut Traffic,
        aeroways: &Aeroways,
        runways: &Runways,
        out: &Path,
    ) -> Result<(), String> {
        let days = input.calendar_days()?;
        let airports: BTreeMap<_, _> = aeroways
            .lines
            .iter()
            .map(|line| {
                let index = line.airport as usize;
                (aeroways.airports[index].key.as_str(), index)
            })
            .collect();
        let by_airport = |rolls: &Rolls| {
            let mut found: BTreeMap<usize, Rolls> = BTreeMap::new();
            for (&key, &values) in rolls {
                found
                    .entry(runways.airport(key.runway) as usize)
                    .or_default()
                    .insert(key, values);
            }
            found
        };
        let observed = by_airport(&self.window_rates());
        let monthly: BTreeMap<_, _> = input
            .counts
            .iter()
            .map(|count| count.month.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|month| {
                let rates = by_airport(&self.rates(&month));
                (month, rates)
            })
            .collect();
        let path = out.join("operations.jsonl");
        let mut report =
            std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        for count in &input.counts {
            let mapped = airports.get(count.airport.as_str()).copied();
            let ambiguous = aeroways.ambiguous_airports.contains(&count.airport);
            let airport = mapped.filter(|_| !ambiguous);
            let select = |rolls: Option<&Rolls>, landing: bool| -> Vec<(RollKey, [f64; PERIODS])> {
                rolls
                    .into_iter()
                    .flatten()
                    .filter(|(key, _)| count.class.contains(key.class) && key.landing == landing)
                    .map(|(&key, &value)| (key, value))
                    .collect()
            };
            let sum = |rolls: &[(RollKey, [f64; PERIODS])]| {
                rolls.iter().flat_map(|(_, values)| values).sum::<f64>()
            };
            let calendar = month_days(&count.month)?;
            let sample_days = self.sampled_days(&count.month);
            let provider_periods = self.provider_periods(&count.month);
            let month_rolls = airport.and_then(|airport| monthly.get(&count.month)?.get(&airport));
            let window_rolls = airport.and_then(|airport| observed.get(&airport));
            let rates = [select(month_rolls, true), select(month_rolls, false)];
            let observed_month = rates.each_ref().map(|rolls| sum(rolls) * calendar);
            let missing = deficits(count.movements as f64, count.arrivals, observed_month);
            let mut added = [0.0; 2];
            let mut unallocated = [0.0; 2];
            let mut mixtures = ["none"; 2];
            for operation in 0..2 {
                if missing[operation] == 0.0 {
                    mixtures[operation] = "not-needed";
                    continue;
                }
                let landing = operation == 0;
                let mut template = rates[operation].clone();
                mixtures[operation] = if !provider_periods.contains(&"month") {
                    "window"
                } else if provider_periods.contains(&"window") {
                    "month/window"
                } else {
                    "month"
                };
                if sum(&template) == 0.0 {
                    template = select(window_rolls, landing);
                    mixtures[operation] = "window";
                }
                if sum(&template) == 0.0 {
                    template = select(window_rolls, !landing);
                    for (key, _) in &mut template {
                        key.landing = landing;
                    }
                    mixtures[operation] = "opposite-operation";
                }
                let template_total = sum(&template);
                if template_total == 0.0 {
                    mixtures[operation] = "none";
                    unallocated[operation] = missing[operation];
                    continue;
                }
                let scale = missing[operation] / (days * template_total);
                for (key, values) in &template {
                    let totals = traffic.rolls.entry(*key).or_default();
                    for (total, value) in totals.iter_mut().zip(values) {
                        *total += value * scale;
                    }
                    traffic.movements[runways.airport(key.runway) as usize][operation] +=
                        values.iter().sum::<f64>() * scale;
                }
                added[operation] = missing[operation];
            }
            writeln!(report, "{}", json!({
                "airport": count.airport, "month": count.month, "class": count.class,
                "source": count.source, "official": count.movements, "official_arrivals": count.arrivals,
                "operation_split": if count.arrivals.is_some() { "published" } else { "flow-balance" },
                "public_ok": input.sources[&count.source].public_ok,
                "sample_days": sample_days, "provider_periods": provider_periods,
                "observed": observed_month, "added": added, "unallocated": unallocated,
                "operation_order": ["arrivals", "departures"], "mixture": mixtures,
                "airport_mapped": mapped.is_some(), "airport_ambiguous": ambiguous,
                "includes_helicopters": count.class == Class::All || count.class == Class::Helicopter,
            })).map_err(|e| e.to_string())?;
        }
        report.flush().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
#[path = "frequency_tests.rs"]
mod tests;

//! Monthly provider rates use their own admitted days; an unsampled provider keeps its window rate.
use super::{ProviderRolls, Rolls};
use crate::airport::lines::Aeroways;
use crate::airport::runways::Runways;
use crate::boxes::Window;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

#[derive(Default)]
pub struct Month {
    samples: [usize; 2],
    rolls: ProviderRolls,
}

pub struct Months(BTreeMap<String, Month>);

fn add(target: &mut Rolls, source: &Rolls, weight: f64) {
    for (&key, values) in source {
        for (total, value) in target.entry(key).or_default().iter_mut().zip(values) {
            *total += value * weight;
        }
    }
}

impl Months {
    pub fn new(window: &Window) -> Self {
        let mut months: BTreeMap<String, Month> = BTreeMap::new();
        for (provider, days) in [&window.baseline_days, &window.increment_days]
            .iter()
            .enumerate()
        {
            for day in *days {
                months.entry(day[..7].into()).or_default().samples[provider] += 1;
            }
        }
        Self(months)
    }

    pub fn add(&mut self, day: &str, rolls: ProviderRolls) {
        let month = self.0.get_mut(&day[..7]).expect("an admitted day");
        for (target, source) in month.rolls.iter_mut().zip(rolls) {
            add(target, &source, 1.0);
        }
    }

    fn window(&self) -> Month {
        let mut window = Month::default();
        for month in self.0.values() {
            for provider in 0..2 {
                window.samples[provider] += month.samples[provider];
                add(&mut window.rolls[provider], &month.rolls[provider], 1.0);
            }
        }
        window
    }

    fn rates_for(&self, month: Option<&Month>, window: &Month) -> Rolls {
        let mut rates = Rolls::new();
        for provider in 0..2 {
            let observed = month
                .filter(|month| month.samples[provider] > 0)
                .unwrap_or(window);
            if observed.samples[provider] > 0 {
                add(
                    &mut rates,
                    &observed.rolls[provider],
                    1.0 / observed.samples[provider] as f64,
                );
            }
        }
        rates
    }

    pub fn rates(&self, month: &str) -> Rolls {
        self.rates_for(self.0.get(month), &self.window())
    }

    pub fn window_rates(&self) -> Rolls {
        let window = self.window();
        self.rates_for(None, &window)
    }

    pub fn sampled_days(&self, month: &str) -> [usize; 2] {
        self.0.get(month).map_or([0, 0], |month| month.samples)
    }

    /// Which period supplies each provider's rates; this is independent of its observed count.
    pub fn provider_periods(&self, month: &str) -> [&'static str; 2] {
        let sampled = self.sampled_days(month);
        std::array::from_fn(|provider| {
            if sampled[provider] > 0 {
                "month"
            } else if self.0.values().any(|month| month.samples[provider] > 0) {
                "window"
            } else {
                "none"
            }
        })
    }

    /// Raw provider counts and their denominators, before any official frequency is added.
    pub fn write(&self, aeroways: &Aeroways, runways: &Runways, out: &Path) -> Result<(), String> {
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(out.join("observed-months.jsonl")).map_err(|e| e.to_string())?,
        );
        for (month, observed) in &self.0 {
            for (provider, rolls) in observed.rolls.iter().enumerate() {
                for (key, values) in rolls {
                    writeln!(file, "{}", json!({"airport": aeroways.airports[runways.airport(key.runway) as usize].key,
                        "month": month, "provider": provider, "sample_days": observed.samples[provider],
                        "runway": key.runway, "forward": key.forward, "landing": key.landing,
                        "class": key.class, "period_counts": values})).map_err(|e| e.to_string())?;
                }
            }
        }
        file.flush().map_err(|e| e.to_string())
    }
}

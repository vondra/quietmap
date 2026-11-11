//! Screen a highway department's per-province counts against the mapped roads by 3-MAD rules.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A screening band is the median within this many scaled median absolute deviations.
const OUTLIER_DEVIATIONS: f64 = 3.0;
/// The median absolute deviation's scale to a normal distribution's standard deviation.
const MAD_TO_SIGMA: f64 = 1.4826;

/// A highway number and the subdivision its rows lie in.
pub(crate) type Pair = (u16, u8);

/// A numbered road's mapped length and the major-road model's expected vehicle-km a day there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Mapped {
    /// Cross-section km: a one-way carriageway counts half the road.
    pub km: f64,
    /// The carriageways' own expected traffic along their whole length.
    pub expected_vehicle_km: f64,
}

/// One pair's department record: vehicle-km a year (light, medium, heavy, motorcycles) and the
/// control sections behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Counted {
    pub vehicle_km: [f64; 4],
    pub sections: usize,
}

/// The median as the 2026 screening took it: the mean of the middle two of an even count; none
/// of no values (a band that admits nothing).
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        values[n / 2]
    } else {
        0.5 * (values[n / 2 - 1] + values[n / 2])
    }
}

/// The bounds of a value's 3-MAD band among `values`.
pub(crate) fn band(values: &[f64]) -> [f64; 2] {
    let centre = median(&mut values.to_vec());
    let spread = MAD_TO_SIGMA
        * median(
            &mut values
                .iter()
                .map(|v| (v - centre).abs())
                .collect::<Vec<_>>(),
        );
    [
        centre - OUTLIER_DEVIATIONS * spread,
        centre + OUTLIER_DEVIATIONS * spread,
    ]
}

fn within([low, high]: [f64; 2], value: f64) -> bool {
    (low..=high).contains(&value)
}

/// The kept highway-subdivision pairs with their daily traffic and shares, and the screening's
/// record. A highway is kept when its vehicle-km over the model's expectation and its mapped km
/// per control section lie in the country's 3-MAD bands; a subdivision of a highway of several
/// when its km per section and its ratio to the expectation, each against its highway's, lie in
/// the bands of all such subdivisions.
pub(crate) fn screen(
    counted: &BTreeMap<Pair, Counted>,
    mapped: &BTreeMap<Pair, Mapped>,
) -> (Vec<(Pair, f64, [f64; 4])>, Value) {
    let mut highways: BTreeMap<u16, (Counted, Mapped, Vec<Pair>)> = BTreeMap::new();
    for (&pair, record) in counted {
        let Some(map) = mapped.get(&pair).filter(|m| m.km > 0.0) else {
            continue;
        };
        if record.vehicle_km.iter().sum::<f64>() <= 0.0 {
            continue;
        }
        let entry = highways.entry(pair.0).or_default();
        for (sum, value) in entry.0.vehicle_km.iter_mut().zip(record.vehicle_km) {
            *sum += value;
        }
        entry.0.sections += record.sections;
        entry.1.km += map.km;
        entry.1.expected_vehicle_km += map.expected_vehicle_km;
        entry.2.push(pair);
    }
    let total = |c: &Counted| c.vehicle_km.iter().sum::<f64>();
    let log_k = |c: &Counted, m: &Mapped| (total(c) / 365.0 / m.expected_vehicle_km).ln();
    let log_km_per_section = |c: &Counted, m: &Mapped| (m.km / c.sections as f64).ln();
    let k_band = band(
        &highways
            .values()
            .map(|(c, m, _)| log_k(c, m))
            .collect::<Vec<_>>(),
    );
    let section_band = band(
        &highways
            .values()
            .map(|(c, m, _)| log_km_per_section(c, m))
            .collect::<Vec<_>>(),
    );
    let kept: BTreeSet<u16> = highways
        .iter()
        .filter(|(_, (c, m, _))| {
            within(k_band, log_k(c, m)) && within(section_band, log_km_per_section(c, m))
        })
        .map(|(h, _)| *h)
        .collect();
    let relative = |pair: &Pair| {
        let (hc, hm, _) = &highways[&pair.0];
        let (c, m) = (&counted[pair], &mapped[pair]);
        (
            log_km_per_section(c, m) - log_km_per_section(hc, hm),
            (total(c) / m.expected_vehicle_km).ln() - (total(hc) / hm.expected_vehicle_km).ln(),
        )
    };
    let shared: Vec<Pair> = kept
        .iter()
        .flat_map(|h| &highways[h].2)
        .filter(|p| highways[&p.0].2.len() > 1)
        .copied()
        .collect();
    let relatives: Vec<(f64, f64)> = shared.iter().map(relative).collect();
    let length_band = band(&relatives.iter().map(|r| r.0).collect::<Vec<_>>());
    let ratio_band = band(&relatives.iter().map(|r| r.1).collect::<Vec<_>>());
    let mut rows = Vec::new();
    let mut dropped = Vec::new();
    for (&number, (c, m, pairs)) in &highways {
        if !kept.contains(&number) {
            dropped.push(json!({"number": number, "k": log_k(c, m).exp(), "km": m.km, "sections": c.sections}));
            continue;
        }
        for pair in pairs {
            let (length, ratio) = relative(pair);
            if pairs.len() > 1 && !(within(length_band, length) && within(ratio_band, ratio)) {
                dropped.push(json!({"number": pair.0, "subdivision": pair.1, "km_per_section_ratio": length.exp(), "k_ratio": ratio.exp()}));
                continue;
            }
            let (record, map) = (&counted[pair], &mapped[pair]);
            let sum = total(record);
            rows.push((
                *pair,
                sum / 365.0 / map.km,
                record.vehicle_km.map(|v| v / sum),
            ));
        }
    }
    let exp = |[low, high]: [f64; 2]| [low.exp(), high.exp()];
    let record = json!({
        "bands": {"k": exp(k_band), "highway_km_per_section": exp(section_band), "km_per_section_ratio": exp(length_band), "k_ratio": exp(ratio_band)},
        "highways": highways.len(), "kept_highways": kept.len(), "pairs": rows.len(),
        "km_kept": rows.iter().map(|(p, _, _)| mapped[p].km).sum::<f64>(),
        "km_numbered": mapped.iter().filter(|((h, _), _)| highways.contains_key(h)).map(|(_, m)| m.km).sum::<f64>(),
        "dropped": dropped,
    });
    (rows, record)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Median 3 and deviations 2, 1, 0, 1, 97 (median 1): 3 +- 3 x 1.4826; an even count takes the
    /// mean of its middle two.
    #[test]
    fn a_band_is_the_median_within_three_scaled_median_deviations() {
        let [low, high] = band(&[1.0, 2.0, 3.0, 4.0, 100.0]);
        assert!((low - (3.0 - 4.4478)).abs() < 1e-12 && (high - (3.0 + 4.4478)).abs() < 1e-12);
        let [low, high] = band(&[1.0, 2.0, 3.0, 4.0]);
        assert!((low - (2.5 - 4.4478)).abs() < 1e-12 && (high - (2.5 + 4.4478)).abs() < 1e-12);
        assert!(band(&[]).iter().all(|b| b.is_nan()));
    }

    /// One pair of `highway` in `subdivision`: `k` times the model's 1,000 vehicle-km a day,
    /// `km` mapped over `sections` control sections, 70 % light vehicles.
    fn pair(
        tables: &mut (BTreeMap<Pair, Counted>, BTreeMap<Pair, Mapped>),
        (highway, subdivision): Pair,
        (k, km, sections): (f64, f64, usize),
    ) {
        let total = 365.0 * 1_000.0 * k;
        tables.0.insert(
            (highway, subdivision),
            Counted {
                vehicle_km: [0.7, 0.1, 0.1, 0.1].map(|share| share * total),
                sections,
            },
        );
        tables.1.insert(
            (highway, subdivision),
            Mapped {
                km,
                expected_vehicle_km: 1_000.0,
            },
        );
    }

    /// Ten highways: five of one province whose k (e^-0.4 .. e^0.4) and km per section (8 .. 12)
    /// spread the bands, 6 with k = e^3, 7 with 1,000 km a section, and three of several
    /// provinces. k's deviations have the median 0.1 (band e^+-0.445), km per section's 0.100
    /// about ln 10 (band 6.40 - 15.6): 6 and 7 keep no row. Among the provinces of 8, 9 and 10
    /// the median deviation is ln 1.1 (both bands e^+-0.424): 9's province of 1 km for a section
    /// (7.75 the highway's) and 10's of 1.75 times its highway's k keep their estimates.
    #[test]
    fn outlying_highways_and_provinces_keep_their_estimates() {
        let mut tables = (BTreeMap::new(), BTreeMap::new());
        for (highway, (log_k, km)) in [
            (-0.4, 8.0),
            (-0.2, 9.0),
            (0.0, 10.0),
            (0.2, 11.0),
            (0.4, 12.0),
        ]
        .into_iter()
        .enumerate()
        {
            pair(
                &mut tables,
                (highway as u16 + 1, 10),
                (f64::exp(log_k), km, 1),
            );
        }
        pair(&mut tables, (6, 10), (3f64.exp(), 10.0, 1));
        pair(&mut tables, (7, 10), (1.0, 1_000.0, 1));
        for (province, (k, km)) in [(10, (0.9, 9.0)), (11, (1.0, 10.0)), (12, (1.1, 11.0))] {
            pair(&mut tables, (8, province), (k, km, 1));
        }
        for (province, km) in [(10, 10.0), (11, 10.0), (12, 10.0), (13, 1.0)] {
            pair(&mut tables, (9, province), (1.0, km, 1));
        }
        for (province, k) in [(10, 0.75), (11, 0.75), (12, 0.75), (13, 1.75)] {
            pair(&mut tables, (10, province), (k, 10.0, 1));
        }
        // A section without mapped rows and one without traffic count for nothing.
        tables.0.insert((11, 10), tables.0[&(3, 10)]);
        pair(&mut tables, (12, 10), (0.0, 10.0, 1));
        let (rows, record) = screen(&tables.0, &tables.1);
        let kept: Vec<Pair> = rows.iter().map(|(pair, _, _)| *pair).collect();
        let mut expected: Vec<Pair> = (1..=5).map(|h| (h, 10)).collect();
        expected.extend([(8, 10), (8, 11), (8, 12), (9, 10), (9, 11), (9, 12)]);
        expected.extend([(10, 10), (10, 11), (10, 12)]);
        assert_eq!(kept, expected);
        let (_, daily, shares) = rows.iter().find(|(pair, _, _)| *pair == (3, 10)).unwrap();
        assert!((daily - 100.0).abs() < 1e-9);
        assert!(
            shares
                .iter()
                .zip([0.7, 0.1, 0.1, 0.1])
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        let bounds = |name: &str| -> Vec<f64> {
            record["bands"][name]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect()
        };
        let k: f64 = 3.0 * 1.4826 * 0.1;
        assert!(
            (bounds("k")[0] - (-k).exp()).abs() < 1e-12 && (bounds("k")[1] - k.exp()).abs() < 1e-12
        );
        let section = 3.0 * 1.4826 * 0.5 * (1.1f64.ln() + (10.0f64 / 9.0).ln());
        assert!((bounds("highway_km_per_section")[1] - 10.0 * section.exp()).abs() < 1e-9);
        let province = 3.0 * 1.4826 * 1.1f64.ln();
        assert!((bounds("k_ratio")[1] - province.exp()).abs() < 1e-12);
        assert!((bounds("km_per_section_ratio")[0] - (-province).exp()).abs() < 1e-12);
        assert_eq!(record["highways"], 10);
    }
}

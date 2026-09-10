//! Union ground membership masks globally and publish identical summaries to every owner.
use super::admission::AllocationBudget;
use super::movements::{self, MovementUnion};
use crate::arrow_io::{read_airport_summary_part, write_airport_summary, AirportSummaryRow};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

pub fn run_airport_summary_reduce(parts_root: &Path, prepared_year: &Path) -> Result<usize> {
    anyhow::ensure!(
        parts_root.is_dir(),
        "airport summary parts directory missing: {}",
        parts_root.display()
    );
    let entries = crate::spatial::square_directories(parts_root)?;
    let mut largest_part = 0;
    for (_, path) in &entries {
        largest_part = largest_part.max(path.join("part.arrow").metadata()?.len());
    }
    // IPC, decoded membership rows and temporary buffers coexist for one part.
    // Global airport/owner keys and memberships are charged separately on growth.
    let mut budget = AllocationBudget::new(
        crate::memory::available_memory_bytes(),
        largest_part
            .checked_mul(6)
            .and_then(|n| n.checked_add(1 << 30))
            .context("airport part allowance overflow")?,
    )?;
    let mut by_airport: HashMap<String, MovementUnion> = HashMap::new();
    let mut airport_owners = Vec::new();
    for (owner, path) in entries {
        let rows = read_airport_summary_part(&path.join("part.arrow"))
            .with_context(|| format!("read airport memberships at {}", path.display()))?;
        let mut keys = Vec::new();
        for row in rows {
            budget.reserve(
                8 * (row.airport_key.len()
                    + std::mem::size_of::<String>()
                    + std::mem::size_of::<AirportSummaryRow>()) as u64,
            )?;
            keys.push(row.airport_key.clone());
            if !by_airport.contains_key(&row.airport_key) {
                budget.reserve_hash_entry::<String, MovementUnion>(by_airport.len())?;
            }
            let union = by_airport.entry(row.airport_key).or_default();
            for (fid, flags) in row.members {
                union.insert(fid, flags, &mut budget)?;
            }
        }
        keys.sort_unstable();
        keys.dedup();
        airport_owners.push((owner, keys));
    }
    let mut rows: Vec<_> = by_airport
        .into_iter()
        .map(|(airport_key, union)| AirportSummaryRow {
            airport_key,
            airport_unique_arr_count: union.count(movements::ARRIVAL),
            airport_unique_dep_count: union.count(movements::DEPARTURE),
            airport_unique_gse_count_per_class: std::array::from_fn(|i| {
                union.count(movements::GSE[i])
            }),
            airport_unique_ops_count_per_kind: std::array::from_fn(|i| {
                union.count(movements::OPS[i])
            }),
            airport_unique_ga_arr_count: union.count(movements::GA_ARRIVAL),
            airport_unique_ga_dep_count: union.count(movements::GA_DEPARTURE),
            airport_unique_ga_ops_count_per_kind: std::array::from_fn(|i| {
                union.count(movements::GA_OPS[i])
            }),
        })
        .collect();
    rows.sort_unstable_by(|a, b| a.airport_key.cmp(&b.airport_key));
    for (owner, keys) in airport_owners {
        let summaries: Vec<_> = keys
            .iter()
            .map(|key| {
                rows[rows
                    .binary_search_by(|row| row.airport_key.cmp(key))
                    .expect("known airport key")]
                .clone()
            })
            .collect();
        write_airport_summary(
            &prepared_year
                .join(crate::spatial::square_path(owner))
                .join(super::AIRPORT_SUMMARY_FILENAME),
            &summaries,
        )?;
    }
    eprintln!(
        "[stage2c/reduce] {} airports; {} B charged allocation allowance",
        rows.len(),
        budget.reserved()
    );
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::{read_airport_summary, write_airport_summary_part};
    use crate::stage_2c::airport_traffic_writer::AirportSummaryPartRow;
    use movements::*;

    #[test]
    fn reducer_unions_every_flag_across_owners_and_keeps_airports_separate() {
        let temp = tempfile::tempdir().unwrap();
        let parts = temp.path().join("parts");
        let out = temp.path().join("prepared");
        for (square, members) in [
            (
                "z9/276/173",
                vec![(1, ARRIVAL | OPS[0]), (2, GA_DEPARTURE | GA_OPS[0])],
            ),
            (
                "z9/277/173",
                vec![
                    (1, DEPARTURE | OPS[1] | GSE[0]),
                    (2, GA_ARRIVAL | GA_OPS[1]),
                    (3, AIRPORT_FLAGS),
                ],
            ),
        ] {
            write_airport_summary_part(
                &parts.join(square).join("part.arrow"),
                &[
                    AirportSummaryPartRow {
                        airport_key: "B".into(),
                        members,
                    },
                    AirportSummaryPartRow {
                        airport_key: "A".into(),
                        members: vec![(1, ARRIVAL)],
                    },
                ],
            )
            .unwrap();
        }
        assert_eq!(run_airport_summary_reduce(&parts, &out).unwrap(), 2);
        let rows = read_airport_summary(&out.join("z9/276/173/airport_summary.arrow")).unwrap();
        assert_eq!(
            rows,
            read_airport_summary(&out.join("z9/277/173/airport_summary.arrow")).unwrap()
        );
        assert_eq!(rows[0].airport_key, "A");
        assert_eq!(rows[0].airport_unique_arr_count, 1);
        let b = &rows[1];
        assert_eq!(
            (b.airport_unique_arr_count, b.airport_unique_dep_count),
            (2, 2)
        );
        assert_eq!(b.airport_unique_gse_count_per_class, [2, 1, 1]);
        assert_eq!(b.airport_unique_ops_count_per_kind, [2, 2, 1]);
        assert_eq!(
            (b.airport_unique_ga_arr_count, b.airport_unique_ga_dep_count),
            (2, 2)
        );
        assert_eq!(b.airport_unique_ga_ops_count_per_kind, [2, 2, 1]);
    }

    #[test]
    fn missing_parts_are_not_a_complete_empty_run() {
        let temp = tempfile::tempdir().unwrap();
        let parts = temp.path().join("parts");
        let out = temp.path().join("prepared");
        assert!(run_airport_summary_reduce(&parts, &out).is_err());
        std::fs::create_dir(&parts).unwrap();
        assert_eq!(run_airport_summary_reduce(&parts, &out).unwrap(), 0);
        assert!(!out.exists());
    }
}

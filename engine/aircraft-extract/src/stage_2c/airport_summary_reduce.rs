//! Union movement identities globally and publish each airport into every traffic-owning z9 cell.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use noise_compute::emission::gse::NUM_GSE_CLASSES;

use crate::arrow_io::{read_airport_summary_part, write_airport_summary, AirportSummaryRow};
use crate::progress::{finished, started, Milestone};

/// Counts remain global unions; copying them into owner cells never sums local counts.
pub fn run_airport_summary_reduce(parts_root: &Path, prepared_year: &Path) -> Result<usize> {
    anyhow::ensure!(
        parts_root.is_dir(),
        "airport summary parts directory missing: {}",
        parts_root.display()
    );

    let mut by_airport: HashMap<String, GlobalAggregate> = HashMap::new();
    let mut airport_owners = Vec::new();

    let part_counter = Milestone::new("stage2c/reduce", "z9 parts", 50);
    let mut n_parts = 0usize;
    let square_entries = crate::spatial::square_directories(parts_root)?;
    started(
        "stage2c/reduce",
        &format!("{} z9 subdirs", square_entries.len()),
    );

    // Walk every z9 subdir and absorb its part.arrow.
    for (owner, path) in square_entries {
        let part_path = path.join("part.arrow");
        let rows = read_airport_summary_part(&part_path)
            .with_context(|| format!("read airport_summary_parts at {}", part_path.display()))?;
        let mut keys: Vec<_> = rows.iter().map(|row| row.airport_key.clone()).collect();
        keys.sort_unstable();
        keys.dedup();
        airport_owners.push((owner, keys));
        for row in rows {
            let entry = by_airport.entry(row.airport_key.clone()).or_default();
            for fid in row.arr_fids {
                entry.arr.insert(fid);
            }
            for fid in row.dep_fids {
                entry.dep.insert(fid);
            }
            for (i, fids) in row.gse_fids_per_class.iter().enumerate() {
                for &fid in fids {
                    entry.gse_per_class[i].insert(fid);
                }
            }
            for (i, fids) in row.ops_fids_per_kind.iter().enumerate() {
                for &fid in fids {
                    entry.ops_per_kind[i].insert(fid);
                }
            }
            // GA-class counts stay separate so the popup divides each
            // sampling window by its own day count.
            for fid in row.ga_arr_fids {
                entry.ga_arr.insert(fid);
            }
            for fid in row.ga_dep_fids {
                entry.ga_dep.insert(fid);
            }
            for (i, fids) in row.ga_ops_fids_per_kind.iter().enumerate() {
                for &fid in fids {
                    entry.ga_ops_per_kind[i].insert(fid);
                }
            }
        }
        n_parts += 1;
        part_counter.add(1);
    }

    let mut rows: Vec<AirportSummaryRow> = by_airport
        .into_iter()
        .map(|(airport_key, acc)| AirportSummaryRow {
            airport_key,
            airport_unique_arr_count: acc.arr.len() as u32,
            airport_unique_dep_count: acc.dep.len() as u32,
            airport_unique_gse_count_per_class: std::array::from_fn(|i| {
                acc.gse_per_class[i].len() as u32
            }),
            airport_unique_ops_count_per_kind: std::array::from_fn(|i| {
                acc.ops_per_kind[i].len() as u32
            }),
            airport_unique_ga_arr_count: acc.ga_arr.len() as u32,
            airport_unique_ga_dep_count: acc.ga_dep.len() as u32,
            airport_unique_ga_ops_count_per_kind: std::array::from_fn(|i| {
                acc.ga_ops_per_kind[i].len() as u32
            }),
        })
        .collect();
    // Deterministic on-disk order.
    rows.sort_by(|a, b| a.airport_key.cmp(&b.airport_key));
    let n = rows.len();
    let by_key: HashMap<_, _> = rows
        .iter()
        .map(|row| (row.airport_key.as_str(), row))
        .collect();
    for (owner, keys) in airport_owners {
        let summaries: Vec<_> = keys
            .iter()
            .map(|key| (*by_key[key.as_str()]).clone())
            .collect();
        write_airport_summary(
            &prepared_year
                .join(crate::spatial::square_path(owner))
                .join(super::AIRPORT_SUMMARY_FILENAME),
            &summaries,
        )?;
    }
    finished(
        "stage2c/reduce",
        &format!(
            "{n} airports from {n_parts} z9 parts → {}",
            prepared_year.display()
        ),
    );
    Ok(n)
}

#[derive(Default)]
struct GlobalAggregate {
    arr: HashSet<u64>,
    dep: HashSet<u64>,
    gse_per_class: [HashSet<u64>; NUM_GSE_CLASSES],
    ops_per_kind: [HashSet<u64>; 3],
    ga_arr: HashSet<u64>,
    ga_dep: HashSet<u64>,
    ga_ops_per_kind: [HashSet<u64>; 3],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrow_io::{read_airport_summary, write_airport_summary_part};
    use crate::stage_2c::airport_traffic_writer::AirportSummaryPartRow;
    use tempfile::tempdir;

    /// Two z9s touch the same airport; reducer UNIONs disjoint arrival
    /// fids → global count = sum (no double-count when sets are disjoint)
    /// AND a fid present in both z9s must collapse to 1.
    #[test]
    fn reduce_unions_across_squares() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("airport_summary_parts");
        // z9 #1: arr fids {10, 11, 12}, dep fids {20}.
        let squarea = parts.join("z9/276/173");
        std::fs::create_dir_all(&squarea).unwrap();
        write_airport_summary_part(
            &squarea.join("part.arrow"),
            &[AirportSummaryPartRow {
                airport_key: "LKPR".into(),
                arr_fids: vec![10, 11, 12],
                dep_fids: vec![20],
                gse_fids_per_class: [vec![100], vec![], vec![]],
                ops_fids_per_kind: [vec![10, 11, 12, 20], vec![10, 12], vec![]],
                ga_arr_fids: vec![900],
                ga_dep_fids: vec![],
                ga_ops_fids_per_kind: [vec![900], vec![], vec![]],
            }],
        )
        .unwrap();
        // z9 #2: arr fids {12, 13} — fid 12 overlaps with z9 #1.
        let squareb = parts.join("z9/277/173");
        std::fs::create_dir_all(&squareb).unwrap();
        write_airport_summary_part(
            &squareb.join("part.arrow"),
            &[AirportSummaryPartRow {
                airport_key: "LKPR".into(),
                arr_fids: vec![12, 13],
                dep_fids: vec![21, 22],
                gse_fids_per_class: [vec![], vec![200], vec![]],
                ops_fids_per_kind: [vec![12, 13, 21, 22], vec![13], vec![]],
                ga_arr_fids: vec![900, 901],
                ga_dep_fids: vec![],
                ga_ops_fids_per_kind: [vec![900, 901], vec![], vec![]],
            }],
        )
        .unwrap();

        let out = dir.path().join("prepared");
        let n = run_airport_summary_reduce(&parts, &out).unwrap();
        assert_eq!(n, 1);
        let rows = read_airport_summary(&out.join("z9/276/173/airport_summary.arrow")).unwrap();
        assert_eq!(
            rows,
            read_airport_summary(&out.join("z9/277/173/airport_summary.arrow")).unwrap()
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].airport_key, "LKPR");
        // Arr union: {10, 11, 12} ∪ {12, 13} = 4 fids (12 dedupes).
        assert_eq!(rows[0].airport_unique_arr_count, 4);
        // Dep union: {20} ∪ {21, 22} = 3 fids.
        assert_eq!(rows[0].airport_unique_dep_count, 3);
        // GSE: class 0 has {100} from z9a; class 1 has {200} from z9b.
        assert_eq!(rows[0].airport_unique_gse_count_per_class, [1, 1, 0]);
        // Ops kind 0 (runway): {10,11,12,20} ∪ {12,13,21,22} = 7 fids.
        assert_eq!(rows[0].airport_unique_ops_count_per_kind[0], 7);
        // Ops kind 1 (taxi): {10,12} ∪ {13} = 3 fids.
        assert_eq!(rows[0].airport_unique_ops_count_per_kind[1], 3);
        // GA arr union: {900} ∪ {900, 901} = 2 (900 dedupes across z9s) —
        // and kept disjoint from the non-GA arr count above.
        assert_eq!(rows[0].airport_unique_ga_arr_count, 2);
        assert_eq!(rows[0].airport_unique_ga_dep_count, 0);
        assert_eq!(rows[0].airport_unique_ga_ops_count_per_kind[0], 2);
    }

    #[test]
    fn reduce_distinguishes_missing_parts_from_a_complete_empty_run() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("missing");
        let out = dir.path().join("prepared");
        assert!(run_airport_summary_reduce(&parts, &out).is_err());
        std::fs::create_dir_all(&parts).unwrap();
        let n = run_airport_summary_reduce(&parts, &out).unwrap();
        assert_eq!(n, 0);
        assert!(!out.exists());
    }

    #[test]
    fn reduce_two_airports_independent_unions() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("airport_summary_parts");
        let square = parts.join("z9/276/173");
        std::fs::create_dir_all(&square).unwrap();
        write_airport_summary_part(
            &square.join("part.arrow"),
            &[
                AirportSummaryPartRow {
                    airport_key: "LKPR".into(),
                    arr_fids: vec![1, 2],
                    dep_fids: vec![3],
                    gse_fids_per_class: [vec![], vec![], vec![]],
                    ops_fids_per_kind: [vec![1, 2, 3], vec![], vec![]],
                    ga_arr_fids: vec![],
                    ga_dep_fids: vec![],
                    ga_ops_fids_per_kind: [vec![], vec![], vec![]],
                },
                AirportSummaryPartRow {
                    airport_key: "LKKB".into(),
                    arr_fids: vec![100],
                    dep_fids: vec![],
                    gse_fids_per_class: [vec![], vec![], vec![]],
                    ops_fids_per_kind: [vec![100], vec![], vec![]],
                    ga_arr_fids: vec![],
                    ga_dep_fids: vec![],
                    ga_ops_fids_per_kind: [vec![], vec![], vec![]],
                },
            ],
        )
        .unwrap();
        let out = dir.path().join("prepared");
        let n = run_airport_summary_reduce(&parts, &out).unwrap();
        assert_eq!(n, 2);
        let rows = read_airport_summary(&out.join("z9/276/173/airport_summary.arrow")).unwrap();
        // Deterministic sort: LKKB < LKPR.
        assert_eq!(rows[0].airport_key, "LKKB");
        assert_eq!(rows[0].airport_unique_arr_count, 1);
        assert_eq!(rows[1].airport_key, "LKPR");
        assert_eq!(rows[1].airport_unique_arr_count, 2);
    }
}

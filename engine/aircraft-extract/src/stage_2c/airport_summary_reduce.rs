//! Stage 2C v5 global reduce phase.
//!
//! After the per-R4 worker pass dumps per-airport raw fid sets to
//! `airport_summary_parts/<r4>/part.arrow`, this reduce reads every
//! R4 part, UNIONs the HashSets per `airport_key`, and writes the
//! global `airport_summary.arrow` (one row per airport).
//!
//! Memory pattern: builds one `HashMap<airport_key, GlobalAggregate>`
//! across ALL airports. Worst-case RAM = Σ (per-airport unique
//! fid sets); at LKPR full year ~50k airports × ~5 MB per large hub
//! fid set → low-GB. Acceptable for the reduce step; if global
//! extracts pinch RAM, swap to a streaming sort+uniq pattern keyed
//! on airport_key.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use noise_compute::emission::gse::NUM_GSE_CLASSES;

use crate::arrow_io::{read_airport_summary_part, write_airport_summary, AirportSummaryRow};
use crate::progress::{finished, started, Milestone};

/// Read all per-R4 `airport_summary_parts/*/part.arrow` files, UNION
/// the raw fid sets per airport_key, write the global
/// `airport_summary.arrow` to `output_path`. Returns the number of
/// airports written.
///
/// Memory pattern: builds one `HashMap<airport_key, AirportAggregate>`
/// keyed across all R4s. At LKPR full year this peaks ~50k fids per
/// airport × ~3 sets × 8 B/u64 ≈ ~5 MB per airport; global ~50k
/// airports × that scale dominated by a few large hubs ≈ low GB total
/// in worst case. Acceptable for the reduce step; if it pinches at
/// global scale, swap to a streaming sort+uniq pattern.
pub fn run_airport_summary_reduce(parts_root: &Path, output_path: &Path) -> Result<usize> {
    if !parts_root.exists() {
        eprintln!(
            "{} [stage2c/reduce] no airport_summary_parts/ at {} — writing empty summary",
            crate::progress::ts(),
            parts_root.display()
        );
        write_airport_summary(output_path, &[])?;
        return Ok(0);
    }

    let mut by_airport: HashMap<String, GlobalAggregate> = HashMap::new();

    let part_counter = Milestone::new("stage2c/reduce", "R4 parts", 50);
    let mut n_parts = 0usize;
    let r4_entries: Vec<_> = std::fs::read_dir(parts_root)
        .with_context(|| format!("read_dir {}", parts_root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    started(
        "stage2c/reduce",
        &format!("{} R4 subdirs", r4_entries.len()),
    );

    // Walk every R4 subdir and absorb its part.arrow.
    for entry in r4_entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let part_path = path.join("part.arrow");
        if !part_path.exists() {
            continue;
        }
        let rows = read_airport_summary_part(&part_path)
            .with_context(|| format!("read airport_summary_parts at {}", part_path.display()))?;
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
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_airport_summary(output_path, &rows)?;
    finished(
        "stage2c/reduce",
        &format!(
            "{n} airports from {n_parts} R4 parts → {}",
            output_path.display()
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

    /// Two R4s touch the same airport; reducer UNIONs disjoint arrival
    /// fids → global count = sum (no double-count when sets are disjoint)
    /// AND a fid present in both R4s must collapse to 1.
    #[test]
    fn reduce_unions_across_r4s() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("airport_summary_parts");
        // R4 #1: arr fids {10, 11, 12}, dep fids {20}.
        let r4a = parts.join("r4_aaaa");
        std::fs::create_dir_all(&r4a).unwrap();
        write_airport_summary_part(
            &r4a.join("part.arrow"),
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
        // R4 #2: arr fids {12, 13} — fid 12 overlaps with R4 #1.
        let r4b = parts.join("r4_bbbb");
        std::fs::create_dir_all(&r4b).unwrap();
        write_airport_summary_part(
            &r4b.join("part.arrow"),
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

        let out = dir.path().join("airport_summary.arrow");
        let n = run_airport_summary_reduce(&parts, &out).unwrap();
        assert_eq!(n, 1);
        let rows = read_airport_summary(&out).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].airport_key, "LKPR");
        // Arr union: {10, 11, 12} ∪ {12, 13} = 4 fids (12 dedupes).
        assert_eq!(rows[0].airport_unique_arr_count, 4);
        // Dep union: {20} ∪ {21, 22} = 3 fids.
        assert_eq!(rows[0].airport_unique_dep_count, 3);
        // GSE: class 0 has {100} from R4a; class 1 has {200} from R4b.
        assert_eq!(rows[0].airport_unique_gse_count_per_class, [1, 1, 0]);
        // Ops kind 0 (runway): {10,11,12,20} ∪ {12,13,21,22} = 7 fids.
        assert_eq!(rows[0].airport_unique_ops_count_per_kind[0], 7);
        // Ops kind 1 (taxi): {10,12} ∪ {13} = 3 fids.
        assert_eq!(rows[0].airport_unique_ops_count_per_kind[1], 3);
        // GA arr union: {900} ∪ {900, 901} = 2 (900 dedupes across R4s) —
        // and kept disjoint from the non-GA arr count above.
        assert_eq!(rows[0].airport_unique_ga_arr_count, 2);
        assert_eq!(rows[0].airport_unique_ga_dep_count, 0);
        assert_eq!(rows[0].airport_unique_ga_ops_count_per_kind[0], 2);
    }

    #[test]
    fn reduce_missing_parts_dir_writes_empty_summary() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("missing");
        let out = dir.path().join("airport_summary.arrow");
        let n = run_airport_summary_reduce(&parts, &out).unwrap();
        assert_eq!(n, 0);
        let rows = read_airport_summary(&out).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn reduce_two_airports_independent_unions() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("airport_summary_parts");
        let r4 = parts.join("r4");
        std::fs::create_dir_all(&r4).unwrap();
        write_airport_summary_part(
            &r4.join("part.arrow"),
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
        let out = dir.path().join("airport_summary.arrow");
        let n = run_airport_summary_reduce(&parts, &out).unwrap();
        assert_eq!(n, 2);
        let rows = read_airport_summary(&out).unwrap();
        // Deterministic sort: LKKB < LKPR.
        assert_eq!(rows[0].airport_key, "LKKB");
        assert_eq!(rows[0].airport_unique_arr_count, 1);
        assert_eq!(rows[1].airport_key, "LKPR");
        assert_eq!(rows[1].airport_unique_arr_count, 2);
    }
}

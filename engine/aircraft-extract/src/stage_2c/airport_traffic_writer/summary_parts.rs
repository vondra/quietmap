//! Per-owner raw airport movement sets for exact global union.
use super::*;

pub(crate) struct AirportSummaryPartRow {
    pub airport_key: String,
    pub arr_fids: Vec<u64>,
    pub dep_fids: Vec<u64>,
    pub gse_fids_per_class: [Vec<u64>; NUM_GSE_CLASSES],
    pub ops_fids_per_kind: [Vec<u64>; 3],
    pub ga_arr_fids: Vec<u64>,
    pub ga_dep_fids: Vec<u64>,
    pub ga_ops_fids_per_kind: [Vec<u64>; 3],
}

pub(super) fn write_airport_summary_parts(
    out_dir: &Path,
    airport_aggs: &HashMap<String, AirportAggregateAcc>,
) -> Result<()> {
    if airport_aggs.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(out_dir)?;
    let mut rows: Vec<AirportSummaryPartRow> = airport_aggs
        .iter()
        .map(|(airport_key, acc)| {
            let mut arr_fids: Vec<u64> = acc.arr.iter().copied().collect();
            arr_fids.sort_unstable();
            let mut dep_fids: Vec<u64> = acc.dep.iter().copied().collect();
            dep_fids.sort_unstable();
            let gse_fids_per_class: [Vec<u64>; NUM_GSE_CLASSES] = std::array::from_fn(|i| {
                let mut v: Vec<u64> = acc.gse_per_class[i].iter().copied().collect();
                v.sort_unstable();
                v
            });
            let sorted = |set: &HashSet<u64>| {
                let mut v: Vec<u64> = set.iter().copied().collect();
                v.sort_unstable();
                v
            };
            let ops_fids_per_kind: [Vec<u64>; 3] =
                std::array::from_fn(|i| sorted(&acc.ops_per_kind[i]));
            let ga_ops_fids_per_kind: [Vec<u64>; 3] =
                std::array::from_fn(|i| sorted(&acc.ga_ops_per_kind[i]));
            AirportSummaryPartRow {
                airport_key: airport_key.clone(),
                arr_fids,
                dep_fids,
                gse_fids_per_class,
                ops_fids_per_kind,
                ga_arr_fids: sorted(&acc.ga_arr),
                ga_dep_fids: sorted(&acc.ga_dep),
                ga_ops_fids_per_kind,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.airport_key.cmp(&b.airport_key));
    crate::arrow_io::write_airport_summary_part(&out_dir.join("part.arrow"), &rows)
}

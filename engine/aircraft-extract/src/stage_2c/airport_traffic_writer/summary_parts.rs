//! Per-owner airport flight identities carry all independent union flags once.
use super::*;

pub(crate) struct AirportSummaryPartRow {
    pub airport_key: String,
    pub members: Vec<(u64, u16)>,
}

pub(super) fn write_airport_summary_parts(
    out_dir: &Path,
    airport_aggs: &HashMap<String, MovementUnion>,
) -> Result<()> {
    let mut rows: Vec<_> = airport_aggs
        .iter()
        .map(|(key, acc)| {
            let mut members: Vec<_> = acc
                .members
                .iter()
                .map(|(&fid, &flags)| (fid, flags))
                .collect();
            members.sort_unstable_by_key(|row| row.0);
            AirportSummaryPartRow {
                airport_key: key.clone(),
                members,
            }
        })
        .collect();
    rows.sort_unstable_by(|a, b| a.airport_key.cmp(&b.airport_key));
    crate::arrow_io::write_airport_summary_part(&out_dir.join("part.arrow"), &rows)
}

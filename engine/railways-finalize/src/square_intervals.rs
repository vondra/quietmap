//! Interval evidence of one z9 square: every country's `rail-intervals.<ISO2>.arrow` in its directory.

use arrow::array::{Float64Array, Int16Array, Int64Array, UInt16Array, UInt8Array};
use arrow::ipc::reader::FileReader;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Interval {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub from_m: f64,
    pub to_m: f64,
    /// Writing timetable's country (`rail-intervals.<ISO2>.arrow`): claims on rows of another
    /// country are that timetable's cross-border services, never its domestic total.
    pub country: [u8; 2],
    pub source_id: u16,
    pub passenger: f64,
    pub freight: f64,
    pub passenger_status: u8,
    pub freight_status: u8,
    pub matching: u8,
}

/// The ISO2 of `rail-intervals.<ISO2>.arrow`; any other file name is not interval evidence.
fn interval_file_country(name: &str) -> Option<&str> {
    name.strip_prefix("rail-intervals.")?.strip_suffix(".arrow")
}

pub fn load_square_intervals(
    square_dir: &Path,
) -> Result<HashMap<(i64, i16), Vec<Interval>>, String> {
    let mut rows: Vec<(Interval, String, i64)> = Vec::new();
    // An unreadable directory is not "no evidence": that would publish class priors over real counts.
    let entries =
        std::fs::read_dir(square_dir).map_err(|e| format!("{}: {e}", square_dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(country) = interval_file_country(&name) else {
            continue;
        };
        let failure = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
        let country_iso: [u8; 2] = country
            .as_bytes()
            .try_into()
            .map_err(|_| failure(&"invalid rail intervals country"))?;
        let reader = FileReader::try_new(File::open(&path).map_err(|e| failure(&e))?, None)
            .map_err(|e| failure(&e))?;
        if reader
            .schema()
            .metadata()
            .get("rail_intervals_contract")
            .map(String::as_str)
            != Some("1")
        {
            return Err(failure(&"unsupported rail intervals contract"));
        }
        for batch in reader {
            let batch = batch.map_err(|e| failure(&e))?;
            macro_rules! column {
                ($name:literal, $kind:ty) => {
                    batch
                        .column_by_name($name)
                        .and_then(|column| column.as_any().downcast_ref::<$kind>())
                        .ok_or_else(|| failure(&concat!("missing ", $name)))?
                };
            }
            let osm_id = column!("osm_id", Int64Array);
            let segment_idx = column!("segment_idx", Int16Array);
            let from_m = column!("from_m", Float64Array);
            let to_m = column!("to_m", Float64Array);
            let occurrence = column!("occurrence", Int64Array);
            let source_id = column!("source_id", UInt16Array);
            let passenger = column!("passenger", Float64Array);
            let freight = column!("freight", Float64Array);
            let passenger_status = column!("passenger_status", UInt8Array);
            let freight_status = column!("freight_status", UInt8Array);
            let matching = column!("matching", UInt8Array);
            for row in 0..batch.num_rows() {
                let interval = Interval {
                    osm_id: osm_id.value(row),
                    segment_idx: segment_idx.value(row),
                    from_m: from_m.value(row),
                    to_m: to_m.value(row),
                    country: country_iso,
                    source_id: source_id.value(row),
                    passenger: passenger.value(row),
                    freight: freight.value(row),
                    passenger_status: passenger_status.value(row),
                    freight_status: freight_status.value(row),
                    matching: matching.value(row),
                };
                if !interval.from_m.is_finite()
                    || !interval.to_m.is_finite()
                    || interval.to_m <= interval.from_m
                    || interval.passenger < 0.0
                    || interval.freight < 0.0
                    || !interval.passenger.is_finite()
                    || !interval.freight.is_finite()
                    || interval.passenger_status > 2
                    || interval.freight_status > 2
                    || interval.matching > 3
                {
                    return Err(failure(&"invalid rail interval"));
                }
                rows.push((interval, country.to_owned(), occurrence.value(row)));
            }
        }
    }
    // Claims of one source are summed in row order, so the country keeps its place in the key.
    rows.sort_by(
        |(a, a_country, a_occurrence), (b, b_country, b_occurrence)| {
            (a.osm_id, a.segment_idx, a.source_id)
                .cmp(&(b.osm_id, b.segment_idx, b.source_id))
                .then_with(|| a_country.cmp(b_country))
                .then_with(|| a.from_m.total_cmp(&b.from_m))
                .then_with(|| a.to_m.total_cmp(&b.to_m))
                .then_with(|| a_occurrence.cmp(b_occurrence))
        },
    );
    let mut intervals: HashMap<_, Vec<Interval>> = HashMap::new();
    for (interval, _, _) in rows {
        intervals
            .entry((interval.osm_id, interval.segment_idx))
            .or_default()
            .push(interval);
    }
    Ok(intervals)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unreadable_square_directory_is_an_error_not_missing_evidence() {
        let missing = std::env::temp_dir().join(format!("qm-no-square-{}", std::process::id()));
        let error = super::load_square_intervals(&missing).unwrap_err();
        assert!(error.contains(&missing.display().to_string()), "{error}");
    }
}

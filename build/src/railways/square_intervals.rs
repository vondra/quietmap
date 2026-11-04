//! Source-way intervals grouped in a deterministic summation order for finalization.
use std::collections::HashMap;
#[derive(Clone, Debug)]
pub struct Interval {
    pub missing_calendar_estimate: [f64; 2],
    pub osm_id: i64,
    pub segment_idx: i16,
    pub from_m: f64,
    pub to_m: f64,
    pub country: [u8; 2],
    pub source_id: u16,
    pub passenger: f64,
    pub freight: f64,
    pub passenger_status: u8,
    pub freight_status: u8,
    pub matching: u8,
}
pub type Intervals = HashMap<(i64, i16), Vec<Interval>>;
pub fn grouped(mut rows: Vec<(Interval, i64)>) -> Intervals {
    rows.sort_by(|(a, ao), (b, bo)| {
        (a.osm_id, a.segment_idx, a.source_id, a.country)
            .cmp(&(b.osm_id, b.segment_idx, b.source_id, b.country))
            .then_with(|| a.from_m.total_cmp(&b.from_m))
            .then_with(|| a.to_m.total_cmp(&b.to_m))
            .then_with(|| ao.cmp(bo))
    });
    let mut groups = HashMap::<_, Vec<Interval>>::new();
    for (row, _) in rows {
        groups
            .entry((row.osm_id, row.segment_idx))
            .or_default()
            .push(row);
    }
    groups
}

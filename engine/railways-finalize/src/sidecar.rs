//! Sidecar interval rows for one z9 square.

use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Interval {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub from_m: f64,
    pub to_m: f64,
    pub source_id: u16,
    pub passenger: f64,
    pub freight: f64,
    pub passenger_status: u8,
    pub freight_status: u8,
    pub matching: u8,
}

pub fn load_square_intervals(
    path: &Path,
    square: &str,
) -> Result<HashMap<(i64, i16), Vec<Interval>>, String> {
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let version: i32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version != 1 {
        return Err(format!(
            "rail-traffic sidecar schema {version} is unsupported (want 1)"
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT osm_id, segment_idx, from_m, to_m, source_id,
                    passenger, freight, passenger_status, freight_status, matching
             FROM rail_interval WHERE square = ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([square], |row| {
            Ok(Interval {
                osm_id: row.get(0)?,
                segment_idx: row.get(1)?,
                from_m: row.get(2)?,
                to_m: row.get(3)?,
                source_id: row.get(4)?,
                passenger: row.get(5)?,
                freight: row.get(6)?,
                passenger_status: row.get(7)?,
                freight_status: row.get(8)?,
                matching: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut intervals: HashMap<_, Vec<Interval>> = HashMap::new();
    for row in rows {
        let interval = row.map_err(|e| e.to_string())?;
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
            return Err("invalid rail sidecar interval".to_owned());
        }
        intervals
            .entry((interval.osm_id, interval.segment_idx))
            .or_default()
            .push(interval);
    }
    Ok(intervals)
}

//! One normalized rail observation format: service paths, line observations and crossing soundings.
use super::types::Crossing;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
};

pub const STREAM_VERSION: u8 = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub source_id: String,
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "geometry", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Location {
    Unlocated {
        reason: String,
    },
    Service {
        stops: Vec<Stop>,
        shape: Vec<[f64; 2]>,
    },
    Station {
        stop: Stop,
    },
    StationPair {
        from: Stop,
        to: Stop,
    },
    Line {
        coordinates: Vec<[f64; 2]>,
        radius_m: f64,
    },
    Region {
        bounds: [f64; 4],
    },
    Interval {
        square: [u16; 2],
        way: i64,
        segment: i16,
        from_m: f64,
        to_m: f64,
        occurrence: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Movement {
    pub id: String,
    pub source_id: u16,
    pub country: String,
    pub passenger: Option<f64>,
    pub freight: Option<f64>,
    pub passenger_status: u8,
    pub freight_status: u8,
    pub matching: u8,
    pub rail_types: Vec<u8>,
    pub coverage: super::coverage::Coverage,
    #[serde(default)]
    pub missing_calendar_estimate: [f64; 2],
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, String>,
    pub location: Location,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "record", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Record {
    Window {
        version: u8,
        start: String,
        end: String,
    },
    Movement(Box<Movement>),
    Crossing(Crossing),
}

impl Movement {
    fn calendar_estimate(&self) -> [f64; 2] {
        let covered = self.coverage.covered_days();
        let missing = u32::from(self.coverage.window_days) - covered;
        [self.passenger, self.freight].map(|observed| {
            if covered == 0 {
                0.0
            } else {
                observed.unwrap_or(0.0) * f64::from(missing) / f64::from(covered)
            }
        })
    }

    pub fn retain_categories_with_annual_evidence(&mut self) -> bool {
        let had_count = self.passenger.is_some() || self.freight.is_some();
        // A zero in an incomplete sample cannot establish annual silence on its line.
        if self.coverage.missing_fraction() > 0.0 {
            for (count, status) in [
                (&mut self.passenger, &mut self.passenger_status),
                (&mut self.freight, &mut self.freight_status),
            ] {
                if *count == Some(0.0) {
                    *count = None;
                    *status = 0;
                }
            }
        }
        !had_count || self.passenger.is_some() || self.freight.is_some()
    }

    pub fn estimate_missing_calendar(&mut self) {
        self.missing_calendar_estimate = self.calendar_estimate();
    }

    fn validate_calendar_estimate(&self) -> Result<(), String> {
        let expected = self.calendar_estimate();
        if self.missing_calendar_estimate != expected {
            return Err(format!(
                "{}: rail calendar estimate does not match its observed count and coverage",
                self.id
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.coverage.validate()?;
        if self.id.is_empty()
            || self.source_id == 0
            || self.country.len() != 2
            || !self.country.bytes().all(|byte| byte.is_ascii_uppercase())
            || self.matching > 3
            || self.rail_types.iter().any(|kind| *kind > 5)
            || self.passenger_status > 2
            || self.freight_status > 2
        {
            return Err(format!(
                "{}: invalid rail observation identity or coverage",
                self.id
            ));
        }
        for ((value, status), estimate) in [
            (self.passenger, self.passenger_status),
            (self.freight, self.freight_status),
        ]
        .into_iter()
        .zip(self.missing_calendar_estimate)
        {
            if !estimate.is_finite()
                || estimate < 0.0
                || (value.is_none() && estimate != 0.0)
                || value.is_some_and(|number| !number.is_finite() || number < 0.0)
                || value.is_none() != (status == 0)
                || (value.is_some_and(|number| number > 0.0)
                    && self.coverage.missing_fraction() == 1.0)
            {
                return Err(format!(
                    "{}: unknown is null; observed zero is a nonnegative count",
                    self.id
                ));
            }
        }
        let valid_point = |latitude: f64, longitude: f64| {
            (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude)
        };
        let valid_stop =
            |stop: &Stop| !stop.id.is_empty() && valid_point(stop.latitude, stop.longitude);
        let valid = match &self.location {
            Location::Unlocated { reason } => !reason.trim().is_empty(),
            Location::Service { stops, shape } => {
                stops.len() >= 2
                    && stops.iter().all(valid_stop)
                    && shape.iter().all(|point| valid_point(point[0], point[1]))
            }
            Location::Station { stop } => valid_stop(stop),
            Location::StationPair { from, to } => valid_stop(from) && valid_stop(to),
            Location::Line {
                coordinates,
                radius_m,
            } => {
                coordinates.len() >= 2
                    && radius_m.is_finite()
                    && *radius_m > 0.0
                    && coordinates
                        .iter()
                        .all(|point| valid_point(point[0], point[1]))
            }
            Location::Region { bounds } => {
                valid_point(bounds[0], bounds[1])
                    && valid_point(bounds[2], bounds[3])
                    && bounds[0] <= bounds[2]
                    && bounds[1] <= bounds[3]
            }
            Location::Interval {
                square,
                from_m,
                to_m,
                ..
            } => {
                square.iter().all(|axis| *axis < 512)
                    && from_m.is_finite()
                    && to_m.is_finite()
                    && *from_m >= 0.0
                    && to_m > from_m
            }
        };
        if !valid {
            return Err(format!("{}: invalid rail observation geometry", self.id));
        }
        Ok(())
    }
}

pub fn write_record(writer: &mut impl Write, record: &Record) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, record).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())
}

pub fn read(
    path: &Path,
    start: &str,
    end: &str,
    mut visit: impl FnMut(Record) -> Result<(), String>,
) -> Result<(), String> {
    let date = |value| {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|error| error.to_string())
    };
    let days = (date(end)? - date(start)?).num_days();
    if !(1..=366).contains(&days) {
        return Err("rail flow window must contain between one and 366 days".into());
    }
    let reader =
        BufReader::new(File::open(path).map_err(|error| format!("{}: {error}", path.display()))?);
    let mut lines = reader.lines();
    let header = lines
        .next()
        .ok_or("rail movements lack their window header")?
        .map_err(|error| error.to_string())?;
    match serde_json::from_str(&header).map_err(|error| format!("rail movement header: {error}"))? {
        Record::Window {
            version: STREAM_VERSION,
            start: a,
            end: b,
        } if a == start && b == end => (),
        _ => return Err("rail movement window does not match the year manifest".into()),
    }
    for (index, line) in lines.enumerate() {
        let line = line.map_err(|error| error.to_string())?;
        let record: Record = serde_json::from_str(&line)
            .map_err(|error| format!("{}:{}: {error}", path.display(), index + 2))?;
        match &record {
            Record::Window { .. } => return Err("repeated rail movement header".into()),
            Record::Movement(movement) => {
                movement.validate()?;
                movement.validate_calendar_estimate()?;
                if movement.coverage.window_days != 0
                    && i64::from(movement.coverage.window_days) != days
                {
                    return Err("rail coverage window does not match its movement header".into());
                }
            }
            Record::Crossing(crossing) => {
                if !(-90.0..=90.0).contains(&crossing.lat)
                    || !(-180.0..=180.0).contains(&crossing.lon)
                    || crossing
                        .soundings
                        .iter()
                        .any(|n| !n.is_finite() || *n < 0.0)
                    || crossing
                        .speed_kmh
                        .is_some_and(|speed| !speed.is_finite() || speed <= 0.0)
                    || crossing.synth_osm >= 0
                    || crossing.source_id == 0
                {
                    return Err("invalid normalized crossing".into());
                }
            }
        }
        visit(record)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "movement_stream_tests.rs"]
mod tests;

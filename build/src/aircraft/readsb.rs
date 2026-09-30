//! Reads one gzipped readsb `trace_full_<address>.json`: rows `[dt, lat, lon, altitude|"ground",
//! ground speed, track, flags, vertical rate, details|null, source, geometric altitude, ..]` are
//! read field by field without building a JSON value tree (a day holds about 100 M rows).

use super::trace::{AircraftTrace, CallsignChange, SURFACE_REPORT, TracePoint};
use flate2::read::GzDecoder;
use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;
use std::io::Read;

/// readsb flag bit 3: the altitude field holds geometric altitude (a UAT rebroadcast may report no
/// pressure altitude at all). It stands in for the pressure altitude, as dev4 read it, and is the
/// sample's geometric altitude when field 10 is empty.
const ALTITUDE_IS_GEOMETRIC: u64 = 1 << 3;

/// One gzipped trace. `Ok(None)` is a valid trace with fewer than two usable rows; a broken gzip or
/// JSON document is an error (the archive reader records it as a corrupt member). Rows of fewer
/// than 7 fields are skipped, as dev4 did.
pub fn parse_trace(gzip_bytes: &[u8]) -> Result<Option<AircraftTrace>, String> {
    let mut json = Vec::with_capacity(gzip_bytes.len() * 8);
    GzDecoder::new(gzip_bytes)
        .read_to_end(&mut json)
        .map_err(|error| format!("gzip: {error}"))?;
    let document: Document =
        serde_json::from_slice(&json).map_err(|error| format!("json: {error}"))?;
    let base = document
        .base_timestamp
        .ok_or("trace is missing its base timestamp")?;
    let rows = document.rows.ok_or("trace is missing its rows")?;
    let (mut points, mut callsigns, mut emitter_category) = (
        Vec::with_capacity(rows.len()),
        Vec::<CallsignChange>::new(),
        0,
    );
    for row in rows.into_iter().flatten() {
        let point = row.point(base);
        if let Some(callsign) = row.callsign
            && callsigns.last().map(|c| c.callsign.as_str()) != Some(callsign.as_str())
        {
            callsigns.push(CallsignChange {
                point_index: points.len(),
                callsign,
            });
        }
        if emitter_category == 0 {
            emitter_category = row.emitter_category;
        }
        points.push(point);
    }
    Ok((points.len() >= 2).then(|| AircraftTrace {
        address: document.address.unwrap_or_default(),
        aircraft_type: document.aircraft_type.unwrap_or_default(),
        emitter_category,
        points,
        callsigns,
    }))
}

struct Document {
    address: Option<String>,
    aircraft_type: Option<String>,
    base_timestamp: Option<f64>,
    rows: Option<Vec<Option<Row>>>,
}

#[derive(Default)]
struct Row {
    offset_s: f64,
    lat: f64,
    lon: f64,
    altitude: Cell,
    ground_speed_kt: f64,
    track_deg: f64,
    readsb_flags: u64,
    vertical_rate_fpm: f64,
    callsign: Option<String>,
    emitter_category: u8,
    geometric_altitude_ft: f64,
}

impl Row {
    fn point(&self, base_timestamp: f64) -> TracePoint {
        let geometric_field = self.readsb_flags & ALTITUDE_IS_GEOMETRIC != 0;
        let geometric = self.geometric_altitude_ft as f32;
        let (altitude_ft, geometric_altitude_ft, flags) = match self.altitude {
            // readsb repeats a stale geometric altitude on the ground.
            Cell::Ground => (f32::NAN, f32::NAN, SURFACE_REPORT),
            Cell::Number(feet) if geometric_field && geometric.is_nan() => {
                (feet as f32, feet as f32, 0)
            }
            Cell::Number(feet) => (feet as f32, geometric, 0),
            _ => (f32::NAN, geometric, 0),
        };
        TracePoint {
            timestamp: base_timestamp + self.offset_s,
            lat: self.lat as f32,
            lon: self.lon as f32,
            altitude_ft,
            geometric_altitude_ft,
            ground_speed_kt: self.ground_speed_kt as f32,
            track_deg: self.track_deg as f32,
            vertical_rate_fpm: self.vertical_rate_fpm as f32,
            flags,
        }
    }
}

/// One JSON value of a row.
#[derive(Default)]
enum Cell {
    Number(f64),
    Ground,
    Details {
        callsign: Option<String>,
        emitter_category: u8,
    },
    #[default]
    Other,
}

impl Cell {
    fn number_or(&self, default: f64) -> f64 {
        match self {
            Cell::Number(value) => *value,
            _ => default,
        }
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(CellVisitor)
    }
}

struct CellVisitor;

impl<'de> Visitor<'de> for CellVisitor {
    type Value = Cell;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a trace row field")
    }

    fn visit_f64<E>(self, value: f64) -> Result<Cell, E> {
        Ok(Cell::Number(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Cell, E> {
        Ok(Cell::Number(value as f64))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Cell, E> {
        Ok(Cell::Number(value as f64))
    }

    fn visit_str<E>(self, value: &str) -> Result<Cell, E> {
        Ok(if value.eq_ignore_ascii_case("ground") {
            Cell::Ground
        } else {
            Cell::Other
        })
    }

    fn visit_bool<E>(self, _: bool) -> Result<Cell, E> {
        Ok(Cell::Other)
    }

    fn visit_unit<E>(self) -> Result<Cell, E> {
        Ok(Cell::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Cell, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Cell::Other)
    }

    /// The details object: only the callsign and the emitter category are kept.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Cell, A::Error> {
        let (mut callsign, mut emitter_category) = (None, 0);
        while let Some(key) = map.next_key::<Key>()? {
            match key {
                Key::Flight => {
                    callsign = map
                        .next_value::<Option<String>>()?
                        .map(|text| text.trim().to_string())
                        .filter(|text| !text.is_empty())
                }
                Key::Category => {
                    emitter_category = category_code(map.next_value::<Option<String>>()?)
                }
                Key::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Cell::Details {
            callsign,
            emitter_category,
        })
    }
}

/// ADS-B emitter category `A0`-`D7` as 0xA0-0xD7; anything else 0.
fn category_code(text: Option<String>) -> u8 {
    match text.as_deref().map(str::as_bytes) {
        Some([letter @ b'A'..=b'D', digit @ b'0'..=b'9']) => {
            ((letter - b'A' + 0xa) << 4) | (digit - b'0')
        }
        _ => 0,
    }
}

/// A details key, read without allocating.
enum Key {
    Flight,
    Category,
    Other,
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;
        impl Visitor<'_> for KeyVisitor {
            type Value = Key;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a key")
            }
            fn visit_str<E>(self, key: &str) -> Result<Key, E> {
                Ok(match key {
                    "flight" => Key::Flight,
                    "category" => Key::Category,
                    _ => Key::Other,
                })
            }
        }
        deserializer.deserialize_str(KeyVisitor)
    }
}

/// A row of at least 7 fields, else `None`.
struct RowOrShort(Option<Row>);

impl<'de> Deserialize<'de> for RowOrShort {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor;
        impl<'de> Visitor<'de> for RowVisitor {
            type Value = RowOrShort;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a trace row")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<RowOrShort, A::Error> {
                let mut row = Row {
                    geometric_altitude_ft: f64::NAN,
                    ..Row::default()
                };
                let mut fields = 0;
                while let Some(cell) = seq.next_element::<Cell>()? {
                    match (fields, cell) {
                        (0, cell) => row.offset_s = cell.number_or(f64::NAN),
                        (1, cell) => row.lat = cell.number_or(f64::NAN),
                        (2, cell) => row.lon = cell.number_or(f64::NAN),
                        (3, cell) => row.altitude = cell,
                        (4, cell) => row.ground_speed_kt = cell.number_or(0.0),
                        (5, cell) => row.track_deg = cell.number_or(0.0),
                        (6, cell) => row.readsb_flags = cell.number_or(0.0) as u64,
                        (7, cell) => row.vertical_rate_fpm = cell.number_or(0.0),
                        (
                            8,
                            Cell::Details {
                                callsign,
                                emitter_category,
                            },
                        ) => (row.callsign, row.emitter_category) = (callsign, emitter_category),
                        (10, cell) => row.geometric_altitude_ft = cell.number_or(f64::NAN),
                        _ => {}
                    }
                    fields += 1;
                }
                Ok(RowOrShort((fields >= 7).then_some(row)))
            }
        }
        deserializer.deserialize_seq(RowVisitor)
    }
}

impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DocumentVisitor;
        impl<'de> Visitor<'de> for DocumentVisitor {
            type Value = Document;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a readsb trace document")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Document, A::Error> {
                let mut document = Document {
                    address: None,
                    aircraft_type: None,
                    base_timestamp: None,
                    rows: None,
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "icao" => document.address = map.next_value()?,
                        "t" => document.aircraft_type = map.next_value()?,
                        "timestamp" => document.base_timestamp = map.next_value()?,
                        "trace" => {
                            let rows: Vec<RowOrShort> = map.next_value()?;
                            document.rows = Some(rows.into_iter().map(|row| row.0).collect());
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(document)
            }
        }
        deserializer.deserialize_map(DocumentVisitor)
    }
}

#[cfg(test)]
#[path = "readsb_tests.rs"]
pub(crate) mod tests;

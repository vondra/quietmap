//! What the click panel shows of a source, as typed fields: one schema per layer (each field's
//! name and how it is stored), a display record per distinct display of a file, a string table
//! holding each distinct text once. The builders hand a display over as the positional JSON array
//! of its layer's fields, the readers get the same array back, so the panel's fields keep their
//! names and JSON types; a number keeps the step of its field.

use super::Layer;
use crate::FormatError;
use serde_json::Value;
use std::collections::HashMap;

/// How a display field is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A text: the u32 offset of its entry in the string table (u32::MAX: null).
    Text,
    /// A whole number in `bytes` (1 or 2) unsigned bytes; the largest is null.
    Whole(u8),
    /// A decimal number to `decimals` places as a signed whole count of 10^-decimals in `bytes`
    /// (2 or 4) bytes; the smallest is null.
    Decimal { decimals: u8, bytes: u8 },
    /// Any number as its f64 (NaN: null), for a few layers' few records.
    Real,
    /// A boolean in a byte (255: null).
    Flag,
    /// Three decimals, one per period (all three null: null).
    Three { decimals: u8, bytes: u8 },
}

impl FieldKind {
    pub fn bytes(self) -> usize {
        match self {
            FieldKind::Text => 4,
            FieldKind::Whole(bytes) | FieldKind::Decimal { bytes, .. } => usize::from(bytes),
            FieldKind::Real => 8,
            FieldKind::Flag => 1,
            FieldKind::Three { bytes, .. } => 3 * usize::from(bytes),
        }
    }
}

/// One display field.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub kind: FieldKind,
}

const fn field(name: &'static str, kind: FieldKind) -> Field {
    Field { name, kind }
}

use FieldKind::{Flag, Real, Text, Whole};

/// A count of tenths in 4 bytes (vehicles a day), of hundredths, of whole units (areas).
const TENTHS_4: FieldKind = FieldKind::Decimal {
    decimals: 1,
    bytes: 4,
};
const TENTHS_2: FieldKind = FieldKind::Decimal {
    decimals: 1,
    bytes: 2,
};
const WHOLE_4: FieldKind = FieldKind::Decimal {
    decimals: 0,
    bytes: 4,
};

const ROAD: [Field; 19] = [
    field("name", Text),
    field("ref", Text),
    field("road_class", Text),
    field("aadt_light", TENTHS_4),
    field("aadt_medium", TENTHS_4),
    field("aadt_heavy", TENTHS_4),
    field("aadt_moto", TENTHS_4),
    field("traffic_estimated", Whole(1)),
    field("cross_section_aadt", WHOLE_4),
    field("speed_posted_kmh", Whole(1)),
    // The mean speed the emission takes (a limit times 0.845 is not a decimal of its own).
    field("speed_kmh", TENTHS_2),
    field("speed_source", Text),
    field("surface", Text),
    field("surface_corr_db", TENTHS_2),
    field("lanes", Whole(1)),
    field("oneway", Flag),
    field("bridge", Flag),
    field("source_id", Whole(2)),
    field(
        "period_shares",
        FieldKind::Three {
            decimals: 3,
            bytes: 2,
        },
    ),
];
const RAILWAY: [Field; 17] = [
    field("name", Text),
    field("ref", Text),
    field("rail_type", Text),
    field("usage", Text),
    // Three significant digits however few trains run (build/src/sources/rail.rs).
    field("trains_passenger_day", Real),
    field("trains_passenger_evening", Real),
    field("trains_passenger_night", Real),
    field("trains_freight_day", Real),
    field("trains_freight_evening", Real),
    field("trains_freight_night", Real),
    field("passenger_status", Whole(1)),
    field("freight_status", Whole(1)),
    field("speed_kmh", TENTHS_2),
    field("speed_source", Text),
    field("bridge", Flag),
    field("passenger_source_id", Whole(2)),
    field("freight_source_id", Whole(2)),
];
const INDUSTRY: [Field; 9] = [
    field("name", Text),
    field("source_type", Text),
    field("area_m2", WHOLE_4),
    field("nace", Text),
    field("grid_points", Whole(2)),
    // A wind turbine's hub and rating as its register gives them.
    field("hub_height_m", Real),
    field("rated_power_kw", Real),
    field("sound_power_dba", TENTHS_2),
    field("source_id", Whole(2)),
];
const BUILDING: [Field; 11] = [
    field("name", Text),
    field("building_type", Text),
    field("height_m", TENTHS_2),
    field("floors", Whole(1)),
    field("area_m2", WHOLE_4),
    field("address", Text),
    field("sound_power_dba", TENTHS_2),
    field("movements_per_day", TENTHS_4),
    field(
        "events_per_day",
        FieldKind::Three {
            decimals: 2,
            bytes: 4,
        },
    ),
    field(
        "duty",
        FieldKind::Three {
            decimals: 5,
            bytes: 4,
        },
    ),
    field("object", Text),
];
const SHIP: [Field; 5] = [
    field("source_type", Text),
    field("area_m2", WHOLE_4),
    field(
        "hours_per_month",
        FieldKind::Three {
            decimals: 2,
            bytes: 4,
        },
    ),
    field("sound_power_dba", TENTHS_2),
    field("source_id", Whole(2)),
];
const AIRCRAFT: [Field; 6] = [
    field("name", Text),
    field("subtype", Text),
    field("airport", Text),
    field("arrivals_per_day", Real),
    field("departures_per_day", Real),
    field("ground_vehicles_per_day", Real),
];

/// The display fields of a layer's records, in the order of their positional arrays.
pub fn display_fields(layer: Layer) -> &'static [Field] {
    match layer {
        Layer::Road => &ROAD,
        Layer::Railway => &RAILWAY,
        Layer::Industry => &INDUSTRY,
        Layer::Building => &BUILDING,
        Layer::Ship => &SHIP,
        Layer::Aircraft => &AIRCRAFT,
    }
}

/// The stored bytes of a record of `count` fields of `layer`: the count, then the fields.
fn record_bytes(layer: Layer, count: usize) -> usize {
    1 + display_fields(layer)[..count]
        .iter()
        .map(|field| field.kind.bytes())
        .sum::<usize>()
}

/// A display's whole count of 10^-decimals, its range that of `bytes` signed bytes less the null.
fn decimal_count(value: f64, decimals: u8, bytes: u8, name: &str) -> i64 {
    let count = (value * 10f64.powi(i32::from(decimals))).round();
    let largest = (1i64 << (8 * bytes - 1)) - 1;
    assert!(
        count.abs() <= largest as f64,
        "display field {name} {value} outside its {bytes} bytes"
    );
    count as i64
}

/// The display records and string table of a file being written.
#[derive(Default)]
pub struct DisplayTable {
    pub records: Vec<u8>,
    pub strings: Vec<u8>,
    record_at: HashMap<Vec<u8>, u32>,
    string_at: HashMap<String, u32>,
}

impl DisplayTable {
    fn text(&mut self, text: &str) -> u32 {
        if let Some(&at) = self.string_at.get(text) {
            return at;
        }
        let at = u32::try_from(self.strings.len()).expect("a tile's strings");
        let length = u16::try_from(text.len()).expect("a display text under 64 KiB");
        self.strings.extend_from_slice(&length.to_le_bytes());
        self.strings.extend_from_slice(text.as_bytes());
        self.string_at.insert(text.to_string(), at);
        at
    }

    /// The offset of a display's record, written once per distinct record.
    pub fn record(&mut self, layer: Layer, display: &[Value]) -> u32 {
        let fields = display_fields(layer);
        assert!(
            display.len() <= fields.len(),
            "{} display fields for {:?}",
            display.len(),
            layer
        );
        let mut record = vec![display.len() as u8];
        for (field, value) in fields.iter().zip(display) {
            self.write_field(&mut record, *field, value);
        }
        if let Some(&at) = self.record_at.get(&record) {
            return at;
        }
        let at = u32::try_from(self.records.len()).expect("a tile's display records");
        self.records.extend_from_slice(&record);
        self.record_at.insert(record, at);
        at
    }

    fn write_field(&mut self, record: &mut Vec<u8>, field: Field, value: &Value) {
        match field.kind {
            Text => {
                let at = match value {
                    Value::Null => u32::MAX,
                    Value::String(text) => self.text(text),
                    _ => cannot_hold(field.name, value),
                };
                record.extend_from_slice(&at.to_le_bytes());
            }
            Whole(bytes) => {
                let largest = (1u64 << (8 * bytes)) - 1;
                let whole = match value {
                    Value::Null => largest,
                    _ => value
                        .as_u64()
                        .filter(|&whole| whole < largest)
                        .unwrap_or_else(|| cannot_hold(field.name, value)),
                };
                record.extend_from_slice(&whole.to_le_bytes()[..usize::from(bytes)]);
            }
            FieldKind::Decimal { decimals, bytes } => {
                let count = match value {
                    Value::Null => -(1i64 << (8 * bytes - 1)),
                    _ => decimal_count(
                        value
                            .as_f64()
                            .unwrap_or_else(|| cannot_hold(field.name, value)),
                        decimals,
                        bytes,
                        field.name,
                    ),
                };
                record.extend_from_slice(&count.to_le_bytes()[..usize::from(bytes)]);
            }
            Real => {
                let real = match value {
                    Value::Null => f64::NAN,
                    _ => value
                        .as_f64()
                        .unwrap_or_else(|| cannot_hold(field.name, value)),
                };
                record.extend_from_slice(&real.to_le_bytes());
            }
            Flag => record.push(match value {
                Value::Null => u8::MAX,
                _ => u8::from(
                    value
                        .as_bool()
                        .unwrap_or_else(|| cannot_hold(field.name, value)),
                ),
            }),
            FieldKind::Three { decimals, bytes } => {
                let null = [Value::Null, Value::Null, Value::Null];
                let values = match value {
                    Value::Null => &null[..],
                    Value::Array(values) if values.len() == 3 => &values[..],
                    _ => cannot_hold(field.name, value),
                };
                for value in values {
                    let entry = Field {
                        name: field.name,
                        kind: FieldKind::Decimal { decimals, bytes },
                    };
                    self.write_field(record, entry, value);
                }
            }
        }
    }
}

fn cannot_hold(name: &str, value: &Value) -> ! {
    panic!("display field {name} cannot hold {value}")
}

/// A display record of `layer` at `at` in `records`, its texts in `strings`, as the positional
/// JSON array it was written from.
pub fn read_record(
    layer: Layer,
    records: &[u8],
    strings: &[u8],
    at: usize,
) -> Result<Vec<Value>, FormatError> {
    let out_of_range = FormatError("sources: display out of range");
    let count = usize::from(*records.get(at).ok_or(out_of_range)?);
    let fields = display_fields(layer);
    if count > fields.len() || at + record_bytes(layer, count) > records.len() {
        return Err(out_of_range);
    }
    let mut cursor = at + 1;
    fields[..count]
        .iter()
        .map(|field| {
            let bytes = &records[cursor..cursor + field.kind.bytes()];
            cursor += field.kind.bytes();
            read_field(field.kind, bytes, strings)
        })
        .collect()
}

fn signed(bytes: &[u8]) -> i64 {
    let mut wide = [0u8; 8];
    wide[..bytes.len()].copy_from_slice(bytes);
    let shift = 64 - 8 * bytes.len() as u32;
    (i64::from_le_bytes(wide) << shift) >> shift
}

fn read_field(kind: FieldKind, bytes: &[u8], strings: &[u8]) -> Result<Value, FormatError> {
    Ok(match kind {
        Text => {
            let at = u32::from_le_bytes(bytes.try_into().unwrap());
            if at == u32::MAX {
                return Ok(Value::Null);
            }
            let bad = FormatError("sources: display text out of range");
            let at = at as usize;
            let length = strings.get(at..at + 2).ok_or(bad)?;
            let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
            let text = strings.get(at + 2..at + 2 + length).ok_or(bad)?;
            Value::String(
                std::str::from_utf8(text)
                    .map_err(|_| FormatError("sources: display text is not UTF-8"))?
                    .to_string(),
            )
        }
        Whole(_) => {
            let mut wide = [0u8; 8];
            wide[..bytes.len()].copy_from_slice(bytes);
            let whole = u64::from_le_bytes(wide);
            if whole == (1u64 << (8 * bytes.len())) - 1 {
                Value::Null
            } else {
                Value::from(whole)
            }
        }
        FieldKind::Decimal { decimals, .. } => {
            let count = signed(bytes);
            if count == -(1i64 << (8 * bytes.len() - 1)) {
                Value::Null
            } else {
                // A whole count over a power of ten: the nearest f64 to the decimal, written back
                // as that decimal.
                Value::from(count as f64 / 10f64.powi(i32::from(decimals)))
            }
        }
        Real => {
            let real = f64::from_le_bytes(bytes.try_into().unwrap());
            if real.is_nan() {
                Value::Null
            } else {
                Value::from(real)
            }
        }
        Flag => match bytes[0] {
            u8::MAX => Value::Null,
            flag => Value::Bool(flag != 0),
        },
        FieldKind::Three {
            decimals,
            bytes: width,
        } => {
            let width = usize::from(width);
            let values: Vec<Value> = bytes
                .chunks_exact(width)
                .map(|bytes| {
                    read_field(
                        FieldKind::Decimal {
                            decimals,
                            bytes: width as u8,
                        },
                        bytes,
                        strings,
                    )
                })
                .collect::<Result<_, _>>()?;
            if values.iter().all(Value::is_null) {
                Value::Null
            } else {
                Value::Array(values)
            }
        }
    })
}

//! The aeroway lines ground traffic runs on and the airports they belong to, read from the dev4
//! prepared tree until the builders read OSM themselves: `airport_lines.arrow` (OSM runways,
//! taxiways, stopways and airstrips as microsegments of at most 250 m) and the aerodromes of
//! `airport_areas.arrow`. Everything after this module works on [`Aeroways`].

use crate::aircraft::flat::flat_distance_m;
use crate::dev4::{Dev4, Square, column, text, z30_corner_degrees};
use arrow_array::{Float32Array, Int16Array, Int32Array, Int64Array, StringArray, UInt8Array};
use physics::emission::airport::GroundOperation;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use tiles::geo::MAX_LATITUDE_DEG;

/// dev4's aeroway code of an aerodrome polygon (`classify::aeroway_type`).
const AERODROME: u8 = 5;
/// A line belongs to the nearest aerodrome whose centroid lies within the larger of 6 km and 1.5
/// radii of a disc of the aerodrome's area (dev4 `aerodrome_radius_m`).
const AERODROME_MIN_RADIUS_M: f64 = 6_000.0;
const AERODROME_RADIUS_FACTOR: f64 = 1.5;

/// One aeroway microsegment.
#[derive(Debug, Clone, PartialEq)]
pub struct AerowayLine {
    pub osm_id: i64,
    pub segment: u16,
    /// Latitude and longitude of both ends (deg).
    pub ends: [[f64; 2]; 2],
    pub operation: GroundOperation,
    /// Index into [`Aeroways::airports`].
    pub airport: u32,
    /// The square whose file lists it: its traffic is written there.
    pub square: Square,
}

/// An airport: its key (ICAO, else IATA, else name, as dev4) and its OSM name.
#[derive(Debug, Clone, PartialEq)]
pub struct Airport {
    pub key: String,
    pub name: String,
}

/// The aeroway lines of a set of squares and their airports.
#[derive(Debug, Default)]
pub struct Aeroways {
    pub lines: Vec<AerowayLine>,
    pub airports: Vec<Airport>,
    keys: HashMap<String, u32>,
    /// The aerodromes of the squares and their neighbours (for lines found later).
    aerodromes: Vec<Aerodrome>,
}

/// Discovered lines are cut into pieces of at most this length (m), as dev4 cut OSM lines.
const PIECE_M: f64 = 250.0;

/// An aerodrome polygon as a candidate airport of lines.
#[derive(Debug, Clone)]
struct Aerodrome {
    osm_id: i64,
    airport: Airport,
    /// Latitude and longitude of the centroid.
    centre: [f64; 2],
    radius_m: f64,
}

/// dev4 aeroway codes of lines: runway 0, stopway 6 and airstrip 7 roll, taxiway 1 taxis.
fn operation_of(aeroway: u8) -> Option<GroundOperation> {
    match aeroway {
        0 | 6 | 7 => Some(GroundOperation::RunwayRoll),
        1 => Some(GroundOperation::Taxi),
        _ => None,
    }
}

fn distance_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    f64::from(flat_distance_m(
        a[0] as f32,
        a[1] as f32,
        b[0] as f32,
        b[1] as f32,
    ))
}

/// The named aerodromes of one square.
fn aerodromes(dev4: &Dev4, square: Square) -> Result<Vec<Aerodrome>, String> {
    let Some(table) = dev4.table(square, "airport_areas.arrow")? else {
        return Ok(Vec::new());
    };
    let context = |error: String| format!("airport_areas.arrow of {square:?}: {error}");
    let mut found = Vec::new();
    for batch in &table.batches {
        let kind = column::<UInt8Array>(batch, "aeroway_type").map_err(context)?;
        let osm_id = column::<Int64Array>(batch, "osm_id").map_err(context)?;
        let (gx, gy) = (
            column::<Int32Array>(batch, "centroid_gx").map_err(context)?,
            column::<Int32Array>(batch, "centroid_gy").map_err(context)?,
        );
        let area = column::<Float32Array>(batch, "area_m2").map_err(context)?;
        let labels = ["icao", "iata", "name"]
            .map(|name| column::<StringArray>(batch, name).map_err(context));
        let [icao, iata, name] = labels;
        let (icao, iata, name) = (icao?, iata?, name?);
        for row in 0..batch.num_rows() {
            if kind.value(row) != AERODROME {
                continue;
            }
            let label = |values: &StringArray| text(values, row).trim().to_string();
            let name = label(name);
            let Some(key) = [label(icao), label(iata), name.clone()]
                .into_iter()
                .find(|key| !key.is_empty())
            else {
                continue;
            };
            let (lat, lon) = z30_corner_degrees(gx.value(row), gy.value(row));
            let area_m2 = crate::dev4::positive(area, row).unwrap_or(0.0);
            found.push(Aerodrome {
                osm_id: osm_id.value(row),
                airport: Airport { key, name },
                centre: [lat, lon],
                radius_m: AERODROME_MIN_RADIUS_M
                    .max(AERODROME_RADIUS_FACTOR * (area_m2 / std::f64::consts::PI).sqrt()),
            });
        }
    }
    Ok(found)
}

/// The airport of a line with its middle at `middle`: the nearest aerodrome within its radius (the
/// first by OSM id on a tie), else an airstrip named by its 0.01 degree cell.
fn airport_at(middle: [f64; 2], aerodromes: &[Aerodrome]) -> Airport {
    let mut nearest: Option<(&Aerodrome, f64)> = None;
    for aerodrome in aerodromes {
        let distance = distance_m(middle, aerodrome.centre);
        if distance <= aerodrome.radius_m && nearest.is_none_or(|(_, best)| distance < best) {
            nearest = Some((aerodrome, distance));
        }
    }
    nearest.map_or_else(
        || Airport {
            key: format!(
                "airstrip {:.2},{:.2}",
                (middle[0] * 100.0).floor() / 100.0,
                (middle[1] * 100.0).floor() / 100.0
            ),
            name: String::new(),
        },
        |(aerodrome, _)| aerodrome.airport.clone(),
    )
}

impl Aeroways {
    /// The lines of `squares` with their airports (aerodromes of each square and its neighbours).
    /// Lines beyond the Mercator latitude limit are left out: no tile can hold them.
    pub fn read(dev4: &Dev4, squares: &[Square]) -> Result<Self, String> {
        let mut aeroways = Aeroways::default();
        let mut by_square: HashMap<Square, Vec<Aerodrome>> = HashMap::new();
        for &square in squares {
            let mut nearby = Vec::new();
            for neighbour in square.with_neighbours() {
                let found = match by_square.entry(neighbour) {
                    Entry::Occupied(found) => found.into_mut(),
                    Entry::Vacant(slot) => slot.insert(aerodromes(dev4, neighbour)?),
                };
                nearby.extend(found.iter().cloned());
            }
            nearby.sort_by_key(|aerodrome| aerodrome.osm_id);
            nearby.dedup_by_key(|aerodrome| aerodrome.osm_id);
            aeroways.aerodromes.extend(nearby.iter().cloned());
            let Some(table) = dev4.table(square, "airport_lines.arrow")? else {
                continue;
            };
            let context = |error: String| format!("airport_lines.arrow of {square:?}: {error}");
            for batch in &table.batches {
                let osm_id = column::<Int64Array>(batch, "osm_id").map_err(context)?;
                let segment = column::<Int16Array>(batch, "segment_idx").map_err(context)?;
                let kind = column::<UInt8Array>(batch, "aeroway_type").map_err(context)?;
                let grid = ["start_gx", "start_gy", "end_gx", "end_gy"]
                    .map(|name| column::<Int32Array>(batch, name).map_err(context));
                let [start_x, start_y, end_x, end_y] = grid;
                let (start_x, start_y, end_x, end_y) = (start_x?, start_y?, end_x?, end_y?);
                for row in 0..batch.num_rows() {
                    let Some(operation) = operation_of(kind.value(row)) else {
                        continue;
                    };
                    let corner = |x: &Int32Array, y: &Int32Array| {
                        let (lat, lon) = z30_corner_degrees(x.value(row), y.value(row));
                        [lat, lon]
                    };
                    let ends = [corner(start_x, start_y), corner(end_x, end_y)];
                    if ends.iter().any(|end| end[0].abs() > MAX_LATITUDE_DEG) {
                        continue;
                    }
                    let (lat, lon) = crate::aircraft::flat::interpolate(
                        ends[0][0] as f32,
                        ends[0][1] as f32,
                        ends[1][0] as f32,
                        ends[1][1] as f32,
                        0.5,
                    );
                    let airport = aeroways.intern(airport_at([lat, lon].map(f64::from), &nearby));
                    aeroways.lines.push(AerowayLine {
                        osm_id: osm_id.value(row),
                        segment: u16::try_from(segment.value(row))
                            .map_err(|_| context("negative segment_idx".into()))?,
                        ends,
                        operation,
                        airport,
                        square,
                    });
                }
            }
        }
        aeroways
            .aerodromes
            .sort_by_key(|aerodrome| aerodrome.osm_id);
        aeroways
            .aerodromes
            .dedup_by_key(|aerodrome| aerodrome.osm_id);
        Ok(aeroways)
    }

    /// Adds discovered airstrips (latitude and longitude of both ends) as runway lines cut into
    /// equal pieces of at most [`PIECE_M`], each strip of the airport its middle belongs to and
    /// each piece in the square of its middle; `square_of` gives a point's square.
    pub fn add_strips(&mut self, strips: &[[[f64; 2]; 2]], square_of: impl Fn([f64; 2]) -> Square) {
        for strip in strips {
            let at = |t: f64| {
                let (lat, lon) = crate::aircraft::flat::interpolate(
                    strip[0][0] as f32,
                    strip[0][1] as f32,
                    strip[1][0] as f32,
                    strip[1][1] as f32,
                    t as f32,
                );
                [f64::from(lat), f64::from(lon)]
            };
            let airport = self.intern(airport_at(at(0.5), &self.aerodromes));
            let pieces = (distance_m(strip[0], strip[1]) / PIECE_M).ceil().max(1.0) as usize;
            let osm_id = super::strips::strip_id(strip);
            for piece in 0..pieces {
                let (from, to) = (
                    piece as f64 / pieces as f64,
                    (piece + 1) as f64 / pieces as f64,
                );
                self.lines.push(AerowayLine {
                    osm_id,
                    segment: piece as u16,
                    ends: [at(from), at(to)],
                    operation: GroundOperation::RunwayRoll,
                    airport,
                    square: square_of(at(0.5 * (from + to))),
                });
            }
        }
    }

    /// The index of an airport, added on first sight (by key; the first name stays).
    pub fn intern(&mut self, airport: Airport) -> u32 {
        if let Some(&index) = self.keys.get(&airport.key) {
            return index;
        }
        let index = self.airports.len() as u32;
        self.keys.insert(airport.key.clone(), index);
        self.airports.push(airport);
        index
    }
}

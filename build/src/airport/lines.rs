//! The aeroway lines ground traffic runs on and the airports they belong to, read from the dev4
//! prepared tree until the builders read OSM themselves: `airport_lines.arrow` (OSM runways,
//! taxiways, stopways and airstrips as microsegments of at most 250 m) and the aerodromes of
//! `airport_areas.arrow`. Everything after this module works on [`Aeroways`].

use super::ownership::{Aerodrome, aerodromes, airport_of};
use crate::aircraft::flat::flat_distance_m;
use crate::dev4::{Dev4, Square, column, z30_corner_degrees};
use arrow_array::{Int16Array, Int32Array, Int64Array, UInt8Array};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use tiles::geo::MAX_LATITUDE_DEG;

/// What an aeroway line is: a runway (or an airstrip), where aircraft roll; a stopway, the
/// pavement past a runway's end that only an aborted take-off uses; a taxiway (aprons are areas,
/// which dev4's ground traffic never met).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aeroway {
    Runway,
    Stopway,
    Taxiway,
}

/// One aeroway microsegment.
#[derive(Debug, Clone, PartialEq)]
pub struct AerowayLine {
    pub osm_id: i64,
    pub segment: u16,
    /// Latitude and longitude of both ends (deg).
    pub ends: [[f64; 2]; 2],
    pub aeroway: Aeroway,
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
#[cfg_attr(test, derive(Clone))]
pub struct Aeroways {
    pub lines: Vec<AerowayLine>,
    pub airports: Vec<Airport>,
    keys: HashMap<String, u32>,
    /// The aerodromes of the squares and their neighbours (for lines found later).
    pub(super) aerodromes: Vec<Aerodrome>,
}

/// Discovered lines are cut into pieces of at most this length (m), as dev4 cut OSM lines.
const PIECE_M: f64 = 250.0;

/// dev4 aeroway codes of lines: runway 0 and airstrip 7, stopway 6, taxiway 1.
fn aeroway_of(code: u8) -> Option<Aeroway> {
    match code {
        0 | 7 => Some(Aeroway::Runway),
        6 => Some(Aeroway::Stopway),
        1 => Some(Aeroway::Taxiway),
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
                    let Some(aeroway) = aeroway_of(kind.value(row)) else {
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
                    let airport = aeroways.intern(airport_of(&[ends], &nearby));
                    aeroways.lines.push(AerowayLine {
                        osm_id: osm_id.value(row),
                        segment: u16::try_from(segment.value(row))
                            .map_err(|_| context("negative segment_idx".into()))?,
                        ends,
                        aeroway,
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
        // Resolve the complete mapped way, including pieces across square boundaries, once.
        let mut runways: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (index, line) in aeroways.lines.iter().enumerate() {
            if line.aeroway == Aeroway::Runway {
                runways.entry(line.osm_id).or_default().push(index);
            }
        }
        for pieces in runways.values() {
            let mut nearby: Vec<Aerodrome> = pieces
                .iter()
                .flat_map(|&index| {
                    aeroways.lines[index]
                        .square
                        .with_neighbours()
                        .into_iter()
                        .flat_map(|square| by_square.get(&square).into_iter().flatten().cloned())
                })
                .collect();
            nearby.sort_by_key(|aerodrome| aerodrome.osm_id);
            nearby.dedup_by_key(|aerodrome| aerodrome.osm_id);
            let geometry: Vec<_> = pieces
                .iter()
                .map(|&index| aeroways.lines[index].ends)
                .collect();
            let airport = aeroways.intern(airport_of(&geometry, &nearby));
            for &index in pieces {
                aeroways.lines[index].airport = airport;
            }
        }
        Ok(aeroways)
    }

    /// Adds discovered airstrips (latitude and longitude of both ends) as runway lines cut into
    /// equal pieces of at most [`PIECE_M`], each strip owned from its complete geometry and
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
            let pieces = (distance_m(strip[0], strip[1]) / PIECE_M).ceil().max(1.0) as usize;
            let geometry: Vec<_> = (0..pieces)
                .map(|piece| {
                    [
                        at(piece as f64 / pieces as f64),
                        at((piece + 1) as f64 / pieces as f64),
                    ]
                })
                .collect();
            let airport = self.airport_for_geometry(&geometry);
            let osm_id = super::strips::strip_id(strip);
            for (piece, ends) in geometry.into_iter().enumerate() {
                let (from, to) = (
                    piece as f64 / pieces as f64,
                    (piece + 1) as f64 / pieces as f64,
                );
                self.lines.push(AerowayLine {
                    osm_id,
                    segment: piece as u16,
                    ends,
                    aeroway: Aeroway::Runway,
                    airport,
                    square: square_of(at(0.5 * (from + to))),
                });
            }
        }
    }

    /// The airport of complete geometry, including joined ways and discovered strips.
    pub(super) fn airport_for_geometry(&mut self, pieces: &[[[f64; 2]; 2]]) -> u32 {
        self.intern(airport_of(pieces, &self.aerodromes))
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

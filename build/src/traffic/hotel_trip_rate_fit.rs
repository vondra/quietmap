//! A hotel's trips per 100 m2 of floor: the observed car trips a room over the gross floor a room
//! of the year's mapped lodgings that state their rooms and stand in hotel-typed buildings.
use super::{demand_storeys, gross_floor_m2, mapped_footprint_m2};
use crate::dev4::{Dev4, Square, Table, cell, column, require_stamp};
use crate::rings::RingIndex;
use crate::sources::cells::ring_cell;
use crate::year::{Kind, YearInputs};
use anyhow::{Context, Result, anyhow, ensure};
use arrow_array::{BinaryArray, Float32Array, Int32Array, Int64Array, UInt8Array};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The builder's use of a hotel, hostel, motel or guest house (`osm/spill/building_tags.rs`).
const HOTEL_USE: u8 = 6;
const KIND_BUILDING: u8 = 0;

/// One factor of the car trip ends a hotel room makes a day, with its provenance.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Factor {
    factor: String,
    value: f64,
    /// `measured` or `assumed`.
    basis: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TripGenerationObservations {
    hotel_car_trip_ends_per_room: Vec<Factor>,
}

/// A line of `qm-build lodgings` (`fetch/lodgings.py`).
#[derive(Debug, PartialEq)]
struct Lodging {
    node: bool,
    id: i64,
    lat: f64,
    lon: f64,
    rooms: u32,
}

fn read_lodgings(path: &Path) -> Result<Vec<Lodging>> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    text.lines()
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            let [kind, id, lat, lon, rooms] = fields[..] else {
                return Err(anyhow!("{}: not a lodging: {line:?}", path.display()));
            };
            Ok(Lodging {
                node: match kind {
                    "node" => true,
                    "way" | "relation" => false,
                    other => return Err(anyhow!("{}: OSM type {other:?}", path.display())),
                },
                id: id.parse()?,
                lat: lat.parse()?,
                lon: lon.parse()?,
                rooms: rooms.parse()?,
            })
        })
        .collect()
}

/// The factors of the one trip-generation observation the year's hotel trip rate depends on.
fn observed_factors(inputs: &YearInputs) -> Result<Vec<Factor>> {
    let manifest = &inputs.manifest;
    let fitted = manifest
        .inputs
        .iter()
        .find(|input| input.kind == Kind::HotelTripRate)
        .context("the year declares no hotel trip rate")?;
    let observed: Vec<_> = fitted
        .depends_on
        .iter()
        .filter(|id| {
            manifest
                .inputs
                .iter()
                .any(|input| &input.id == *id && input.kind == Kind::ModelObservations)
        })
        .collect();
    ensure!(
        observed.len() == 1,
        "the hotel trip rate depends on {} trip-generation observations, not one",
        observed.len()
    );
    let path = inputs.by_id(observed[0]).map_err(anyhow::Error::msg)?;
    let parsed: TripGenerationObservations = serde_json::from_slice(&std::fs::read(&path)?)
        .with_context(|| path.display().to_string())?;
    let factors = parsed.hotel_car_trip_ends_per_room;
    ensure!(
        !factors.is_empty()
            && factors.iter().all(|factor| {
                factor.value.is_finite()
                    && factor.value > 0.0
                    && matches!(factor.basis.as_str(), "measured" | "assumed")
                    && !factor.factor.trim().is_empty()
                    && !factor.source.trim().is_empty()
            }),
        "{}: each hotel factor needs a positive value, a basis (measured or assumed) and a source",
        path.display()
    );
    Ok(factors)
}

/// A square's OSM buildings (kind 0 with an OSM id): use, gross floor and centroid, by id and by
/// outline.
struct SquareBuildings {
    buildings: Vec<(u8, f64, (i32, i32))>,
    by_id: HashMap<i64, usize>,
    outlines: RingIndex,
}

impl SquareBuildings {
    fn read(table: &Table) -> Result<Self> {
        let mut buildings = Vec::new();
        let mut by_id = HashMap::new();
        let mut outlines = RingIndex::default();
        for batch in &table.batches {
            let error = anyhow::Error::msg;
            let bytes = |name: &str| column::<UInt8Array>(batch, name).map_err(error);
            let (kinds, uses) = (bytes("kind")?, bytes("building_type")?);
            let (storeys, floors) = (bytes("storeys")?, bytes("floors")?);
            let ids = column::<Int64Array>(batch, "osm_id").map_err(error)?;
            let areas = column::<Float32Array>(batch, "area_m2").map_err(error)?;
            let rings = column::<BinaryArray>(batch, "emission_geom").map_err(error)?;
            let centroid = |name: &str| column::<Int32Array>(batch, name).map_err(error);
            let (x, y) = (centroid("centroid_gx")?, centroid("centroid_gy")?);
            for row in 0..batch.num_rows() {
                let Some(id) = cell(ids, row).filter(|_| kinds.value(row) == KIND_BUILDING) else {
                    continue;
                };
                let at = buildings.len();
                buildings.push((
                    cell(uses, row).unwrap_or(0),
                    gross_floor_m2(
                        demand_storeys(cell(storeys, row), cell(floors, row)),
                        mapped_footprint_m2(cell(areas, row)),
                    ),
                    (x.value(row), y.value(row)),
                ));
                by_id.entry(id).or_insert(at);
                let ring = ring_cell(rings, row);
                if ring.len() >= 3 {
                    outlines.add(at as u64, vec![vec![ring]]);
                }
            }
        }
        Ok(Self {
            buildings,
            by_id,
            outlines,
        })
    }

    /// The use and gross floor of a lodging's building: a way or relation's own row; for a node,
    /// of the outlines holding it a hotel-typed one first, then the one whose centroid lies nearest
    /// (a building inside a mapped complex, not the complex: 2026's rule).
    fn of(&self, lodging: &Lodging) -> Option<(u8, f64)> {
        let at = if lodging.node {
            let (x, y) = crate::osm::geometry::lonlat_to_grid(lodging.lon, lodging.lat);
            self.outlines.holding((x, y)).min_by_key(|&at| {
                let (building_use, _, (cx, cy)) = self.buildings[at as usize];
                let distance =
                    (i64::from(cx) - i64::from(x)).abs() + (i64::from(cy) - i64::from(y)).abs();
                (building_use != HOTEL_USE, distance)
            })? as usize
        } else {
            *self.by_id.get(&lodging.id)?
        };
        let (building_use, floor, _) = self.buildings[at];
        Some((building_use, floor))
    }
}

fn square_of(lodging: &Lodging) -> Square {
    let square = crate::osm::geometry::square_of(lodging.lat, lodging.lon);
    Square {
        x: square.x.into(),
        y: square.y.into(),
    }
}

/// Lodgings, their stated rooms and their buildings' gross floor (m2).
#[derive(Clone, Copy, Default)]
struct Sums {
    lodgings: u64,
    rooms: u64,
    gross_floor_m2: f64,
}

impl Sums {
    fn add(&mut self, rooms: u32, gross_floor_m2: f64) {
        self.lodgings += 1;
        self.rooms += u64::from(rooms);
        self.gross_floor_m2 += gross_floor_m2;
    }
}

pub fn fit(inputs: &YearInputs, output: &Path) -> Result<()> {
    ensure!(
        !output.exists(),
        "the hotel trip rate needs a new output path"
    );
    let factors = observed_factors(inputs)?;
    let car_trip_ends_per_room: f64 = factors.iter().map(|factor| factor.value).product();
    let structures = Dev4 {
        prepared: inputs
            .required(Kind::Structures)
            .map_err(anyhow::Error::msg)?
            .into(),
        rasters: PathBuf::new(),
    };
    let mut squares = BTreeMap::<Square, Vec<Lodging>>::new();
    for lodging in read_lodgings(
        inputs
            .required(Kind::Lodgings)
            .map_err(anyhow::Error::msg)?,
    )? {
        squares
            .entry(square_of(&lodging))
            .or_default()
            .push(lodging);
    }
    // By how a lodging met its building and that building's use; the rate reads hotel uses only.
    let mut matched = BTreeMap::<(&str, u8), Sums>::new();
    let mut unmatched = BTreeMap::<&str, Sums>::new();
    let mut rated_squares = Vec::new();
    let squares = squares.into_iter().collect::<Vec<_>>();
    // Each square's buildings are read in parallel; the sums below add in square order, as an f64 sum's
    // value depends on its order.
    let buildings_of_lodgings = squares
        .par_iter()
        .map(|(square, lodgings)| {
            let Some(table) = structures
                .table(*square, "structures.arrow")
                .map_err(anyhow::Error::msg)?
            else {
                return Ok(None);
            };
            require_stamp(&table, "structures_contract", "structures_v5")
                .map_err(anyhow::Error::msg)?;
            let buildings = SquareBuildings::read(&table)?;
            Ok(Some(
                lodgings
                    .iter()
                    .map(|lodging| buildings.of(lodging))
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    for ((square, lodgings), buildings) in squares.iter().zip(buildings_of_lodgings) {
        let Some(buildings) = buildings else {
            for lodging in lodgings {
                let without = unmatched.entry("square-without-structures").or_default();
                without.add(lodging.rooms, 0.0);
            }
            continue;
        };
        let mut rated = Sums::default();
        for (lodging, building) in lodgings.iter().zip(buildings) {
            let how = if lodging.node { "node-inside" } else { "by-id" };
            match building {
                Some((building_use, floor)) => {
                    matched
                        .entry((how, building_use))
                        .or_default()
                        .add(lodging.rooms, floor);
                    if building_use == HOTEL_USE {
                        rated.add(lodging.rooms, floor);
                    }
                }
                None => {
                    let reason = if lodging.node {
                        "node-in-no-building"
                    } else {
                        "id-without-building"
                    };
                    unmatched.entry(reason).or_default().add(lodging.rooms, 0.0);
                }
            }
        }
        if rated.lodgings > 0 {
            rated_squares.push((*square, rated));
        }
    }
    let rated = rated_squares
        .iter()
        .fold(Sums::default(), |total, (_, s)| Sums {
            lodgings: total.lodgings + s.lodgings,
            rooms: total.rooms + s.rooms,
            gross_floor_m2: total.gross_floor_m2 + s.gross_floor_m2,
        });
    ensure!(
        rated.rooms > 0 && rated.gross_floor_m2 > 0.0,
        "no lodging that states its rooms stands in a hotel-typed building"
    );
    let floor_per_room_m2 = rated.gross_floor_m2 / rated.rooms as f64;
    let trips_per_100_m2 = car_trip_ends_per_room * 100.0 / floor_per_room_m2;
    let sums = |s: &Sums| json!({"lodgings": s.lodgings, "rooms": s.rooms, "gross_floor_m2": s.gross_floor_m2});
    let support = json!({
        "factors": factors,
        "car_trip_ends_per_room": car_trip_ends_per_room,
        "floor_per_room_m2": floor_per_room_m2,
        "rated": sums(&rated),
        "matched": matched.iter().map(|((how, building_use), s)|
            json!({"match": how, "building_use": building_use, "sums": sums(s)})).collect::<Vec<_>>(),
        "unmatched": unmatched.iter().map(|(reason, s)|
            json!({"match": reason, "lodgings": s.lodgings, "rooms": s.rooms})).collect::<Vec<_>>(),
        "squares": rated_squares.iter().map(|(square, s)|
            json!([square.x, square.y, s.lodgings, s.rooms, s.gross_floor_m2])).collect::<Vec<_>>(),
    });
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({"rows": [trips_per_100_m2]}))?,
    )?;
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec(&support)?,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "hotel_trip_rate_fit_tests.rs"]
mod tests;

//! What the buildings make of the roads nobody counts. Every building's daily vehicle trip ends
//! (its dwellings times its country's trips per dwelling, or its activity's trips by floor area:
//! dev4's rates) join the nearest road within 50 m. Down the local streets (residential, living,
//! service, unclassified) each trip end travels to the nearest main road along a shortest-path
//! tree, so a street carries what the buildings behind it make, and no street more than the one
//! it drains into (dev4 added a background of 327-590 vehicles a day to every street and gave a
//! whole street its busiest row). A street beyond a bridge of the local network, seen from the
//! main roads, is a dead end: nothing passes through it. The trip ends are also summed per cell
//! of the square: their sum around a road measures how much its surroundings generate.
//!
//! One file per square, `<out>/z9/<x>/<y>.traffic`: per row of the square's `roads.arrow` the
//! trip ends routed through it (NaN where the row is no local street of a tree) and its dead-end
//! flag, then the square's grid of trip ends.

use crate::dev4::{Dev4, Square, Z9_PER_AXIS, cell, column, require_stamp};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, Int32Type, UInt8Type, UInt16Type};
use arrow_array::{Array, Float32Array, Int32Array, Int64Array, UInt8Array};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};

/// Cells per side of a square's grid of trip ends (about 200 m at 50 degrees).
pub const GRID_SIDE: usize = 256;
/// z30 cells per z9 square side and per grid cell.
const SQUARE_Z30: i64 = 1 << 21;
const CELL_Z30: i64 = SQUARE_Z30 / GRID_SIDE as i64;
/// The side of a dev4 z30 cell in Web Mercator metres.
const Z30_QUANTUM_M: f64 = 0.037_322_767_717_044_72;
/// A building joins the nearest road within this distance (dev4's frontage).
const FRONTAGE_M: f64 = 50.0;
const MAGIC: &[u8; 8] = b"QMTRAF01";

/// dev4 road classes: the local streets a tree runs down; tracks carry nothing.
const LOCAL_CLASSES: [u8; 4] = [5, 6, 7, 9];
const TRACK_CLASS: u8 = 8;
/// dev4 access codes closed to motor vehicles (no, and dev4's other excluded code).
const CLOSED_ACCESS: [u8; 2] = [2, 4];

/// Vehicle trip ends per occupied dwelling and day (dev4's country fleet table: MiD 2017, UK NTS,
/// NHTS 2022 and household surveys by continent, times 0.92 occupancy, clamped to 0.8-6).
const TRIPS_PER_DWELLING: [([u8; 2], f64); 238] = [
    (*b"AD", 3.40),
    (*b"AE", 2.30),
    (*b"AF", 2.30),
    (*b"AG", 3.22),
    (*b"AI", 3.22),
    (*b"AL", 3.40),
    (*b"AM", 2.30),
    (*b"AO", 1.38),
    (*b"AQ", 3.68),
    (*b"AR", 2.02),
    (*b"AS", 3.40),
    (*b"AT", 3.40),
    (*b"AU", 3.40),
    (*b"AW", 3.22),
    (*b"AX", 3.40),
    (*b"AZ", 2.30),
    (*b"BA", 3.40),
    (*b"BB", 3.22),
    (*b"BD", 2.30),
    (*b"BE", 3.40),
    (*b"BF", 1.38),
    (*b"BG", 3.40),
    (*b"BH", 2.30),
    (*b"BI", 1.38),
    (*b"BJ", 1.38),
    (*b"BL", 3.22),
    (*b"BM", 3.22),
    (*b"BN", 2.30),
    (*b"BO", 2.02),
    (*b"BR", 2.02),
    (*b"BS", 3.22),
    (*b"BT", 2.30),
    (*b"BW", 1.38),
    (*b"BY", 3.40),
    (*b"BZ", 3.22),
    (*b"CA", 3.22),
    (*b"CD", 1.38),
    (*b"CF", 1.38),
    (*b"CG", 1.38),
    (*b"CH", 3.40),
    (*b"CI", 1.38),
    (*b"CK", 3.40),
    (*b"CL", 2.02),
    (*b"CM", 1.38),
    (*b"CN", 1.38),
    (*b"CO", 2.02),
    (*b"CR", 3.22),
    (*b"CU", 3.22),
    (*b"CV", 1.38),
    (*b"CW", 3.22),
    (*b"CY", 2.30),
    (*b"CZ", 3.40),
    (*b"DE", 3.59),
    (*b"DJ", 1.38),
    (*b"DK", 3.40),
    (*b"DM", 3.22),
    (*b"DO", 3.22),
    (*b"DZ", 1.38),
    (*b"EC", 2.02),
    (*b"EE", 3.40),
    (*b"EG", 1.84),
    (*b"EH", 1.38),
    (*b"ER", 1.38),
    (*b"ES", 3.40),
    (*b"ET", 1.38),
    (*b"FI", 3.40),
    (*b"FJ", 3.40),
    (*b"FK", 2.02),
    (*b"FM", 3.40),
    (*b"FO", 3.40),
    (*b"FR", 3.40),
    (*b"GA", 1.38),
    (*b"GB", 3.40),
    (*b"GD", 3.22),
    (*b"GE", 2.30),
    (*b"GG", 3.40),
    (*b"GH", 1.38),
    (*b"GI", 3.40),
    (*b"GL", 3.22),
    (*b"GM", 1.38),
    (*b"GN", 1.38),
    (*b"GQ", 1.38),
    (*b"GR", 3.40),
    (*b"GS", 3.68),
    (*b"GT", 3.22),
    (*b"GU", 3.40),
    (*b"GW", 1.38),
    (*b"GY", 2.02),
    (*b"HK", 2.30),
    (*b"HM", 3.68),
    (*b"HN", 3.22),
    (*b"HR", 3.40),
    (*b"HT", 3.22),
    (*b"HU", 3.40),
    (*b"ID", 2.30),
    (*b"IE", 3.40),
    (*b"IL", 2.30),
    (*b"IM", 3.40),
    (*b"IN", 0.92),
    (*b"IO", 2.30),
    (*b"IQ", 2.30),
    (*b"IR", 2.30),
    (*b"IS", 3.40),
    (*b"IT", 3.40),
    (*b"JE", 3.40),
    (*b"JM", 3.22),
    (*b"JO", 2.30),
    (*b"JP", 2.30),
    (*b"KE", 1.38),
    (*b"KG", 2.30),
    (*b"KH", 2.30),
    (*b"KI", 3.40),
    (*b"KM", 1.38),
    (*b"KN", 3.22),
    (*b"KP", 2.30),
    (*b"KR", 2.67),
    (*b"KW", 2.30),
    (*b"KY", 3.22),
    (*b"KZ", 2.30),
    (*b"LA", 2.30),
    (*b"LB", 2.30),
    (*b"LC", 3.22),
    (*b"LI", 3.40),
    (*b"LK", 2.30),
    (*b"LR", 1.38),
    (*b"LS", 1.38),
    (*b"LT", 3.40),
    (*b"LU", 3.40),
    (*b"LV", 3.40),
    (*b"LY", 1.38),
    (*b"MA", 1.38),
    (*b"MC", 3.40),
    (*b"MD", 3.40),
    (*b"ME", 3.40),
    (*b"MF", 3.22),
    (*b"MG", 1.38),
    (*b"MH", 3.40),
    (*b"MK", 3.40),
    (*b"ML", 1.38),
    (*b"MM", 2.30),
    (*b"MN", 2.30),
    (*b"MO", 2.30),
    (*b"MP", 3.40),
    (*b"MR", 1.38),
    (*b"MS", 3.22),
    (*b"MT", 3.40),
    (*b"MU", 1.38),
    (*b"MV", 2.30),
    (*b"MW", 1.38),
    (*b"MX", 3.22),
    (*b"MY", 2.30),
    (*b"MZ", 1.38),
    (*b"NA", 1.38),
    (*b"NC", 3.40),
    (*b"NE", 1.38),
    (*b"NF", 3.40),
    (*b"NG", 1.38),
    (*b"NI", 3.22),
    (*b"NL", 3.40),
    (*b"NO", 3.40),
    (*b"NP", 2.30),
    (*b"NR", 3.40),
    (*b"NU", 3.40),
    (*b"NZ", 3.40),
    (*b"OM", 2.30),
    (*b"PA", 3.22),
    (*b"PE", 2.02),
    (*b"PF", 3.40),
    (*b"PG", 3.40),
    (*b"PH", 2.30),
    (*b"PK", 2.30),
    (*b"PL", 3.40),
    (*b"PM", 3.22),
    (*b"PN", 3.40),
    (*b"PR", 3.22),
    (*b"PS", 2.30),
    (*b"PT", 3.40),
    (*b"PW", 3.40),
    (*b"PY", 2.02),
    (*b"QA", 2.30),
    (*b"RO", 3.40),
    (*b"RS", 3.40),
    (*b"RU", 3.40),
    (*b"RW", 1.38),
    (*b"SA", 2.30),
    (*b"SB", 3.40),
    (*b"SC", 1.38),
    (*b"SD", 1.38),
    (*b"SE", 3.40),
    (*b"SG", 2.30),
    (*b"SH", 1.38),
    (*b"SI", 3.40),
    (*b"SK", 3.40),
    (*b"SL", 1.38),
    (*b"SM", 3.40),
    (*b"SN", 1.38),
    (*b"SO", 1.38),
    (*b"SR", 2.02),
    (*b"SS", 1.38),
    (*b"ST", 1.38),
    (*b"SV", 3.22),
    (*b"SX", 3.22),
    (*b"SY", 2.30),
    (*b"SZ", 1.38),
    (*b"TC", 3.22),
    (*b"TD", 1.38),
    (*b"TF", 1.38),
    (*b"TG", 1.38),
    (*b"TH", 2.30),
    (*b"TJ", 2.30),
    (*b"TL", 2.30),
    (*b"TM", 2.30),
    (*b"TN", 1.38),
    (*b"TO", 3.40),
    (*b"TR", 2.30),
    (*b"TT", 3.22),
    (*b"TV", 3.40),
    (*b"TW", 2.30),
    (*b"TZ", 1.38),
    (*b"UA", 3.40),
    (*b"UG", 1.38),
    (*b"UM", 3.22),
    (*b"US", 3.22),
    (*b"UY", 2.02),
    (*b"UZ", 2.30),
    (*b"VA", 3.40),
    (*b"VC", 3.22),
    (*b"VE", 2.02),
    (*b"VG", 3.22),
    (*b"VI", 3.22),
    (*b"VN", 2.30),
    (*b"VU", 3.40),
    (*b"WF", 3.40),
    (*b"WS", 3.40),
    (*b"YE", 2.30),
    (*b"ZA", 1.38),
    (*b"ZM", 1.38),
    (*b"ZW", 1.38),
];
/// Countries the table does not list.
const WORLD_TRIPS_PER_DWELLING: f64 = 3.68;

/// One road row as the tree reads it.
struct RoadRow {
    ends: [(i32, i32); 2],
    class: u8,
    length_m: f64,
    /// Neither a tunnel nor closed to motor vehicles.
    open: bool,
    country: u16,
}

impl RoadRow {
    fn local(&self) -> bool {
        LOCAL_CLASSES.contains(&self.class)
    }

    /// A local street of the trees: an open local row, counted or not (a count does not move
    /// what the buildings make, and the counted streets calibrate the trees).
    fn in_tree(&self) -> bool {
        self.open && self.local()
    }

    /// A row the trees drain into: an open main road.
    fn drains(&self) -> bool {
        self.open && self.class != TRACK_CLASS && !self.local()
    }
}

/// One building's daily trip ends before its country's trips per dwelling: dwellings, or trips.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BuildingLoad {
    at: (i32, i32),
    dwellings: f64,
    trips: f64,
}

/// A building of unknown use this small is a shed or a garage (m2).
const SHED_UP_TO_M2: f64 = 30.0;
/// A building of unknown use up to this footprint (m2) and storeys is a house: one or two
/// dwellings, not one per 80 m2 (dev4 counted a 200 m2 villa of three storeys as seven flats).
const HOUSE_UP_TO_M2: f64 = 250.0;
const HOUSE_UP_TO_STOREYS: u8 = 3;

/// dev4's trip generation (`trip-rates.ts`) by building type 0-13: dwellings by gross floor area
/// for homes, else trips per 100 m2 (gross floor area or footprint) between a floor and a cap
/// (ITE rates damped x0.3 outside US suburbs), fixed trips for small fixtures, none for sheds;
/// a building of unknown use is a shed, a house or flats by its size.
pub(crate) fn building_load(building_type: u8, storeys: u8, area_m2: Option<f64>) -> (f64, f64) {
    let footprint = area_m2.unwrap_or(100.0);
    let gfa = footprint * f64::from(storeys.max(1));
    let dwellings =
        |per_dwelling: f64, cap: f64| ((gfa / per_dwelling).floor().max(1.0).min(cap), 0.0);
    if building_type == 0 && area_m2.is_some_and(|area| area < SHED_UP_TO_M2) {
        return (0.0, 0.0);
    }
    if building_type == 0 && footprint <= HOUSE_UP_TO_M2 && storeys <= HOUSE_UP_TO_STOREYS {
        return dwellings(150.0, 2.0);
    }
    let trips = |size: f64, per_100: f64, floor: f64, cap: f64| {
        (0.0, (size / 100.0 * per_100).clamp(floor, cap))
    };
    match building_type {
        1 => trips(gfa, 12.0, 5.0, 3_000.0),
        2 => trips(gfa, 1.6, 4.0, 1_500.0),
        3 => trips(gfa, 0.46, 4.0, 368.0),
        4 => trips(gfa, 33.5, 20.0, 1_104.0),
        5 => (0.0, 7.36),
        6 => trips(gfa, 9.7, 8.0, 1_472.0),
        7 => (0.0, 3.68),
        8 => trips(gfa, 1.84, 2.0, 184.0),
        9 => trips(gfa, 1.23, 2.0, 368.0),
        10 => (0.0, 0.0),
        11 => dwellings(120.0, 4.0),
        12 => trips(footprint, 30.0, 20.0, 20_000.0),
        13 => trips(footprint, 30.0, 15.0, 2_000.0),
        _ => dwellings(80.0, 200.0),
    }
}

fn trips_per_dwelling(country_iso: u16) -> f64 {
    let iso = country_iso.to_le_bytes();
    TRIPS_PER_DWELLING
        .binary_search_by(|(code, _)| code[..].cmp(&iso[..]))
        .map_or(WORLD_TRIPS_PER_DWELLING, |index| {
            TRIPS_PER_DWELLING[index].1
        })
}

/// A square's traffic file.
pub struct SquareTraffic {
    /// Per row of `roads.arrow`: the trip ends routed through it, NaN where the row is no local
    /// street of a tree.
    pub flows: Vec<f32>,
    /// Per row: a local street in a dead end of the network.
    pub dead_end: Vec<bool>,
    /// Trip ends per grid cell, rows from the north.
    pub grid: Vec<f32>,
}

impl SquareTraffic {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + 5 * self.flows.len() + 4 * self.grid.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(self.flows.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(GRID_SIDE as u32).to_le_bytes());
        for flow in &self.flows {
            bytes.extend_from_slice(&flow.to_le_bytes());
        }
        bytes.extend(self.dead_end.iter().map(|&dead| u8::from(dead)));
        for value in &self.grid {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 16 || &bytes[..8] != MAGIC {
            return Err("traffic: bad magic".into());
        }
        let count = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (rows, side) = (count(8), count(12));
        if side != GRID_SIDE || bytes.len() != 16 + 5 * rows + 4 * side * side {
            return Err("traffic: length does not match the counts".into());
        }
        let floats = |from: usize, n: usize| -> Vec<f32> {
            bytes[from..from + 4 * n]
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
                .collect()
        };
        Ok(SquareTraffic {
            flows: floats(16, rows),
            dead_end: bytes[16 + 4 * rows..16 + 5 * rows]
                .iter()
                .map(|&b| b != 0)
                .collect(),
            grid: floats(16 + 5 * rows, side * side),
        })
    }
}

pub fn path(out: &Path, square: Square) -> PathBuf {
    out.join("z9")
        .join(square.x.to_string())
        .join(format!("{}.traffic", square.y))
}

/// The traffic file of a square, `None` when the square has none (no roads).
pub fn read(out: &Path, square: Square) -> Result<Option<SquareTraffic>, String> {
    let path = path(out, square);
    match std::fs::read(&path) {
        Ok(bytes) => SquareTraffic::decode(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// The grid of a square's traffic file alone (the rows' flows are not read).
fn read_grid(out: &Path, square: Square) -> Result<Option<Vec<f32>>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = path(out, square);
    let context = |error: std::io::Error| format!("{}: {error}", path.display());
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(context(error)),
    };
    let mut header = [0u8; 16];
    file.read_exact(&mut header).map_err(context)?;
    if &header[..8] != MAGIC {
        return Err(format!("{}: bad magic", path.display()));
    }
    let rows = u32::from_le_bytes(header[8..12].try_into().unwrap()) as u64;
    file.seek(SeekFrom::Start(16 + 5 * rows)).map_err(context)?;
    let mut bytes = vec![0u8; 4 * GRID_SIDE * GRID_SIDE];
    file.read_exact(&mut bytes).map_err(context)?;
    Ok(Some(
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect(),
    ))
}

/// Builds the traffic files of `squares` (a square already written is kept, so a rerun
/// resumes); returns how many it wrote.
pub fn build(dev4: &Dev4, squares: &[Square], out: &Path) -> Result<usize, String> {
    let written = squares
        .par_iter()
        .map(|&square| -> Result<usize, String> {
            let target = path(out, square);
            if target.exists() {
                return Ok(0);
            }
            let Some(traffic) = square_traffic(dev4, square)? else {
                return Ok(0);
            };
            std::fs::create_dir_all(target.parent().expect("a square's directory"))
                .map_err(|error| error.to_string())?;
            let partial = target.with_extension("partial");
            std::fs::write(&partial, traffic.encode()).map_err(|error| error.to_string())?;
            std::fs::rename(&partial, &target).map_err(|error| error.to_string())?;
            Ok(1)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(written.iter().sum())
}

fn read_roads(dev4: &Dev4, square: Square) -> Result<Option<Vec<RoadRow>>, String> {
    let Some(table) = dev4.table(square, "roads.arrow")? else {
        return Ok(None);
    };
    require_stamp(&table, "grid", "z30")?;
    let mut rows = Vec::new();
    for batch in &table.batches {
        let get = |name: &str| {
            batch
                .column_by_name(name)
                .ok_or_else(|| format!("roads.arrow: no column {name}"))
        };
        let i32s = |name| get(name).map(|c| c.as_primitive::<Int32Type>());
        let u8s = |name| get(name).map(|c| c.as_primitive::<UInt8Type>());
        let (sx, sy, ex, ey) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        let (class, access) = (u8s("road_class")?, u8s("access")?);
        let length = get("length_m")?.as_primitive::<Float32Type>();
        let tunnel = get("tunnel")?.as_boolean();
        let country = get("country_iso")?.as_primitive::<UInt16Type>();
        for row in 0..batch.num_rows() {
            rows.push(RoadRow {
                ends: [
                    (sx.value(row), sy.value(row)),
                    (ex.value(row), ey.value(row)),
                ],
                class: class.value(row),
                length_m: f64::from(length.value(row)),
                open: !tunnel.value(row) && !CLOSED_ACCESS.contains(&access.value(row)),
                country: country.value(row),
            });
        }
    }
    Ok(Some(rows))
}

/// The OSM buildings of `structures.arrow` (kind 0 with an OSM id) at their emission centroid
/// (else their centroid), with dev4's demand storeys.
fn read_buildings(dev4: &Dev4, square: Square) -> Result<Vec<BuildingLoad>, String> {
    let Some(table) = dev4.table(square, "structures.arrow")? else {
        return Ok(Vec::new());
    };
    require_stamp(&table, "structures_contract", "structures_v5")?;
    let mut buildings = Vec::new();
    for batch in &table.batches {
        let bytes = |name: &str| column::<UInt8Array>(batch, name);
        let integers = |name: &str| column::<Int32Array>(batch, name);
        let (kinds, types, storeys, floors) = (
            bytes("kind")?,
            bytes("building_type")?,
            bytes("storeys")?,
            bytes("floors")?,
        );
        let centroid = [integers("centroid_gx")?, integers("centroid_gy")?];
        let emission = [
            integers("emission_centroid_gx")?,
            integers("emission_centroid_gy")?,
        ];
        let ids = column::<Int64Array>(batch, "osm_id")?;
        let areas = column::<Float32Array>(batch, "area_m2")?;
        for row in 0..batch.num_rows() {
            if kinds.value(row) != 0 || !ids.is_valid(row) {
                continue;
            }
            let [x, y] = if emission.iter().all(|values| values.is_valid(row)) {
                emission
            } else {
                centroid
            };
            let storey_count = cell(storeys, row)
                .filter(|&count| count > 0)
                .or_else(|| cell(floors, row))
                .unwrap_or(1);
            let area = cell(areas, row)
                .filter(|area| area.is_finite() && *area > 0.0)
                .map(f64::from);
            let (dwellings, trips) =
                building_load(cell(types, row).unwrap_or(0), storey_count, area);
            if dwellings > 0.0 || trips > 0.0 {
                buildings.push(BuildingLoad {
                    at: (x.value(row), y.value(row)),
                    dwellings,
                    trips,
                });
            }
        }
    }
    Ok(buildings)
}

/// The share of households whose cars park on the street at home, in houses (up to four
/// dwellings to a building) and in blocks of flats. Dwellings relying on street parking in the
/// English Housing Survey 2009 (Figure 3.7): rural 6 %, rural residential 15 %, village centre
/// 17 %, suburban residential 28 %, other urban centre 59 %, city centre 52 %; cars parked on the
/// street at home in MiD 2017 (Figure 47): villages and small towns 8-11 %, metropolises 49 %.
/// Houses take the rural residential areas' share, flats the urban centres'; visits to shops and
/// offices the flats'.
const STREET_PARKING_HOUSES: f64 = 0.15;
const STREET_PARKING_FLATS: f64 = 0.55;
const HOUSE_UP_TO_DWELLINGS: f64 = 4.0;

/// A building's weight among its cell's trip ends (dwellings, or visits in dwellings' worth) and
/// the share of them made by cars that park on the street.
fn street_parking_weight(building: &BuildingLoad) -> (f64, f64) {
    if building.dwellings > 0.0 {
        let share = if building.dwellings <= HOUSE_UP_TO_DWELLINGS {
            STREET_PARKING_HOUSES
        } else {
            STREET_PARKING_FLATS
        };
        (building.dwellings, share)
    } else {
        (
            building.trips / WORLD_TRIPS_PER_DWELLING,
            STREET_PARKING_FLATS,
        )
    }
}

/// Per grid cell of a square: the share of its buildings' trip ends made by cars that park on
/// the street, dwellings and visits weighed alike; the flats' share in a cell without buildings.
pub fn street_parking_shares(dev4: &Dev4, square: Square) -> Result<Vec<f32>, String> {
    let mut cells = vec![(0.0f64, 0.0f64); GRID_SIDE * GRID_SIDE];
    for building in read_buildings(dev4, square)? {
        let (row, column) = grid_cell(square, building.at);
        let (weight, share) = street_parking_weight(&building);
        let cell = &mut cells[row * GRID_SIDE + column];
        cell.0 += weight;
        cell.1 += weight * share;
    }
    Ok(cells
        .into_iter()
        .map(|(weight, street)| {
            if weight > 0.0 {
                (street / weight) as f32
            } else {
                STREET_PARKING_FLATS as f32
            }
        })
        .collect())
}

/// The cell of a square's grid (row from the north, column) a z30 point falls in, clamped to the
/// square.
pub fn grid_cell(square: Square, at: (i32, i32)) -> (usize, usize) {
    let (west, north) = (
        i64::from(square.x) * SQUARE_Z30,
        (i64::from(Z9_PER_AXIS) - i64::from(square.y)) * SQUARE_Z30,
    );
    let column = ((i64::from(at.0) - west) / CELL_Z30).clamp(0, GRID_SIDE as i64 - 1);
    let row = ((north - 1 - i64::from(at.1)) / CELL_Z30).clamp(0, GRID_SIDE as i64 - 1);
    (row as usize, column as usize)
}

/// Ground metres per z30 cell at a z30 northing (Web Mercator's scale).
fn metres_per_z30(gy: i32) -> f64 {
    let y_m = (i64::from(gy) - (1 << 29)) as f64 * Z30_QUANTUM_M;
    let latitude = 2.0 * (y_m / tiles::geo::WGS84_A_M).exp().atan() - std::f64::consts::FRAC_PI_2;
    Z30_QUANTUM_M * latitude.cos()
}

fn square_traffic(dev4: &Dev4, square: Square) -> Result<Option<SquareTraffic>, String> {
    let Some(roads) = read_roads(dev4, square)? else {
        return Ok(None);
    };
    let buildings = read_buildings(dev4, square)?;
    let context = |error: String| format!("traffic of {square:?}: {error}");
    // Local metres: the square's mean scale is exact to 1 % across it.
    let scale = metres_per_z30(roads.first().map_or(1 << 29, |row| row.ends[0].1));
    let metres = |(gx, gy): (i32, i32)| [f64::from(gx) * scale, f64::from(gy) * scale];
    let loads = assign(&roads, &buildings, scale, &metres);
    let (flows, dead_end) = trees(&roads, &loads).map_err(context)?;
    // The grid: every building's trip ends in its cell (rows from the north).
    let mut grid = vec![0f32; GRID_SIDE * GRID_SIDE];
    for building in &buildings {
        let country = loads.country_of(building);
        let trip_ends = building.dwellings * trips_per_dwelling(country) + building.trips;
        let (row, column) = grid_cell(square, building.at);
        grid[row * GRID_SIDE + column] += trip_ends as f32;
    }
    Ok(Some(SquareTraffic {
        flows,
        dead_end,
        grid,
    }))
}

/// The buildings' trip ends per road row (each building on its nearest open road within the
/// frontage, any class but a track), and each building's country (its road's).
struct Loads {
    per_row: Vec<f64>,
    country: HashMap<(i32, i32), u16>,
    fallback_country: u16,
}

impl Loads {
    fn country_of(&self, building: &BuildingLoad) -> u16 {
        self.country
            .get(&building.at)
            .copied()
            .unwrap_or(self.fallback_country)
    }
}

fn assign(
    roads: &[RoadRow],
    buildings: &[BuildingLoad],
    scale: f64,
    metres: &dyn Fn((i32, i32)) -> [f64; 2],
) -> Loads {
    let cell_z30 = (FRONTAGE_M / scale).ceil() as i64;
    let key = |(gx, gy): (i32, i32)| (i64::from(gx) / cell_z30, i64::from(gy) / cell_z30);
    let mut grid: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (index, row) in roads.iter().enumerate() {
        if !row.open || row.class == TRACK_CLASS {
            continue;
        }
        let (a, b) = (key(row.ends[0]), key(row.ends[1]));
        for x in a.0.min(b.0) - 1..=a.0.max(b.0) + 1 {
            for y in a.1.min(b.1) - 1..=a.1.max(b.1) + 1 {
                grid.entry((x, y)).or_default().push(index as u32);
            }
        }
    }
    let fallback_country = roads
        .iter()
        .map(|row| row.country)
        .find(|&c| c != 0)
        .unwrap_or(0);
    let mut loads = Loads {
        per_row: vec![0.0; roads.len()],
        country: HashMap::new(),
        fallback_country,
    };
    for building in buildings {
        let p = metres(building.at);
        let mut best: Option<(f64, usize)> = None;
        for &index in grid.get(&key(building.at)).into_iter().flatten() {
            let row = &roads[index as usize];
            let (a, b) = (metres(row.ends[0]), metres(row.ends[1]));
            let distance =
                distance_to_segment([p[0] - a[0], p[1] - a[1]], [b[0] - a[0], b[1] - a[1]]);
            if distance <= FRONTAGE_M
                && best.is_none_or(|(d, i)| (distance, index as usize) < (d, i))
            {
                best = Some((distance, index as usize));
            }
        }
        if let Some((_, index)) = best {
            let country = roads[index].country;
            loads.country.insert(building.at, country);
            loads.per_row[index] +=
                building.dwellings * trips_per_dwelling(country) + building.trips;
        }
    }
    loads
}

/// Distance from `p` to the segment from the origin to `d`.
fn distance_to_segment(p: [f64; 2], d: [f64; 2]) -> f64 {
    let length_sq = d[0] * d[0] + d[1] * d[1];
    let t = if length_sq > 1e-12 {
        ((p[0] * d[0] + p[1] * d[1]) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - t * d[0]).hypot(p[1] - t * d[1])
}

/// The service trees: per row the trip ends routed through it (NaN off the trees) and its
/// dead-end flag.
fn trees(roads: &[RoadRow], loads: &Loads) -> Result<(Vec<f32>, Vec<bool>), String> {
    let mut ids: HashMap<(i32, i32), u32> = HashMap::new();
    let mut node = |at: (i32, i32)| {
        let next = ids.len() as u32;
        *ids.entry(at).or_insert(next)
    };
    let ends: Vec<[u32; 2]> = roads
        .iter()
        .map(|row| [node(row.ends[0]), node(row.ends[1])])
        .collect();
    let nodes = ids.len();
    // Adjacency of the tree rows (CSR) and the nodes a tree drains into.
    let mut degree = vec![0u32; nodes + 1];
    let mut drain = vec![false; nodes];
    for (row, [a, b]) in roads.iter().zip(&ends) {
        if row.in_tree() {
            degree[*a as usize] += 1;
            degree[*b as usize] += 1;
        } else if row.drains() {
            drain[*a as usize] = true;
            drain[*b as usize] = true;
        }
    }
    let mut start = vec![0u32; nodes + 1];
    for n in 0..nodes {
        start[n + 1] = start[n] + degree[n];
    }
    let mut fill = start.clone();
    let mut incident = vec![0u32; start[nodes] as usize];
    for (index, (row, [a, b])) in roads.iter().zip(&ends).enumerate() {
        if row.in_tree() {
            for n in [*a, *b] {
                incident[fill[n as usize] as usize] = index as u32;
                fill[n as usize] += 1;
            }
        }
    }
    let incident_of =
        |n: u32| &incident[start[n as usize] as usize..start[n as usize + 1] as usize];
    let other = |index: u32, n: u32| {
        let [a, b] = ends[index as usize];
        if a == n { b } else { a }
    };
    // Multi-source Dijkstra from the drained nodes over the tree rows, by length; then each
    // component no drained node reached from its busiest junction (its trips go nowhere).
    let mut distance = vec![f64::INFINITY; nodes];
    let mut down = vec![u32::MAX; nodes];
    let mut heap = BinaryHeap::new();
    let mut search = |sources: &[u32], distance: &mut Vec<f64>, down: &mut Vec<u32>| {
        for &n in sources {
            distance[n as usize] = 0.0;
            heap.push(Reverse((Ordered(0.0), n)));
        }
        while let Some(Reverse((Ordered(d), u))) = heap.pop() {
            if d > distance[u as usize] {
                continue;
            }
            for &index in incident_of(u) {
                let v = other(index, u);
                let next = d + roads[index as usize].length_m.max(1.0);
                if next < distance[v as usize] {
                    distance[v as usize] = next;
                    down[v as usize] = index;
                    heap.push(Reverse((Ordered(next), v)));
                }
            }
        }
    };
    let drained: Vec<u32> = (0..nodes as u32)
        .filter(|&n| drain[n as usize] && !incident_of(n).is_empty())
        .collect();
    search(&drained, &mut distance, &mut down);
    for n in 0..nodes as u32 {
        if distance[n as usize].is_finite() || incident_of(n).is_empty() {
            continue;
        }
        // The unreached component of `n`: its busiest junction is its root.
        let mut component = vec![n];
        let mut seen = std::collections::HashSet::from([n]);
        let mut at = 0;
        while at < component.len() {
            let u = component[at];
            at += 1;
            for &index in incident_of(u) {
                let v = other(index, u);
                if seen.insert(v) {
                    component.push(v);
                }
            }
        }
        let root = *component
            .iter()
            .max_by_key(|&&u| (incident_of(u).len(), Reverse(u)))
            .expect("a component holds its first node");
        search(&[root], &mut distance, &mut down);
    }
    // Accumulate from the farthest node: each tree row carries its own buildings and what flows
    // into its far end.
    let mut flow: Vec<f64> = roads
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if row.in_tree() {
                loads.per_row[index]
            } else {
                0.0
            }
        })
        .collect();
    let mut order: Vec<u32> = (0..nodes as u32)
        .filter(|&n| distance[n as usize].is_finite() && !incident_of(n).is_empty())
        .collect();
    order.sort_by(|&a, &b| {
        distance[b as usize]
            .total_cmp(&distance[a as usize])
            .then(a.cmp(&b))
    });
    for &u in &order {
        let mut inflow = 0.0;
        for &index in incident_of(u) {
            let v = other(index, u);
            if distance[v as usize] > distance[u as usize] {
                inflow += flow[index as usize];
            }
        }
        let parent = down[u as usize];
        if parent != u32::MAX {
            flow[parent as usize] += inflow;
        }
    }
    let dead_end = dead_ends(roads, &ends, &drain, nodes);
    let flows = roads
        .iter()
        .zip(&flow)
        .map(|(row, &value)| {
            if row.in_tree() {
                value as f32
            } else {
                f32::NAN
            }
        })
        .collect();
    Ok((flows, dead_end))
}

/// A total order on finite distances for the heap.
#[derive(Clone, Copy, PartialEq)]
struct Ordered(f64);
impl Eq for Ordered {}
impl PartialOrd for Ordered {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Ordered {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// The tree rows in dead ends: every tree row but those in the 2-edge-connected part of the
/// local network that holds the main roads (all drained nodes joined into one).
fn dead_ends(roads: &[RoadRow], ends: &[[u32; 2]], drain: &[bool], nodes: usize) -> Vec<bool> {
    // Tarjan's bridges, iteratively, over the tree rows with every drained node merged into one
    // super node (index `nodes`).
    let merged = |n: u32| if drain[n as usize] { nodes as u32 } else { n };
    let total = nodes + 1;
    let mut neighbours: Vec<Vec<(u32, u32)>> = vec![Vec::new(); total];
    for (index, row) in roads.iter().enumerate() {
        if !row.in_tree() {
            continue;
        }
        let [a, b] = ends[index];
        let (a, b) = (merged(a), merged(b));
        if a == b {
            continue;
        }
        neighbours[a as usize].push((b, index as u32));
        neighbours[b as usize].push((a, index as u32));
    }
    let mut order = vec![u32::MAX; total];
    let mut low = vec![0u32; total];
    let mut bridge = vec![false; roads.len()];
    let mut counter = 0u32;
    for root in 0..total as u32 {
        if order[root as usize] != u32::MAX || neighbours[root as usize].is_empty() {
            continue;
        }
        // Stack of (node, the row it was entered by, next neighbour to look at).
        let mut stack: Vec<(u32, u32, usize)> = vec![(root, u32::MAX, 0)];
        order[root as usize] = counter;
        low[root as usize] = counter;
        counter += 1;
        while let Some(&mut (u, via, ref mut next)) = stack.last_mut() {
            if let Some(&(v, index)) = neighbours[u as usize].get(*next) {
                *next += 1;
                if index == via {
                    continue;
                }
                if order[v as usize] == u32::MAX {
                    order[v as usize] = counter;
                    low[v as usize] = counter;
                    counter += 1;
                    stack.push((v, index, 0));
                } else {
                    low[u as usize] = low[u as usize].min(order[v as usize]);
                }
            } else {
                stack.pop();
                if let Some(&(parent, _, _)) = stack.last() {
                    low[parent as usize] = low[parent as usize].min(low[u as usize]);
                    if low[u as usize] > order[parent as usize] {
                        bridge[via as usize] = true;
                    }
                }
            }
        }
    }
    // The core: what the super node reaches without crossing a bridge.
    let mut core = vec![false; total];
    let mut queue = vec![nodes as u32];
    core[nodes] = true;
    while let Some(u) = queue.pop() {
        for &(v, index) in &neighbours[u as usize] {
            if !bridge[index as usize] && !core[v as usize] {
                core[v as usize] = true;
                queue.push(v);
            }
        }
    }
    roads
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if !row.in_tree() {
                return false;
            }
            let [a, b] = ends[index];
            bridge[index] || !(core[merged(a) as usize] && core[merged(b) as usize])
        })
        .collect()
}

/// The trip ends of the grids of a square and its neighbours, summed around a point.
pub struct Surroundings {
    /// The trip ends of the 3 x 3 squares around `square` summed from their north-west corner:
    /// entry (r + 1, c + 1) holds every cell north and west of cell (r, c) inclusive, rows from
    /// the north (squares without a traffic file count none).
    sums: Vec<f64>,
    square: Square,
}

/// Cells per side of a square's neighbourhood.
const WINDOW: usize = 3 * GRID_SIDE;

impl Surroundings {
    /// The grids of `square`'s neighbourhood (squares without a traffic file count none).
    pub fn load(out: &Path, square: Square) -> Result<Self, String> {
        let mut grids = HashMap::new();
        for neighbour in square.with_neighbours() {
            if let Some(grid) = read_grid(out, neighbour)? {
                grids.insert(neighbour, grid);
            }
        }
        Ok(Self::from_grids(square, &grids))
    }

    fn from_grids(square: Square, grids: &HashMap<Square, Vec<f32>>) -> Self {
        let side = GRID_SIDE as i64;
        let world = i64::from(Z9_PER_AXIS);
        let mut cells = vec![0.0f64; WINDOW * WINDOW];
        for dy in -1i64..=1 {
            let y = i64::from(square.y) + dy;
            if !(0..world).contains(&y) {
                continue;
            }
            for dx in -1i64..=1 {
                let x = (i64::from(square.x) + dx).rem_euclid(world);
                let Some(grid) = grids.get(&Square {
                    x: x as u32,
                    y: y as u32,
                }) else {
                    continue;
                };
                let (row0, column0) = (((dy + 1) * side) as usize, ((dx + 1) * side) as usize);
                for r in 0..GRID_SIDE {
                    for c in 0..GRID_SIDE {
                        cells[(row0 + r) * WINDOW + column0 + c] =
                            f64::from(grid[r * GRID_SIDE + c]);
                    }
                }
            }
        }
        let stride = WINDOW + 1;
        let mut sums = vec![0.0f64; stride * stride];
        for r in 0..WINDOW {
            let mut row_sum = 0.0;
            for c in 0..WINDOW {
                row_sum += cells[r * WINDOW + c];
                sums[(r + 1) * stride + c + 1] = sums[r * stride + c + 1] + row_sum;
            }
        }
        Surroundings { sums, square }
    }

    /// The trip ends within `radius_m` (a square box) of the z30 point `at`, as far as the
    /// neighbourhood reaches.
    pub fn around(&self, at: (i32, i32), radius_m: f64) -> f64 {
        let reach = (radius_m / metres_per_z30(at.1) / CELL_Z30 as f64).round() as i64;
        let side = GRID_SIDE as i64;
        let world_columns = i64::from(Z9_PER_AXIS) * side;
        let column = i64::from(at.0) / CELL_Z30;
        let row_from_north = (i64::from(Z9_PER_AXIS) * SQUARE_Z30 - 1 - i64::from(at.1)) / CELL_Z30;
        // The neighbourhood's own coordinates: its north-west square's north-west cell is (0, 0).
        let column = (column - (i64::from(self.square.x) - 1) * side).rem_euclid(world_columns);
        let row = row_from_north - (i64::from(self.square.y) - 1) * side;
        let window = WINDOW as i64;
        let (r0, r1) = ((row - reach).max(0), (row + reach + 1).min(window));
        let (c0, c1) = ((column - reach).max(0), (column + reach + 1).min(window));
        if r0 >= r1 || c0 >= c1 {
            return 0.0;
        }
        let sum = |r: i64, c: i64| self.sums[r as usize * (WINDOW + 1) + c as usize];
        sum(r1, c1) - sum(r0, c1) - sum(r1, c0) + sum(r0, c0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(a: (i32, i32), b: (i32, i32), class: u8) -> RoadRow {
        RoadRow {
            ends: [a, b],
            class,
            length_m: 100.0,
            open: true,
            country: u16::from_le_bytes(*b"CZ"),
        }
    }

    fn loads(per_row: Vec<f64>) -> Loads {
        Loads {
            per_row,
            country: HashMap::new(),
            fallback_country: 0,
        }
    }

    #[test]
    fn a_cul_de_sac_carries_its_houses_and_its_access_street_all_behind_it() {
        // Main road 0-1; a street 1-2-3 off it with a branch 2-4.
        let roads = vec![
            row((0, 0), (1, 0), 3),
            row((1, 0), (2, 0), 5),
            row((2, 0), (3, 0), 5),
            row((2, 0), (2, 1), 5),
        ];
        let (flows, dead) = trees(&roads, &loads(vec![500.0, 10.0, 20.0, 30.0])).unwrap();
        assert!(flows[0].is_nan());
        assert_eq!(&flows[1..], &[60.0, 20.0, 30.0]);
        assert_eq!(dead, vec![false, true, true, true]);
    }

    #[test]
    fn a_loop_between_two_main_roads_is_no_dead_end() {
        // Two main roads, a street 1-2-3 joining them, a cul-de-sac 2-5.
        let roads = vec![
            row((0, 0), (1, 0), 2),
            row((3, 0), (4, 0), 2),
            row((1, 0), (2, 0), 5),
            row((2, 0), (3, 0), 5),
            row((2, 0), (2, 5), 5),
        ];
        let (flows, dead) = trees(&roads, &loads(vec![0.0, 0.0, 4.0, 4.0, 10.0])).unwrap();
        assert_eq!(dead, vec![false, false, false, false, true]);
        // Node 2 lies 100 m from both ends: its trips drain to one of them, never both.
        assert_eq!(flows[4], 10.0);
        assert!((flows[2] + flows[3] - 18.0).abs() < 1e-9);
    }

    #[test]
    fn homes_and_shops_make_dev4s_trips() {
        assert_eq!(building_load(11, 2, Some(150.0)), (2.0, 0.0));
        assert_eq!(building_load(0, 4, Some(400.0)), (20.0, 0.0));
        assert_eq!(building_load(0, 3, Some(200.0)), (2.0, 0.0), "a villa");
        assert_eq!(building_load(0, 1, Some(120.0)), (1.0, 0.0), "a house");
        assert_eq!(building_load(0, 1, Some(18.0)), (0.0, 0.0), "a garage");
        assert_eq!(building_load(1, 2, Some(500.0)), (0.0, 120.0));
        assert_eq!(building_load(10, 1, Some(20.0)), (0.0, 0.0));
        assert_eq!(trips_per_dwelling(u16::from_le_bytes(*b"AT")), 3.4);
    }

    /// The neighbourhood's box sums match adding the cells one by one, across the square edges
    /// and the antimeridian, and stop at the neighbourhood's edge.
    #[test]
    fn surroundings_sum_boxes_like_cell_by_cell() {
        let square = Square { x: 0, y: 200 };
        let mut grids = HashMap::new();
        for neighbour in square.with_neighbours() {
            let grid: Vec<f32> = (0..GRID_SIDE * GRID_SIDE)
                .map(|k| ((k * 7 + neighbour.x as usize * 13 + neighbour.y as usize) % 11) as f32)
                .collect();
            grids.insert(neighbour, grid);
        }
        let around = Surroundings::from_grids(square, &grids);
        let brute = |at: (i32, i32), radius_m: f64| {
            let reach = (radius_m / metres_per_z30(at.1) / CELL_Z30 as f64).round() as i64;
            let column = i64::from(at.0) / CELL_Z30;
            let row = (i64::from(Z9_PER_AXIS) * SQUARE_Z30 - 1 - i64::from(at.1)) / CELL_Z30;
            let side = GRID_SIDE as i64;
            let world_columns = i64::from(Z9_PER_AXIS) * side;
            let mut sum = 0.0;
            for r in row - reach..=row + reach {
                for c in column - reach..=column + reach {
                    let c = c.rem_euclid(world_columns);
                    let cell_square = Square {
                        x: (c / side) as u32,
                        y: (r / side) as u32,
                    };
                    if let Some(grid) = grids.get(&cell_square) {
                        sum += f64::from(grid[(r % side * side + c % side) as usize]);
                    }
                }
            }
            sum
        };
        let north = (i64::from(Z9_PER_AXIS) - 200) * SQUARE_Z30;
        for (dx, dy, radius) in [
            (3, 5, 1_000.0),
            (250, 3, 5_000.0),
            (2, 250, 15_000.0),
            (128, 128, 50_000.0),
        ] {
            let at = (
                (dx * CELL_Z30) as i32 + 1,
                (north - dy * CELL_Z30 - 1) as i32,
            );
            let (fast, slow) = (around.around(at, radius), brute(at, radius));
            assert!(
                (fast - slow).abs() < 1e-6,
                "{dx} {dy} {radius}: {fast} vs {slow}"
            );
        }
    }

    /// A villa's and a terraced row's cars park mostly on their plots (15 % on the street), a
    /// block of flats' cars and a shop's visitors mostly on the street (55 %).
    #[test]
    fn houses_park_on_their_plots_and_flats_on_the_street() {
        let load = |(dwellings, trips)| BuildingLoad {
            at: (0, 0),
            dwellings,
            trips,
        };
        let villa = street_parking_weight(&load(building_load(0, 3, Some(200.0))));
        assert_eq!(villa, (2.0, STREET_PARKING_HOUSES));
        let terrace = street_parking_weight(&load(building_load(11, 2, Some(240.0))));
        assert_eq!(terrace.1, STREET_PARKING_HOUSES);
        let flats = street_parking_weight(&load(building_load(0, 5, Some(600.0))));
        assert_eq!(flats, (37.0, STREET_PARKING_FLATS));
        let shop = street_parking_weight(&load(building_load(1, 2, Some(500.0))));
        assert!((shop.0 - 120.0 / WORLD_TRIPS_PER_DWELLING).abs() < 1e-9);
        assert_eq!(shop.1, STREET_PARKING_FLATS);
    }

    #[test]
    fn traffic_files_round_trip() {
        let traffic = SquareTraffic {
            flows: vec![1.5, f32::NAN, 7.0],
            dead_end: vec![true, false, false],
            grid: (0..GRID_SIDE * GRID_SIDE).map(|i| i as f32).collect(),
        };
        let back = SquareTraffic::decode(&traffic.encode()).unwrap();
        assert_eq!(back.flows[0], 1.5);
        assert!(back.flows[1].is_nan());
        assert_eq!(back.dead_end, traffic.dead_end);
        assert_eq!(back.grid, traffic.grid);
    }
}

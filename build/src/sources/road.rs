//! Road pieces of the dev4 prepared tree as sources: the speed cascade, CNOSSOS-EU emission per
//! period computed here once, and the display record of the road group.

use super::bus::BusRoutes;
use super::decks::Decks;
use super::motorcycles::{LocalMotorcycles, country_share};
use super::period_shares::period_shares;
use super::road_counts::held_counts;
use super::road_junctions::{Junctions, row_junctions, traffic_signals};
use super::road_slope::{SquareHeights, WayRow, chains, row_slopes};
use super::road_traffic::{BuildingTraffic, Guess, local_km, thai_rural_road_ref};
use super::tagged_speeds::tagged_speed_kmh;
use super::{Converted, Reach, fields, group_key, metres, split_at_tile_edges};
use crate::climate::Temperature;
use crate::dev4::{Dev4, Square, require_stamp, z30_corner_degrees, z30_to_global};
use crate::roads::signs::{DERESTRICTED_CODE, fastest_legal_kmh, signed_speed};
use crate::source_tables::SourceTables;
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int16Type, Int32Type, Int64Type, UInt8Type, UInt16Type};
use arrow_array::{Array, RecordBatch};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::emission::road::{CategoryFlow, Junction, VehicleCategory, line_emission_db};
use physics::emission::road_surface::{
    BRUSHED_CONCRETE, HARD_ELEMENTS, HARD_ELEMENTS_HERRINGBONE, REFERENCE_SURFACE, RoadSurface,
    car_effect_db,
};
use tiles::sources::{
    Attribute, BUILDING_TRAFFIC_SOURCE_ID, GUESSED_SPLIT_SOURCES, Layer, PRIOR_SOURCE_ID,
    SERVICE_TREE_SOURCE_ID,
};

/// Source height above the carriageway (CNOSSOS-EU 2.4.1).
const ROAD_SOURCE_HEIGHT_M: f64 = 0.05;
/// Platform half-width per lane and shoulder (lanes x 3.5 m / 2 + 1.5 m), two lanes if untagged.
const PLATFORM_HALF_LANE_M: f64 = 1.75;
const PLATFORM_SHOULDER_M: f64 = 1.5;
pub(super) const DEFAULT_LANES: u8 = 2;
/// The speed of a derestricted road (`maxspeed=none`).
pub(crate) const DERESTRICTED_SPEED_KMH: f64 = 130.0;
/// dev4's `junction` code of a roundabout row.
const ROUNDABOUT_CODE: u8 = 1;
/// dev4 class codes 0-12.
const CLASS_NAMES: [&str; 13] = [
    "motorway",
    "trunk",
    "primary",
    "secondary",
    "tertiary",
    "residential",
    "living_street",
    "service",
    "track",
    "unclassified",
    "motorway_link",
    "trunk_link",
    "primary_link",
];
const CLASS_DEFAULT_SPEED_KMH: [f64; 13] = [
    100.0, 70.0, 50.0, 50.0, 50.0, 30.0, 20.0, 20.0, 20.0, 50.0, 60.0, 50.0, 50.0,
];
/// The extractor's surface codes: asphalt (or none), sett, cobblestone or paving stones, concrete,
/// gravel or compacted or unpaved.
const SURFACE_NAMES: [&str; 5] = ["asphalt", "sett", "paving_stones", "concrete", "unpaved"];
/// Their CNOSSOS-EU surfaces (Table F-4): setts are hard elements not in herring-bone (NL11), the
/// code that holds paving stones (most of its length) and cobblestones hard elements in
/// herring-bone (NL10), concrete brushed down concrete (NL06); unpaved roads, which the table has
/// not, keep dev4's flat 2 dB on rolling noise.
const SURFACES: [RoadSurface; 5] = [
    REFERENCE_SURFACE,
    HARD_ELEMENTS,
    HARD_ELEMENTS_HERRINGBONE,
    BRUSHED_CONCRETE,
    RoadSurface {
        alpha: [[2.0; BANDS]; 3],
        ..REFERENCE_SURFACE
    },
];
/// dev4's `traffic_estimated` bits of a row whose four categories all come from a prior.
const ALL_CATEGORIES_ESTIMATED: u8 = 15;
/// The medium and heavy shares (%) of local streets, as dev4's service tree splits them.
const LOCAL_SHARES: [f64; 2] = [1.0, 2.0];

/// The medium and heavy shares (fractions) of a class at a built-up code (1 rural, 2 urban,
/// unknown the mean of both).
fn counted_shares(tables: &SourceTables, class: usize, built_up: u8) -> [f64; 2] {
    let road = match class {
        10 => 0,
        11 => 1,
        12 => 2,
        main if main < 5 => main,
        _ => return LOCAL_SHARES.map(|percent| percent / 100.0),
    };
    let Some(&[rural, urban]) = tables.counted_class_shares.get(road) else {
        return LOCAL_SHARES.map(|percent| percent / 100.0);
    };
    std::array::from_fn(|k| {
        let percent = match built_up {
            1 => rural[k],
            2 => urban[k],
            _ => 0.5 * (rural[k] + urban[k]),
        };
        percent / 100.0
    })
}

/// The daily flows (light, medium, heavy, motorcycles) of a row whose four categories are all a
/// guess ([`GUESSED_SPLIT_SOURCES`]): a class prior of a secondary or tertiary row in a country
/// with representative counts takes that country's cell and its medium and heavy shares
/// (motorcycles scaled with the flow); any other keeps its flow and motorcycles and takes the
/// counted medium and heavy shares of its class. Every other row keeps `prior`.
fn country_flows(
    tables: &SourceTables,
    prior: [f64; 4],
    class: usize,
    built_up: u8,
    country_iso: u16,
    source_id: u16,
    estimated: u8,
) -> [f64; 4] {
    if estimated != ALL_CATEGORIES_ESTIMATED || !GUESSED_SPLIT_SOURCES.contains(&source_id) {
        return prior;
    }
    let split = |total: f64, motorcycles: f64, [medium, heavy]: [f64; 2]| {
        let (medium, heavy) = (total * medium, total * heavy);
        [
            (total - medium - heavy - motorcycles).max(0.0),
            medium,
            heavy,
            motorcycles,
        ]
    };
    let total = prior.iter().sum::<f64>();
    let row = class.checked_sub(3).filter(|row| *row < 2);
    let iso = country_iso.to_le_bytes();
    let country = (tables.class_priors.get(iso), tables.class_shares.get(iso));
    let (Some(row), PRIOR_SOURCE_ID, (Some(cells), Some(shares))) = (row, source_id, country)
    else {
        return split(total, prior[3], counted_shares(tables, class, built_up));
    };
    let cell = usize::from(built_up.min(2));
    let scale = cells[row][cell] / tables.road_preparation_priors[row][cell];
    // Unknown built-up takes the mean of the rural and urban shares.
    let share = |k: usize| {
        let [rural, urban] = [shares[row][0][k], shares[row][1][k]];
        let percent = match built_up {
            1 => rural,
            2 => urban,
            _ => 0.5 * (rural + urban),
        };
        percent / 100.0
    };
    split(total * scale, prior[3] * scale, [share(0), share(1)])
}

/// The daily flows (light, medium, heavy, motorcycles) of a modelled `total`: the counted
/// medium and heavy shares of the class; the motorcycles are set by [`with_motorcycles`].
fn modelled_flows(tables: &SourceTables, total: f64, class: usize, built_up: u8) -> [f64; 4] {
    let [medium, heavy] = counted_shares(tables, class, built_up).map(|share| share * total);
    [(total - medium - heavy).max(0.0), medium, heavy, 0.0]
}

/// Guessed flows with `share` of their vehicles motorcycles, taken from the light vehicles (the
/// trips the buildings make, and dev4's priors, count every motor vehicle).
fn with_motorcycles(flows: [f64; 4], share: f64) -> [f64; 4] {
    let total: f64 = flows.iter().sum();
    let others = flows[1] + flows[2];
    let motorcycles = (total * share).min((total - others).max(0.0));
    [
        (total - others - motorcycles).max(0.0),
        flows[1],
        flows[2],
        motorcycles,
    ]
}

/// The rows whose count is a main road's copy (`road_copies`): the counted rows, their ends, ways'
/// nodes and roads' cross-sections, and the rows in tunnels, at the latitude of the first.
pub(super) fn copied_counts(
    table: &crate::dev4::Table,
) -> Result<std::collections::HashSet<usize>, String> {
    let (mut counted, mut tunnels) = (Vec::new(), Vec::new());
    let mut first = 0;
    for batch in &table.batches {
        let c = Columns { batch };
        let i32s = |name| c.get(name).map(|a| a.as_primitive::<Int32Type>());
        let f64s = |name| c.get(name).map(|a| a.as_primitive::<Float64Type>());
        let (start_x, start_y, end_x, end_y) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        let class = c.get("road_class")?.as_primitive::<UInt8Type>();
        let oneway = c.get("oneway")?.as_primitive::<UInt8Type>();
        let estimated = c.get("traffic_estimated")?.as_primitive::<UInt8Type>();
        let source_id = c.get("source_id")?.as_primitive::<UInt16Type>();
        let names = c.get("name")?.as_string::<i32>();
        let aadt = [
            f64s("aadt_light")?,
            f64s("aadt_medium")?,
            f64s("aadt_heavy")?,
            f64s("aadt_moto")?,
        ];
        let cross_section = f64s("cross_section_aadt")?;
        let tunnel = c.get("tunnel")?.as_boolean();
        let (first_node, last_node) = (
            c.get("way_start_node")?.as_primitive::<Int64Type>(),
            c.get("way_end_node")?.as_primitive::<Int64Type>(),
        );
        for row in 0..batch.num_rows() {
            let source = source_id.value(row);
            let vehicles: f64 = aadt.iter().map(|column| column.value(row)).sum();
            let ends = [
                (start_x.value(row), start_y.value(row)),
                (end_x.value(row), end_y.value(row)),
            ];
            let nodes = [first_node.value(row), last_node.value(row)];
            if tunnel.value(row) {
                tunnels.push(super::road_copies::TunnelRow {
                    class: class.value(row),
                    ends,
                    nodes,
                });
            }
            if source == PRIOR_SOURCE_ID || guessed(source, estimated.value(row)) || vehicles <= 0.0
            {
                continue;
            }
            counted.push(super::road_copies::CountedRow {
                row: first + row,
                class: class.value(row),
                oneway: oneway.value(row) != 0,
                name: names.value(row).to_string(),
                ends,
                nodes,
                vehicles,
                cross_section: if cross_section.value(row) > 0.0 {
                    cross_section.value(row)
                } else {
                    vehicles
                },
            });
        }
        first += batch.num_rows();
    }
    let latitude = counted.first().map_or(0.0, |row| {
        z30_corner_degrees(row.ends[0].0, row.ends[0].1).0
    });
    Ok(super::road_copies::copied_counts(
        &counted, &tunnels, latitude,
    ))
}

/// The square's counted rows that count motorcycles, by class group, but those whose count is a
/// main road's copy (`copies`, rows of the table).
fn local_motorcycles(
    table: &crate::dev4::Table,
    copies: &std::collections::HashSet<usize>,
) -> Result<LocalMotorcycles, String> {
    let mut local = LocalMotorcycles::default();
    let mut batch_first_row = 0;
    for batch in &table.batches {
        let c = Columns { batch };
        let class = c.get("road_class")?.as_primitive::<UInt8Type>();
        let estimated = c.get("traffic_estimated")?.as_primitive::<UInt8Type>();
        let source_id = c.get("source_id")?.as_primitive::<UInt16Type>();
        let observation_estimated = batch
            .column_by_name("traffic_observation_estimated")
            .map(|a| a.as_primitive::<UInt8Type>());
        let aadt = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
            .map(|name| c.get(name).map(|a| a.as_primitive::<Float64Type>()));
        let aadt = [
            aadt[0].clone()?,
            aadt[1].clone()?,
            aadt[2].clone()?,
            aadt[3].clone()?,
        ];
        let observation_id = batch
            .column_by_name("traffic_observation_id")
            .map(|a| a.as_string::<i32>());
        let measured = batch
            .column_by_name("traffic_observation_measured")
            .map(|a| a.as_boolean());
        for row in 0..batch.num_rows() {
            let counted = observation_estimated.map_or_else(
                || super::motorcycles::counted(source_id.value(row), estimated.value(row)),
                |status| status.value(row) & 9 == 0,
            );
            if counted && !copies.contains(&(batch_first_row + row)) {
                let class_index = usize::from(class.value(row)).min(CLASS_NAMES.len() - 1);
                let flows = std::array::from_fn(|k| aadt[k].value(row));
                if let Some(identities) = observation_id {
                    if source_id.value(row) != crate::roads::CONTINUITY_SOURCE
                        && measured.is_some_and(|c| c.value(row))
                    {
                        local.add_observation(
                            source_id.value(row),
                            identities.value(row),
                            class_index,
                            flows,
                        );
                    }
                } else {
                    local.add(class_index, flows);
                }
            }
        }
        batch_first_row += batch.num_rows();
    }
    Ok(local)
}

/// Whether dev4 guessed a row's traffic: all four categories estimated by a class prior, a
/// country default, a classification fallback or the service tree.
pub(super) fn guessed(source_id: u16, estimated: u8) -> bool {
    estimated == ALL_CATEGORIES_ESTIMATED
        && (GUESSED_SPLIT_SOURCES.contains(&source_id) || source_id == SERVICE_TREE_SOURCE_ID)
}

/// The battery-electric share (fraction) of a country's cars.
fn electric_share(tables: &SourceTables, country_iso: u16) -> f64 {
    let iso = country_iso.to_le_bytes();
    tables.electric_percent.get(iso).copied().unwrap_or(0.0) / 100.0
}

/// Global truck/bus limit where the admitted table has no country row (km/h).
const HEAVY_SPEED_CAP_KMH: f64 = 80.0;

/// The speed of medium and heavy vehicles (lorries and buses) on a road of `base_speed` in a
/// country: on a rural single carriageway selected by [`free_flows_below_limit`] they take DfT
/// SPE0102's HGV free-flow share of the 60 mph rural single-carriageway limit (47.93 / 60 = 0.799,
/// where HGVs over 7.5 t have a 50 mph limit, applied to medium and heavy vehicles including buses
/// as a proxy), still under their national cap.
fn heavy_speed(
    tables: &SourceTables,
    base_speed: f64,
    rural_free_flow: bool,
    country_iso: u16,
) -> f64 {
    let iso = country_iso.to_le_bytes();
    let cap = tables
        .heavy_speeds
        .get(iso)
        .copied()
        .unwrap_or(HEAVY_SPEED_CAP_KMH);
    let speed = if rural_free_flow {
        base_speed * tables.rural_free_flow[1]
    } else {
        base_speed
    };
    speed.min(cap)
}

/// A road's day, evening and night shares of its daily vehicles, all categories together (the
/// popup's passes per hour), to three decimals.
/// The day, evening and night shares of a category's `vehicles` a day of which `buses` are a row's
/// buses: they run on their timetables' shares, the rest (lorries) on `lorry_shares`.
fn with_buses(
    tables: &SourceTables,
    lorry_shares: [f64; PERIODS],
    vehicles: f64,
    buses: f64,
) -> [f64; PERIODS] {
    std::array::from_fn(|period| {
        ((vehicles - buses) * lorry_shares[period] + buses * tables.bus_model.period_shares[period])
            / vehicles.max(f64::MIN_POSITIVE)
    })
}

fn road_period_shares(daily: [f64; 4], shares: &[[f64; PERIODS]; 4]) -> [f64; PERIODS] {
    let total: f64 = daily.iter().sum();
    std::array::from_fn(|p| {
        let share = if total > 0.0 {
            (0..4).map(|c| daily[c] * shares[c][p]).sum::<f64>() / total
        } else {
            shares[0][p]
        };
        (share * 1_000.0).round() / 1_000.0
    })
}

/// A national rural limit: this fast or faster (km/h).
const RURAL_LIMIT_FROM_KMH: f64 = 80.0;

/// Whether a row's vehicles drive at the rural free-flow share: a two-way rural row other than a
/// motorway or motorway link at or above `max(80 km/h, national_kmh)`. Because an unknown national
/// rural limit is passed as `0`, rural roads at 80 km/h or more in countries without a listed
/// national rural limit (such as FR, GB and US in 2024) are also accepted; a lower posted limit
/// below the national rural limit is driven as posted.
fn free_flows_below_limit(
    class_index: usize,
    built_up: u8,
    oneway: bool,
    limit_kmh: f64,
    national_kmh: f64,
) -> bool {
    built_up == 1
        && !oneway
        && !matches!(class_index, 0 | 10)
        && limit_kmh >= RURAL_LIMIT_FROM_KMH.max(national_kmh)
}

/// The country's legal limit for main classes (km/h; urban or rural by the row's built-up flag, 0
/// for unknown density, other classes and countries without a table). A trunk in a built-up area
/// takes the urban limit like any street there; elsewhere the country's motorroad limit where it
/// has one.
fn legal_speed(tables: &SourceTables, class: usize, country_iso: u16, built_up: u8) -> u8 {
    let iso = country_iso.to_le_bytes();
    let legal = tables.legal_speeds.get(iso).copied();
    legal.map_or(0, |[urban, rural, motorway, motorroad]| match class {
        0 => motorway,
        1 if built_up == 2 => urban,
        1 if motorroad > 0 => motorroad,
        1 => rural,
        2 | 3 | 4 | 9 => match built_up {
            2 => urban,
            1 => rural,
            _ => 0,
        },
        _ => 0,
    })
}

/// The speed of an untagged road: the country's legal limit ([`legal_speed`]), else the class
/// default, or the country's median signed speed of the class where it is lower. A main class whose
/// limit the country's table lacks (the United States, Canada, Brazil's towns) takes the median
/// sign either way: half of the United States' rural primary rows are unsigned, driven at the
/// class's 50 km/h where the signed ones' median is 89 (55 mph).
pub(crate) fn default_speed(
    tables: &SourceTables,
    class: usize,
    country_iso: u16,
    built_up: u8,
) -> (f64, &'static str) {
    let value = legal_speed(tables, class, country_iso, built_up);
    let (speed, source) = if value > 0 {
        (f64::from(value), "country_legal_default")
    } else {
        (CLASS_DEFAULT_SPEED_KMH[class], "default_by_class")
    };
    match tagged_speed_kmh(tables, class, country_iso, built_up) {
        Some(tagged) if tagged < speed || (value == 0 && matches!(class, 2 | 3 | 4 | 9)) => {
            (tagged, "tagged_median")
        }
        _ => (speed, source),
    }
}

struct Columns<'a> {
    batch: &'a RecordBatch,
}

impl<'a> Columns<'a> {
    fn get(&self, name: &str) -> Result<&'a dyn Array, String> {
        self.batch
            .column_by_name(name)
            .map(|c| c.as_ref())
            .ok_or_else(|| format!("roads.arrow: no column {name}"))
    }
}

/// The rows of every way, whichever batches they are in, and the table's row count; with `reach`
/// only the ways with a row reaching into it (no other row is converted).
fn way_rows(
    table: &crate::dev4::Table,
    reach: Option<Reach>,
) -> Result<(std::collections::HashMap<i64, Vec<WayRow>>, usize), String> {
    let mut ways: std::collections::HashMap<i64, Vec<WayRow>> = std::collections::HashMap::new();
    let mut wanted = std::collections::HashSet::new();
    let mut first_row = 0;
    for batch in &table.batches {
        let c = Columns { batch };
        let i32s = |name| c.get(name).map(|a| a.as_primitive::<Int32Type>());
        let (start_x, start_y, end_x, end_y) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        let osm_id = c.get("osm_id")?.as_primitive::<Int64Type>();
        let segment_index = c.get("segment_idx")?.as_primitive::<Int16Type>();
        let (tunnel, bridge) = (c.get("tunnel")?.as_boolean(), c.get("bridge")?.as_boolean());
        for row in 0..batch.num_rows() {
            let (start, end) = (
                (start_x.value(row), start_y.value(row)),
                (end_x.value(row), end_y.value(row)),
            );
            if reach.is_none_or(|reach| {
                reach.touches(z30_to_global(start.0, start.1), z30_to_global(end.0, end.1))
            }) {
                wanted.insert(osm_id.value(row));
            }
            ways.entry(osm_id.value(row)).or_default().push(WayRow {
                row: first_row + row,
                segment_index: segment_index.value(row),
                start: z30_corner_degrees(start.0, start.1),
                end: z30_corner_degrees(end.0, end.1),
                off_ground: bridge.value(row) || tunnel.value(row),
            });
        }
        first_row += batch.num_rows();
    }
    ways.retain(|way, _| wanted.contains(way));
    Ok((ways, first_row))
}

/// Converts the road rows of one dev4 square, or with `reach` only those reaching into another
/// square, the guessed traffic replaced by the building traffic under `traffic` when given and
/// the buses of `bus` added to it, a row on a bridge on its deck from `decks`; returns whether
/// the square's buildings are mapped (its building traffic modelled), which bounds its parking.
pub fn convert(
    (dev4, temperature, traffic, bus, tables): (
        &Dev4,
        &Temperature,
        Option<&std::path::Path>,
        Option<&BusRoutes>,
        &SourceTables,
    ),
    square: Square,
    reach: Option<Reach>,
    decks: &Decks,
    out: &mut Vec<Converted>,
) -> Result<bool, String> {
    let Some(table) = dev4.table(square, "roads.arrow")? else {
        return Ok(false);
    };
    for (key, value) in [
        ("grid", "z30"),
        ("roads_contract", "country_baked_v1"),
        ("road_traffic_contract", "1"),
    ] {
        require_stamp(&table, key, value)?;
    }
    let heights = SquareHeights::load(dev4, square)?;
    // The square's traffic signals and the vertices of its roundabouts (CNOSSOS-EU 2.2.5).
    let mut stops: Vec<((f64, f64), Junction)> = traffic_signals(dev4, square)?
        .into_iter()
        .map(|place| (place, Junction::TrafficLights))
        .collect();
    for batch in &table.batches {
        let c = Columns { batch };
        let i32s = |name| c.get(name).map(|a| a.as_primitive::<Int32Type>());
        let junction = c.get("junction")?.as_primitive::<UInt8Type>();
        let ends = [
            (i32s("start_gx")?, i32s("start_gy")?),
            (i32s("end_gx")?, i32s("end_gy")?),
        ];
        for row in (0..batch.num_rows()).filter(|&row| junction.value(row) == ROUNDABOUT_CODE) {
            for (x, y) in ends {
                stops.push((
                    z30_corner_degrees(x.value(row), y.value(row)),
                    Junction::Roundabout,
                ));
            }
        }
    }
    let latitude = stops.first().map_or(0.0, |(place, _)| place.0);
    let junctions = Junctions::new(latitude, stops);
    let building_traffic = match traffic {
        Some(dir) => {
            let mut lengths = Vec::new();
            for batch in &table.batches {
                let c = Columns { batch };
                let class = c.get("road_class")?.as_primitive::<UInt8Type>();
                let length = c
                    .get("length_m")?
                    .as_primitive::<arrow_array::types::Float32Type>();
                lengths.extend(
                    (0..batch.num_rows())
                        .map(|row| (class.value(row), f64::from(length.value(row)))),
                );
            }
            BuildingTraffic::load(dir, square, local_km(lengths.into_iter()))?
        }
        None => None,
    };
    let copies = copied_counts(&table)?;
    let held = held_counts(
        &table,
        building_traffic
            .as_ref()
            .map(|traffic| (traffic, &tables.road_models)),
        &copies,
    )?;
    let replaced: std::collections::HashSet<usize> =
        copies.union(&held.implausible).copied().collect();
    let motorcycles = local_motorcycles(&table, &replaced)?;
    let (mut ways, rows) = way_rows(&table, reach)?;
    let chains = chains(&mut ways);
    let slopes = row_slopes(&chains, &heights, rows);
    let row_stops = row_junctions(&chains, &junctions, rows);
    let fastest_legal = fastest_legal_kmh(&tables.legal_speeds);
    let mut first_row = 0;
    for batch in &table.batches {
        let batch_first_row = first_row;
        first_row += batch.num_rows();
        let c = Columns { batch };
        let i32s = |name| c.get(name).map(|a| a.as_primitive::<Int32Type>());
        let u8s = |name| c.get(name).map(|a| a.as_primitive::<UInt8Type>());
        let (start_x, start_y, end_x, end_y) = (
            i32s("start_gx")?,
            i32s("start_gy")?,
            i32s("end_gx")?,
            i32s("end_gy")?,
        );
        let (class, speed_limit, surface) = (
            u8s("road_class")?,
            u8s("speed_limit")?,
            u8s("surface_type")?,
        );
        // The graded-transition speed exists only where dev4's taper step ran; elsewhere none.
        let speed_taper = u8s("speed_taper").ok();
        let (lanes, oneway, junction, built_up) = (
            u8s("lanes")?,
            u8s("oneway")?,
            u8s("junction")?,
            u8s("built_up")?,
        );
        let access = u8s("access")?;
        let (tunnel, bridge) = (c.get("tunnel")?.as_boolean(), c.get("bridge")?.as_boolean());
        let (names, refs) = (
            c.get("name")?.as_string::<i32>(),
            c.get("ref")?.as_string::<i32>(),
        );
        let osm_id = c.get("osm_id")?.as_primitive::<Int64Type>();
        let within: Vec<bool> = (0..batch.num_rows())
            .map(|row| {
                reach.is_none_or(|reach| {
                    reach.touches(
                        z30_to_global(start_x.value(row), start_y.value(row)),
                        z30_to_global(end_x.value(row), end_y.value(row)),
                    )
                })
            })
            .collect();
        let country = c.get("country_iso")?.as_primitive::<UInt16Type>();
        for row in 0..batch.num_rows() {
            if !within[row] {
                continue;
            }
            // A row on a bridge stands on its deck and climbs with it (CNOSSOS-EU 2.2.4), where
            // the bare earth's slope would read the valley under it.
            let row_ends = [start_x, start_y, end_x, end_y].map(|end| end.value(row));
            let deck_m = bridge
                .value(row)
                .then(|| decks.roads.get(&row_ends).copied())
                .flatten();
            let slope = match deck_m {
                Some([from, to]) => {
                    let length = metres(
                        z30_corner_degrees(row_ends[0], row_ends[1]),
                        z30_corner_degrees(row_ends[2], row_ends[3]),
                    );
                    if length > 1.0 {
                        100.0 * (to - from) / length
                    } else {
                        0.0
                    }
                }
                None => slopes[batch_first_row + row],
            };
            let class_index = usize::from(class.value(row)).min(CLASS_NAMES.len() - 1);
            // A counted row carries its run's count (`road_counts`); a row whose count or estimate is
            // not the road's takes its model, bounded by its lanes' capacity.
            let carried = held.of(batch_first_row + row);
            let prior = carried.flows;
            let copy = copies.contains(&(batch_first_row + row));
            let implausible = held.implausible.contains(&(batch_first_row + row));
            let row_guessed = guessed(carried.source, carried.estimated) || copy || implausible;
            // Which categories are estimates: all of a copy's, none of its count its own.
            let estimated_categories = if copy || implausible {
                ALL_CATEGORIES_ESTIMATED
            } else {
                carried.estimated
            };
            let guess = if copy {
                Guess::Copy
            } else if implausible {
                Guess::Implausible
            } else if carried.source == PRIOR_SOURCE_ID {
                Guess::ClassPrior
            } else {
                Guess::Fallback
            };
            // A row closed to motor vehicles carries no guessed traffic (way 977414128 had 317 a
            // day): only what was counted on it and its buses.
            let closed = row_guessed && crate::traffic::CLOSED_ACCESS.contains(&access.value(row));
            let middle_z30 = (
                ((i64::from(start_x.value(row)) + i64::from(end_x.value(row))) / 2) as i32,
                ((i64::from(start_y.value(row)) + i64::from(end_y.value(row))) / 2) as i32,
            );
            // A Thai national highway's rows (motorway to tertiary and unclassified; links and
            // streets carrying its number are not the highway, nor a way carrying its count beside
            // it) take the department's traffic of the highway in the row's province: its
            // vehicle-km there over these rows' length.
            let highway = (row_guessed && !closed && !copy && matches!(class_index, 0..=4 | 9))
                .then(|| {
                    let (lat, lon) = z30_corner_degrees(middle_z30.0, middle_z30.1);
                    tables.regional_roads.reference_count(
                        country.value(row).to_le_bytes(),
                        refs.value(row),
                        lat,
                        lon,
                    )
                })
                .flatten();
            let modelled = building_traffic
                .as_ref()
                .filter(|_| row_guessed && !closed)
                .and_then(|traffic| {
                    traffic.total(
                        &tables.road_models,
                        batch_first_row + row,
                        (
                            class.value(row),
                            built_up.value(row),
                            oneway.value(row) != 0,
                            bus.is_some_and(|bus| bus.serves(osm_id.value(row))),
                        ),
                        (middle_z30, country.value(row), guess),
                        (thai_rural_road_ref(refs.value(row)), lanes.value(row)),
                    )
                });
            let mut daily = match modelled {
                Some(total) => modelled_flows(tables, total, class_index, built_up.value(row)),
                // A copy has no count of its own: where no model gives it one it carries none, its
                // main road beside it carrying the count.
                None if copy => [0.0; 4],
                None if implausible => prior,
                None => country_flows(
                    tables,
                    prior,
                    class_index,
                    built_up.value(row),
                    country.value(row),
                    carried.source,
                    carried.observation_estimated,
                ),
            };
            // The department's mean is the traffic across both directions and its shares count
            // every category, motorcycles and buses among them; a one-way carriageway carries half.
            if let Some(&(_, _, mean, shares, _)) = highway {
                let total = mean * if oneway.value(row) != 0 { 0.5 } else { 1.0 };
                daily = shares.map(|share| share * total);
            }
            // A US road whose vehicle types nobody counted (HPMS's national layer counts the total
            // only) carries its state's mix on its road group in its area (VM-4).
            let regional_mix =
                (copy || implausible || carried.observation_estimated == ALL_CATEGORIES_ESTIMATED)
                    .then(|| {
                        let (lat, lon) = z30_corner_degrees(middle_z30.0, middle_z30.1);
                        tables.regional_roads.vehicle_shares(
                            country.value(row).to_le_bytes(),
                            lat,
                            lon,
                            class_index,
                            built_up.value(row),
                        )
                    })
                    .flatten();
            if let Some(shares) = regional_mix {
                let total: f64 = daily.iter().sum();
                daily = shares.map(|share| share * total);
            }
            if closed {
                daily = [0.0; 4];
            }
            // A copy's or a rejected row's replacement is bounded as a count is, and an estimate over
            // its lanes cut to them: no lane carries more than the busiest counted lanes (an HOV
            // lane's model read 31,905 on one lane).
            if copy || implausible || held.capped.contains(&(batch_first_row + row)) {
                let most = held.capacity(batch_first_row + row);
                let total: f64 = daily.iter().sum();
                if total > most {
                    daily = daily.map(|flow| flow * most / total);
                }
            }
            // Guessed traffic takes the motorcycles of the square's counted roads, else of the
            // country's fleet (dev4 put 2 % on Vietnam's and 15 % on Thailand's class priors).
            if row_guessed && highway.is_none() && regional_mix.is_none() {
                let share = motorcycles
                    .share(tables, class_index, country.value(row))
                    .unwrap_or_else(|| country_share(tables, country.value(row), class_index));
                daily = with_motorcycles(daily, share);
            }
            // Buses on an uncounted row: two-axle city buses medium, a third articulated heavy.
            let mut scheduled = [0.0; 2];
            if let Some(bus) = bus.filter(|_| row_guessed && highway.is_none()) {
                let model = &tables.bus_model;
                let service = building_traffic.as_ref().map_or(
                    model.service_by_built_up[usize::from(built_up.value(row).min(2))],
                    |traffic| traffic.bus_service(model, middle_z30),
                );
                let (buses, coaches) = bus.daily(&tables.bus_model, osm_id.value(row), service);
                scheduled = [buses * 2.0 / 3.0, buses / 3.0 + coaches];
                daily[1] += scheduled[0];
                daily[2] += scheduled[1];
            }
            let cross_section = held.cross_section(
                batch_first_row + row,
                daily.iter().sum(),
                copy || implausible,
            );
            if tunnel.value(row) || daily.iter().sum::<f64>() <= 0.0 {
                continue;
            }
            let start = z30_to_global(start_x.value(row), start_y.value(row));
            let end = z30_to_global(end_x.value(row), end_y.value(row));
            if start == end {
                continue;
            }
            let taper = speed_taper.map_or(0, |taper| taper.value(row));
            // A posted value no main road is signed at is a typo, no sign.
            let sign =
                signed_speed(class_index, speed_limit.value(row), fastest_legal).unwrap_or(0);
            let (base_speed, base_source, posted) = match sign {
                DERESTRICTED_CODE => (DERESTRICTED_SPEED_KMH, "derestricted", None),
                0 if taper > 0 => (f64::from(taper), "graded_transition", None),
                0 => {
                    let (speed, source) =
                        default_speed(tables, class_index, country.value(row), built_up.value(row));
                    (speed, source, None)
                }
                posted => (f64::from(posted), "osm_posted", Some(posted)),
            };
            // CNOSSOS-EU drives roundabouts at their legal speed and corrects the braking and
            // pulling away (2.2.5), where dev4 capped them at 30 km/h (-4.1 dB at 50). The law
            // wants the mean speed: on a rural single carriageway at a national limit free-flowing
            // cars drive below it.
            let rural_free_flow = free_flows_below_limit(
                class_index,
                built_up.value(row),
                oneway.value(row) != 0,
                base_speed,
                f64::from(legal_speed(tables, class_index, country.value(row), 1)),
            );
            let (speed, speed_source) = if rural_free_flow {
                (base_speed * tables.rural_free_flow[0], "rural_free_flow")
            } else {
                (base_speed, base_source)
            };
            let (a, b) = (
                z30_corner_degrees(start_x.value(row), start_y.value(row)),
                z30_corner_degrees(end_x.value(row), end_y.value(row)),
            );
            let middle = (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
            let stop = if junction.value(row) == ROUNDABOUT_CODE {
                Some((Junction::Roundabout, 0.0))
            } else {
                row_stops[batch_first_row + row]
            };
            // CNOSSOS-EU 2.2.2: rolling noise at the place's yearly mean air temperature.
            let air_temperature_c = temperature.at(middle.0, middle.1);
            let surface_index = usize::from(surface.value(row));
            let road_surface = SURFACES.get(surface_index).unwrap_or(&REFERENCE_SURFACE);
            // Each category's day, evening and night shares of its daily flow, by country and road
            // group: light vehicles and motorcycles, medium and heavy vehicles (lorries run more of
            // their day at night).
            let [light_shares, heavy_shares] = [false, true]
                .map(|heavy| period_shares(tables, country.value(row), class_index, heavy));
            let shares = [
                light_shares,
                with_buses(tables, heavy_shares, daily[1], scheduled[0]),
                with_buses(tables, heavy_shares, daily[2], scheduled[1]),
                light_shares,
            ];
            let categories = [
                VehicleCategory::Light,
                VehicleCategory::Medium,
                VehicleCategory::Heavy,
                VehicleCategory::Motorcycle,
            ];
            // 2.2.4: a two-way flow is half uphill, half downhill; a one-way flow runs along the
            // way (1 tagged, 3 roundabout, 4 motorway) or against it (2, oneway=-1).
            let directions: &[(f64, f64)] = match oneway.value(row) {
                0 => &[(0.5, 1.0), (0.5, -1.0)],
                2 => &[(1.0, -1.0)],
                _ => &[(1.0, 1.0)],
            };
            let electric = electric_share(tables, country.value(row));
            let heavy_speed = heavy_speed(tables, base_speed, rural_free_flow, country.value(row));
            let emission: [[f64; BANDS]; PERIODS] = std::array::from_fn(|period| {
                let flows: Vec<CategoryFlow> = directions
                    .iter()
                    .flat_map(|&(share, sign)| {
                        (0..4).map(move |c| CategoryFlow {
                            vehicles_per_hour: share * daily[c] * shares[c][period]
                                / PERIOD_HOURS[period],
                            speed_kmh: match categories[c] {
                                VehicleCategory::Light | VehicleCategory::Motorcycle => speed,
                                VehicleCategory::Medium | VehicleCategory::Heavy => heavy_speed,
                            },
                            category: categories[c],
                            slope_percent: sign * slope,
                            junction: stop,
                            electric_share: electric,
                        })
                    })
                    .collect();
                line_emission_db(&flows, (road_surface, air_temperature_c))
            });
            let lanes_used = if lanes.value(row) == 0 {
                DEFAULT_LANES
            } else {
                lanes.value(row)
            };
            let (name, reference) = (names.value(row), refs.value(row));
            let display = serde_json::json!([
                name,
                reference,
                CLASS_NAMES[class_index],
                (daily[0] * 10.0).round() / 10.0,
                (daily[1] * 10.0).round() / 10.0,
                (daily[2] * 10.0).round() / 10.0,
                (daily[3] * 10.0).round() / 10.0,
                estimated_categories,
                cross_section.round(),
                posted,
                speed,
                speed_source,
                SURFACE_NAMES
                    .get(surface_index)
                    .copied()
                    .unwrap_or("asphalt"),
                (car_effect_db(road_surface, speed) * 10.0).round() / 10.0,
                lanes.value(row),
                oneway.value(row) != 0,
                bridge.value(row),
                if let Some(highway) = highway {
                    highway.4
                } else if modelled.is_some() {
                    BUILDING_TRAFFIC_SOURCE_ID
                } else if copy || implausible {
                    // Its buses alone, or its estimate bounded: no dataset counted what it carries.
                    PRIOR_SOURCE_ID
                } else {
                    carried.source
                },
                road_period_shares(daily, &shares),
            ]);
            let key = if name.is_empty() && reference.is_empty() {
                group_key(&["road-way", &osm_id.value(row).to_string()])
            } else {
                group_key(&["road", reference, name, CLASS_NAMES[class_index]])
            };
            let attribute = Attribute {
                layer: Layer::Road,
                height_m: ROAD_SOURCE_HEIGHT_M,
                ground_percent: 0,
                platform_half_width_m: f64::from(lanes_used) * PLATFORM_HALF_LANE_M
                    + PLATFORM_SHOULDER_M,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: key,
                emission,
                display: fields(display),
            };
            for part in split_at_tile_edges(start, end) {
                out.push(Converted {
                    tile: part.tile,
                    ends: part.ends,
                    attribute: attribute.clone(),
                    deck_m: part.deck(deck_m),
                });
            }
        }
    }
    Ok(building_traffic.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rows_buses_keep_their_timetables_shares_beside_its_lorries() {
        let tables = SourceTables::test_fixture();
        let lorries = [0.793, 0.064, 0.143];
        let close = |a: [f64; PERIODS], b: [f64; PERIODS]| {
            (0..PERIODS).all(|p| (a[p] - b[p]).abs() < 1e-12)
        };
        assert!(close(with_buses(tables, lorries, 300.0, 0.0), lorries));
        assert!(close(
            with_buses(tables, lorries, 300.0, 300.0),
            tables.bus_model.period_shares
        ));
        let half: [f64; PERIODS] =
            std::array::from_fn(|p| 0.5 * (lorries[p] + tables.bus_model.period_shares[p]));
        assert!(close(with_buses(tables, lorries, 300.0, 150.0), half));
        assert_eq!(with_buses(tables, lorries, 0.0, 0.0), [0.0; PERIODS]);
    }

    /// A prior secondary or tertiary row in a country with representative counts takes its
    /// country's cell and split; counted rows, other classes, rows a dataset filled and other
    /// countries keep their flows.
    #[test]
    fn class_priors_of_secondary_and_tertiary_roads_take_their_countrys_cell() {
        let tables = SourceTables::test_fixture();
        let cz = u16::from_le_bytes(*b"CZ");
        let estimated = ALL_CATEGORIES_ESTIMATED;
        // dev4's urban tertiary prior: 2,562 a day, 3.2 % medium, 4.8 % heavy, 1 % motorcycles.
        let prior = [2_331.4, 82.0, 123.0, 25.6];
        let village = country_flows(tables, prior, 4, 2, cz, PRIOR_SOURCE_ID, estimated);
        assert!(
            (village.iter().sum::<f64>() - 962.0).abs() < 1e-6,
            "{village:?}"
        );
        assert!((village[1] - 962.0 * 0.0338).abs() < 1e-6);
        assert!((village[2] - 962.0 * 0.0192).abs() < 1e-6);
        assert!((village[3] - 25.6 * 962.0 / 2_562.0).abs() < 1e-6);
        let se = u16::from_le_bytes(*b"SE");
        let rural = country_flows(tables, prior, 3, 1, se, PRIOR_SOURCE_ID, estimated);
        assert!((rural.iter().sum::<f64>() - 2_562.0 * 844.0 / 2_061.0).abs() < 1e-6);
        assert_eq!(
            country_flows(tables, prior, 4, 2, cz, 20, 0),
            prior,
            "counted"
        );
        assert_eq!(
            country_flows(tables, prior, 4, 2, cz, 11, estimated),
            prior,
            "service tree"
        );
        assert_eq!(
            country_flows(tables, prior, 4, 2, cz, 1041, estimated),
            prior,
            "a dataset's split"
        );
    }

    #[test]
    fn a_historical_preparation_prior_is_rescaled_from_its_own_admitted_baseline() {
        let mut tables = SourceTables::fixture_owned();
        tables.road_preparation_priors[1] = [800.0; 3];
        let flows = country_flows(
            &tables,
            [720.0, 20.0, 40.0, 20.0],
            4,
            2,
            u16::from_le_bytes(*b"CZ"),
            PRIOR_SOURCE_ID,
            15,
        );
        assert_eq!(flows.iter().sum::<f64>().round(), 962.0);
    }

    /// A guessed split keeps the flow and the motorcycles and takes the counted medium and heavy
    /// shares: a world-prior urban primary falls from 15.2 % to 7.5 %, a Kazakh residential street
    /// from 37 % to the service tree's 3 %, a rural motorway link reads its motorway.
    #[test]
    fn guessed_splits_take_the_counted_shares_of_their_class() {
        let tables = SourceTables::test_fixture();
        let de = u16::from_le_bytes(*b"DE");
        let estimated = ALL_CATEGORIES_ESTIMATED;
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let primary = [8_486.0, 610.0, 910.0, 200.0];
        let urban = country_flows(tables, primary, 2, 2, de, PRIOR_SOURCE_ID, estimated);
        let total: f64 = primary.iter().sum();
        assert!(close(urban.iter().sum::<f64>(), total), "{urban:?}");
        assert!(close(urban[1], total * 0.012) && close(urban[2], total * 0.063));
        assert!(close(urban[3], 200.0));
        let kz = u16::from_le_bytes(*b"KZ");
        let street = [600.0, 40.0, 330.0, 30.0];
        let local = country_flows(tables, street, 5, 2, kz, 9398, estimated);
        assert!(close(local[1], 10.0) && close(local[2], 20.0), "{local:?}");
        let link = country_flows(tables, primary, 10, 1, de, PRIOR_SOURCE_ID, estimated);
        assert!(close(link[2], total * 0.102));
        let unknown = country_flows(tables, primary, 2, 0, de, PRIOR_SOURCE_ID, estimated);
        assert!(close(unknown[2], total * 0.071));
        let mopeds = country_flows(tables, [10.0, 0.0, 0.0, 990.0], 2, 2, de, 9873, estimated);
        assert!(
            close(mopeds[0], 0.0) && close(mopeds[3], 990.0),
            "{mopeds:?}"
        );
    }

    /// Heavy and medium vehicles (lorries and buses over 3.5 t) keep to 80 km/h unless their
    /// country lets them faster.
    #[test]
    fn heavy_vehicles_keep_their_countrys_limit() {
        let tables = SourceTables::test_fixture();
        let code = |iso: &[u8; 2]| u16::from_le_bytes(*iso);
        assert_eq!(heavy_speed(tables, 130.0, false, code(b"DE")), 80.0);
        assert_eq!(heavy_speed(tables, 113.0, false, code(b"US")), 105.0);
        assert_eq!(heavy_speed(tables, 60.0, false, code(b"US")), 60.0);
        assert_eq!(heavy_speed(tables, 130.0, false, code(b"FR")), 90.0);
    }

    #[test]
    fn countries_drive_their_battery_electric_shares() {
        let tables = SourceTables::test_fixture();
        let code = |iso: &[u8; 2]| u16::from_le_bytes(*iso);
        assert!((electric_share(tables, code(b"NO")) - 0.2764).abs() < 1e-12);
        assert!((electric_share(tables, code(b"DE")) - 0.0337).abs() < 1e-12);
        assert_eq!(electric_share(tables, code(b"CZ")), 0.0, "not listed");
    }

    #[test]
    fn untagged_speeds_follow_the_country_and_built_up_density() {
        let tables = SourceTables::test_fixture();
        let cz = u16::from_le_bytes(*b"CZ");
        assert_eq!(default_speed(tables, 0, cz, 0).0, 130.0);
        assert_eq!(default_speed(tables, 1, cz, 0).0, 110.0, "Czech motorroads");
        assert_eq!(
            default_speed(tables, 1, cz, 2),
            (50.0, "country_legal_default"),
            "a trunk through a town"
        );
        assert_eq!(default_speed(tables, 1, cz, 1).0, 110.0);
        assert_eq!(
            default_speed(tables, 3, cz, 2),
            (50.0, "country_legal_default")
        );
        assert_eq!(default_speed(tables, 3, cz, 1).0, 90.0);
        assert_eq!(
            default_speed(tables, 3, cz, 0),
            (50.0, "default_by_class"),
            "unknown density keeps the class default"
        );
        assert_eq!(
            default_speed(tables, 5, cz, 2),
            (30.0, "default_by_class"),
            "local streets keep the class default"
        );
        assert_eq!(default_speed(tables, 0, 0, 0), (100.0, "default_by_class"));
        // Without a legal limit a main class takes its signs: the United States' rural primary
        // roads 89 km/h (55 mph), Brazil's urban ones 60; its local streets keep the class default.
        let us = u16::from_le_bytes(*b"US");
        assert_eq!(default_speed(tables, 2, us, 1), (89.0, "tagged_median"));
        assert_eq!(default_speed(tables, 9, us, 1), (80.0, "tagged_median"));
        assert_eq!(default_speed(tables, 5, us, 2), (30.0, "default_by_class"));
        let br = u16::from_le_bytes(*b"BR");
        assert_eq!(default_speed(tables, 2, br, 2), (60.0, "tagged_median"));
        assert_eq!(default_speed(tables, 2, br, 1), (80.0, "tagged_median"));
    }

    /// A road's shares for the popup weigh each category's by its vehicles: a German Landesstrasse
    /// with one lorry in ten runs between its cars' night and its lorries'.
    #[test]
    fn a_roads_period_shares_weigh_its_categories_by_their_vehicles() {
        let tables = SourceTables::test_fixture();
        let de = u16::from_le_bytes(*b"DE");
        let [cars, lorries] = [false, true].map(|heavy| period_shares(tables, de, 3, heavy));
        let shares = [cars, lorries, lorries, cars];
        let mixed = road_period_shares([900.0, 0.0, 100.0, 0.0], &shares);
        assert!(mixed[2] > cars[2] && mixed[2] < lorries[2], "{mixed:?}");
        assert!((mixed.iter().sum::<f64>() - 1.0).abs() < 0.003);
        assert_eq!(
            road_period_shares([0.0; 4], &shares),
            cars.map(|s| (s * 1e3).round() / 1e3)
        );
    }

    /// A two-way rural road at its country's national limit runs at 0.845 of it (DfT's free-flow
    /// speeds); motorways, one-way carriageways, towns, a lower posted limit and slow lanes keep
    /// their limit.
    #[test]
    fn rural_roads_run_below_their_national_limit() {
        let tables = SourceTables::test_fixture();
        assert!(free_flows_below_limit(4, 1, false, 90.0, 90.0));
        assert!(
            !free_flows_below_limit(4, 1, false, 80.0, 100.0),
            "a road signed 80 where the open road allows 100"
        );
        assert!(!free_flows_below_limit(4, 2, false, 90.0, 90.0), "a town");
        // Spain's rural limit is 90 while its primaries are signed 80 at the median: a primary
        // signed 80 is driven at 80, as the national limit is the law's, not the median sign.
        let es = u16::from_le_bytes(*b"ES");
        assert_eq!(default_speed(tables, 2, es, 1), (80.0, "tagged_median"));
        assert!(!free_flows_below_limit(
            2,
            1,
            false,
            80.0,
            f64::from(legal_speed(tables, 2, es, 1))
        ));
        assert!(
            !free_flows_below_limit(0, 1, false, 130.0, 130.0),
            "a motorway"
        );
        assert!(
            !free_flows_below_limit(1, 1, true, 110.0, 110.0),
            "a dual carriageway"
        );
        assert!(
            !free_flows_below_limit(5, 1, false, 50.0, 30.0),
            "a village lane"
        );
        assert!((90.0 * tables.rural_free_flow[0] - 76.05).abs() < 1e-9);
        let fr = u16::from_le_bytes(*b"FR");
        // On an 80 km/h rural single carriageway lorries take their own 0.799 share (63.92 km/h,
        // below cars' 67.60 km/h), while on a 110 km/h motorway they keep their national cap.
        assert!((heavy_speed(tables, 80.0, true, fr) - 63.92).abs() < 1e-9);
        assert!((heavy_speed(tables, 110.0, false, fr) - 90.0).abs() < 1e-9);
        let de = u16::from_le_bytes(*b"DE");
        assert!((heavy_speed(tables, 100.0, true, de) - 79.9).abs() < 1e-9);
    }

    /// Production path through `road::convert`: a rural two-way single carriageway in France at
    /// 80 km/h emits its heavy-vehicle flow at 80 * 0.799 = 63.92 km/h (not 80 or cars' 67.60),
    /// while a one-way motorway at 110 km/h caps heavy vehicles at 90 km/h without the rural ratio.
    #[test]
    fn convert_applies_heavy_rural_free_flow_ratio_on_rural_single_carriageways() {
        use arrow_array::{
            ArrayRef, BooleanArray, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array,
            StringArray, UInt8Array, UInt16Array,
        };
        use arrow_schema::{Field, Schema};
        use std::collections::HashMap;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let square = Square { x: 256, y: 256 };
        let sq_dir = dir.path().join("prepared/z9/256/256");
        std::fs::create_dir_all(&sq_dir).unwrap();
        let center = 1 << 29;
        let fr = u16::from_le_bytes(*b"FR");
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("osm_id", Arc::new(Int64Array::from(vec![101, 102]))),
            ("segment_idx", Arc::new(Int16Array::from(vec![0, 0]))),
            (
                "start_gx",
                Arc::new(Int32Array::from(vec![center + 1_000, center + 1_000])),
            ),
            (
                "start_gy",
                Arc::new(Int32Array::from(vec![center + 1_000, center + 2_000])),
            ),
            (
                "end_gx",
                Arc::new(Int32Array::from(vec![center + 2_000, center + 2_000])),
            ),
            (
                "end_gy",
                Arc::new(Int32Array::from(vec![center + 1_000, center + 2_000])),
            ),
            ("road_class", Arc::new(UInt8Array::from(vec![3, 0]))),
            ("speed_limit", Arc::new(UInt8Array::from(vec![80, 110]))),
            ("surface_type", Arc::new(UInt8Array::from(vec![0, 0]))),
            ("lanes", Arc::new(UInt8Array::from(vec![2, 2]))),
            ("oneway", Arc::new(UInt8Array::from(vec![0, 1]))),
            ("junction", Arc::new(UInt8Array::from(vec![0, 0]))),
            ("built_up", Arc::new(UInt8Array::from(vec![1, 1]))),
            ("access", Arc::new(UInt8Array::from(vec![0, 0]))),
            ("tunnel", Arc::new(BooleanArray::from(vec![false, false]))),
            ("bridge", Arc::new(BooleanArray::from(vec![false, false]))),
            ("name", Arc::new(StringArray::from(vec!["D1", "A1"]))),
            ("ref", Arc::new(StringArray::from(vec!["D 1", "A 1"]))),
            ("country_iso", Arc::new(UInt16Array::from(vec![fr, fr]))),
            ("length_m", Arc::new(Float32Array::from(vec![100.0, 100.0]))),
            ("source_id", Arc::new(UInt16Array::from(vec![1041, 1041]))),
            ("traffic_estimated", Arc::new(UInt8Array::from(vec![0, 0]))),
            ("aadt_light", Arc::new(Float64Array::from(vec![0.0, 0.0]))),
            ("aadt_medium", Arc::new(Float64Array::from(vec![0.0, 0.0]))),
            (
                "aadt_heavy",
                Arc::new(Float64Array::from(vec![1_000.0, 1_000.0])),
            ),
            ("aadt_moto", Arc::new(Float64Array::from(vec![0.0, 0.0]))),
            (
                "cross_section_aadt",
                Arc::new(Float64Array::from(vec![1_000.0, 1_000.0])),
            ),
            ("way_start_node", Arc::new(Int64Array::from(vec![1, 3]))),
            ("way_end_node", Arc::new(Int64Array::from(vec![2, 4]))),
        ];
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, array)| Field::new(*name, array.data_type().clone(), false))
            .collect();
        let schema = Arc::new(Schema::new_with_metadata(
            fields,
            HashMap::from([
                ("grid".into(), "z30".into()),
                ("roads_contract".into(), "country_baked_v1".into()),
                ("road_traffic_contract".into(), "1".into()),
            ]),
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            columns.into_iter().map(|(_, a)| a).collect(),
        )
        .unwrap();
        let mut writer = arrow_ipc::writer::FileWriter::try_new(
            std::fs::File::create(sq_dir.join("roads.arrow")).unwrap(),
            &schema,
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();

        // Sea grid: Temperature::at returns REFERENCE_AIR_TEMPERATURE_C (20 C) everywhere.
        let temp_path = dir.path().join("bio1.f32");
        let sea: Vec<u8> = (-3.4e38f32).to_le_bytes().repeat(2_160 * 1_080);
        std::fs::write(&temp_path, sea).unwrap();
        let temperature = Temperature::load(&temp_path).unwrap();
        let dev4 = Dev4 {
            prepared: dir.path().join("prepared"),
            rasters: dir.path().join("rasters"),
        };
        let tables = SourceTables::test_fixture();
        let mut out = Vec::new();
        convert(
            (&dev4, &temperature, None, None, tables),
            square,
            None,
            &Decks::default(),
            &mut out,
        )
        .unwrap();
        assert_eq!(out.len(), 2);

        let expected_day = |class_index: usize, speed_kmh: f64| {
            let share = period_shares(tables, fr, class_index, true)[0];
            let vph = 1_000.0 * share / PERIOD_HOURS[0];
            line_emission_db(
                &[CategoryFlow {
                    vehicles_per_hour: vph,
                    speed_kmh,
                    category: VehicleCategory::Heavy,
                    slope_percent: 0.0,
                    junction: None,
                    electric_share: electric_share(tables, fr),
                }],
                (
                    &REFERENCE_SURFACE,
                    physics::emission::road::REFERENCE_AIR_TEMPERATURE_C,
                ),
            )
        };

        // Row 0 (rural 2-way secondary, 80 km/h): heavy speed = 80 * 0.799 = 63.92 km/h.
        let rural_at_hgv = expected_day(3, 80.0 * 0.799);
        let rural_at_unscaled = expected_day(3, 80.0);
        let rural_at_car = expected_day(3, 80.0 * 0.845);
        for (band, (&actual, &expected)) in out[0].attribute.emission[0]
            .iter()
            .zip(&rural_at_hgv)
            .enumerate()
        {
            assert!(
                (actual - expected).abs() < 1e-9,
                "band {band}: {actual} vs {expected}"
            );
        }
        assert!(
            (out[0].attribute.emission[0][4] - rural_at_unscaled[4]).abs() > 1.0,
            "rural heavy flow must not run at unscaled 80 km/h"
        );
        assert!(
            (out[0].attribute.emission[0][4] - rural_at_car[4]).abs() > 0.2,
            "rural heavy flow must use 0.799, not cars' 0.845"
        );

        // Row 1 (one-way motorway, 110 km/h): heavy speed = FR cap 90 km/h (no rural ratio).
        let motorway_at_cap = expected_day(0, 90.0);
        for (band, (&actual, &expected)) in out[1].attribute.emission[0]
            .iter()
            .zip(&motorway_at_cap)
            .enumerate()
        {
            assert!(
                (actual - expected).abs() < 1e-9,
                "band {band}: {actual} vs {expected}"
            );
        }
    }
}

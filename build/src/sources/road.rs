//! Road pieces of the dev4 prepared tree as sources: the speed cascade, CNOSSOS-EU emission per
//! period computed here once, and the display record of the road group.

use super::bus::BusRoutes;
use super::country_speeds::COUNTRY_SPEEDS;
use super::motorcycles::{LocalMotorcycles, country_share};
use super::period_shares::period_shares;
use super::road_junctions::{Junctions, traffic_signals};
use super::road_slope::{SquareHeights, WayRow, row_slopes};
use super::road_traffic::{
    BUS_SERVICE_BY_BUILT_UP, BuildingTraffic, local_km, thai_rural_road_ref,
};
use super::tagged_speeds::tagged_speed_kmh;
use super::{Converted, Reach, group_key, split_at_tile_edges};
use crate::climate::Temperature;
use crate::dev4::{Dev4, Square, require_stamp, z30_corner_degrees, z30_to_global};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int16Type, Int32Type, Int64Type, UInt8Type, UInt16Type};
use arrow_array::{Array, RecordBatch};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::emission::road::{CategoryFlow, Junction, VehicleCategory, line_emission_db};
use tiles::sources::{Attribute, Layer};

/// Source height above the carriageway (CNOSSOS-EU 2.4.1).
const ROAD_SOURCE_HEIGHT_M: f64 = 0.05;
/// Platform half-width per lane and shoulder (lanes x 3.5 m / 2 + 1.5 m), two lanes if untagged.
const PLATFORM_HALF_LANE_M: f64 = 1.75;
const PLATFORM_SHOULDER_M: f64 = 1.5;
const DEFAULT_LANES: u8 = 2;
/// dev4 `speed_limit` code of `maxspeed=none`, driven at 130 km/h.
pub(crate) const DERESTRICTED_CODE: u8 = 255;
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
/// Rolling-noise corrections by the extractor's surface code: asphalt, sett, cobblestone or
/// paving stones, concrete, unpaved.
const SURFACE_NAMES: [&str; 5] = ["asphalt", "sett", "paving_stones", "concrete", "unpaved"];
const SURFACE_CORRECTION_DB: [f64; 5] = [0.0, 4.0, 4.0, 1.0, 2.0];
/// dev4's fitted carriageway priors of secondary and tertiary roads (built-up unknown, rural,
/// urban; two-way sections, one-way cells scaled alike). They are medians of counted roads, and
/// where counting is selective the counted roads are the busy ones of their class: Swedish NVDB,
/// which counts every state road, reads them 5.7 dB high, and an uncounted Czech III-class road
/// through a village took 2,562 vehicles a day where its counted neighbour carries 1,252.
const DEV4_CLASS_PRIORS: [[f64; 3]; 2] = [[3_000.0, 2_061.0, 6_445.0], [1_506.0, 1_002.0, 2_562.0]];
/// The same cells from roads counted without selection, for the countries whose counts give them
/// (evidence 2026-09-30, traffic priors; unknown built-up is the geometric mean of rural and
/// urban): Sweden (NVDB counts every state road), Great Britain (the DfT counts A and B roads whole
/// and minor roads by a random sample), Czechia (the census counts II-class roads whole; its
/// III-class roads are the II-class medians times the Swedish and British tertiary-to-secondary
/// ratios, 0.24 rural and 0.37 urban). Countries differ fivefold on the same class, so no country
/// lends its cells to another: elsewhere dev4's priors stay until counts represent the country.
const COUNTRY_CLASS_PRIORS: [([u8; 2], [[f64; 3]; 2]); 3] = [
    (*b"CZ", [[2_126.0, 1_739.0, 2_600.0], [632.0, 415.0, 962.0]]),
    (
        *b"GB",
        [[3_794.0, 2_369.0, 6_076.0], [1_585.0, 753.0, 3_337.0]],
    ),
    (*b"SE", [[1_294.0, 844.0, 1_983.0], [273.0, 151.0, 494.0]]),
];
/// Their medium and heavy vehicle shares (% of the flow; rural, urban; median counted rows of the
/// same classes): the prior's world split read 4.0 % and 6.0 % on Czech secondary roads and 3.2 %
/// and 4.8 % on tertiary ones, where the census counts 3.4 % and 2.9-3.2 %, 3.4 % and 1.9-2.1 %
/// (+0.7 dB).
/// Per class (secondary, tertiary), per built-up (rural, urban): medium and heavy shares (%).
type ClassShares = [[[f64; 2]; 2]; 2];
const COUNTRY_CLASS_SHARES: [([u8; 2], ClassShares); 3] = [
    (
        *b"CZ",
        [[[3.48, 3.20], [3.41, 2.91]], [[3.42, 2.08], [3.38, 1.92]]],
    ),
    (
        *b"GB",
        [[[0.42, 2.19], [0.59, 1.39]], [[0.18, 1.98], [0.65, 0.73]]],
    ),
    (
        *b"SE",
        [[[1.32, 4.95], [1.17, 3.43]], [[3.85, 2.51], [3.85, 2.22]]],
    ),
];
/// dev4's source id of a class prior (no dataset).
const PRIOR_SOURCE_ID: u16 = 0;
/// dev4's service-tree heuristic of local streets (a background plus routed trips).
const SERVICE_TREE_SOURCE_ID: u16 = 11;
/// The source id this converter gives a row whose traffic the buildings model.
const BUILDING_TRAFFIC_SOURCE_ID: u16 = 30;
/// dev4's `traffic_estimated` bits of a row whose four categories all come from a prior.
const ALL_CATEGORIES_ESTIMATED: u8 = 15;
/// dev4's sources whose category split is a guess, not a count: the class priors (0), the
/// country-tuned CNOSSOS class defaults (Algeria, DR Congo, Ethiopia, Iran, Iraq, Kazakhstan,
/// Kenya, Morocco, Nigeria, Russia, Sudan, Turkey, Ukraine, Egypt, Tanzania, Uzbekistan: "no open
/// per-segment AADT") and the road-classification fallbacks (Japan, Argentina, Chile, Colombia,
/// Indonesia, Peru, Riyadh, Thailand). They put 9-15 % medium and heavy vehicles on urban main
/// roads and up to 40 % on every class, residential streets included.
const GUESSED_SPLIT_SOURCES: [u16; 25] = [
    PRIOR_SOURCE_ID,
    9012,
    9180,
    9231,
    9364,
    9368,
    9398,
    9404,
    9504,
    9566,
    9643,
    9729,
    9792,
    9804,
    9818,
    9834,
    9860,
    9865,
    9870,
    9871,
    9872,
    9873,
    9874,
    9875,
    9876,
];
/// Medium and heavy shares (%) of counted roads (motorway, trunk, primary, secondary, tertiary;
/// rural, urban): the medians over the counts of 16 countries (CZ, DE, GB, FR, IE, PL, ES, IT, NO,
/// SE, FI, DK, NL, US, CA, NZ; evidence 2026-10-01, road shares), links as their roads. Urban
/// primaries carry 7.5 % where the world prior put 15 %.
const COUNTED_CLASS_SHARES: [[[f64; 2]; 2]; 5] = [
    [[1.4, 10.2], [1.1, 8.7]],
    [[1.4, 9.5], [1.5, 6.8]],
    [[1.6, 7.9], [1.2, 6.3]],
    [[1.5, 6.4], [1.3, 5.5]],
    [[1.0, 5.0], [1.0, 4.4]],
];
/// The medium and heavy shares (%) of local streets, as dev4's service tree splits them.
const LOCAL_SHARES: [f64; 2] = [1.0, 2.0];

/// The medium and heavy shares (fractions) of a class at a built-up code (1 rural, 2 urban,
/// unknown the mean of both).
fn counted_shares(class: usize, built_up: u8) -> [f64; 2] {
    let road = match class {
        10 => 0,
        11 => 1,
        12 => 2,
        main if main < 5 => main,
        _ => return LOCAL_SHARES.map(|percent| percent / 100.0),
    };
    let [rural, urban] = COUNTED_CLASS_SHARES[road];
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
    let country = (
        COUNTRY_CLASS_PRIORS.iter().find(|(code, _)| *code == iso),
        COUNTRY_CLASS_SHARES.iter().find(|(code, _)| *code == iso),
    );
    let (Some(row), PRIOR_SOURCE_ID, (Some((_, cells)), Some((_, shares)))) =
        (row, source_id, country)
    else {
        return split(total, prior[3], counted_shares(class, built_up));
    };
    let cell = usize::from(built_up.min(2));
    let scale = cells[row][cell] / DEV4_CLASS_PRIORS[row][cell];
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
fn modelled_flows(total: f64, class: usize, built_up: u8) -> [f64; 4] {
    let [medium, heavy] = counted_shares(class, built_up).map(|share| share * total);
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

/// Local-class rows (residential, living street, service) whose count repeats the daily total of
/// a main-class counted row of the same name in the square: copies a conflation put on a
/// boulevard's side lanes and squares (Madrid's Castellana side lane carried the main
/// carriageway's 35,989 vehicles a day, the station by it read 5 dB too loud). They take the
/// traffic their buildings make, as uncounted rows do.
fn copied_counts(table: &crate::dev4::Table) -> Result<std::collections::HashSet<usize>, String> {
    let mut main: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    let mut locals = Vec::new();
    let mut first = 0;
    for batch in &table.batches {
        let c = Columns { batch };
        let class = c.get("road_class")?.as_primitive::<UInt8Type>();
        let estimated = c.get("traffic_estimated")?.as_primitive::<UInt8Type>();
        let source_id = c.get("source_id")?.as_primitive::<UInt16Type>();
        let names = c.get("name")?.as_string::<i32>();
        let aadt = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
            .map(|name| c.get(name).map(|a| a.as_primitive::<Float64Type>()));
        let aadt = [
            aadt[0].clone()?,
            aadt[1].clone()?,
            aadt[2].clone()?,
            aadt[3].clone()?,
        ];
        for row in 0..batch.num_rows() {
            let (source, name) = (source_id.value(row), names.value(row));
            if source == PRIOR_SOURCE_ID || guessed(source, estimated.value(row)) || name.is_empty()
            {
                continue;
            }
            let total = aadt
                .iter()
                .map(|column| column.value(row))
                .sum::<f64>()
                .round() as i64;
            match class.value(row) {
                5..=7 if total > 0 => locals.push((first + row, name.to_string(), total)),
                0..=4 | 10..=12 if total > 0 => {
                    main.entry(name.to_string()).or_default().push(total)
                }
                _ => {}
            }
        }
        first += batch.num_rows();
    }
    Ok(locals
        .into_iter()
        .filter(|(_, name, total)| main.get(name).is_some_and(|totals| totals.contains(total)))
        .map(|(row, _, _)| row)
        .collect())
}

/// The square's counted rows that count motorcycles, by class group.
fn local_motorcycles(table: &crate::dev4::Table) -> Result<LocalMotorcycles, String> {
    let mut local = LocalMotorcycles::default();
    for batch in &table.batches {
        let c = Columns { batch };
        let class = c.get("road_class")?.as_primitive::<UInt8Type>();
        let estimated = c.get("traffic_estimated")?.as_primitive::<UInt8Type>();
        let source_id = c.get("source_id")?.as_primitive::<UInt16Type>();
        let aadt = ["aadt_light", "aadt_medium", "aadt_heavy", "aadt_moto"]
            .map(|name| c.get(name).map(|a| a.as_primitive::<Float64Type>()));
        let aadt = [
            aadt[0].clone()?,
            aadt[1].clone()?,
            aadt[2].clone()?,
            aadt[3].clone()?,
        ];
        for row in 0..batch.num_rows() {
            if super::motorcycles::counted(source_id.value(row), estimated.value(row)) {
                let class_index = usize::from(class.value(row)).min(CLASS_NAMES.len() - 1);
                local.add(class_index, std::array::from_fn(|k| aadt[k].value(row)));
            }
        }
    }
    Ok(local)
}

/// Whether dev4 guessed a row's traffic: all four categories estimated by a class prior, a
/// country default, a classification fallback or the service tree.
fn guessed(source_id: u16, estimated: u8) -> bool {
    estimated == ALL_CATEGORIES_ESTIMATED
        && (GUESSED_SPLIT_SOURCES.contains(&source_id) || source_id == SERVICE_TREE_SOURCE_ID)
}

/// Battery-electric cars in each country's car fleet (% of the stock, IEA Global EV Data Explorer
/// 2025, 2024 stock: EV stock share times the battery-electric part of the EV stock; CC BY 4.0).
/// Countries the explorer does not list take none. Their tyres roll as any car's, their drive
/// adds no propulsion noise: Norway's 27.6 % takes 0.23 dB off a 50 km/h car flow.
const BATTERY_ELECTRIC_PERCENT: [([u8; 2], f64); 44] = [
    (*b"AE", 2.33),
    (*b"AT", 3.85),
    (*b"AU", 1.56),
    (*b"BE", 4.09),
    (*b"BR", 0.31),
    (*b"CA", 2.35),
    (*b"CH", 4.60),
    (*b"CL", 0.19),
    (*b"CN", 6.87),
    (*b"CO", 0.45),
    (*b"CR", 1.96),
    (*b"DE", 3.37),
    (*b"DK", 12.56),
    (*b"ES", 0.84),
    (*b"FI", 3.81),
    (*b"FR", 3.08),
    (*b"GB", 3.79),
    (*b"GR", 0.44),
    (*b"ID", 0.59),
    (*b"IL", 4.21),
    (*b"IN", 0.48),
    (*b"IS", 9.69),
    (*b"IT", 0.69),
    (*b"JO", 0.84),
    (*b"JP", 0.54),
    (*b"KH", 0.47),
    (*b"KR", 2.32),
    (*b"LA", 1.58),
    (*b"MX", 0.20),
    (*b"MY", 0.27),
    (*b"NL", 6.02),
    (*b"NO", 27.64),
    (*b"NZ", 2.28),
    (*b"PH", 0.14),
    (*b"PL", 0.67),
    (*b"PT", 2.27),
    (*b"SE", 7.52),
    (*b"SG", 4.48),
    (*b"TH", 0.85),
    (*b"TR", 1.29),
    (*b"US", 1.86),
    (*b"UY", 0.97),
    (*b"UZ", 0.17),
    (*b"VN", 4.39),
];

/// The battery-electric share (fraction) of a country's cars.
fn electric_share(country_iso: u16) -> f64 {
    let iso = country_iso.to_le_bytes();
    BATTERY_ELECTRIC_PERCENT
        .binary_search_by(|(code, _)| code[..].cmp(&iso[..]))
        .map_or(0.0, |index| BATTERY_ELECTRIC_PERCENT[index].1 / 100.0)
}

/// Heavy vehicles drive no faster than this (km/h): the 80 km/h most of Europe sets them, where
/// a country lets them faster its limit or their speed limiters' setting (Australia 100,
/// Belgium, Brazil, France, Ireland, Japan, New Zealand, Russia and Spain 90 on motorways, Canada
/// 105 by Ontario's and Quebec's limiters, China 100 on expressways, Great Britain 90 by the
/// limiters, the United States 105 for the 65 mph most trucks are governed to).
const HEAVY_SPEED_CAP_KMH: f64 = 80.0;
const COUNTRY_HEAVY_SPEED_CAP_KMH: [([u8; 2], f64); 13] = [
    (*b"AU", 100.0),
    (*b"BE", 90.0),
    (*b"BR", 90.0),
    (*b"CA", 105.0),
    (*b"CN", 100.0),
    (*b"ES", 90.0),
    (*b"FR", 90.0),
    (*b"GB", 90.0),
    (*b"IE", 90.0),
    (*b"JP", 90.0),
    (*b"NZ", 90.0),
    (*b"RU", 90.0),
    (*b"US", 105.0),
];

/// The speed of a category on a road of `speed_kmh` in a country.
fn category_speed(category: VehicleCategory, speed_kmh: f64, country_iso: u16) -> f64 {
    if category != VehicleCategory::Heavy {
        return speed_kmh;
    }
    let iso = country_iso.to_le_bytes();
    let cap = COUNTRY_HEAVY_SPEED_CAP_KMH
        .binary_search_by(|(code, _)| code[..].cmp(&iso[..]))
        .map_or(HEAVY_SPEED_CAP_KMH, |index| {
            COUNTRY_HEAVY_SPEED_CAP_KMH[index].1
        });
    speed_kmh.min(cap)
}

/// A road's day, evening and night shares of its daily vehicles, all categories together (the
/// popup's passes per hour), to three decimals.
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

/// Free-flowing cars on rural single carriageways at the national limit drive at 0.845 of it:
/// DfT's automatic counters across Great Britain, 2024 (SPE0102): 50.7 mph on 60 mph single
/// carriageways, where motorways run at 68.4 of 70 and 30 mph roads at 29.6. The counting sites
/// are straight, free-flowing sections, so bends and junctions only lower a road's mean further.
const RURAL_FREE_FLOW_OF_LIMIT: f64 = 0.845;
/// A national rural limit: this fast or faster (km/h).
const RURAL_LIMIT_FROM_KMH: f64 = 80.0;

/// Whether a row's cars drive below its limit by the rural free-flow share: a two-way rural row
/// other than a motorway at a national rural limit.
fn free_flows_below_limit(class_index: usize, built_up: u8, oneway: bool, limit_kmh: f64) -> bool {
    built_up == 1 && !oneway && !matches!(class_index, 0 | 10) && limit_kmh >= RURAL_LIMIT_FROM_KMH
}

/// The speed of an untagged road: the country's legal limit for main classes (urban or rural by
/// the row's built-up flag, unknown density keeps the class default), else the class default. A
/// trunk in a built-up area takes the urban limit like any street there; elsewhere the country's
/// motorroad limit where it has one.
pub(crate) fn default_speed(class: usize, country_iso: u16, built_up: u8) -> (f64, &'static str) {
    let iso = country_iso.to_le_bytes();
    let legal = COUNTRY_SPEEDS
        .binary_search_by(|(code, _)| code[..].cmp(&iso[..]))
        .ok()
        .map(|i| COUNTRY_SPEEDS[i].1);
    let value = legal.map_or(0, |[urban, rural, motorway, motorroad]| match class {
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
    });
    let (speed, source) = if value > 0 {
        (f64::from(value), "country_legal_default")
    } else {
        (CLASS_DEFAULT_SPEED_KMH[class], "default_by_class")
    };
    match tagged_speed_kmh(class, country_iso, built_up) {
        Some(tagged) if tagged < speed => (tagged, "tagged_median"),
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

/// Converts the road rows of one dev4 square, or with `reach` only those reaching into another
/// square, the guessed traffic replaced by the building traffic under `traffic` when given and
/// the buses of `bus` added to it; returns how many rows emit.
pub fn convert(
    (dev4, temperature, traffic, bus): (
        &Dev4,
        &Temperature,
        Option<&std::path::Path>,
        Option<&BusRoutes>,
    ),
    square: Square,
    reach: Option<Reach>,
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "roads.arrow")? else {
        return Ok(0);
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
    let motorcycles = local_motorcycles(&table)?;
    let copies = copied_counts(&table)?;
    let mut emitting = 0;
    let mut first_row = 0;
    for batch in &table.batches {
        let batch_first_row = first_row;
        first_row += batch.num_rows();
        let c = Columns { batch };
        let i32s = |name| c.get(name).map(|a| a.as_primitive::<Int32Type>());
        let u8s = |name| c.get(name).map(|a| a.as_primitive::<UInt8Type>());
        let f64s = |name| c.get(name).map(|a| a.as_primitive::<Float64Type>());
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
        let (lanes, oneway, junction, built_up, estimated) = (
            u8s("lanes")?,
            u8s("oneway")?,
            u8s("junction")?,
            u8s("built_up")?,
            u8s("traffic_estimated")?,
        );
        let aadt = [
            f64s("aadt_light")?,
            f64s("aadt_medium")?,
            f64s("aadt_heavy")?,
            f64s("aadt_moto")?,
        ];
        let cross_section = f64s("cross_section_aadt")?;
        let (tunnel, bridge) = (c.get("tunnel")?.as_boolean(), c.get("bridge")?.as_boolean());
        let (names, refs) = (
            c.get("name")?.as_string::<i32>(),
            c.get("ref")?.as_string::<i32>(),
        );
        let osm_id = c.get("osm_id")?.as_primitive::<Int64Type>();
        let segment_index = c.get("segment_idx")?.as_primitive::<Int16Type>();
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
        // A row's slope reads its whole way: the ways of the rows within.
        let wanted: std::collections::HashSet<i64> = (0..batch.num_rows())
            .filter(|&row| within[row])
            .map(|row| osm_id.value(row))
            .collect();
        let mut ways: std::collections::HashMap<i64, Vec<WayRow>> =
            std::collections::HashMap::new();
        for row in (0..batch.num_rows()).filter(|&row| wanted.contains(&osm_id.value(row))) {
            ways.entry(osm_id.value(row)).or_default().push(WayRow {
                row,
                segment_index: segment_index.value(row),
                start: z30_corner_degrees(start_x.value(row), start_y.value(row)),
                end: z30_corner_degrees(end_x.value(row), end_y.value(row)),
                bridge: bridge.value(row),
            });
        }
        let slopes = row_slopes(ways, &heights, batch.num_rows());
        let country = c.get("country_iso")?.as_primitive::<UInt16Type>();
        let source_id = c.get("source_id")?.as_primitive::<UInt16Type>();
        for (row, &slope) in slopes.iter().enumerate() {
            if !within[row] {
                continue;
            }
            let class_index = usize::from(class.value(row)).min(CLASS_NAMES.len() - 1);
            let prior: [f64; 4] = std::array::from_fn(|category| aadt[category].value(row));
            let row_guessed = guessed(source_id.value(row), estimated.value(row))
                || copies.contains(&(batch_first_row + row));
            let middle_z30 = (
                ((i64::from(start_x.value(row)) + i64::from(end_x.value(row))) / 2) as i32,
                ((i64::from(start_y.value(row)) + i64::from(end_y.value(row))) / 2) as i32,
            );
            let modelled = building_traffic
                .as_ref()
                .filter(|_| row_guessed)
                .and_then(|traffic| {
                    traffic.total(
                        batch_first_row + row,
                        (
                            class.value(row),
                            built_up.value(row),
                            oneway.value(row) != 0,
                        ),
                        (
                            middle_z30,
                            country.value(row),
                            source_id.value(row) == PRIOR_SOURCE_ID,
                        ),
                        thai_rural_road_ref(refs.value(row)),
                    )
                });
            let mut daily = match modelled {
                Some(total) => modelled_flows(total, class_index, built_up.value(row)),
                None => country_flows(
                    prior,
                    class_index,
                    built_up.value(row),
                    country.value(row),
                    source_id.value(row),
                    estimated.value(row),
                ),
            };
            // Guessed traffic takes the motorcycles of the square's counted roads, else of the
            // country's fleet (dev4 put 2 % on Vietnam's and 15 % on Thailand's class priors).
            if row_guessed {
                let share = motorcycles
                    .share(class_index, country.value(row))
                    .unwrap_or_else(|| country_share(country.value(row), class_index));
                daily = with_motorcycles(daily, share);
            }
            // Buses on an uncounted row: two-axle city buses medium, a third articulated heavy.
            if let Some(bus) = bus.filter(|_| row_guessed) {
                let service = building_traffic.as_ref().map_or(
                    BUS_SERVICE_BY_BUILT_UP[usize::from(built_up.value(row).min(2))],
                    |traffic| traffic.bus_service(middle_z30),
                );
                let (buses, coaches) = bus.daily(osm_id.value(row), service);
                daily[1] += buses * 2.0 / 3.0;
                daily[2] += buses / 3.0 + coaches;
            }
            let scale =
                daily.iter().sum::<f64>() / prior.iter().sum::<f64>().max(f64::MIN_POSITIVE);
            if tunnel.value(row) || daily.iter().sum::<f64>() <= 0.0 {
                continue;
            }
            let start = z30_to_global(start_x.value(row), start_y.value(row));
            let end = z30_to_global(end_x.value(row), end_y.value(row));
            if start == end {
                continue;
            }
            let taper = speed_taper.map_or(0, |taper| taper.value(row));
            let (base_speed, base_source, posted) = match speed_limit.value(row) {
                DERESTRICTED_CODE => (DERESTRICTED_SPEED_KMH, "derestricted", None),
                0 if taper > 0 => (f64::from(taper), "graded_transition", None),
                0 => {
                    let (speed, source) =
                        default_speed(class_index, country.value(row), built_up.value(row));
                    (speed, source, None)
                }
                posted => (f64::from(posted), "osm_posted", Some(posted)),
            };
            // CNOSSOS-EU drives roundabouts at their legal speed and corrects the braking and
            // pulling away (2.2.5), where dev4 capped them at 30 km/h (-4.1 dB at 50). The law
            // wants the mean speed: on a rural single carriageway at a national limit free-flowing
            // cars drive below it.
            let (speed, speed_source) = if free_flows_below_limit(
                class_index,
                built_up.value(row),
                oneway.value(row) != 0,
                base_speed,
            ) {
                (base_speed * RURAL_FREE_FLOW_OF_LIMIT, "rural_free_flow")
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
                junctions.nearest(middle)
            };
            // CNOSSOS-EU 2.2.2: rolling noise at the place's yearly mean air temperature.
            let air_temperature_c = temperature.at(middle.0, middle.1);
            let surface_index = usize::from(surface.value(row));
            let surface_correction = SURFACE_CORRECTION_DB
                .get(surface_index)
                .copied()
                .unwrap_or(0.0);
            // Each category's day, evening and night shares of its daily flow, by country and road
            // group: light vehicles and motorcycles, medium and heavy vehicles (lorries run more of
            // their day at night).
            let [light_shares, heavy_shares] =
                [false, true].map(|heavy| period_shares(country.value(row), class_index, heavy));
            let shares = [light_shares, heavy_shares, heavy_shares, light_shares];
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
            let electric = electric_share(country.value(row));
            let country_iso = country.value(row);
            let emission: [[f64; BANDS]; PERIODS] = std::array::from_fn(|period| {
                let flows: Vec<CategoryFlow> = directions
                    .iter()
                    .flat_map(|&(share, sign)| {
                        (0..4).map(move |c| CategoryFlow {
                            vehicles_per_hour: share * daily[c] * shares[c][period]
                                / PERIOD_HOURS[period],
                            speed_kmh: category_speed(categories[c], speed, country_iso),
                            category: categories[c],
                            slope_percent: sign * slope,
                            junction: stop,
                            electric_share: electric,
                        })
                    })
                    .collect();
                line_emission_db(&flows, (surface_correction, air_temperature_c))
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
                estimated.value(row),
                (scale * cross_section.value(row)).round(),
                posted,
                speed,
                speed_source,
                SURFACE_NAMES
                    .get(surface_index)
                    .copied()
                    .unwrap_or("asphalt"),
                surface_correction,
                lanes.value(row),
                oneway.value(row) != 0,
                bridge.value(row),
                if modelled.is_some() {
                    BUILDING_TRAFFIC_SOURCE_ID
                } else {
                    source_id.value(row)
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
                display: display.to_string(),
            };
            for (tile, ends) in split_at_tile_edges(start, end) {
                out.push(Converted {
                    tile,
                    ends,
                    attribute: attribute.clone(),
                });
            }
            emitting += 1;
        }
    }
    Ok(emitting)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prior secondary or tertiary row in a country with representative counts takes its
    /// country's cell and split; counted rows, other classes, rows a dataset filled and other
    /// countries keep their flows.
    #[test]
    fn class_priors_of_secondary_and_tertiary_roads_take_their_countrys_cell() {
        let cz = u16::from_le_bytes(*b"CZ");
        let estimated = ALL_CATEGORIES_ESTIMATED;
        // dev4's urban tertiary prior: 2,562 a day, 3.2 % medium, 4.8 % heavy, 1 % motorcycles.
        let prior = [2_331.4, 82.0, 123.0, 25.6];
        let village = country_flows(prior, 4, 2, cz, PRIOR_SOURCE_ID, estimated);
        assert!(
            (village.iter().sum::<f64>() - 962.0).abs() < 1e-6,
            "{village:?}"
        );
        assert!((village[1] - 962.0 * 0.0338).abs() < 1e-6);
        assert!((village[2] - 962.0 * 0.0192).abs() < 1e-6);
        assert!((village[3] - 25.6 * 962.0 / 2_562.0).abs() < 1e-6);
        let se = u16::from_le_bytes(*b"SE");
        let rural = country_flows(prior, 3, 1, se, PRIOR_SOURCE_ID, estimated);
        assert!((rural.iter().sum::<f64>() - 2_562.0 * 844.0 / 2_061.0).abs() < 1e-6);
        assert_eq!(country_flows(prior, 4, 2, cz, 20, 0), prior, "counted");
        assert_eq!(
            country_flows(prior, 4, 2, cz, 11, estimated),
            prior,
            "service tree"
        );
        assert_eq!(
            country_flows(prior, 4, 2, cz, 1041, estimated),
            prior,
            "a dataset's split"
        );
    }

    /// A guessed split keeps the flow and the motorcycles and takes the counted medium and heavy
    /// shares: a world-prior urban primary falls from 15.2 % to 7.5 %, a Kazakh residential street
    /// from 37 % to the service tree's 3 %, a rural motorway link reads its motorway.
    #[test]
    fn guessed_splits_take_the_counted_shares_of_their_class() {
        let de = u16::from_le_bytes(*b"DE");
        let estimated = ALL_CATEGORIES_ESTIMATED;
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let primary = [8_486.0, 610.0, 910.0, 200.0];
        let urban = country_flows(primary, 2, 2, de, PRIOR_SOURCE_ID, estimated);
        let total: f64 = primary.iter().sum();
        assert!(close(urban.iter().sum::<f64>(), total), "{urban:?}");
        assert!(close(urban[1], total * 0.012) && close(urban[2], total * 0.063));
        assert!(close(urban[3], 200.0));
        let kz = u16::from_le_bytes(*b"KZ");
        let street = [600.0, 40.0, 330.0, 30.0];
        let local = country_flows(street, 5, 2, kz, 9398, estimated);
        assert!(close(local[1], 10.0) && close(local[2], 20.0), "{local:?}");
        let link = country_flows(primary, 10, 1, de, PRIOR_SOURCE_ID, estimated);
        assert!(close(link[2], total * 0.102));
        let unknown = country_flows(primary, 2, 0, de, PRIOR_SOURCE_ID, estimated);
        assert!(close(unknown[2], total * 0.071));
        let mopeds = country_flows([10.0, 0.0, 0.0, 990.0], 2, 2, de, 9873, estimated);
        assert!(
            close(mopeds[0], 0.0) && close(mopeds[3], 990.0),
            "{mopeds:?}"
        );
    }

    /// Heavy vehicles keep to 80 km/h unless their country lets them faster; the others drive
    /// the road's speed.
    #[test]
    fn heavy_vehicles_keep_their_countrys_limit() {
        let code = |iso: &[u8; 2]| u16::from_le_bytes(*iso);
        let heavy = VehicleCategory::Heavy;
        assert_eq!(category_speed(heavy, 130.0, code(b"DE")), 80.0);
        assert_eq!(category_speed(heavy, 113.0, code(b"US")), 105.0);
        assert_eq!(category_speed(heavy, 60.0, code(b"US")), 60.0);
        assert_eq!(category_speed(heavy, 130.0, code(b"FR")), 90.0);
        assert_eq!(
            category_speed(VehicleCategory::Medium, 130.0, code(b"DE")),
            130.0
        );
    }

    #[test]
    fn countries_drive_their_battery_electric_shares() {
        let code = |iso: &[u8; 2]| u16::from_le_bytes(*iso);
        assert!((electric_share(code(b"NO")) - 0.2764).abs() < 1e-12);
        assert!((electric_share(code(b"DE")) - 0.0337).abs() < 1e-12);
        assert_eq!(electric_share(code(b"CZ")), 0.0, "not listed");
    }

    #[test]
    fn untagged_speeds_follow_the_country_and_built_up_density() {
        let cz = u16::from_le_bytes(*b"CZ");
        assert_eq!(default_speed(0, cz, 0).0, 130.0);
        assert_eq!(default_speed(1, cz, 0).0, 110.0, "Czech motorroads");
        assert_eq!(
            default_speed(1, cz, 2),
            (50.0, "country_legal_default"),
            "a trunk through a town"
        );
        assert_eq!(default_speed(1, cz, 1).0, 110.0);
        assert_eq!(default_speed(3, cz, 2), (50.0, "country_legal_default"));
        assert_eq!(default_speed(3, cz, 1).0, 90.0);
        assert_eq!(
            default_speed(3, cz, 0),
            (50.0, "default_by_class"),
            "unknown density keeps the class default"
        );
        assert_eq!(
            default_speed(5, cz, 2),
            (30.0, "default_by_class"),
            "local streets keep the class default"
        );
        assert_eq!(default_speed(0, 0, 0), (100.0, "default_by_class"));
    }

    /// A road's shares for the popup weigh each category's by its vehicles: a German Landesstrasse
    /// with one lorry in ten runs between its cars' night and its lorries'.
    #[test]
    fn a_roads_period_shares_weigh_its_categories_by_their_vehicles() {
        let de = u16::from_le_bytes(*b"DE");
        let [cars, lorries] = [false, true].map(|heavy| period_shares(de, 3, heavy));
        let shares = [cars, lorries, lorries, cars];
        let mixed = road_period_shares([900.0, 0.0, 100.0, 0.0], &shares);
        assert!(mixed[2] > cars[2] && mixed[2] < lorries[2], "{mixed:?}");
        assert!((mixed.iter().sum::<f64>() - 1.0).abs() < 0.003);
        assert_eq!(
            road_period_shares([0.0; 4], &shares),
            cars.map(|s| (s * 1e3).round() / 1e3)
        );
    }

    /// A two-way rural road at a national limit runs at 0.845 of it (DfT's free-flow speeds);
    /// motorways, one-way carriageways, towns and slower limits keep their limit.
    #[test]
    fn rural_roads_run_below_their_national_limit() {
        assert!(free_flows_below_limit(4, 1, false, 90.0));
        assert!(!free_flows_below_limit(4, 2, false, 90.0), "a town");
        assert!(!free_flows_below_limit(0, 1, false, 130.0), "a motorway");
        assert!(
            !free_flows_below_limit(1, 1, true, 110.0),
            "a dual carriageway"
        );
        assert!(!free_flows_below_limit(5, 1, false, 50.0), "a village lane");
        assert!((90.0 * RURAL_FREE_FLOW_OF_LIMIT - 76.05).abs() < 1e-9);
    }
}

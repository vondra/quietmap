//! Road pieces of the dev4 prepared tree as sources: the speed cascade, CNOSSOS-EU emission per
//! period computed here once, and the display record of the road group.

use super::country_speeds::COUNTRY_SPEEDS;
use super::road_junctions::{Junctions, traffic_signals};
use super::road_slope::{SquareHeights, WayRow, row_slopes};
use super::{Converted, Reach, group_key, split_at_tile_edges};
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
const DERESTRICTED_CODE: u8 = 255;
const DERESTRICTED_SPEED_KMH: f64 = 130.0;
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
/// dev4's `traffic_estimated` bits of a row whose four categories all come from a prior.
const ALL_CATEGORIES_ESTIMATED: u8 = 15;

/// The daily flows (light, medium, heavy, motorcycles) of a row: a dev4 class prior of a secondary
/// or tertiary row in a country with representative counts takes that country's cell and its
/// medium and heavy shares (motorcycles scaled with the flow); every other row keeps `prior`.
fn country_flows(
    prior: [f64; 4],
    class: usize,
    built_up: u8,
    country_iso: u16,
    source_id: u16,
    estimated: u8,
) -> [f64; 4] {
    if source_id != PRIOR_SOURCE_ID || estimated != ALL_CATEGORIES_ESTIMATED {
        return prior;
    }
    let Some(row) = class.checked_sub(3).filter(|row| *row < 2) else {
        return prior;
    };
    let iso = country_iso.to_le_bytes();
    let (Some((_, cells)), Some((_, shares))) = (
        COUNTRY_CLASS_PRIORS.iter().find(|(code, _)| *code == iso),
        COUNTRY_CLASS_SHARES.iter().find(|(code, _)| *code == iso),
    ) else {
        return prior;
    };
    let cell = usize::from(built_up.min(2));
    let scale = cells[row][cell] / DEV4_CLASS_PRIORS[row][cell];
    let total = prior.iter().sum::<f64>() * scale;
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
    let motorcycles = prior[3] * scale;
    let (medium, heavy) = (total * share(0), total * share(1));
    [
        total - medium - heavy - motorcycles,
        medium,
        heavy,
        motorcycles,
    ]
}

/// Day/evening/night shares of the daily flow: motorways, trunks and their links; other roads.
const MOTORWAY_PERIOD_SHARES: [f64; PERIODS] = [0.65, 0.20, 0.15];
const URBAN_PERIOD_SHARES: [f64; PERIODS] = [0.70, 0.18, 0.12];

/// The speed of an untagged road: the country's legal limit for main classes (urban or rural by
/// the row's built-up flag, unknown density keeps the class default), else the class default. A
/// trunk in a built-up area takes the urban limit like any street there; elsewhere the country's
/// motorroad limit where it has one.
fn default_speed(class: usize, country_iso: u16, built_up: u8) -> (f64, &'static str) {
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
    if value > 0 {
        (f64::from(value), "country_legal_default")
    } else {
        (CLASS_DEFAULT_SPEED_KMH[class], "default_by_class")
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
/// square; returns how many rows emit.
pub fn convert(
    dev4: &Dev4,
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
    let mut emitting = 0;
    for batch in &table.batches {
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
            let daily = country_flows(
                prior,
                class_index,
                built_up.value(row),
                country.value(row),
                source_id.value(row),
                estimated.value(row),
            );
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
            // pulling away (2.2.5), where dev4 capped them at 30 km/h (-4.1 dB at 50).
            let (speed, speed_source) = (base_speed, base_source);
            let stop = if junction.value(row) == ROUNDABOUT_CODE {
                Some((Junction::Roundabout, 0.0))
            } else {
                let (a, b) = (
                    z30_corner_degrees(start_x.value(row), start_y.value(row)),
                    z30_corner_degrees(end_x.value(row), end_y.value(row)),
                );
                junctions.nearest((0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1)))
            };
            let surface_index = usize::from(surface.value(row));
            let surface_correction = SURFACE_CORRECTION_DB
                .get(surface_index)
                .copied()
                .unwrap_or(0.0);
            let shares = if matches!(class_index, 0 | 1 | 10 | 11) {
                MOTORWAY_PERIOD_SHARES
            } else {
                URBAN_PERIOD_SHARES
            };
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
            let emission: [[f64; BANDS]; PERIODS] = std::array::from_fn(|period| {
                let flows: Vec<CategoryFlow> = directions
                    .iter()
                    .flat_map(|&(share, sign)| {
                        (0..4).map(move |c| CategoryFlow {
                            vehicles_per_hour: share * daily[c] * shares[period]
                                / PERIOD_HOURS[period],
                            speed_kmh: speed,
                            category: categories[c],
                            slope_percent: sign * slope,
                            junction: stop,
                        })
                    })
                    .collect();
                line_emission_db(&flows, surface_correction)
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
                source_id.value(row),
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
            country_flows(prior, 5, 2, cz, PRIOR_SOURCE_ID, estimated),
            prior
        );
        assert_eq!(
            country_flows(prior, 2, 2, cz, PRIOR_SOURCE_ID, estimated),
            prior
        );
        let de = u16::from_le_bytes(*b"DE");
        assert_eq!(
            country_flows(prior, 4, 2, de, PRIOR_SOURCE_ID, estimated),
            prior
        );
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
}

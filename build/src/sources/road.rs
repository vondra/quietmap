//! Road pieces of the dev4 prepared tree as sources: the speed cascade, CNOSSOS-EU emission per
//! period computed here once, and the display record of the road group.

use super::country_speeds::COUNTRY_SPEEDS;
use super::{Converted, group_key, split_at_tile_edges};
use crate::dev4::{Dev4, Square, require_stamp, z30_to_global};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int32Type, Int64Type, UInt8Type, UInt16Type};
use arrow_array::{Array, RecordBatch};
use physics::bands::{BANDS, PERIOD_HOURS, PERIODS};
use physics::emission::road::{CategoryFlow, VehicleCategory, line_emission_db};
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
/// Roundabouts (`junction` 1) are driven at no more than this.
const ROUNDABOUT_SPEED_KMH: f64 = 30.0;
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

/// Converts the road rows of one dev4 square; returns how many rows emit.
pub fn convert(dev4: &Dev4, square: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
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
        let country = c.get("country_iso")?.as_primitive::<UInt16Type>();
        let source_id = c.get("source_id")?.as_primitive::<UInt16Type>();
        for row in 0..batch.num_rows() {
            let daily: [f64; 4] = std::array::from_fn(|category| aadt[category].value(row));
            if tunnel.value(row) || daily.iter().sum::<f64>() <= 0.0 {
                continue;
            }
            let start = z30_to_global(start_x.value(row), start_y.value(row));
            let end = z30_to_global(end_x.value(row), end_y.value(row));
            if start == end {
                continue;
            }
            let class_index = usize::from(class.value(row)).min(CLASS_NAMES.len() - 1);
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
            let (speed, speed_source) =
                if junction.value(row) == 1 && base_speed > ROUNDABOUT_SPEED_KMH {
                    (ROUNDABOUT_SPEED_KMH, "roundabout_cap")
                } else {
                    (base_speed, base_source)
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
            let emission: [[f64; BANDS]; PERIODS] = std::array::from_fn(|period| {
                let flows = std::array::from_fn::<_, 4, _>(|c| CategoryFlow {
                    vehicles_per_hour: daily[c] * shares[period] / PERIOD_HOURS[period],
                    speed_kmh: speed,
                    category: categories[c],
                });
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
                cross_section.value(row).round(),
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

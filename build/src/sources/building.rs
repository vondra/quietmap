//! Buildings of dev4's `structures.arrow` (kind 0 rows with an OSM id) as sources: the settlement
//! area law over the gross floor area, homes their dwellings' outdoor units by country and climate
//! (`building_plant`), shops and offices their cooling plant and the buildings of bars and
//! restaurants their cooling plant and, where a restaurant cooks, its kitchen extract, heard in the
//! hours they run (`physics::emission::plant`); at half the building's height (its height tag, else
//! floors x 3 m, else 8 m) or 1.5 m for a ground activity (an emission-only area); cells of 30 m
//! above 2,000 m2. Each carries the id of its screening footprint, so it never screens itself. The
//! bells of churches and the calls of mosques (`bells`, `calls`) hang from them.

use super::bells::convert_bells;
use super::building_plant::{cooled_service_share, dwelling_plant};
pub use super::building_rows::BuildingRow;
use super::building_rows::{place_outside, read_batch};
use super::calls::convert_calls;
use super::cells::{AUDIBILITY_FLOOR_DBA, Site, push_site_points, resolve_area_m2, site_points};
use super::outside::OutsidePlacer;
use super::people::convert_people;
use super::venues::{PlaceHours, VenueSite, customs, place_hours, unmapped_place_hours};
use super::worship::Host;
use super::{Converted, Places, displayed_duty, group_key, object_key};
use crate::climate::Climate;
use crate::dev4::{Dev4, Square, degrees_to_z30, require_stamp, z30_corner_degrees};
use crate::rings::RingIndex;
use crate::screening::BUILDING_HEIGHT_MAX_M;
use crate::traffic::building_load;
use physics::bands::WeekHours;
use physics::emission::plant::{BUSINESS_HOURS, Running, cooling_plant, kitchen_extract};
use physics::emission::settlement::{
    COMMERCIAL, HOSPITALITY, building_sound_power, is_footprint_scaled, is_home, plant_sound_power,
};
use physics::emission::spectrum::SoundPower;
use serde_json::{Value, json};
use std::collections::HashMap;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// dev4's class of a residential block, and of every building tagged only building=yes.
const RESIDENTIAL_MULTI: u8 = 0;
const GROUND_ACTIVITY_SOURCE_HEIGHT_M: f64 = 1.5;
/// Storey height and the height of a building with neither a height nor a floor count (dev4).
const FLOOR_HEIGHT_M: f64 = 3.0;
const DEFAULT_HEIGHT_M: f64 = 8.0;
/// The footprint of a building with neither a stored area nor a ring.
const DEFAULT_FOOTPRINT_M2: f64 = 100.0;
const SINGLE_POINT_UP_TO_M2: f64 = 2_000.0;
const CELL_M: f64 = 30.0;
const WORSHIP: u8 = 5;

/// How a building is heard: steady, by its class's area law or its dwellings' units, or its plant in
/// use, each plant apart by the name its display gives it (a restaurant's kitchen extract, the
/// building's cooling under its class).
#[derive(Debug, PartialEq)]
pub enum Emitted {
    Steady(SoundPower),
    InUse(Vec<(&'static str, Running)>),
}

/// The display's name of a kitchen extract, a part of its building's row.
const KITCHEN_EXTRACT: &str = "kitchen_extract";

/// What a building emits and the geometry it was computed for.
#[derive(Debug, PartialEq)]
pub struct BuildingEmission {
    pub emitted: Emitted,
    pub source_height_m: f64,
    pub height_m: f64,
    pub floors: u8,
    pub area_m2: f64,
}

/// A building's emission in `country_iso` under its climate: a home's from the outdoor units of its
/// dwellings; a shop's or office's from its cooling plant over its storeys in business hours; a
/// building of bars and restaurants' from its cooling plant over its footprint while the places
/// mapped in it are open and a kitchen extract while one cooks (`hosted`); any other's by its
/// class's area law. `None` for silent classes and inaudible levels.
pub fn building_emission(
    row: &BuildingRow,
    (country_iso, climate): (u16, &Climate),
    hosted: Option<&PlaceHours>,
) -> Option<BuildingEmission> {
    let (height_m, floors) = if row.ground_activity {
        (0.0, 0)
    } else {
        let height_m = if row.height_tag_m > 0.0 {
            row.height_tag_m
        } else if row.floors > 0 {
            f64::from(row.floors) * FLOOR_HEIGHT_M
        } else {
            DEFAULT_HEIGHT_M
        }
        .min(BUILDING_HEIGHT_MAX_M);
        let floors = if row.floors > 0 {
            row.floors
        } else {
            (height_m / FLOOR_HEIGHT_M).ceil() as u8
        };
        (height_m, floors)
    };
    let area_m2 = resolve_area_m2(row.area_m2, &row.ring, DEFAULT_FOOTPRINT_M2);
    let (lat, lon) = z30_corner_degrees(row.centroid.0, row.centroid.1);
    let emitted = match row.class {
        class if is_home(class) => {
            let (dwellings, _) = building_load(row.class, row.storeys.max(1), Some(area_m2));
            let uses = dwelling_plant(country_iso, climate.degree_days(lat, lon));
            Emitted::Steady(plant_sound_power(&uses, dwellings)?)
        }
        COMMERCIAL | HOSPITALITY => {
            // Unknown storeys count one: the 8 m height of an untagged building is no floor count.
            let floor_area_m2 = if is_footprint_scaled(row.class) || row.ground_activity {
                area_m2
            } else {
                area_m2 * f64::from(row.storeys.max(1))
            };
            let cooling = |open: &WeekHours| {
                let cooling_degree_days = climate.degree_days(lat, lon).cooling;
                let cooled_share = cooled_service_share(country_iso);
                cooling_plant(floor_area_m2, cooling_degree_days, cooled_share, open)
            };
            let extract = (hosted.and_then(|places| places.kitchen.as_ref()))
                .and_then(|kitchen| kitchen_extract(area_m2, kitchen));
            let open = hosted.map_or(&BUSINESS_HOURS, |places| &places.open);
            let plants: Vec<(&'static str, Running)> = [
                extract.map(|plant| (KITCHEN_EXTRACT, plant)),
                cooling(open).map(|plant| (class_label(row.class), plant)),
            ]
            .into_iter()
            .flatten()
            .filter(|(_, plant)| {
                plant
                    .sound_power()
                    .is_some_and(|sound| sound.loudest_dba() >= AUDIBILITY_FLOOR_DBA)
            })
            .collect();
            if plants.is_empty() {
                return None;
            }
            Emitted::InUse(plants)
        }
        _ => Emitted::Steady(building_sound_power(row.class, area_m2, floors)?),
    };
    if let Emitted::Steady(sound) = &emitted
        && sound.day_dba < AUDIBILITY_FLOOR_DBA
    {
        return None;
    }
    let source_height_m = if row.ground_activity {
        GROUND_ACTIVITY_SOURCE_HEIGHT_M
    } else {
        height_m / 2.0
    };
    Some(BuildingEmission {
        emitted,
        source_height_m,
        height_m,
        floors,
        area_m2,
    })
}

fn class_label(class: u8) -> &'static str {
    const LABELS: [&str; 14] = [
        "residential_multi",
        "commercial",
        "warehouse",
        "education",
        "healthcare",
        "worship",
        "hotel",
        "garage",
        "farm",
        "public",
        "silent",
        "residential_house",
        "food_retail",
        "restaurant_bar",
    ];
    LABELS.get(usize::from(class)).copied().unwrap_or("default")
}

/// Converts the building rows of one dev4 square; returns how many rows emit.
pub fn convert(
    dev4: &Dev4,
    (climate, places): (&Climate, Places),
    (square, industrial_sites): (Square, &RingIndex),
    out: &mut Vec<Converted>,
) -> Result<usize, String> {
    // A square without buildings still has its mapped places.
    let table = dev4.table(square, "structures.arrow")?;
    let country_iso = dev4.square_country(square)?;
    let context = |error: String| format!("structures.arrow of {square:?}: {error}");
    if let Some(table) = &table {
        for (key, value) in [("grid", "z30"), ("structures_contract", "structures_v5")] {
            require_stamp(table, key, value).map_err(context)?;
        }
    }
    let batches = table
        .as_ref()
        .map_or(&[][..], |table| table.batches.as_slice());
    let mut emitting = 0;
    // Where bells and loudspeakers hang: every building's centre, height, footprint and name.
    let mut hosts: Vec<Host> = Vec::new();
    let mut emit = |row: &BuildingRow, emission: &BuildingEmission, out: &mut Vec<Converted>| {
        let key = group_key(&["building", &row.osm_id.to_string()]);
        if places.worship.is_some() {
            hosts.push(Host {
                centre: z30_corner_degrees(row.centroid.0, row.centroid.1),
                height_m: emission.height_m,
                footprint_id: row.footprint_id,
                name: row.name.clone(),
                worship: row.class == WORSHIP,
                group_key: key,
            });
        }
        // A building dev4 could only call a residential block (building=yes among them) inside
        // an industrial site is the site's: its sound is the site's own. It still hosts the bells
        // or the call mapped in it, which its walls must not screen.
        if row.class == RESIDENTIAL_MULTI && industrial_sites.containing(row.centroid).is_some() {
            return 0;
        }
        push_building(row, emission, key, out);
        1
    };
    // The buildings of bars, restaurants and cafés wait for the places mapped in them, whose hours
    // their plant runs; the people mapped inside one are a part of it.
    let mut hospitality_rows = Vec::new();
    let mut hospitality: HashMap<u64, u64> = HashMap::new();
    for batch in batches {
        for row in read_batch(batch, square).map_err(context)? {
            if row.class == HOSPITALITY {
                if row.footprint_id != 0 {
                    let key = group_key(&["building", &row.osm_id.to_string()]);
                    hospitality.insert(row.footprint_id, key);
                }
                hospitality_rows.push(row);
            } else if let Some(emission) = building_emission(&row, (country_iso, climate), None) {
                emitting += emit(&row, &emission, out);
            }
        }
    }
    let venues = places
        .venues
        .map_or(&[][..], |venues| venues.in_square(square.x, square.y));
    let mut placer = OutsidePlacer::new(
        venues
            .iter()
            .map(|site| degrees_to_z30(site.lat, site.lon))
            .collect(),
        venues.first().map_or(0.0, |site| site.lat),
    );
    if !venues.is_empty() {
        for batch in batches {
            place_outside(batch, (square, &hospitality), &mut placer).map_err(context)?;
        }
    }
    let mut hosted: HashMap<u64, Vec<&VenueSite>> = HashMap::new();
    for (site, host) in venues.iter().zip(placer.hosts()) {
        if let Some(key) = host {
            hosted.entry(*key).or_default().push(site);
        }
    }
    let customs = customs(country_iso);
    for row in &hospitality_rows {
        let key = group_key(&["building", &row.osm_id.to_string()]);
        let hours = hosted.get(&key).map_or_else(
            || unmapped_place_hours(&customs),
            |sites| place_hours(sites, &customs),
        );
        if let Some(emission) = building_emission(row, (country_iso, climate), Some(&hours)) {
            emitting += emit(row, &emission, out);
        }
    }
    if let Some(worship) = places.worship {
        let sites = worship.around(square);
        emitting += convert_bells((&sites, square), &hosts, country_iso, out);
        emitting += convert_calls((&sites, square), &hosts, country_iso, out);
    }
    if !venues.is_empty() {
        emitting += convert_people(
            venues,
            (&placer.positions(), placer.hosts()),
            (country_iso, climate),
            out,
        );
    }
    Ok(emitting)
}

/// Pushes the sources of a building that emits under its group `key`: one steady, writing its sound
/// power by day, or one a plant heard in use, writing its name, its sound power while running and
/// the share of each period it runs, a second plant a part of the first's object.
fn push_building(
    row: &BuildingRow,
    emission: &BuildingEmission,
    key: u64,
    out: &mut Vec<Converted>,
) {
    let site = Site {
        centroid: z30_corner_degrees(row.centroid.0, row.centroid.1),
        ring: &row.ring,
        area_m2: emission.area_m2,
        single_point_up_to_m2: SINGLE_POINT_UP_TO_M2,
        cell_m: CELL_M,
    };
    let points = site_points(&site);
    let decibels = |level: f64| (level * 10.0).round() / 10.0;
    let mut push =
        |sound: &SoundPower, (group_key, name): (u64, &str), level: f64, heard: &[Value]| {
            let mut display = vec![
                json!(row.name),
                json!(name),
                json!(decibels(emission.height_m)),
                json!(emission.floors),
                json!(emission.area_m2.round()),
                json!(row.address),
                json!(decibels(level)),
            ];
            display.extend_from_slice(heard);
            let attribute = Attribute {
                layer: Layer::Building,
                height_m: emission.source_height_m,
                ground_percent: GROUND_FROM_TERRAIN,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: row.footprint_id,
                group_key,
                emission: sound.band_levels_db(),
                display,
            };
            push_site_points(&points, emission.area_m2, &attribute, out);
        };
    match &emission.emitted {
        Emitted::Steady(sound) => push(sound, (key, class_label(row.class)), sound.day_dba, &[]),
        Emitted::InUse(plants) => {
            for (index, (name, plant)) in plants.iter().enumerate() {
                let Some(sound) = plant.sound_power() else {
                    continue;
                };
                let (part, object) = match index {
                    0 => (key, Value::Null),
                    _ => (
                        group_key(&["building", &row.osm_id.to_string(), name]),
                        json!(object_key(key)),
                    ),
                };
                let duty = json!(displayed_duty(plant.duty));
                let heard = [Value::Null, Value::Null, duty, object];
                push(&sound, (part, name), plant.running_dba, &heard);
            }
        }
    }
}

#[cfg(test)]
#[path = "building_tests.rs"]
mod tests;

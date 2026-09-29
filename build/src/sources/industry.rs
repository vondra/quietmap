//! Industrial sites, solar farms, substations and wind turbines of dev4's `industrial.arrow` as
//! point and area sources. The facility joins run here, not at the click, over the target square
//! and its neighbours: they hold every facility part and transformer of any row reaching the
//! target's tiles. Silent: dead sites (`suppressed`), gas stations, rail yards (PLAN-z13 DROP),
//! wind-farm outlines, inactive sites, transformers (their rating joins their substation), unknown
//! types and solar generators inside their plant (the plant emits).

use super::cells::{
    AUDIBILITY_FLOOR_DBA, Site, Z30Ring, decode_z30_ring, point_piece, push_site_points,
    resolve_area_m2, ring_area_m2, site_points,
};
use super::facilities::{
    FacilityJoins, Tags, facility_share, is_gas_substation, is_solar_plant, parse_tags,
    plant_output_mw, substation_power,
};
use super::{Converted, column, group_key};
use crate::dev4::{Dev4, Square, Table, require_stamp, z30_corner_degrees};
use arrow_array::{
    Array, BinaryArray, Float32Array, Int32Array, Int64Array, StringArray, UInt8Array, UInt16Array,
};
use physics::emission::industrial::*;
use physics::emission::spectrum::SoundPower;
use physics::emission::wind::{TURBINE_MAXIMUM_PLAUSIBLE_POWER_KW, turbine_sound_power};
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// Sites up to this area emit from one point, larger ones from cells of this side (dev4).
const SINGLE_POINT_UP_TO_M2: f64 = 5_000.0;
const CELL_M: f64 = 75.0;
/// The area of a site with neither a stored area nor a ring.
const DEFAULT_SITE_AREA_M2: f64 = 10_000.0;
/// Hub heights: the known-data median when untagged; taller tags are errors (dev4 audit I-10b).
const DEFAULT_HUB_HEIGHT_M: f64 = 105.0;
const MAXIMUM_HUB_HEIGHT_M: f64 = 175.0;

/// One `industrial.arrow` row as the conversion reads it; tags are kept for power rows only.
pub struct IndustrialRow {
    pub osm_kind: String,
    pub osm_id: i64,
    pub centroid: (i32, i32),
    pub source_type: u8,
    pub site_subtype: u8,
    pub name: String,
    pub hub_height_m: Option<f64>,
    pub rated_power_kw: Option<f64>,
    pub ring: Z30Ring,
    pub area_m2: Option<f64>,
    pub nace: Option<u16>,
    pub tags: Tags,
    pub suppressed: bool,
    pub source_id: u16,
}

/// What a row emits: a turbine at its hub, or a site over its area.
#[derive(Debug, PartialEq)]
pub enum RowEmission {
    Turbine {
        sound: SoundPower,
        hub_height_m: f64,
        rated_power_kw: Option<f64>,
    },
    Site {
        sound: SoundPower,
        height_m: f64,
        area_m2: f64,
        label: &'static str,
    },
}

/// A row's emission with the facility evidence of `joins`; `None` when it is silent.
pub fn row_emission(row: &IndustrialRow, joins: &FacilityJoins) -> Option<RowEmission> {
    let silent_type = matches!(
        row.source_type,
        SOURCE_RAIL_YARD | SOURCE_WIND_OUTLINE | SOURCE_INACTIVE
    ) || row.source_type >= SOURCE_TRANSFORMER;
    if row.suppressed || silent_type || is_gas_substation(&row.tags) {
        return None;
    }
    if row.source_type == SOURCE_WIND_TURBINE {
        let rated_power_kw = row
            .rated_power_kw
            .filter(|kw| *kw <= TURBINE_MAXIMUM_PLAUSIBLE_POWER_KW);
        return Some(RowEmission::Turbine {
            sound: turbine_sound_power(rated_power_kw),
            hub_height_m: row
                .hub_height_m
                .map_or(DEFAULT_HUB_HEIGHT_M, |hub| hub.min(MAXIMUM_HUB_HEIGHT_M)),
            rated_power_kw,
        });
    }
    if row.source_type == SOURCE_SOLAR_FARM
        && !is_solar_plant(&row.tags)
        && joins.inside_solar_plant(row.centroid)
    {
        return None;
    }
    let solar = row.source_type == SOURCE_SOLAR_FARM || row.nace == Some(SOLAR_NACE);
    let substation = row.source_type == SOURCE_SUBSTATION;
    let nameplate_mw = plant_output_mw(&row.tags);
    // An untagged solar node has neither output nor footprint: no default area invents one.
    let footprint = row.area_m2.is_some() || ring_area_m2(&row.ring).is_some();
    if solar && nameplate_mw.is_none() && row.rated_power_kw.is_none() && !footprint {
        return None;
    }
    let area_m2 = resolve_area_m2(row.area_m2, &row.ring, DEFAULT_SITE_AREA_M2);
    // Facility totals (a plant nameplate, a substation's rating) are shared by part area.
    let share_db = 10.0 * facility_share(&row.tags).log10();
    let (sound, height_m, label) = if solar {
        let unit_mw = row.rated_power_kw.map(|kw| kw / 1000.0);
        let mut sound = solar_farm_sound_power(nameplate_mw.or(unit_mw), area_m2);
        if nameplate_mw.is_some() {
            sound.day_dba += share_db;
        }
        (sound, 3.0, "solar_farm")
    } else if substation {
        let osm = (row.osm_kind.as_str(), row.osm_id);
        let (mva, class) = substation_power(&row.tags, &joins.substation_feed(osm, &row.ring));
        let mut sound = substation_sound_power(mva.unwrap_or_else(|| substation_class_mva(class)));
        sound.day_dba += share_db;
        (sound, 5.0, "substation")
    } else {
        let profile = row
            .nace
            .and_then(nace_profile)
            .or_else(|| subtype_profile(row.site_subtype))
            .unwrap_or_else(|| source_type_profile(row.source_type));
        let cap_m2 = sector_area_cap_m2(row.nace, row.site_subtype);
        (
            area_law_sound_power(&profile, area_m2, cap_m2),
            site_height_m(row),
            type_label(row.source_type),
        )
    };
    (sound.day_dba >= AUDIBILITY_FLOOR_DBA).then_some(RowEmission::Site {
        sound,
        height_m,
        area_m2,
        label,
    })
}

/// Quarries 8 m; stacks, flares and plant of mining, refining, minerals, metals and power 10 m.
fn site_height_m(row: &IndustrialRow) -> f64 {
    if row.source_type == SOURCE_QUARRY {
        8.0
    } else if row
        .nace
        .is_some_and(|code| matches!(code / 100, 5 | 7 | 8 | 19 | 23 | 24 | 35))
    {
        10.0
    } else {
        5.0
    }
}

fn type_label(source_type: u8) -> &'static str {
    match source_type {
        SOURCE_QUARRY => "quarry",
        2 => "farm",
        3 => "factory",
        4 => "wastewater",
        _ => "industrial_area",
    }
}

/// Converts the industrial rows of `target` and its neighbours; returns how many rows emit.
pub fn convert(dev4: &Dev4, target: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
    let mut rows = Vec::new();
    for square in target.with_neighbours() {
        if let Some(table) = dev4.table(square, "industrial.arrow")? {
            read_rows(&table, &mut rows)
                .map_err(|error| format!("industrial.arrow of {square:?}: {error}"))?;
        }
    }
    let mut joins = FacilityJoins::default();
    for row in &rows {
        let osm = (row.osm_kind.as_str(), row.osm_id);
        joins.add_row(row.source_type, osm, row.centroid, &row.ring, &row.tags);
    }
    let mut emitting = 0;
    for row in &rows {
        if let Some(emission) = row_emission(row, &joins) {
            place(row, emission, out);
            emitting += 1;
        }
    }
    Ok(emitting)
}

fn place(row: &IndustrialRow, emission: RowEmission, out: &mut Vec<Converted>) {
    let centroid = z30_corner_degrees(row.centroid.0, row.centroid.1);
    let nace = row.nace.map(|code| format!("{code:04}"));
    let decibels = |level: f64| (level * 10.0).round() / 10.0;
    let mut attribute = Attribute {
        layer: Layer::Industry,
        height_m: 0.0,
        ground_percent: GROUND_FROM_TERRAIN,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: group_key(&["industry", &row.osm_kind, &row.osm_id.to_string()]),
        emission: [[f64::NEG_INFINITY; tiles::sources::BANDS]; tiles::sources::PERIODS],
        display: String::new(),
    };
    match emission {
        RowEmission::Turbine {
            sound,
            hub_height_m,
            rated_power_kw,
        } => {
            attribute.height_m = hub_height_m;
            attribute.emission = sound.band_levels_db();
            attribute.display = json!([
                row.name,
                "wind_turbine",
                0,
                nace,
                1,
                hub_height_m,
                rated_power_kw,
                decibels(sound.day_dba),
                row.source_id
            ])
            .to_string();
            let (tile, ends) = point_piece(centroid.0, centroid.1);
            out.push(Converted {
                tile,
                ends,
                attribute,
            });
        }
        RowEmission::Site {
            sound,
            height_m,
            area_m2,
            label,
        } => {
            let site = Site {
                centroid,
                ring: &row.ring,
                area_m2,
                single_point_up_to_m2: SINGLE_POINT_UP_TO_M2,
                cell_m: CELL_M,
            };
            let points = site_points(&site);
            attribute.height_m = height_m;
            attribute.emission = sound.band_levels_db();
            attribute.display = json!([
                row.name,
                label,
                area_m2.round(),
                nace,
                points.len(),
                null,
                null,
                decibels(sound.day_dba),
                row.source_id
            ])
            .to_string();
            push_site_points(&points, area_m2, &attribute, out);
        }
    }
}

fn read_rows(table: &Table, rows: &mut Vec<IndustrialRow>) -> Result<(), String> {
    for (key, value) in [
        ("grid", "z30"),
        ("industrial_contract", "country_land_baked_v1"),
        ("osm_industrial_contract", "2"),
    ] {
        require_stamp(table, key, value)?;
    }
    for batch in &table.batches {
        let (gx, gy) = (
            column::<Int32Array>(batch, "centroid_gx")?,
            column::<Int32Array>(batch, "centroid_gy")?,
        );
        let (kinds, ids) = (
            column::<StringArray>(batch, "osm_kind")?,
            column::<Int64Array>(batch, "osm_id")?,
        );
        let (types, subtypes) = (
            column::<UInt8Array>(batch, "source_type")?,
            column::<UInt8Array>(batch, "site_subtype")?,
        );
        let (names, tags) = (
            column::<StringArray>(batch, "name")?,
            column::<StringArray>(batch, "osm_tags")?,
        );
        let (hubs, powers) = (
            column::<Float32Array>(batch, "hub_height")?,
            column::<Float32Array>(batch, "rated_power_kw")?,
        );
        let (geometry, areas) = (
            column::<BinaryArray>(batch, "geom")?,
            column::<Float32Array>(batch, "area_m2")?,
        );
        let (naces, sources) = (
            column::<UInt16Array>(batch, "nace_4digit")?,
            column::<UInt16Array>(batch, "source_id")?,
        );
        let suppressed = column::<UInt8Array>(batch, "suppressed")?;
        let positive = |values: &Float32Array, row: usize| {
            (values.is_valid(row) && values.value(row) > 0.0).then(|| f64::from(values.value(row)))
        };
        for row in 0..batch.num_rows() {
            let source_type = types.value(row);
            let power = matches!(
                source_type,
                SOURCE_SOLAR_FARM | SOURCE_SUBSTATION | SOURCE_TRANSFORMER
            );
            rows.push(IndustrialRow {
                osm_kind: kinds.value(row).to_string(),
                osm_id: ids.value(row),
                centroid: (gx.value(row), gy.value(row)),
                source_type,
                site_subtype: subtypes.value(row),
                name: if names.is_valid(row) {
                    names.value(row).to_string()
                } else {
                    String::new()
                },
                hub_height_m: positive(hubs, row),
                rated_power_kw: positive(powers, row),
                ring: geometry
                    .is_valid(row)
                    .then(|| decode_z30_ring(geometry.value(row)))
                    .flatten()
                    .unwrap_or_default(),
                area_m2: positive(areas, row),
                nace: Some(naces.value(row)).filter(|code| *code > 0),
                tags: if power {
                    parse_tags(tags.value(row))
                } else {
                    Tags::new()
                },
                suppressed: suppressed.value(row) != 0,
                source_id: sources.value(row),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "industry_tests.rs"]
mod tests;

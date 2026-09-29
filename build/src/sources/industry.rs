//! Industrial sites, solar farms, substations and wind turbines of dev4's `industrial.arrow` as
//! point and area sources, the facility joins run here over the target square and its neighbours
//! (they hold every part and transformer of a facility reaching the target's tiles).

use super::cells::{
    AUDIBILITY_FLOOR_DBA, Site, Z30Ring, point_piece, push_site_points, resolve_area_m2,
    ring_area_m2, ring_cell, site_points,
};
use super::facilities::{
    FacilityJoins, Tags, facility_share, is_gas_substation, is_solar_plant, parse_tags,
    plant_output_mw, substation_power,
};
use super::{Converted, group_key};
use crate::dev4::{Dev4, Square, Table, column, positive, require_stamp, text, z30_corner_degrees};
use arrow_array::{
    BinaryArray, Float32Array, Int32Array, Int64Array, StringArray, UInt8Array, UInt16Array,
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

/// What a row emits: its sound power, source height, display label, and its site's area (`None`
/// for a turbine, one point at its hub without an exclusion radius) or turbine rating.
#[derive(Debug, PartialEq)]
pub struct RowEmission {
    pub sound: SoundPower,
    pub height_m: f64,
    pub label: &'static str,
    pub area_m2: Option<f64>,
    pub rated_power_kw: Option<f64>,
}

/// A row's emission with the facility evidence of `joins`; `None` when silent: a dead site
/// (`suppressed`), a gas station, a rail yard (PLAN-z13 DROP), a wind-farm outline, an inactive
/// site, a transformer (its rating joins its substation), an unknown type, a solar generator
/// inside its plant (the plant emits) or an untagged solar node without a footprint.
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
        return Some(RowEmission {
            sound: turbine_sound_power(rated_power_kw),
            height_m: row
                .hub_height_m
                .map_or(DEFAULT_HUB_HEIGHT_M, |hub| hub.min(MAXIMUM_HUB_HEIGHT_M)),
            label: "wind_turbine",
            area_m2: None,
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
    // No default area may invent a footprint for an untagged solar node.
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
    (sound.day_dba >= AUDIBILITY_FLOOR_DBA).then_some(RowEmission {
        sound,
        height_m,
        label,
        area_m2: Some(area_m2),
        rated_power_kw: None,
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
    let points = emission.area_m2.map(|area_m2| {
        site_points(&Site {
            centroid,
            ring: &row.ring,
            area_m2,
            single_point_up_to_m2: SINGLE_POINT_UP_TO_M2,
            cell_m: CELL_M,
        })
    });
    let display = json!([
        row.name,
        emission.label,
        emission.area_m2.unwrap_or(0.0).round(),
        row.nace.map(|code| format!("{code:04}")),
        points.as_ref().map_or(1, Vec::len),
        emission.area_m2.is_none().then_some(emission.height_m),
        emission.rated_power_kw,
        (emission.sound.day_dba * 10.0).round() / 10.0,
        row.source_id
    ]);
    let attribute = Attribute {
        layer: Layer::Industry,
        height_m: emission.height_m,
        ground_percent: GROUND_FROM_TERRAIN,
        platform_half_width_m: 0.0,
        exclusion_radius_m: 0.0,
        footprint_id: 0,
        group_key: group_key(&["industry", &row.osm_kind, &row.osm_id.to_string()]),
        emission: emission.sound.band_levels_db(),
        display: display.to_string(),
    };
    match (points, emission.area_m2) {
        (Some(points), Some(area_m2)) => push_site_points(&points, area_m2, &attribute, out),
        _ => {
            let (tile, ends) = point_piece(centroid.0, centroid.1);
            out.push(Converted {
                tile,
                ends,
                attribute,
            });
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
        // Only the centroid is required; older squares lack some columns, which then read as
        // absent values (as dev4 reads them, `point_sources.rs`).
        let (gx, gy) = (
            column::<Int32Array>(batch, "centroid_gx")?,
            column::<Int32Array>(batch, "centroid_gy")?,
        );
        let bytes = |name: &str| column::<UInt8Array>(batch, name).ok();
        let (types, subtypes, suppressed) = (
            bytes("source_type"),
            bytes("site_subtype"),
            bytes("suppressed"),
        );
        let words = |name: &str| column::<UInt16Array>(batch, name).ok();
        let (naces, sources) = (words("nace_4digit"), words("source_id"));
        let floats = |name: &str| column::<Float32Array>(batch, name).ok();
        let (hubs, powers, areas) = (
            floats("hub_height"),
            floats("rated_power_kw"),
            floats("area_m2"),
        );
        let texts = |name: &str| column::<StringArray>(batch, name).ok();
        let (kinds, names, tags) = (texts("osm_kind"), texts("name"), texts("osm_tags"));
        let ids = column::<Int64Array>(batch, "osm_id").ok();
        let geometry = column::<BinaryArray>(batch, "geom").ok();
        for row in 0..batch.num_rows() {
            let source_type = types.map_or(0, |values| values.value(row));
            let power = matches!(
                source_type,
                SOURCE_SOLAR_FARM | SOURCE_SUBSTATION | SOURCE_TRANSFORMER
            );
            rows.push(IndustrialRow {
                osm_kind: kinds.map_or("", |values| text(values, row)).to_string(),
                osm_id: ids.map_or(0, |values| values.value(row)),
                centroid: (gx.value(row), gy.value(row)),
                source_type,
                site_subtype: subtypes.map_or(0, |values| values.value(row)),
                name: names.map_or("", |values| text(values, row)).to_string(),
                hub_height_m: hubs.and_then(|values| positive(values, row)),
                rated_power_kw: powers.and_then(|values| positive(values, row)),
                ring: geometry.map_or_else(Vec::new, |values| ring_cell(values, row)),
                area_m2: areas.and_then(|values| positive(values, row)),
                nace: naces
                    .map(|values| values.value(row))
                    .filter(|code| *code > 0),
                tags: match tags {
                    Some(values) if power => parse_tags(text(values, row)),
                    _ => Tags::new(),
                },
                suppressed: suppressed.is_some_and(|values| values.value(row) != 0),
                source_id: sources.map_or(0, |values| values.value(row)),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "industry_tests.rs"]
mod tests;

//! Open-air leisure areas and shooting ranges of dev4's `leisure.arrow` as area sources 1.5 m
//! above the ground, in the Building layer as dev4 shows them: the area law over the polygon (a
//! node takes its class's reference footprint; a line is no area and emits from its centroid),
//! ranges by shots a year unless roofed. Motorsport (PLAN-z13 DROP) and unknown classes are silent.

use super::cells::{
    AUDIBILITY_FLOOR_DBA, Site, Z30Ring, push_site_points, resolve_area_m2, ring_cell, site_points,
};
use super::facilities::{Tags, parse_tags};
use super::{Converted, group_key};
use crate::dev4::{Dev4, Square, column, positive, require_stamp, text, z30_corner_degrees};
use arrow_array::{BinaryArray, Float32Array, Int32Array, Int64Array, StringArray, UInt8Array};
use physics::emission::leisure::*;
use physics::emission::spectrum::SoundPower;
use serde_json::json;
use tiles::sources::{Attribute, GROUND_FROM_TERRAIN, Layer};

/// Voices and rackets on open ground.
const SOURCE_HEIGHT_M: f64 = 1.5;
/// Areas up to this emit from one point, larger ones from cells of this side (dev4).
const SINGLE_POINT_UP_TO_M2: f64 = 5_000.0;
const CELL_M: f64 = 75.0;
/// The area of a range with neither a stored area nor a ring.
const DEFAULT_RANGE_AREA_M2: f64 = 10_000.0;
/// dev4 `geometry_kind` of a line row (raceways and tracks).
const LINE_GEOMETRY: u8 = 2;

/// One `leisure.arrow` row as the conversion reads it; tags are kept for shooting ranges only.
pub struct LeisureRow {
    pub osm_kind: String,
    pub osm_id: i64,
    pub centroid: (i32, i32),
    pub class: u8,
    pub name: String,
    pub ring: Z30Ring,
    pub area_m2: Option<f64>,
    pub is_line: bool,
    pub tags: Tags,
}

impl LeisureRow {
    /// The polygon the row emits over: none for a line.
    fn area_ring(&self) -> &[(i32, i32)] {
        if self.is_line { &[] } else { &self.ring }
    }
}

/// A row's sound power and area; `None` when silent.
pub fn row_emission(row: &LeisureRow) -> Option<(SoundPower, f64)> {
    let (sound, area_m2) = if row.class == SHOOTING {
        let roofed = |key: &str| row.tags.get(key).is_some_and(|value| value != "no");
        if roofed("building") || roofed("indoor") {
            return None;
        }
        let details: Vec<&str> = row
            .tags
            .iter()
            .filter(|(key, _)| key.starts_with("shooting:"))
            .map(|(_, value)| value.as_str())
            .collect();
        let shooting = row.tags.get("shooting").map(String::as_str);
        let sound = shooting_sound_power(shooting_subtype(shooting, &details, &row.name))?;
        (
            sound,
            resolve_area_m2(row.area_m2, row.area_ring(), DEFAULT_RANGE_AREA_M2),
        )
    } else {
        let reference_m2 = leisure_profile(row.class)?.reference_area_m2;
        let area_m2 = resolve_area_m2(row.area_m2, row.area_ring(), reference_m2);
        (leisure_sound_power(row.class, area_m2)?, area_m2)
    };
    (sound.day_dba >= AUDIBILITY_FLOOR_DBA).then_some((sound, area_m2))
}

fn class_label(class: u8) -> &'static str {
    match class {
        PITCH => "sports_pitch",
        PADEL => "padel_court",
        TENNIS => "tennis_court",
        BASKETBALL => "ball_court",
        PLAYGROUND => "playground",
        POOL => "swimming_pool",
        OUTDOOR_SEATING => "outdoor_seating",
        STADIUM => "stadium",
        CAR_PARK => "car_park",
        CAR_PARK_STREET => "street_parking",
        SHOOTING => "shooting",
        ARTIFICIAL_TURF_PITCH => "artificial_turf_pitch",
        _ => "unknown",
    }
}

/// Converts the leisure rows of one dev4 square; returns how many rows emit.
pub fn convert(dev4: &Dev4, square: Square, out: &mut Vec<Converted>) -> Result<usize, String> {
    let Some(table) = dev4.table(square, "leisure.arrow")? else {
        return Ok(0);
    };
    let context = |error: String| format!("leisure.arrow of {square:?}: {error}");
    for (key, value) in [("grid", "z30"), ("leisure_contract", "leisure_v5")] {
        require_stamp(&table, key, value).map_err(context)?;
    }
    let mut emitting = 0;
    for batch in &table.batches {
        let rows = read_batch(batch).map_err(context)?;
        for row in rows {
            let Some((sound, area_m2)) = row_emission(&row) else {
                continue;
            };
            let site = Site {
                centroid: z30_corner_degrees(row.centroid.0, row.centroid.1),
                ring: row.area_ring(),
                area_m2,
                single_point_up_to_m2: SINGLE_POINT_UP_TO_M2,
                cell_m: CELL_M,
            };
            let decibels = (sound.day_dba * 10.0).round() / 10.0;
            let attribute = Attribute {
                layer: Layer::Building,
                height_m: SOURCE_HEIGHT_M,
                ground_percent: GROUND_FROM_TERRAIN,
                platform_half_width_m: 0.0,
                exclusion_radius_m: 0.0,
                footprint_id: 0,
                group_key: group_key(&["leisure", &row.osm_kind, &row.osm_id.to_string()]),
                emission: sound.band_levels_db(),
                display: json!([
                    row.name,
                    class_label(row.class),
                    0.0,
                    0,
                    area_m2.round(),
                    "",
                    decibels
                ])
                .to_string(),
            };
            push_site_points(&site_points(&site), area_m2, &attribute, out);
            emitting += 1;
        }
    }
    Ok(emitting)
}

fn read_batch(batch: &arrow_array::RecordBatch) -> Result<Vec<LeisureRow>, String> {
    let integers = |name: &str| column::<Int32Array>(batch, name);
    let (gx, gy) = (integers("centroid_gx")?, integers("centroid_gy")?);
    let texts = |name: &str| column::<StringArray>(batch, name);
    let (kinds, names, tags) = (texts("osm_kind")?, texts("name")?, texts("osm_tags")?);
    let bytes = |name: &str| column::<UInt8Array>(batch, name);
    let (classes, geometry_kinds) = (bytes("sport")?, bytes("geometry_kind")?);
    let (ids, geometry) = (
        column::<Int64Array>(batch, "osm_id")?,
        column::<BinaryArray>(batch, "geom")?,
    );
    let areas = column::<Float32Array>(batch, "area_m2")?;
    Ok((0..batch.num_rows())
        .map(|row| LeisureRow {
            osm_kind: kinds.value(row).to_string(),
            osm_id: ids.value(row),
            centroid: (gx.value(row), gy.value(row)),
            class: classes.value(row),
            name: text(names, row).to_string(),
            ring: ring_cell(geometry, row),
            area_m2: positive(areas, row),
            is_line: geometry_kinds.value(row) == LINE_GEOMETRY,
            tags: match classes.value(row) {
                SHOOTING => parse_tags(tags.value(row)),
                _ => Tags::new(),
            },
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: u8) -> LeisureRow {
        LeisureRow {
            osm_kind: "node".into(),
            osm_id: 1,
            centroid: (1 << 29, 1 << 29),
            class,
            name: String::new(),
            ring: Vec::new(),
            area_m2: None,
            is_line: false,
            tags: Tags::new(),
        }
    }

    /// A padel node is the reference court: active 90 minus 9 dB of season and duty, 81 dB(A).
    #[test]
    fn a_node_is_its_class_reference_footprint_and_a_line_is_no_area() {
        let (sound, area_m2) = row_emission(&node(PADEL)).unwrap();
        assert_eq!(area_m2, 200.0);
        assert!((sound.day_dba - 81.0).abs() < 0.2);
        let (x, y) = (1 << 29, 1 << 29);
        let chain = vec![(x, y), (x + 20_000, y + 20_000), (x, y + 30_000)];
        let line = LeisureRow {
            is_line: true,
            ring: chain,
            ..node(PITCH)
        };
        assert_eq!(row_emission(&line), row_emission(&node(PITCH)));
    }

    #[test]
    fn motorsport_unknown_and_roofed_ranges_are_silent() {
        assert_eq!(row_emission(&node(MOTORSPORT)), None);
        assert_eq!(row_emission(&node(ARTIFICIAL_TURF_PITCH + 1)), None);
        let roofed = LeisureRow {
            tags: parse_tags(r#"{"indoor":"yes"}"#),
            ..node(SHOOTING)
        };
        assert_eq!(row_emission(&roofed), None);
        let archery = LeisureRow {
            tags: parse_tags(r#"{"shooting":"archery"}"#),
            ..node(SHOOTING)
        };
        assert_eq!(row_emission(&archery), None);
        let (range, area_m2) = row_emission(&node(SHOOTING)).unwrap();
        assert!((range.day_dba - 110.0).abs() < 0.05 && area_m2 == 10_000.0);
    }
}

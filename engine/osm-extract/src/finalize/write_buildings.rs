//! `buildings.arrow` writer: one row per building footprint with the
//! POI-footprint join applied and area pre-computed on the SNAPPED ring.
//! Stamps the `buildings_v3` per-file contract. See `finalize` for dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use grid::poly::{encode_grid_poly, ring_area_m2};
use std::path::Path;
use std::sync::Arc;

use super::{
    decode_tsv_ring, parse_grid_cell, polygon_row_bbox, schema_with_contract,
    write_arrow_spatially_batched, BUILDINGS_CONTRACT_V3,
};
use crate::poi_join::{joined_building_type, JoinStats, PoiIndex};
use grid::Square;

/// A footprint larger than any real restaurant / bar — above this an
/// `amenity=restaurant/bar` is a tenant of a big structure, not the structure.
/// Tuned by eye: a huge restaurant / food court tops out ~3000 m²; terminals,
/// breweries and stadiums sit far above.
const MAX_RESTAURANT_FOOTPRINT_M2: f64 = 4000.0;

/// Downgrade an oversized `amenity`-derived restaurant to commercial. Restaurants
/// (HOSPITALITY) on a 10⁴–10⁵ m² footprint are airport terminals / breweries /
/// stadium concourses where the bar is one tenant; the footprint area-law would
/// otherwise stamp the whole envelope at ~90–100 dB. FOOD_RETAIL is exempt —
/// hypermarkets legitimately reach 5–15 k m².
fn downgrade_oversized_restaurant(bt: u8, area_m2: Option<f64>) -> u8 {
    use crate::ids as st;
    if bt == st::SETTLEMENT_HOSPITALITY && area_m2.is_some_and(|a| a > MAX_RESTAURANT_FOOTPRINT_M2) {
        return 1; // commercial — generic HVAC envelope
    }
    bt
}

pub(super) fn write_buildings(
    rows: &[Vec<String>],
    path: &Path,
    square: Square,
    poi_index: &PoiIndex,
    join_stats: &JoinStats,
) -> Result<()> {
    let n = rows.len();
    let schema = schema_with_contract(
        vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("building_type", DataType::UInt8, false),
            Field::new("building_use", DataType::UInt8, false),
            Field::new("height", DataType::Float32, true),
            Field::new("floors", DataType::UInt8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("addr_street", DataType::Utf8, true),
            Field::new("addr_housenumber", DataType::Utf8, true),
            Field::new("geom", DataType::Binary, true),
            // area_m2: pre-computed on the snapped ring with cos(lat) Shoelace.
            // WHY: runtime decode is slow; pre-compute once at extract time.
            Field::new("area_m2", DataType::Float32, true),
            // Dataset provenance — 0 = unspecified, populated by enrich-buildings-*.ts.
            Field::new("source_id", DataType::UInt16, false),
            // opening_hours day-fraction (0=unknown,1=24/7,2=day,3=evening/night).
            Field::new("opening_hours_frac", DataType::UInt8, false),
        ],
        "buildings_contract",
        BUILDINGS_CONTRACT_V3,
    );

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut cgx = Int32Builder::with_capacity(n);
    let mut cgy = Int32Builder::with_capacity(n);
    let mut btype = UInt8Builder::with_capacity(n);
    let mut buse = UInt8Builder::with_capacity(n);
    let mut height = Float32Builder::with_capacity(n);
    let mut floors = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 8);
    let mut street = StringBuilder::with_capacity(n, n * 12);
    let mut housenumber = StringBuilder::with_capacity(n, n * 4);
    let mut area_m2 = Float32Builder::with_capacity(n);
    let mut source_id = UInt16Builder::with_capacity(n);
    let mut geom = BinaryBuilder::with_capacity(n, n * 100);
    let mut opening = UInt8Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    // Centroids of the REAL buildings in this square (those with a `building`
    // tag, area_source=0). A functional-AREA source that geometrically contains
    // one of these is suppressed below. A clean area with no buildings inside
    // survives as the only source.
    let real_centroids: Vec<(i32, i32)> = rows
        .iter()
        .filter(|r| r.len() >= 13 && r[12].as_str() == "0")
        .filter_map(|r| Some((r[2].parse().ok()?, r[3].parse().ok()?)))
        .collect();

    for row in rows {
        // TSV: sq(0) osm_id(1) c_gx(2) c_gy(3) btype(4) buse(5) height(6) floors(7)
        //      name(8) street(9) housenumber(10) opening_hours(11) area_source(12) ring(13)
        if row.len() < 14 {
            continue;
        }
        let ring = decode_tsv_ring(row.get(13).map(|s| s.as_str()).unwrap_or(""));
        // Overlap suppression: a functional-area source wrapping real buildings.
        if row[12].as_str() == "1" {
            if let Some(ref ring) = ring {
                if real_centroids
                    .iter()
                    .any(|&(gx, gy)| grid::poly::ring_contains(ring, gx, gy))
                {
                    continue;
                }
            }
        }
        let area_opt = ring.as_ref().and_then(|r| ring_area_m2(r));
        // POI footprint join: reclassify `building=yes` when a function POI
        // sits inside it.
        let bt = joined_building_type(
            row[4].parse().unwrap_or(0),
            ring.as_deref().unwrap_or(&[]),
            square,
            poi_index,
            join_stats,
        );
        let bt = downgrade_oversized_restaurant(bt, area_opt);
        let c_gx = parse_grid_cell(&row[2]);
        let c_gy = parse_grid_cell(&row[3]);
        row_bboxes.push(polygon_row_bbox(ring.as_deref(), c_gx, c_gy));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        cgx.append_value(c_gx);
        cgy.append_value(c_gy);
        btype.append_value(bt);
        buse.append_value(row[5].parse().unwrap_or(0));
        let h: f32 = row[6].parse().unwrap_or(0.0);
        if h > 0.0 {
            height.append_value(h);
        } else {
            height.append_null();
        }
        floors.append_value(row[7].parse().unwrap_or(0));
        name.append_value(row.get(8).unwrap_or(&String::new()));
        street.append_value(row.get(9).unwrap_or(&String::new()));
        housenumber.append_value(row.get(10).unwrap_or(&String::new()));
        opening.append_value(row[11].parse().unwrap_or(0));
        match ring {
            Some(ring) => {
                match area_opt {
                    Some(a) => area_m2.append_value(a as f32),
                    None => area_m2.append_null(),
                }
                geom.append_value(encode_grid_poly(&ring));
            }
            None => {
                geom.append_null();
                area_m2.append_null();
            }
        }
        source_id.append_value(0);
    }

    write_arrow_spatially_batched(
        path,
        schema,
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(cgx.finish()),
            Arc::new(cgy.finish()),
            Arc::new(btype.finish()),
            Arc::new(buse.finish()),
            Arc::new(height.finish()),
            Arc::new(floors.finish()),
            Arc::new(name.finish()),
            Arc::new(street.finish()),
            Arc::new(housenumber.finish()),
            Arc::new(geom.finish()),
            Arc::new(area_m2.finish()),
            Arc::new(source_id.finish()),
            Arc::new(opening.finish()),
        ],
        &row_bboxes,
    )
}

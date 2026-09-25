//! Industrial sites and ship cells become normalized emission points.

use super::spatial::{INDUSTRIAL_QUERY_RADIUS_M, SHIP_QUERY_RADIUS_M};
use noise_compute::constants::INDUSTRIAL_MAX_RADIUS;
use arrow::array::Array;
use square_store::grid_cols::{
    col_binary, col_f32, col_i32, col_i64, col_str, col_u16, col_u8, decode_geom, grid_cell_lonlat,
};
use square_store::store::SquareData;

pub(super) fn collect_industrial(
    data: &SquareData,
    lat: f64,
    lng: f64,
    output: &mut Vec<noise_compute::types::PointSource>,
) -> Result<(), String> {
    let mut transformers: Option<Vec<square_store::osm_evidence::TransformerUnit>> = None;
    for batch in &data
        .industrial
        .batches_within(lat, lng, INDUSTRIAL_QUERY_RADIUS_M)?
    {
        let n = batch.num_rows();
        let (Some(cgx), Some(cgy)) = (col_i32(batch, "centroid_gx"), col_i32(batch, "centroid_gy"))
        else {
            continue;
        };
        let source_types = col_u8(batch, "source_type");
        let hub_heights = col_f32(batch, "hub_height");
        let rated_powers = col_f32(batch, "rated_power_kw");
        let names = col_str(batch, "name");
        let geom_col = col_binary(batch, "geom");
        let area_col = col_f32(batch, "area_m2");

        for i in 0..n {
            let (c_lon, c_lat) = grid_cell_lonlat(cgx.value(i), cgy.value(i));
            if col_u8(batch, "suppressed").map(|a| a.value(i)).unwrap_or(0) != 0 {
                continue;
            }

            let source_type = source_types.map(|a| a.value(i)).unwrap_or(0);
            let name = names.map(|a| a.value(i).to_string()).unwrap_or_default();
            let osm_id = col_i64(batch, "osm_id").map(|a| a.value(i)).unwrap_or(0);
            // Wind turbines are point sources even when the row carries geometry.
            let polygon_grid: grid::poly::GridRing = if source_type == 10 {
                Vec::new()
            } else {
                geom_col
                    .filter(|g| !g.is_null(i))
                    .and_then(|g| decode_geom(Some(g.value(i))))
                    .unwrap_or_default()
            };
            // The gate is the polygon EDGE, not its centroid: a receiver at the
            // east end of Garzweiler stands 5.6 km from the mine's centroid but
            // 250 m from its boundary, and the old 5 km centroid gate dropped
            // the mine (+32 dB at the east end once admitted). The painter
            // never had a centroid gate — its per-point reach caps at
            // `INDUSTRIAL_MAX_RADIUS` — so the popup admits a row exactly when
            // its edge can reach: centroid distance minus ring radius ≤ 4 km.
            // Rows without a ring are points (radius 0), as the painter treats
            // them (a ringless row discretises to one centroid point).
            let ring_radius_m = polygon_grid
                .iter()
                .map(|&(gx, gy)| {
                    let (lon, lat) = grid_cell_lonlat(gx, gy);
                    grid::geo::flat_dist(c_lat, c_lon, lat, lon)
                })
                .fold(0.0f64, f64::max);
            let dist = grid::geo::flat_dist(lat, lng, c_lat, c_lon);
            if dist - ring_radius_m > INDUSTRIAL_MAX_RADIUS {
                continue;
            }
            let positive_value = |column: Option<&arrow::array::Float32Array>| {
                column
                    .map(|values| values.value(i))
                    .filter(|value| *value > 0.0)
            };

            let site_subtype = col_u8(batch, "site_subtype")
                .map(|a| a.value(i))
                .unwrap_or(0);
            // Power evidence comes from the retained tags, not columns: solar
            // MW from `plant:output:electricity`, substation MVA from the
            // per-square transformer join (built lazily, only when a
            // substation row is admitted — most popups admit none).
            let is_power = matches!(
                source_type,
                noise_compute::emission::industrial::SOURCE_SOLAR_FARM
                    | noise_compute::emission::industrial::SOURCE_SUBSTATION
            );
            let row_tags = is_power.then(|| square_store::osm_evidence::optional_tags(batch, i));
            // A gas-network station carries no transformer hum.
            if row_tags
                .as_ref()
                .is_some_and(square_store::osm_evidence::is_gas_substation)
            {
                continue;
            }
            let plant_output_mw = row_tags
                .as_ref()
                .and_then(square_store::osm_evidence::plant_output_mw);
            let (substation_mva, substation_class) = match row_tags.as_ref() {
                Some(tags)
                    if source_type
                        == noise_compute::emission::industrial::SOURCE_SUBSTATION =>
                {
                    if transformers.is_none() {
                        transformers = Some(square_store::osm_evidence::transformer_units(
                            &data.industrial.batches_all()?,
                        ));
                    }
                    let feed = square_store::osm_evidence::substation_feed(
                        transformers.as_deref().unwrap_or(&[]),
                        &polygon_grid,
                    );
                    square_store::osm_evidence::substation_power(tags, &feed)
                }
                _ => (None, 0),
            };
            let prepared_points = noise_compute::normalize::prepare_industrial_points(
                noise_compute::normalize::RawIndustrialInput {
                    centroid_lat: c_lat,
                    centroid_lon: c_lon,
                    source_type,
                    site_subtype,
                    hub_height_m: positive_value(hub_heights),
                    rated_power_kw: positive_value(rated_powers),
                    area_m2: positive_value(area_col).map(f64::from),
                    polygon_grid: &polygon_grid,
                    nace_4digit: col_u16(batch, "nace_4digit")
                        .map(|a| a.value(i))
                        .filter(|&v| v > 0),
                    plant_output_mw,
                    substation_mva,
                    substation_class,
                },
            );
            let row_source_id = col_u16(batch, "source_id").map(|a| a.value(i)).unwrap_or(0);
            for prepared in prepared_points {
                let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                let mut point = prepared.with_metadata(
                    osm_id,
                    source_type,
                    name.clone(),
                    polygon_grid.clone(),
                    pt_dist,
                );
                point.source_id = row_source_id;
                output.push(point);
            }
        }
    }
    Ok(())
}

pub(super) fn collect_ships(
    ship_batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lng: f64,
    output: &mut Vec<noise_compute::types::PointSource>,
) -> Result<(), String> {
    for batch in ship_batches {
        let (Some(cgx), Some(cgy), Some(area), Some(large), Some(work), Some(leisure)) = (
            col_i32(batch, "centroid_gx"),
            col_i32(batch, "centroid_gy"),
            col_f32(batch, "area_m2"),
            col_f32(batch, "hours_large"),
            col_f32(batch, "hours_work"),
            col_f32(batch, "hours_leisure"),
        ) else {
            return Err(
                "ships.arrow lacks its cell columns — rerun scripts/ships/build_ships.py"
                    .to_string(),
            );
        };
        let source_ids = col_u16(batch, "source_id");
        for i in 0..batch.num_rows() {
            let (c_lon, c_lat) = grid_cell_lonlat(cgx.value(i), cgy.value(i));
            let dist = grid::geo::flat_dist(lat, lng, c_lat, c_lon);
            if dist > SHIP_QUERY_RADIUS_M {
                continue;
            }
            let Some((prepared_points, class)) = noise_compute::normalize::prepare_ship_points(
                noise_compute::normalize::RawShipInput {
                    centroid_lat: c_lat,
                    centroid_lon: c_lon,
                    area_m2: area.value(i),
                    hours_per_month: [large.value(i), work.value(i), leisure.value(i)],
                },
            ) else {
                continue;
            };
            // All sub-cells share the identity of their original z30 cell.
            let cell_id = (i64::from(cgx.value(i)) << 32) | i64::from(cgy.value(i) as u32);
            let row_source_id = source_ids.map(|a| a.value(i)).unwrap_or(0);
            for prepared in prepared_points {
                let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
                let mut point = prepared.with_metadata(
                    cell_id,
                    class as u8,
                    String::new(),
                    Vec::new(),
                    pt_dist,
                );
                point.source_id = row_source_id;
                output.push(point);
            }
        }
    }
    Ok(())
}

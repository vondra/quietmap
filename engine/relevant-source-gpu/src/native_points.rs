//! Building, leisure, industrial and ship-cell adapters preserve native emission geometry.
use super::*;

fn polygon(batch: &RecordBatch, row: usize, name: &str) -> Result<Vec<(i32, i32)>> {
    let Some(bytes) = col_binary(batch, name)
        .filter(|c| !c.is_null(row))
        .map(|c| c.value(row))
    else {
        return Ok(Vec::new());
    };
    decode_geom(Some(bytes)).with_context(|| format!("invalid {name} emission polygon"))
}

pub(super) fn points(batch: &RecordBatch, row: usize, name: &str) -> Result<Vec<PreparedPoint>> {
    if name == "structures"
        && (byte(batch, "kind", row) != square_store::store::STRUCTURE_KIND_BUILDING
            || col_i64(batch, "osm_id").is_none_or(|c| c.is_null(row)))
    {
        return Ok(Vec::new());
    }
    if name == "industrial" && byte(batch, "suppressed", row) != 0 {
        return Ok(Vec::new());
    }
    if name == "ships" {
        let [centroid_lat, centroid_lon] = position(batch, row, "centroid")?;
        let hours = |column| {
            float(batch, column, row).with_context(|| format!("ships.arrow lacks {column}"))
        };
        return Ok(prepare_ship_points(RawShipInput {
            centroid_lat,
            centroid_lon,
            area_m2: float(batch, "area_m2", row).context("ships.arrow lacks area_m2")?,
            hours_per_month: [hours("hours_large")?, hours("hours_work")?, hours("hours_leisure")?],
        })
        .map(|(points, _)| points)
        .unwrap_or_default());
    }
    let position = if name == "structures"
        && col_i32(batch, "emission_centroid_gx").is_some_and(|c| !c.is_null(row))
        && col_i32(batch, "emission_centroid_gy").is_some_and(|c| !c.is_null(row))
    {
        position(batch, row, "emission_centroid")?
    } else {
        position(batch, row, "centroid")?
    };
    let [centroid_lat, centroid_lon] = position;
    let area_m2 = float(batch, "area_m2", row)
        .filter(|v| *v > 0.0)
        .map(f64::from);
    let polygon_grid = if name == "industrial" && byte(batch, "source_type", row) == 10 {
        Vec::new()
    } else {
        polygon(
            batch,
            row,
            if name == "structures" {
                "emission_geom"
            } else {
                "geom"
            },
        )?
    };
    Ok(match name {
        "structures" => prepare_building_points(RawBuildingInput {
            area_source: square_store::structure_contract::is_emission_only_area(batch, row),
            centroid_lat,
            centroid_lon,
            height_m: float(batch, "height", row).unwrap_or(0.0),
            floors: byte(batch, "floors", row),
            building_type: byte(batch, "building_type", row),
            area_m2,
            polygon_grid: &polygon_grid,
        }),
        "leisure" => prepare_leisure_points(RawLeisureInput {
            centroid_lat,
            centroid_lon,
            sport: byte(batch, "sport", row),
            area_m2,
            polygon_grid: &polygon_grid,
        }),
        "industrial" => prepare_industrial_points(RawIndustrialInput {
            centroid_lat,
            centroid_lon,
            source_type: byte(batch, "source_type", row),
            site_subtype: byte(batch, "site_subtype", row),
            hub_height_m: float(batch, "hub_height", row).filter(|v| *v > 0.0),
            rated_power_kw: float(batch, "rated_power_kw", row).filter(|v| *v > 0.0),
            nace_4digit: Some(short(batch, "nace_4digit", row)).filter(|v| *v > 0),
            area_m2,
            polygon_grid: &polygon_grid,
        }),
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, BinaryArray, Float32Array, Int16Array, Int32Array, Int64Array, UInt8Array};
    use arrow::datatypes::{Field, Schema};
    use std::sync::Arc;

    #[test]
    fn popup_and_painter_preserve_ground_activity_without_building_defaults() {
        let ring = grid::poly::encode_grid_poly(&[
            (1 << 29, 1 << 29), ((1 << 29) + 100, 1 << 29),
            ((1 << 29) + 100, (1 << 29) + 100), (1 << 29, 1 << 29),
        ]);
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("kind", Arc::new(UInt8Array::from(vec![0, 0, 0]))),
            ("osm_id", Arc::new(Int64Array::from(vec![1, 2, 3]))),
            ("centroid_gx", Arc::new(Int32Array::from(vec![1 << 29; 3]))),
            ("centroid_gy", Arc::new(Int32Array::from(vec![1 << 29; 3]))),
            ("building_type", Arc::new(UInt8Array::from(vec![1, 1, 1]))),
            ("height", Arc::new(Float32Array::from(vec![Some(24.0), None, Some(0.25)]))),
            ("floors", Arc::new(UInt8Array::from(vec![8, 0, 0]))),
            ("height_m", Arc::new(Int16Array::from(vec![0, 8, 0]))),
            ("height_source", Arc::new(UInt8Array::from(vec![7, 2, 0]))),
            // Missing geometry and rounded sub-metre height must preserve real-building identity.
            ("geom", Arc::new(BinaryArray::from(vec![None::<&[u8]>; 3]))),
            ("emission_geom", Arc::new(BinaryArray::from(vec![Some(ring.as_slice()); 3]))),
            ("area_m2", Arc::new(Float32Array::from(vec![1_000.0; 3]))),
        ];
        let fields = columns.iter().map(|(name, array)|
            Field::new(*name, array.data_type().clone(), array.null_count() != 0)).collect::<Vec<_>>();
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)),
            columns.into_iter().map(|(_, column)| column).collect()).unwrap();
        let popup = source_reader::query_buildings_from_batches(std::slice::from_ref(&batch), 0.0, 0.0, 100.0);
        assert_eq!(popup.len(), 3);
        for (index, row) in popup.iter().enumerate() {
            assert_eq!(row.area_source, index == 0);
            let popup_points = prepare_building_points(RawBuildingInput {
                centroid_lat: row.centroid_lat, centroid_lon: row.centroid_lon,
                height_m: row.height, floors: row.floors, area_source: row.area_source,
                building_type: row.building_type, area_m2: Some(row.area_m2 as f64),
                polygon_grid: &row.polygon_grid,
            });
            let painter = points(&batch, index, "structures").unwrap();
            assert_eq!(painter.len(), 1);
            assert_eq!(painter[0].lw_day, popup_points[0].lw_day);
            assert_eq!(painter[0].source_height_m, popup_points[0].source_height_m);
            assert_eq!((painter[0].floors, painter[0].source_height_m),
                [(0, 1.5), (3, 4.0), (1, 0.125)][index]);
        }
    }
}

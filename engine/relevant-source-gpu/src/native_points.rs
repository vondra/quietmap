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

/// Per-square join contexts for one industrial/leisure file: the transformer
/// units a substation polygon joins against, and the raceway lines that
/// silence their enclosing motorsport polygon. Built once per file in
/// `load_sources`; empty for every other layer.
#[derive(Default)]
pub(super) struct FileJoins {
    pub transformers: Vec<square_store::osm_evidence::TransformerUnit>,
    pub motorsport_lines: Vec<square_store::osm_evidence::MotorsportLine>,
}

pub(super) fn points(
    batch: &RecordBatch,
    row: usize,
    name: &str,
    joins: &FileJoins,
) -> Result<Vec<PreparedPoint>> {
    if name == "structures"
        && (byte(batch, "kind", row) != square_store::store::STRUCTURE_KIND_BUILDING
            || col_i64(batch, "osm_id").is_none_or(|c| c.is_null(row)))
    {
        return Ok(Vec::new());
    }
    if name == "industrial" && byte(batch, "suppressed", row) != 0 {
        return Ok(Vec::new());
    }
    // A roofed motorsport/shooting row stays silent (its building footprint
    // carries the emission), as does a motorsport polygon enclosing a
    // raceway line (the lines carry the emission).
    if name == "leisure"
        && noise_compute::emission::leisure::is_formula_class(byte(batch, "sport", row))
    {
        let tags = square_store::osm_evidence::optional_tags(batch, row);
        if square_store::osm_evidence::tags_indicate_indoor(&tags) {
            return Ok(Vec::new());
        }
        if byte(batch, "sport", row) == noise_compute::emission::leisure::MOTORSPORT
            && !square_store::osm_evidence::row_is_leisure_line(batch, row)
            && square_store::osm_evidence::encloses_motorsport_line(
                &joins.motorsport_lines,
                &polygon(batch, row, "geom")?,
            )
        {
            return Ok(Vec::new());
        }
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
        "leisure" => {
            let sport = byte(batch, "sport", row);
            let row_name = col_str(batch, "name")
                .filter(|c| !c.is_null(row))
                .map(|c| c.value(row))
                .unwrap_or("");
            let formula = if noise_compute::emission::leisure::is_formula_class(sport) {
                let tags = square_store::osm_evidence::optional_tags(batch, row);
                let details: Vec<&str> = tags
                    .iter()
                    .filter(|(key, _)| key.starts_with("shooting:"))
                    .map(|(_, value)| value.as_str())
                    .collect();
                noise_compute::emission::leisure::formula_for_row(
                    sport,
                    tags.get("sport").map(String::as_str).unwrap_or(""),
                    tags.get("shooting").map(String::as_str),
                    &details,
                    row_name,
                )
            } else {
                None
            };
            prepare_leisure_points(RawLeisureInput {
                centroid_lat,
                centroid_lon,
                sport,
                area_m2,
                polygon_grid: &polygon_grid,
                formula,
                is_line: square_store::osm_evidence::row_is_leisure_line(batch, row),
            })
        }
        "industrial" => {
            let source_type = byte(batch, "source_type", row);
            let row_tags = matches!(
                source_type,
                noise_compute::emission::industrial::SOURCE_SOLAR_FARM
                    | noise_compute::emission::industrial::SOURCE_SUBSTATION
            )
            .then(|| square_store::osm_evidence::optional_tags(batch, row));
            // A gas-network station carries no transformer hum.
            if row_tags
                .as_ref()
                .is_some_and(square_store::osm_evidence::is_gas_substation)
            {
                return Ok(Vec::new());
            }
            let plant_output_mw = row_tags
                .as_ref()
                .and_then(square_store::osm_evidence::plant_output_mw);
            let (substation_mva, substation_class) = match row_tags.as_ref() {
                Some(tags)
                    if source_type
                        == noise_compute::emission::industrial::SOURCE_SUBSTATION =>
                {
                    let feed = square_store::osm_evidence::substation_feed(
                        &joins.transformers,
                        &polygon_grid,
                    );
                    square_store::osm_evidence::substation_power(tags, &feed)
                }
                _ => (None, 0),
            };
            prepare_industrial_points(RawIndustrialInput {
                centroid_lat,
                centroid_lon,
                source_type,
                site_subtype: byte(batch, "site_subtype", row),
                hub_height_m: float(batch, "hub_height", row).filter(|v| *v > 0.0),
                rated_power_kw: float(batch, "rated_power_kw", row).filter(|v| *v > 0.0),
                nace_4digit: Some(short(batch, "nace_4digit", row)).filter(|v| *v > 0),
                area_m2,
                polygon_grid: &polygon_grid,
                plant_output_mw,
                substation_mva,
                substation_class,
            })
        }
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
            ("height_tier", Arc::new(UInt8Array::from(vec![2, 2, 0]))),
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
            let painter = points(&batch, index, "structures", &FileJoins::default()).unwrap();
            assert_eq!(painter.len(), 1);
            assert_eq!(painter[0].lw_day, popup_points[0].lw_day);
            assert_eq!(painter[0].source_height_m, popup_points[0].source_height_m);
            assert_eq!((painter[0].floors, painter[0].source_height_m),
                [(0, 1.5), (3, 4.0), (1, 0.125)][index]);
        }
    }
}

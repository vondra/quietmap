//! Building, leisure and industrial area adapters preserve native emission geometry.
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

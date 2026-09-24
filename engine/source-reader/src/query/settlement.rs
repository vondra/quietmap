//! Building and leisure emission rows become the settlement point sources.

use super::spatial::{BUILDING_QUERY_RADIUS_M, LEISURE_QUERY_RADIUS_M};
use arrow::array::Array;
use square_store::grid_cols::{
    col_binary, col_f32, col_i32, col_i64, col_str, col_u8, decode_geom, grid_cell_lonlat,
};
use square_store::store::{SquareData, STRUCTURE_KIND_BUILDING};

pub struct BuildingResult {
    pub osm_id: i64,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub height: f32,
    pub floors: u8,
    pub area_source: bool,
    pub area_m2: f32,
    pub building_type: u8,
    pub name: String,
    pub addr_street: String,
    pub addr_housenumber: String,
    pub polygon_grid: grid::poly::GridRing,
}

// Emission overrides retain OSM geometry when screening uses a matched footprint.
pub fn query_buildings_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<BuildingResult> {
    let mut results = Vec::new();

    for batch in batches {
        let n = batch.num_rows();
        let kind = col_u8(batch, "kind");
        let osm_id = col_i64(batch, "osm_id");
        let cgx = col_i32(batch, "centroid_gx");
        let cgy = col_i32(batch, "centroid_gy");

        let (Some(kind), Some(osm_id), Some(cgx), Some(cgy)) = (kind, osm_id, cgx, cgy) else {
            continue;
        };

        let egx = col_i32(batch, "emission_centroid_gx");
        let egy = col_i32(batch, "emission_centroid_gy");
        let height = col_f32(batch, "height");
        let floors = col_u8(batch, "floors");
        let area = col_f32(batch, "area_m2");
        let btype = col_u8(batch, "building_type");
        let name = col_str(batch, "name");
        let street = col_str(batch, "addr_street");
        let house = col_str(batch, "addr_housenumber");
        let emission_geom = col_binary(batch, "emission_geom");

        for i in 0..n {
            if kind.value(i) != STRUCTURE_KIND_BUILDING || osm_id.is_null(i) {
                continue;
            }
            let present_grid = |col: Option<&arrow::array::Int32Array>, j: usize| {
                col.filter(|a| !a.is_null(j)).map(|a| a.value(j))
            };
            let (e_lon, e_lat) = match (present_grid(egx, i), present_grid(egy, i)) {
                (Some(gx), Some(gy)) => grid_cell_lonlat(gx, gy),
                _ => grid_cell_lonlat(cgx.value(i), cgy.value(i)),
            };
            let dist = grid::geo::flat_dist(lat, lon, e_lat, e_lon);
            if dist > max_radius {
                continue;
            }

            let polygon_grid: grid::poly::GridRing = emission_geom
                .filter(|a| !a.is_null(i))
                .and_then(|a| decode_geom(Some(a.value(i))))
                .unwrap_or_default();

            let opt_f32 = |col: Option<&arrow::array::Float32Array>| {
                col.filter(|a| !a.is_null(i)).map(|a| a.value(i))
            };
            let opt_u8 = |col: Option<&arrow::array::UInt8Array>| {
                col.filter(|a| !a.is_null(i)).map(|a| a.value(i))
            };
            let opt_str = |col: Option<&arrow::array::StringArray>| {
                col.filter(|a| !a.is_null(i))
                    .map(|a| a.value(i).to_string())
                    .unwrap_or_default()
            };
            results.push(BuildingResult {
                osm_id: osm_id.value(i),
                centroid_lat: e_lat,
                centroid_lon: e_lon,
                area_source: square_store::structure_contract::is_emission_only_area(batch, i),
                height: opt_f32(height).unwrap_or(0.0),
                floors: opt_u8(floors).unwrap_or(0),
                area_m2: opt_f32(area).unwrap_or(0.0),
                building_type: opt_u8(btype).unwrap_or(0),
                name: opt_str(name),
                addr_street: opt_str(street),
                addr_housenumber: opt_str(house),
                polygon_grid,
            });
        }
    }

    results
}

pub struct LeisureResult {
    pub osm_id: i64,
    pub centroid_lat: f64,
    pub centroid_lon: f64,
    pub sport: u8,
    pub area_m2: f32,
    pub name: String,
    pub polygon_grid: grid::poly::GridRing,
}

pub fn query_leisure_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Vec<LeisureResult> {
    let mut results = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        let (Some(osm_id), Some(cgx), Some(cgy)) = (
            col_i64(batch, "osm_id"),
            col_i32(batch, "centroid_gx"),
            col_i32(batch, "centroid_gy"),
        ) else {
            continue;
        };
        let sport = col_u8(batch, "sport");
        let area = col_f32(batch, "area_m2");
        let name = col_str(batch, "name");
        let geom = col_binary(batch, "geom");

        let suppressed = col_u8(batch, "suppressed");
        for i in 0..n {
            // Silenced by the extractor (an enclosing motorsport polygon whose
            // raceway lines carry the emission; an indoor range) — absent in
            // v3 files, where no row is silenced.
            if suppressed.map(|a| a.value(i)).unwrap_or(0) != 0 {
                continue;
            }
            let (c_lon, c_lat) = grid_cell_lonlat(cgx.value(i), cgy.value(i));
            let sport_id = sport.map(|a| a.value(i)).unwrap_or(0);
            let polygon_grid: grid::poly::GridRing = geom
                .filter(|a| !a.is_null(i))
                .and_then(|a| decode_geom(Some(a.value(i))))
                .unwrap_or_default();
            // Class-aware gate: area classes keep the 2 km centroid horizon;
            // formula classes (motorsport/shooting) reach like industrial
            // rows — polygon edge within 4 km.
            let dist = grid::geo::flat_dist(lat, lon, c_lat, c_lon);
            if noise_compute::emission::leisure::leisure_formula(sport_id).is_some() {
                let ring_radius_m = polygon_grid
                    .iter()
                    .map(|&(gx, gy)| {
                        let (lon, lat) = grid_cell_lonlat(gx, gy);
                        grid::geo::flat_dist(c_lat, c_lon, lat, lon)
                    })
                    .fold(0.0f64, f64::max);
                if dist - ring_radius_m > max_radius {
                    continue;
                }
            } else if dist > BUILDING_QUERY_RADIUS_M {
                continue;
            }
            results.push(LeisureResult {
                osm_id: osm_id.value(i),
                centroid_lat: c_lat,
                centroid_lon: c_lon,
                sport: sport.map(|a| a.value(i)).unwrap_or(0),
                area_m2: area.map(|a| a.value(i)).unwrap_or(0.0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                polygon_grid,
            });
        }
    }
    results
}

pub(super) fn collect_buildings(
    data: &SquareData,
    lat: f64,
    lng: f64,
    output: &mut Vec<noise_compute::types::PointSource>,
) -> Result<(), String> {
    let building_batches = data
        .structures
        .batches_within(lat, lng, BUILDING_QUERY_RADIUS_M)?;
    let buildings =
        query_buildings_from_batches(&building_batches, lat, lng, BUILDING_QUERY_RADIUS_M);
    for b in buildings {
        let display_name = if !b.name.is_empty() {
            b.name
        } else if !b.addr_street.is_empty() {
            if !b.addr_housenumber.is_empty() {
                format!("{} {}", b.addr_street, b.addr_housenumber)
            } else {
                b.addr_street
            }
        } else {
            String::new()
        };

        let prepared_points = noise_compute::normalize::prepare_building_points(
            noise_compute::normalize::RawBuildingInput {
                centroid_lat: b.centroid_lat,
                centroid_lon: b.centroid_lon,
                area_source: b.area_source,
                height_m: b.height,
                floors: b.floors,
                building_type: b.building_type,
                area_m2: (b.area_m2 > 0.0).then_some(b.area_m2 as f64),
                polygon_grid: &b.polygon_grid,
            },
        );
        for prepared in prepared_points {
            let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
            output.push(prepared.with_metadata(
                b.osm_id,
                b.building_type,
                display_name.clone(),
                b.polygon_grid.clone(),
                pt_dist,
            ));
        }
    }
    Ok(())
}

pub(super) fn collect_leisure(
    leisure_batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lng: f64,
    output: &mut Vec<noise_compute::types::PointSource>,
) {
    let leisure = query_leisure_from_batches(leisure_batches, lat, lng, LEISURE_QUERY_RADIUS_M);
    for lz in leisure {
        let source_type = noise_compute::types::LEISURE_TYPE_BASE.saturating_add(lz.sport);
        let prepared_points = noise_compute::normalize::prepare_leisure_points(
            noise_compute::normalize::RawLeisureInput {
                centroid_lat: lz.centroid_lat,
                centroid_lon: lz.centroid_lon,
                sport: lz.sport,
                area_m2: (lz.area_m2 > 0.0).then_some(lz.area_m2 as f64),
                polygon_grid: &lz.polygon_grid,
            },
        );
        for prepared in prepared_points {
            let pt_dist = grid::geo::flat_dist(lat, lng, prepared.lat, prepared.lon);
            output.push(prepared.with_metadata(
                lz.osm_id,
                source_type,
                lz.name.clone(),
                lz.polygon_grid.clone(),
                pt_dist,
            ));
        }
    }
}

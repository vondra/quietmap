//! Prepared roads decoded and collected within their existing source reach.

use super::spatial::{line_midpoint_exceeds_reach, segment_length_m};
use square_store::grid_cols::{
    col_bool, col_f32, col_i16, col_i32, col_i64, col_str, col_u16, col_u8, grid_cell_lonlat,
    RoadDirections,
};
use square_store::store::SquareData;

#[derive(Debug, serde::Serialize)]
pub struct RoadResult {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub road_class: u8,
    pub speed_limit: u8,
    pub speed_taper: u8,
    pub surface_type: u8,
    pub oneway: bool,
    pub lanes: u8,
    pub name: String,
    #[serde(rename = "ref")]
    pub road_ref: String,
    pub bridge: bool,
    pub tunnel: bool,
    pub junction: u8,
    pub built_up: u8,
    pub aadt_light: f64,
    pub aadt_medium: f64,
    pub aadt_heavy: f64,
    pub aadt_moto: f64,
    /// Estimated AADT categories: light 1, medium 2, heavy 4, moto 8.
    pub traffic_estimated: u8,
    /// Whole-road vehicles/day, both directions; 0 where only this carriageway's direction is known.
    pub cross_section_aadt: f64,
    #[serde(skip_serializing)]
    pub time_profile: Option<noise_compute::normalize::RoadTimeProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_profile_attribution: Option<noise_compute::normalize::RoadTimeProfileAttribution>,
    pub source_id: u16,
    pub dist_m: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub fraction: f64,
    /// Missing baked identity uses the receiver country and city.
    #[serde(skip_serializing)]
    pub square_country_city: Option<noise_compute::square_country_city::SquareCountryCity>,
}

pub fn query_roads_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Result<Vec<RoadResult>, String> {
    let mut results = Vec::new();

    let square_country_city =
        noise_compute::square_country_city::square_country_city_for_latlng(lat, lon);

    for batch in batches {
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id");
        let seg_idx = col_i16(batch, "segment_idx");
        let sgx = col_i32(batch, "start_gx");
        let sgy = col_i32(batch, "start_gy");
        let egx = col_i32(batch, "end_gx");
        let egy = col_i32(batch, "end_gy");
        let len = col_f32(batch, "length_m");
        let rclass = col_u8(batch, "road_class");
        let speed = col_u8(batch, "speed_limit");
        let speed_taper_col = col_u8(batch, "speed_taper");
        let surface = col_u8(batch, "surface_type");
        let directions = RoadDirections::read(batch)?;
        let traffic_columns = crate::road_traffic::RoadTrafficColumns::read(batch)?;
        let lanes = col_u8(batch, "lanes");
        let name = col_str(batch, "name");
        let road_ref = col_str(batch, "ref");
        let bridge_col = col_bool(batch, "bridge");
        let tunnel_col = col_bool(batch, "tunnel");
        let junction_col = col_u8(batch, "junction");
        let built_up_col = col_u8(batch, "built_up");
        let source_id_col = col_u16(batch, "source_id");
        // Present zero means WORLD defaults; only an absent column uses the receiver.
        let country_iso_col = col_u16(batch, "country_iso");
        let city_id_col = col_u16(batch, "city_id");
        let continent_col = col_u8(batch, "continent");

        let (Some(osm_id), Some(sgx), Some(sgy), Some(egx), Some(egy)) =
            (osm_id, sgx, sgy, egx, egy)
        else {
            continue;
        };

        for i in 0..n {
            let (s_lon, s_lat) = grid_cell_lonlat(sgx.value(i), sgy.value(i));
            let (e_lon, e_lat) = grid_cell_lonlat(egx.value(i), egy.value(i));
            let road_class = rclass.map(|a| a.value(i)).unwrap_or(0);
            let effective_radius =
                max_radius.min(noise_compute::normalize::road_max_distance_m(road_class));
            if line_midpoint_exceeds_reach(lat, lon, s_lat, s_lon, e_lat, e_lon, effective_radius) {
                continue;
            }

            let source_id = source_id_col.map(|a| a.value(i)).unwrap_or(0);
            let row_square_country_city = country_iso_col.map(|iso| {
                noise_compute::defaults::baked_square_country_city(
                    iso.value(i),
                    city_id_col.map(|c| c.value(i)).unwrap_or(0),
                    continent_col.map(|c| c.value(i)).unwrap_or(0),
                )
            });
            let raw = noise_compute::normalize::RawRoadInput {
                road_class,
                speed_limit: speed.map(|a| a.value(i)).unwrap_or(0),
                speed_taper: speed_taper_col.map(|a| a.value(i)).unwrap_or(0),
                surface_type: surface.map(|a| a.value(i)).unwrap_or(0),
                traffic: traffic_columns.row(i),
                tunnel: tunnel_col.map(|a| a.value(i)).unwrap_or(false),
                junction: junction_col.map(|a| a.value(i)).unwrap_or(0),
                built_up: built_up_col.map(|a| a.value(i)).unwrap_or(0),
            };
            let Some(norm) = noise_compute::normalize::normalize_road(
                raw,
                row_square_country_city.unwrap_or(square_country_city),
            ) else {
                continue;
            };
            debug_assert_eq!(effective_radius, max_radius.min(norm.max_distance_m));

            let cp = grid::geo::closest_point_on_segment(lat, lon, s_lat, s_lon, e_lat, e_lon);
            if cp.dist_m > effective_radius {
                continue;
            }

            results.push(RoadResult {
                osm_id: osm_id.value(i),
                segment_idx: seg_idx.map(|a| a.value(i)).unwrap_or(0),
                start_lat: s_lat,
                start_lon: s_lon,
                end_lat: e_lat,
                end_lon: e_lon,
                length_m: segment_length_m(len.map(|a| a.value(i)), s_lat, s_lon, e_lat, e_lon),
                road_class: raw.road_class,
                speed_limit: raw.speed_limit,
                speed_taper: raw.speed_taper,
                surface_type: raw.surface_type,
                oneway: directions.is_oneway(i),
                lanes: lanes.map(|a| a.value(i)).unwrap_or(0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                road_ref: road_ref.map(|a| a.value(i).to_string()).unwrap_or_default(),
                bridge: bridge_col.map(|a| a.value(i)).unwrap_or(false),
                tunnel: raw.tunnel,
                junction: raw.junction,
                built_up: raw.built_up,
                aadt_light: raw.traffic.light,
                aadt_medium: raw.traffic.medium,
                aadt_heavy: raw.traffic.heavy,
                aadt_moto: raw.traffic.moto,
                traffic_estimated: raw.traffic.estimated,
                cross_section_aadt: raw.traffic.cross_section_aadt,
                time_profile: raw.traffic.time_profile,
                time_profile_attribution: traffic_columns.attribution(i).cloned(),
                source_id,
                dist_m: cp.dist_m,
                cp_lat: cp.lat,
                cp_lon: cp.lon,
                fraction: cp.fraction,
                square_country_city: row_square_country_city,
            });
        }
    }

    Ok(results)
}

pub(super) fn collect_roads(
    data: &SquareData,
    lat: f64,
    lng: f64,
    output: &mut Vec<noise_compute::types::RoadSegment>,
) -> Result<(), String> {
    if let Some(schema) = data.roads.schema() {
        let empty = arrow::record_batch::RecordBatch::new_empty(schema.clone());
        square_store::grid_cols::RoadDirections::read(&empty)?;
        crate::road_traffic::RoadTrafficColumns::read(&empty)?;
    }
    let road_batches =
        data.roads
            .batches_within(lat, lng, noise_compute::constants::ROAD_MAX_RADIUS[0])?;
    let roads = query_roads_from_batches(
        &road_batches,
        lat,
        lng,
        noise_compute::constants::ROAD_MAX_RADIUS[0],
    )?;
    for r in roads {
        output.push(noise_compute::types::RoadSegment {
            osm_id: r.osm_id,
            square_country_city: r.square_country_city,
            segment_idx: r.segment_idx,
            start_lat: r.start_lat,
            start_lon: r.start_lon,
            end_lat: r.end_lat,
            end_lon: r.end_lon,
            length_m: r.length_m,
            road_class: r.road_class,
            speed_limit: r.speed_limit,
            speed_taper: r.speed_taper,
            surface_type: r.surface_type,
            oneway: r.oneway,
            lanes: r.lanes,
            traffic: noise_compute::normalize::RoadTraffic {
                light: r.aadt_light,
                medium: r.aadt_medium,
                heavy: r.aadt_heavy,
                moto: r.aadt_moto,
                estimated: r.traffic_estimated,
                time_profile: r.time_profile,
                cross_section_aadt: r.cross_section_aadt,
            },
            time_profile_attribution: r.time_profile_attribution,
            source_id: r.source_id,
            name: r.name,
            road_ref: r.road_ref,
            bridge: r.bridge,
            tunnel: r.tunnel,
            junction: r.junction,
            built_up: r.built_up,
            dist_m: r.dist_m,
            cp_lat: r.cp_lat,
            cp_lon: r.cp_lon,
            fraction: r.fraction,
        });
    }
    Ok(())
}

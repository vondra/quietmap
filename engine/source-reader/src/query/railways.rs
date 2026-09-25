//! Prepared railways decoded and collected within their existing source reach.

use super::spatial::{line_midpoint_exceeds_reach, segment_length_m};
use square_store::grid_cols::{
    col_bool, col_f32, col_i16, col_i32, col_i64, col_str, col_u16, col_u8, grid_cell_lonlat,
};
use square_store::store::SquareData;

#[derive(serde::Serialize)]
pub struct RailResult {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub rail_type: u8,
    pub usage: u8,
    pub maxspeed: u16,
    pub name: String,
    pub rail_ref: String,
    pub bridge: bool,
    pub tunnel: bool,
    pub service: u8,
    pub highspeed: bool,
    pub traffic: noise_compute::normalize::RailTraffic,
    pub dist_m: f64,
    pub cp_lat: f64,
    pub cp_lon: f64,
    pub fraction: f64,
    /// Missing baked identity uses the receiver country and city.
    #[serde(skip_serializing)]
    pub square_country_city: Option<noise_compute::square_country_city::SquareCountryCity>,
}

pub fn query_railways_from_batches(
    batches: &[arrow::record_batch::RecordBatch],
    lat: f64,
    lon: f64,
    max_radius: f64,
) -> Result<Vec<RailResult>, String> {
    let mut results = Vec::new();

    for batch in batches {
        let traffic = crate::rail_traffic::RailTrafficColumns::read(batch)?;
        let n = batch.num_rows();
        let osm_id = col_i64(batch, "osm_id");
        let sgx = col_i32(batch, "start_gx");
        let sgy = col_i32(batch, "start_gy");
        let egx = col_i32(batch, "end_gx");
        let egy = col_i32(batch, "end_gy");

        let (Some(osm_id), Some(sgx), Some(sgy), Some(egx), Some(egy)) =
            (osm_id, sgx, sgy, egx, egy)
        else {
            continue;
        };

        let seg_idx = col_i16(batch, "segment_idx");
        let len = col_f32(batch, "length_m");
        let rtype = col_u8(batch, "rail_type");
        let usage = col_u8(batch, "usage");
        let maxspd = col_u16(batch, "maxspeed");
        let name = col_str(batch, "name");
        let rail_ref = col_str(batch, "ref");
        let bridge_col = col_bool(batch, "bridge");
        let tunnel_col = col_bool(batch, "tunnel");
        let service_col = col_u8(batch, "service");
        let highspeed_col = col_bool(batch, "highspeed");
        let country_iso_col = col_u16(batch, "country_iso");
        let city_id_col = col_u16(batch, "city_id");
        let continent_col = col_u8(batch, "continent");

        for i in 0..n {
            let (s_lon, s_lat) = grid_cell_lonlat(sgx.value(i), sgy.value(i));
            let (e_lon, e_lat) = grid_cell_lonlat(egx.value(i), egy.value(i));

            if line_midpoint_exceeds_reach(lat, lon, s_lat, s_lon, e_lat, e_lon, max_radius) {
                continue;
            }

            let cp = grid::geo::closest_point_on_segment(lat, lon, s_lat, s_lon, e_lat, e_lon);
            if cp.dist_m > max_radius {
                continue;
            }

            results.push(RailResult {
                osm_id: osm_id.value(i),
                segment_idx: seg_idx.map(|a| a.value(i)).unwrap_or(0),
                start_lat: s_lat,
                start_lon: s_lon,
                end_lat: e_lat,
                end_lon: e_lon,
                length_m: segment_length_m(len.map(|a| a.value(i)), s_lat, s_lon, e_lat, e_lon),
                rail_type: rtype.map(|a| a.value(i)).unwrap_or(0),
                usage: usage.map(|a| a.value(i)).unwrap_or(0),
                maxspeed: maxspd.map(|a| a.value(i)).unwrap_or(0),
                name: name.map(|a| a.value(i).to_string()).unwrap_or_default(),
                rail_ref: rail_ref.map(|a| a.value(i).to_string()).unwrap_or_default(),
                bridge: bridge_col.map(|a| a.value(i)).unwrap_or(false),
                tunnel: tunnel_col.map(|a| a.value(i)).unwrap_or(false),
                service: service_col.map(|a| a.value(i)).unwrap_or(0),
                highspeed: highspeed_col.map(|a| a.value(i)).unwrap_or(false),
                traffic: traffic.row(i),
                dist_m: cp.dist_m,
                cp_lat: cp.lat,
                cp_lon: cp.lon,
                fraction: cp.fraction,
                square_country_city: country_iso_col.map(|iso| {
                    noise_compute::emission::railway::baked_square_country_city(
                        iso.value(i),
                        city_id_col.map(|c| c.value(i)).unwrap_or(0),
                        continent_col.map(|c| c.value(i)).unwrap_or(0),
                    )
                }),
            });
        }
    }
    Ok(results)
}

pub(super) fn collect_railways(
    data: &SquareData,
    lat: f64,
    lng: f64,
    output: &mut Vec<noise_compute::types::RailSegment>,
) -> Result<(), String> {
    if let Some(schema) = data.railways.schema() {
        crate::rail_traffic::RailTrafficColumns::read(
            &arrow::record_batch::RecordBatch::new_empty(schema.clone()),
        )?;
    }
    // Rows apply their emission-dependent reach after this conservative batch gate.
    let railway_batches =
        data.railways
            .batches_within(lat, lng, noise_compute::propagation::relevance_bound::LINE_REACH_CEILING_M)?;
    let railways = query_railways_from_batches(
        &railway_batches,
        lat,
        lng,
        noise_compute::propagation::relevance_bound::LINE_REACH_CEILING_M,
    )?;
    for r in railways {
        let norm =
            noise_compute::normalize::normalize_rail(noise_compute::normalize::RawRailInput {
                rail_type: r.rail_type,
                maxspeed: r.maxspeed,
                highspeed: r.highspeed,
                traffic: r.traffic,
            });
        let speed_source: u8 = if r.maxspeed > 0 {
            0
        } else if r.highspeed {
            1
        } else {
            2
        };

        output.push(noise_compute::types::RailSegment {
            osm_id: r.osm_id,
            square_country_city: r.square_country_city,
            segment_idx: r.segment_idx,
            start_lat: r.start_lat,
            start_lon: r.start_lon,
            end_lat: r.end_lat,
            end_lon: r.end_lon,
            length_m: r.length_m,
            rail_type: r.rail_type,
            usage: r.usage,
            maxspeed: r.maxspeed,
            traffic: r.traffic,
            speed_kmh: norm.speed_kmh,
            track_count: 1,
            name: r.name,
            rail_ref: r.rail_ref,
            bridge: r.bridge,
            tunnel: r.tunnel,
            service: r.service > 0,
            highspeed: r.highspeed,
            speed_source,
            dist_m: r.dist_m,
            cp_lat: r.cp_lat,
            cp_lon: r.cp_lon,
            fraction: r.fraction,
        });
    }
    Ok(())
}

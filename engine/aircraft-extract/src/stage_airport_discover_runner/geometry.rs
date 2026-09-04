//! Discovered strip geometry snapped once to the prepared coordinate grid.

use super::*;

pub(super) fn emit_lines_for_strip(
    strip: &DiscoveredStrip,
    airport_key: String,
    out: &mut Vec<SynthAirportLineRow>,
) {
    let centroid_lat = strip.center_lat as f64;
    let centroid_lon = strip.center_lon as f64;
    let osm_id = synth_osm_id_for(centroid_lat, centroid_lon);
    let name = synth_display_name(
        centroid_lat,
        centroid_lon,
        strip.length_m,
        strip.vertex_count,
    );
    for ms in microsegment_strip(strip) {
        out.push(SynthAirportLineRow {
            osm_id,
            segment_idx: ms.segment_idx,
            airport_key: airport_key.clone(),
            start_gx: grid::lonlat_to_grid(ms.start_lon, ms.start_lat).0,
            start_gy: grid::lonlat_to_grid(ms.start_lon, ms.start_lat).1,
            end_gx: grid::lonlat_to_grid(ms.end_lon, ms.end_lat).0,
            end_gy: grid::lonlat_to_grid(ms.end_lon, ms.end_lat).1,
            length_m: ms.length_m,
            heading_deg: strip.heading_deg,
            aeroway_type: AIRSTRIP_AEROWAY_TYPE,
            name: name.clone(),
        });
    }
}

/// One emitted microsegment of a synthetic runway.
pub(super) struct MicroSegment {
    pub(super) segment_idx: u16,
    pub(super) start_lat: f64,
    pub(super) start_lon: f64,
    pub(super) end_lat: f64,
    pub(super) end_lon: f64,
    pub(super) length_m: f32,
}

/// Slice the cluster line (length `strip.length_m` along
/// `strip.heading_deg`, centred at `(center_lat, center_lon)`) into
/// `ceil(length / 50 m)` microsegments. The synthetic line is
/// straight; real OSM runways are usually straight too, so a polyline
/// model isn't needed.
pub(super) fn microsegment_strip(strip: &DiscoveredStrip) -> Vec<MicroSegment> {
    let n = ((strip.length_m / SYNTH_MICROSEGMENT_M).ceil() as u32).max(1);
    let step_m = strip.length_m / n as f32;
    let half = strip.length_m * 0.5;
    let bearing_rad = (strip.heading_deg as f64).to_radians();
    // Compass heading: 0 = north, 90 = east. East unit = sin, north unit = cos.
    let east_unit = bearing_rad.sin();
    let north_unit = bearing_rad.cos();
    let cos_lat = (strip.center_lat as f64).to_radians().cos();

    let center_lat_f64 = strip.center_lat as f64;
    let center_lon_f64 = strip.center_lon as f64;

    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let along_start_m = -(half as f64) + (i as f64) * (step_m as f64);
        let along_end_m = along_start_m + (step_m as f64);
        let (slat, slon) = offset_latlon(
            center_lat_f64,
            center_lon_f64,
            east_unit,
            north_unit,
            cos_lat,
            along_start_m,
        );
        let (elat, elon) = offset_latlon(
            center_lat_f64,
            center_lon_f64,
            east_unit,
            north_unit,
            cos_lat,
            along_end_m,
        );
        out.push(MicroSegment {
            segment_idx: i as u16,
            start_lat: slat,
            start_lon: slon,
            end_lat: elat,
            end_lon: elon,
            length_m: step_m,
        });
    }
    out
}

/// Add `along_m` along the unit vector `(east_unit, north_unit)` to
/// the anchor `(anchor_lat, anchor_lon)`. Local equirectangular —
/// fine for ≤4 km runway extents at any latitude off the poles.
/// Longitude is wrapped to `(-180, 180]` so a strip running across
/// the antimeridian (Pacific airstrips, Fiji-style coverage) emits
/// valid OSM-compatible coordinates instead of values like `181.5`.
pub(super) fn offset_latlon(
    anchor_lat: f64,
    anchor_lon: f64,
    east_unit: f64,
    north_unit: f64,
    cos_lat: f64,
    along_m: f64,
) -> (f64, f64) {
    let east_m = east_unit * along_m;
    let north_m = north_unit * along_m;
    let dlat = north_m / M_PER_DEG_LAT as f64;
    let dlon = east_m / (M_PER_DEG_LON_EQUATOR as f64 * cos_lat);
    let lat = anchor_lat + dlat;
    let mut lon = anchor_lon + dlon;
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon <= -180.0 {
        lon += 360.0;
    }
    (lat, lon)
}

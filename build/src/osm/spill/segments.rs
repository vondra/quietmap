//! Serialize linear source pieces with original node intervals.

use super::*;

impl Spiller {
    /// Emit a linear segment (road, railway, barrier). Endpoints snap to grid
    /// here; length/bearing stay float (microsegment math, proven). A transport
    /// row ends with its one-field `transport::piece_tail`.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_segment(
        &mut self,
        ftype: &FeatureType,
        square: Square,
        osm_id: i64,
        seg_idx: i16,
        seg: &([f64; 2], [f64; 2], f32),
        tags: &Tags,
        piece_tail: Option<&str>,
    ) -> Result<()> {
        if !self.selects(square) {
            return Ok(());
        }
        let bucket = self.bucket(square);
        let name = ftype.name();
        let speed = if *ftype == FeatureType::Road {
            self.speeds.road_speed(tags)
        } else {
            tags.get("maxspeed")
                .map(|value| self.speeds.explicit_speed(value))
                .unwrap_or(0)
        };
        let w = self.get_writer(name, bucket)?;
        write_segment_row_prefix(w, square, osm_id, seg_idx, seg)?;

        // Append feature-specific tags as key=value pairs
        match ftype {
            FeatureType::Road => {
                let highway = tags.get("highway").map(|s| s.as_str()).unwrap_or("");
                // highway=track is physically unpaved by OSM convention — default
                // the surface when unspecified. +3 dB rolling correction is material
                // (see SURFACE_CORR in noise-compute/constants.rs).
                let surface = tags
                    .get("surface")
                    .map(|s| s.as_str())
                    .or_else(|| (highway == "track").then_some("unpaved"));
                let bridge = matches!(
                    tags.get("bridge").map(|s| s.as_str()),
                    Some("yes" | "viaduct" | "cantilever" | "movable")
                );
                let tunnel = matches!(
                    tags.get("tunnel").map(|s| s.as_str()),
                    Some("yes" | "building_passage" | "culvert")
                );
                let toll = tags.get("toll").map(|s| s.as_str()) == Some("yes");
                let lit = match tags.get("lit").map(|s| s.as_str()) {
                    Some("yes") => 1u8,
                    Some("no") => 2,
                    _ => 0, // 0=unknown
                };
                write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    classify::road_class(highway),
                    match speed {
                        classify::MAXSPEED_NONE => classify::SPEED_LIMIT_DERESTRICTED,
                        v => v.min(254) as u8,
                    },
                    classify::surface_type(surface),
                    classify::oneway_direction(
                        highway,
                        tags.get("oneway").map(String::as_str),
                        tags.get("junction").map(String::as_str),
                    ),
                    tags.get("lanes")
                        .and_then(|s| s.parse::<u8>().ok())
                        .unwrap_or(0),
                    tsv_tag(tags, "name"),
                    tsv_road_ref(tags),
                    if bridge { 1 } else { 0 },
                    if tunnel { 1 } else { 0 },
                    if toll { 1 } else { 0 },
                    lit,
                    classify::junction_type(tags.get("junction").map(|s| s.as_str())),
                    classify::access_type(
                        tags.get("access").map(|s| s.as_str()),
                        tags.get("motor_vehicle").map(|s| s.as_str()),
                        tags.get("vehicle").map(|s| s.as_str()),
                    ),
                )?;
            }
            FeatureType::Railway => {
                let railway = tags.get("railway").map(|s| s.as_str()).unwrap_or("rail");
                let electrified = match tags.get("electrified").map(|s| s.as_str()) {
                    Some("contact_line") | Some("yes") | Some("rail") => 1u8,
                    Some("no") => 2,
                    _ => 0, // unknown
                };
                let gauge = tags
                    .get("gauge")
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(0);
                let bridge = matches!(
                    tags.get("bridge").map(|s| s.as_str()),
                    Some("yes" | "viaduct" | "cantilever" | "movable")
                );
                let tunnel = classify::railway_is_underground(
                    railway,
                    tags.get("tunnel").map(|s| s.as_str()),
                    tags.get("layer").map(|s| s.as_str()),
                    tags.get("location").map(|s| s.as_str()),
                    tags.get("covered").map(|s| s.as_str()),
                );
                write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    classify::rail_type(railway),
                    classify::rail_usage_type(tags.get("usage").map(|s| s.as_str())),
                    // `none` is a road concept; on rail drop it so it falls to
                    // the same 0 = untagged → type-default speed downstream.
                    // u16 column (finalize.rs): 300+ km/h survives.
                    if speed == classify::MAXSPEED_NONE {
                        0
                    } else {
                        speed
                    },
                    tsv_tag(tags, "name"),
                    tsv_tag(tags, "ref"),
                    electrified,
                    gauge,
                    if bridge { 1 } else { 0 },
                    if tunnel { 1 } else { 0 },
                    if tags.get("highspeed").map(|s| s.as_str()) == Some("yes") {
                        1
                    } else {
                        0
                    },
                    classify::rail_service_type(tags.get("service").map(|s| s.as_str())),
                    classify::rail_traffic_mode(
                        tags.get("railway:traffic_mode").map(|s| s.as_str())
                    ),
                )?;
            }
            FeatureType::Barrier => {
                // height_tier: 0 = mapped height tag, 2 = none (height 0). The
                // structures builder gives an unmapped wall its country's mean height.
                let mapped = tags.get("height").and_then(|s| parse_height(s));
                write!(
                    w,
                    "\t{}\t{}\t{}",
                    mapped.unwrap_or(0.0),
                    classify::barrier_material_type(tags.get("material").map(|s| s.as_str())),
                    if mapped.is_some() { 0 } else { 2 },
                )?;
            }
            FeatureType::AirportLine => {
                let heading = microsegment::bearing_deg(seg.0[0], seg.0[1], seg.1[0], seg.1[1]);
                write!(
                    w,
                    "\t{:.1}\t{}\t{}\t{}\t",
                    heading,
                    classify::aeroway_type(tags),
                    tsv_tag(tags, "ref"),
                    tsv_tag(tags, "surface"),
                )?;
                if let Some(v) = classify::parse_width_m(tags.get("width").map(|s| s.as_str())) {
                    write!(w, "{v:.1}")?;
                }
            }
            _ => {}
        }
        if matches!(ftype, FeatureType::Road | FeatureType::Railway) {
            write!(w, "\t{}", tags_json(tags)?)?;
        }
        if let Some(piece_tail) = piece_tail {
            write!(w, "\t{piece_tail}")?;
        }
        writeln!(w)?;
        Ok(())
    }
}

//! Serialize point and polygon source rows with their OSM evidence.

use super::*;

impl Spiller {
    /// Emit a polygon/point feature (building, industrial, wind turbine).
    /// Centroid and ring snap to grid here; area is computed on the SNAPPED
    /// ring in finalize (stored geometry is the truth, self-consistent).
    /// Eight flat params, no bundle struct (see `emit_node` in main.rs).
    #[allow(clippy::too_many_arguments)]
    pub fn emit_polygon(
        &mut self,
        ftype: &FeatureType,
        square: Square,
        osm_id: i64,
        osm_kind: &str,
        clat: f64,
        clon: f64,
        tags: &Tags,
        ring: Option<&[[f64; 2]]>,
    ) -> Result<()> {
        if !self.selects(square) {
            return Ok(());
        }
        let leisure_line = *ftype == FeatureType::Leisure
            && osm_kind == "way"
            && classify::is_leisure_line(
                |key| tags.get(key).map(String::as_str),
                ring.is_some_and(classify::is_a_closed_ring),
            );
        if matches!(
            ftype,
            FeatureType::Industrial | FeatureType::WindTurbine | FeatureType::Leisure
        ) && osm_kind != "node"
            && !ring.is_some_and(|ring| {
                if leisure_line {
                    ring.len() >= 2
                } else {
                    classify::is_a_closed_ring(ring)
                }
            })
        {
            return Ok(());
        }
        let bucket = self.bucket(square);
        let name = ftype.name();

        let building_bt =
            matches!(ftype, FeatureType::Building).then(|| building_type_from_tags(tags));
        // What stands on this Building row (the TSV's `area_source` field).
        let ground = building_bt.map_or(GROUND_HOLDS_A_BUILDING, |_| ground_state(tags));
        let (cgx, cgy) = lonlat_to_grid(clon, clat);
        let snapped: Vec<(i32, i32)> = ring
            .map(|r| r.iter().map(|c| lonlat_to_grid(c[1], c[0])).collect())
            .unwrap_or_default();

        let w = self.get_writer(name, bucket)?;
        write!(w, "{}\t{}\t{}\t{}", spill_key(square), osm_id, cgx, cgy)?;

        match ftype {
            FeatureType::Building => {
                // Use amenity/shop/healthcare tags to override building type classification.
                // WHY: building=yes + amenity=school → type 3 (school), not 0 (residential).
                let bt = building_bt.unwrap();
                write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    bt,
                    building_use(tags),
                    tags.get("height")
                        .and_then(|s| parse_height(s))
                        .unwrap_or(0.0),
                    tags.get("building:levels")
                        .and_then(|s| s.parse::<u8>().ok())
                        .unwrap_or(0),
                    tsv_tag(tags, "name"),
                    tsv_tag(tags, "addr:street"),
                    tsv_tag(tags, "addr:housenumber"),
                    // settlement v2 phase 2: opening_hours → day-fraction u8.
                    classify::opening_hours_fraction(tags.get("opening_hours").map(|s| s.as_str())),
                    ground,
                )?;
            }
            FeatureType::Leisure => {
                // No capacity column: the area-law unification dropped capacity
                // scaling, and this contract bump is the moment to delete it.
                write!(
                    w,
                    "\t{}\t{}\t{}",
                    classify::leisure_sport_class(tags),
                    classify::opening_hours_fraction(tags.get("opening_hours").map(|s| s.as_str())),
                    tsv_tag(tags, "name"),
                )?;
            }
            FeatureType::Industrial | FeatureType::WindTurbine => {
                let src_type: u8 = if let Some(class) = classify::industrial_class(tags) {
                    class
                } else if matches!(ftype, FeatureType::WindTurbine) {
                    10
                }
                // wind_turbine
                else {
                    site_type_from_tags(tags)
                };
                write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}",
                    src_type,
                    site_subtype_from_tags(tags),
                    tsv_tag(tags, "name"),
                    tags.get("height")
                        .and_then(|s| parse_height(s))
                        .unwrap_or(0.0),
                    parse_power_kw(tags.get("generator:output:electricity").map(|s| s.as_str())),
                )?;
            }
            FeatureType::AirportArea => {
                let airport_ref = tags
                    .get("ref")
                    .or_else(|| tags.get("local_ref"))
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let width_m = classify::parse_width_m(tags.get("width").map(|s| s.as_str()))
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_default();
                write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    classify::aeroway_type(tags),
                    tsv_tag(tags, "name"),
                    TsvText(airport_ref),
                    tsv_tag(tags, "icao"),
                    tsv_tag(tags, "iata"),
                    tsv_tag(tags, "operator"),
                    tsv_tag(tags, "surface"),
                    width_m,
                    tsv_tag(tags, "aerodrome:type"),
                    tsv_tag(tags, "access"),
                )?;
            }
            _ => {}
        }

        // Snapped ring as grid text (empty when the feature is a point).
        write!(w, "\t{}", encode_ring_text(&snapped))?;
        if matches!(
            ftype,
            FeatureType::Industrial | FeatureType::WindTurbine | FeatureType::Leisure
        ) {
            write!(w, "\t{}\t{osm_kind}", tags_json(tags)?)?;
        }

        if *ftype == FeatureType::Leisure {
            // Preserve the original line decision: snapping can close nearby endpoints.
            write!(w, "\t{}", u8::from(leisure_line))?;
        }
        writeln!(w)?;
        Ok(())
    }
}

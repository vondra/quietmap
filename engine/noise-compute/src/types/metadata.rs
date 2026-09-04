//! Contributor + source-metadata types — one top-N `Contributor` per grouped
//! source with its A-weighted per-effect impact breakdown for the popup.
use super::*;

/// Individual noise contributor (top-N with full breakdown). The five
/// `*_impact_db` fields are A-weighted `ΔL_A = full_lden − no_effect_lden`
/// aggregated across every grouped segment — the only physically meaningful
/// single-number summary of a per-band attenuation. `ground_impact_db` is
/// signed: over soft ground at 63/125 Hz CF[i] is negative, so ground can
/// *boost* LF energy (no_ground < full ⇒ impact > 0).
#[derive(Debug, Clone, Serialize)]
pub struct Contributor {
    pub source_type: LayerKind,
    pub osm_id: Option<i64>,
    pub name: String,
    pub subtype: String,
    pub distance_m: f64,
    pub periods: NoisePeriods,
    pub periods_free: NoisePeriods,
    pub emission_db: f64,
    pub baseline: PropagationBaseline,
    pub terrain: TerrainBreakdown,
    pub screening: ScreeningBreakdown,
    pub vegetation: VegetationBreakdown,
    pub terrain_impact_db: f64,     // A-weighted ΔL_A (≤ 0)
    pub screening_impact_db: f64,   // A-weighted ΔL_A (≤ 0)
    pub vegetation_impact_db: f64,  // A-weighted ΔL_A (≤ 0)
    pub atmospheric_impact_db: f64, // A-weighted ΔL_A (≤ 0)
    pub ground_impact_db: f64,      // A-weighted ΔL_A (SIGNED)
    pub received_bands: [f64; NUM_BANDS],
    /// GeoJSON geometry for map highlight (LineString coords or null)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<serde_json::Value>,
    /// Per-source engine metadata (raw + effective inputs + aggregation stats).
    /// Populated only on popup path; pipeline tile output does not use it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SourceMetadata>,
}

/// Per-source contributor metadata, typed discriminated union.
/// Popup exposes this verbatim to the frontend; pipeline does not populate it.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceMetadata {
    Road(RoadMetadata),
    Rail(RailMetadata),
    Building(BuildingMetadata),
    Industrial(IndustrialMetadata),
    // Boxed: AircraftMetadata is ~3× the next-largest variant (it nests the full
    // airborne/ground-ops detail), so inlining it would bloat every Contributor's
    // `Option<SourceMetadata>` to that size. serde sees through the Box, so the
    // `{"kind":"aircraft", ..}` JSON is byte-identical (see metadata_tests).
    Aircraft(Box<AircraftMetadata>),
}

/// Road contributor metadata — values from the **dominant segment** (highest
/// received energy contribution), plus aggregate stats over all grouped segments.
/// This ensures the popup shows what actually drives the noise result.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RoadMetadata {
    // Raw (from Arrow, pre-factor) — at dominant segment
    pub aadt_light_raw: i32,
    pub aadt_medium_raw: i32,
    pub aadt_heavy_raw: i32,
    pub aadt_moto_raw: i32,
    pub traffic_source: &'static str, // "matched_external" | "estimated_service_tree" | "default_by_class"
    pub dominant_source_id: u16, // dataset identity (single source of truth: pipeline/lib/sources.ts → engine/noise-compute/src/sources.rs; 0 = unspecified). Resolved into `provenance` field below.
    pub speed_posted_kmh: Option<u8>, // raw OSM maxspeed (Some(0) = untagged); None = derestricted (maxspeed=none) — no number exists to display

    // Nominal ("road total, both directions") — arrow's raw number if
    // enriched, else class default. Pre-factor (no oneway / access /
    // lane_ratio applied). Frontend uses these as the authoritative
    // display number so per-OSM-way mechanics never leak into the summary.
    pub aadt_light_nominal: f64,
    pub aadt_medium_nominal: f64,
    pub aadt_heavy_nominal: f64,
    pub aadt_moto_nominal: f64,

    // Effective (what CNOSSOS consumed) — at dominant segment
    pub aadt_light_effective: f64,
    pub aadt_medium_effective: f64,
    pub aadt_heavy_effective: f64,
    pub aadt_moto_effective: f64,
    pub speed_kmh: f64,
    pub speed_source: &'static str, // "osm_posted" | "default_by_class" | "roundabout_cap" | "derestricted"

    // Descriptive — at dominant segment
    pub road_class: &'static str, // "motorway" .. "living_street"
    pub surface: &'static str,    // "asphalt" | "paving" | "concrete" | "unpaved" | "gravel"
    pub surface_corr_db: f64,
    pub lanes: u8,
    pub oneway: bool,

    // Dominant segment (loudest contributor)
    pub dominant_segment_idx: i16,
    pub dominant_distance_m: f64,

    // Closest segment distance (for "nearest point" display)
    pub closest_distance_m: f64,

    // Speed range across all segments in group
    pub speed_min_kmh: f64,
    pub speed_max_kmh: f64,

    // Oneway breakdown
    pub oneway_segment_count: u32,
    pub twoway_segment_count: u32,

    // Aggregation over all grouped segments
    pub segment_count: u32,
    pub total_length_m: f64,
    pub bridge_count: u32,

    // Group-level screening obstacle histogram (cheap scan across every segment)
    pub obstacle_segment_count: u32,   // segments with max_bh > 2.0 m
    pub obstacle_avg_height_m: f64,    // mean of those max_bh values
    pub obstacle_max_height_m: f64,    // tallest obstacle seen across all segments
    pub obstacle_max_segment_idx: i16, // which segment carries the tallest

    /// Dataset attribution resolved from `dominant_source_id`. See
    /// `crate::sources::DatasetMeta` and `EmissionTrace::Road.provenance`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::sources::DatasetMeta>,
}

/// Railway contributor metadata — raw + effective train counts and speed.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RailMetadata {
    // Effective at the energy-dominant (loudest) segment, post service ×
    // parallel_divisor scaling. Stored as f64 so the popup shows the same value
    // the emission used — the Arrow-side raw integer count is scaled by
    // service/parallel factors before reaching the engine; truncating back to
    // i32 here would reintroduce the precision loss Milník A fixed.
    pub trains_passenger_raw: f64,
    pub trains_freight_raw: f64,
    pub trains_passenger_source: &'static str, // "arrow" | "default_by_type"
    pub trains_freight_source: &'static str,
    pub source_id: u16, // dataset identity (single source of truth: pipeline/lib/sources.ts → engine/noise-compute/src/sources.rs; 0 = unspecified). Resolved into `provenance` field below.
    pub maxspeed_posted_kmh: u16, // u16: 300+ km/h high-speed postings survive

    // Effective (post service × parallel_divisor scaling)
    pub trains_passenger_effective: f64,
    pub trains_freight_effective: f64,
    pub speed_kmh: f64,
    pub speed_source: &'static str, // "osm_maxspeed" | "highspeed_default" | "type_default"

    // Descriptive
    pub rail_type: &'static str, // "rail" | "tram" | ...
    pub usage: &'static str,     // "main" | "branch" | "industrial"
    pub service: bool,
    pub highspeed: bool,
    pub parallel_divisor: u8,
    pub bridge: bool,

    // Geometry context: which segment the headline metadata is from (dominant
    // by received energy) and where the nearest track edge is (closest by
    // perpendicular distance). Mirrors `RoadMetadata`. Lets the popup annotate
    // "dominant segment, 200 m away (closest 50 m)" so users see why the
    // displayed speed / trains may not match the track right next to them.
    pub dominant_segment_idx: i16,
    pub dominant_distance_m: f64,
    pub closest_distance_m: f64,

    // Aggregation
    pub segment_count: u32,
    pub total_length_m: f64,

    // Group-level screening obstacle histogram
    pub obstacle_segment_count: u32,
    pub obstacle_avg_height_m: f64,
    pub obstacle_max_height_m: f64,
    pub obstacle_max_segment_idx: i16,

    /// Dataset attribution resolved from `source_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<crate::sources::DatasetMeta>,
}

/// Building contributor metadata.
#[derive(Debug, Clone, Serialize, Default)]
pub struct BuildingMetadata {
    pub height_m: f64,
    pub floors: u8,
    pub area_m2: f64,
    pub building_type: &'static str,
    pub address: String,
}

/// Industrial site contributor metadata.
#[derive(Debug, Clone, Serialize, Default)]
pub struct IndustrialMetadata {
    pub area_m2: f64,
    pub source_type: &'static str, // "industrial_area" | "quarry" | "wastewater" | "wind_turbine" ...
    pub nace: Option<String>,
    pub grid_point_count: u16, // engine discretized the site into N points
    pub source_id: u16, // dataset identity stamp (0 = unspecified); resolved into `provenance`
    pub provenance: Option<crate::sources::DatasetMeta>,
}

/// Aircraft contributor metadata — wraps AircraftBandData so aircraft flows
/// through the same discriminated union as road/rail/etc.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AircraftMetadata {
    pub variant: String, // "airborne" | "ground_ops"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airport_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airport_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airborne: Option<AircraftAirborneDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground_ops: Option<AircraftGroundOpsDetail>,
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    /// Boxing `SourceMetadata::Aircraft` must NOT change the wire JSON the popup
    /// frontend consumes: serde sees through the `Box`, so the internally-tagged
    /// enum still flattens the `AircraftMetadata` fields next to `"kind":"aircraft"`,
    /// with no `Box` indirection appearing in the output. Covers both production
    /// shapes (airborne `Some` / ground-ops `Some`) and asserts the literal byte
    /// stream — field order, not just the key/value set.
    #[test]
    fn boxed_aircraft_variant_serializes_transparently() {
        let cases = [
            AircraftMetadata {
                variant: "airborne".to_string(),
                airport_name: Some("LKPR".to_string()),
                airport_key: Some("lkpr".to_string()),
                airborne: Some(AircraftAirborneDetail::default()),
                ground_ops: None,
            },
            AircraftMetadata {
                variant: "ground_ops".to_string(),
                airport_name: Some("LKPR".to_string()),
                airport_key: Some("lkpr".to_string()),
                airborne: None,
                ground_ops: Some(AircraftGroundOpsDetail::default()),
            },
        ];
        for inner in cases {
            // Byte-level: the tag, then the inner struct's fields in declaration
            // order — exactly what an un-boxed `Aircraft(AircraftMetadata)` emitted
            // (serializing through the `Box` delegates to the same `AircraftMetadata`
            // impl, so it cannot reorder or drop a field).
            let body = serde_json::to_string(&inner).expect("serialize inner");
            let expected = format!("{{\"kind\":\"aircraft\",{}", &body[1..]);
            let got = serde_json::to_string(&SourceMetadata::Aircraft(Box::new(inner.clone())))
                .expect("serialize boxed variant");
            assert_eq!(got, expected, "boxed {} JSON drifted", inner.variant);

            // And no `Box`/tuple wrapper key leaked into the object.
            let v = serde_json::to_value(SourceMetadata::Aircraft(Box::new(inner))).unwrap();
            assert_eq!(v["kind"], "aircraft");
            assert!(v.get("0").is_none(), "no tuple/Box wrapper in the JSON");
        }
    }
}

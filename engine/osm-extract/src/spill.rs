//! Spill bucket writer.
//!
//! Features are hashed by square id % num_buckets and appended to intermediate files.
//! Each bucket is TSV (simple during development — Arrow IPC in finalize step).
//!
//! First column is always the numeric square id (`y * 512 + x`); finalize sorts
//! on it. Coordinates are already snapped z30 grid cells — the spill carries
//! ints, never floats or WKB. Rings encode as `gx,gy;gx,gy;…` (empty = none).

use crate::classify::{self, FeatureType, Tags};
use crate::ids;
use crate::microsegment;
use anyhow::Result;
use grid::{lonlat_to_grid, Square};
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Transient row-major partition key for spill/sort: `y * 512 + x`, fits u32.
/// Scratch-only (spill text + bucketing) — never the persistent [`grid::square_id`].
pub fn spill_key(square: Square) -> u32 {
    u32::from(square.y) * 512 + u32::from(square.x)
}

/// Square back from a [`spill_key`]. `None` on out-of-range (stale spill guard).
pub fn square_from_spill_key(id: u32) -> Option<Square> {
    if id >= 512 * 512 {
        return None;
    }
    Some(Square {
        x: (id % 512) as u16,
        y: (id / 512) as u16,
    })
}

/// Encode a snapped ring for the TSV ring column. Empty ring = empty string.
pub fn encode_ring_text(ring: &[(i32, i32)]) -> String {
    ring.iter()
        .map(|(gx, gy)| format!("{gx},{gy}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Parse a TSV ring column. `None` on any malformation (caller stores null).
pub fn parse_ring_text(s: &str) -> Option<Vec<(i32, i32)>> {
    if s.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for pt in s.split(';') {
        let (gx, gy) = pt.split_once(',')?;
        out.push((gx.parse().ok()?, gy.parse().ok()?));
    }
    if out.len() < 3 {
        None
    } else {
        Some(out)
    }
}

/// OSM strings at the TSV boundary. Tabs and record separators are data in
/// PBF tags but structural in the transient spill format, so render them as
/// spaces while preserving every other Unicode scalar verbatim.
struct TsvText<'a>(&'a str);

impl fmt::Display for TsvText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for character in self.0.chars() {
            formatter.write_char(if matches!(character, '\t' | '\n' | '\r') {
                ' '
            } else {
                character
            })?;
        }
        Ok(())
    }
}

fn tsv_tag<'a>(tags: &'a Tags, key: &str) -> TsvText<'a> {
    TsvText(tags.get(key).map(String::as_str).unwrap_or(""))
}

/// Road `ref` with `old_ref` fallback: renumbered motorways keep matching
/// census sections cut against the old numbering. Proven by planet-260831
/// way 100515391 (no `ref`, `int_ref=E 55;E 65`, `old_ref=D1`), whose empty
/// ref dropped the direct RSD match to a continuity fill (-14.6 dB received
/// on the Chodov point). `int_ref` is deliberately not consulted: census
/// sections are cut against national numbering, not E-roads. Rail/airport
/// refs keep the literal tag: no renumbering case is proven for them.
fn tsv_road_ref(tags: &Tags) -> TsvText<'_> {
    let direct = tags.get("ref").map(String::as_str).unwrap_or("");
    TsvText(if direct.is_empty() {
        tags.get("old_ref").map(String::as_str).unwrap_or("")
    } else {
        direct
    })
}

/// Per-feature-type, per-bucket writer.
struct BucketFile {
    writer: BufWriter<File>,
}

/// Observability for the two SILENT failure modes of building classification (so
/// a blind spot is never invisible — reported at the end of every extract):
/// an unrecognised `building=*` value that fell to the residential(0) default.
/// The routing fall-through (a functional area that vanished) is counted in
/// `main` where the `None` happens.
#[derive(Default)]
pub struct ExtractAudit {
    /// `building=X` → residential(0) where X is neither recognised nor a known
    /// residential synonym (a real `building=yes` is the intended default, not a trap).
    pub default_residential: HashMap<String, u64>,
}

pub struct Spiller {
    dir: PathBuf,
    num_buckets: usize,
    /// (feature_type_name, bucket_idx) → writer
    writers: HashMap<(String, usize), BucketFile>,
    pub audit: ExtractAudit,
}

impl Spiller {
    pub fn new(dir: &Path, num_buckets: usize) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Spiller {
            dir: dir.to_path_buf(),
            num_buckets,
            writers: HashMap::new(),
            audit: ExtractAudit::default(),
        })
    }

    fn get_writer(&mut self, ftype: &str, bucket: usize) -> &mut BucketFile {
        let key = (ftype.to_string(), bucket);
        self.writers.entry(key).or_insert_with(|| {
            let path = self.dir.join(format!("{}_{:03}.tsv", ftype, bucket));
            let file = File::create(&path).expect("cannot create spill file");
            BucketFile {
                writer: BufWriter::with_capacity(1 << 20, file),
            }
        })
    }

    fn bucket(&self, square: Square) -> usize {
        spill_key(square) as usize % self.num_buckets
    }

    /// Emit a linear segment (road, railway, barrier). Endpoints snap to grid
    /// here; length/bearing stay float (microsegment math, proven).
    pub fn emit_segment(
        &mut self,
        ftype: &FeatureType,
        square: Square,
        osm_id: i64,
        seg_idx: i16,
        seg: &([f64; 2], [f64; 2], f32),
        tags: &Tags,
    ) {
        let bucket = self.bucket(square);
        let name = ftype.name();

        let (sgx, sgy) = lonlat_to_grid(seg.0[1], seg.0[0]);
        let (egx, egy) = lonlat_to_grid(seg.1[1], seg.1[0]);

        // TSV: sq, osm_id, seg_idx, s_gx, s_gy, e_gx, e_gy, length_m, tags...
        let w = &mut self.get_writer(name, bucket).writer;
        let _ = write!(
            w,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
            spill_key(square),
            osm_id,
            seg_idx,
            sgx,
            sgy,
            egx,
            egy,
            seg.2
        );

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
                let _ = write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    classify::road_class(highway),
                    tags.get("maxspeed")
                        .map(|s| match classify::parse_maxspeed_kmh(s) {
                            // u8 column unchanged: `none` → sentinel 255,
                            // real limits clamp to 254 so they can't collide.
                            classify::MAXSPEED_NONE => classify::SPEED_LIMIT_DERESTRICTED,
                            v => v.min(254) as u8,
                        })
                        .unwrap_or(0),
                    classify::surface_type(surface),
                    if tags.get("oneway").map(|s| s.as_str()) == Some("yes") {
                        1
                    } else {
                        0
                    },
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
                );
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
                let tunnel = matches!(
                    tags.get("tunnel").map(|s| s.as_str()),
                    Some("yes" | "building_passage" | "culvert")
                );
                let _ = write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    classify::rail_type(railway),
                    classify::rail_usage_type(tags.get("usage").map(|s| s.as_str())),
                    // `none` is a road concept; on rail drop it so it falls to
                    // the same 0 = untagged → type-default speed downstream.
                    // u16 column (finalize.rs): 300+ km/h survives.
                    tags.get("maxspeed")
                        .map(|s| classify::parse_maxspeed_kmh(s))
                        .filter(|&v| v != classify::MAXSPEED_NONE)
                        .unwrap_or(0u16),
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
                );
            }
            FeatureType::Barrier => {
                // height_tier mirrors the structure-table ladder: 0 = mapped
                // height tag, 2 = the 3.0 m default (the merged structures.arrow
                // carries the tier per wall; the builder reads it from here).
                let mapped = tags.get("height").and_then(|s| parse_height(s));
                let _ = write!(
                    w,
                    "\t{}\t{}\t{}",
                    mapped.unwrap_or(3.0),
                    classify::barrier_material_type(tags.get("material").map(|s| s.as_str())),
                    if mapped.is_some() { 0 } else { 2 },
                );
            }
            FeatureType::AirportLine => {
                let heading = microsegment::bearing_deg(seg.0[0], seg.0[1], seg.1[0], seg.1[1]);
                let _ = write!(
                    w,
                    "\t{:.1}\t{}\t{}\t{}\t",
                    heading,
                    classify::aeroway_type(tags),
                    tsv_tag(tags, "ref"),
                    tsv_tag(tags, "surface"),
                );
                if let Some(v) = classify::parse_width_m(tags.get("width").map(|s| s.as_str())) {
                    let _ = write!(w, "{v:.1}");
                }
            }
            _ => {}
        }

        let _ = writeln!(w);
    }

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
        clat: f64,
        clon: f64,
        tags: &Tags,
        ring: Option<&[[f64; 2]]>,
    ) {
        let bucket = self.bucket(square);
        let name = ftype.name();

        // Classify the building ONCE before borrowing a writer, so observability can
        // record a silent residential-default without a second classification pass.
        let building_bt =
            matches!(ftype, FeatureType::Building).then(|| building_type_from_tags(tags));
        // A Building feature with NO `building` tag is a FUNCTIONAL AREA (mall /
        // hospital / school / zone routed by function). Flagged so finalize can
        // suppress it where it merely wraps real buildings (anti-double-count).
        let is_area_source = building_bt.is_some() && !tags.contains_key("building");
        if building_bt == Some(0) {
            if let Some(b) = tags.get("building").filter(|b| {
                !matches!(
                    b.as_str(),
                    "yes" | "" | "residential" | "apartments" | "dormitory"
                )
            }) {
                *self.audit.default_residential.entry(b.clone()).or_insert(0) += 1;
            }
        }

        let (cgx, cgy) = lonlat_to_grid(clon, clat);
        let snapped: Vec<(i32, i32)> = ring
            .map(|r| r.iter().map(|c| lonlat_to_grid(c[1], c[0])).collect())
            .unwrap_or_default();

        let w = &mut self.get_writer(name, bucket).writer;
        let _ = write!(w, "{}\t{}\t{}\t{}", spill_key(square), osm_id, cgx, cgy);

        match ftype {
            FeatureType::Building => {
                // Use amenity/shop/healthcare tags to override building type classification.
                // WHY: building=yes + amenity=school → type 3 (school), not 0 (residential).
                let bt = building_bt.unwrap();
                let _ = write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    bt,
                    tags.get("building:use")
                        .map(|s| building_use(s))
                        .unwrap_or(0),
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
                    is_area_source as u8,
                );
            }
            FeatureType::Leisure => {
                // No capacity column: the area-law unification dropped capacity
                // scaling, and this contract bump is the moment to delete it.
                let _ = write!(
                    w,
                    "\t{}\t{}\t{}",
                    classify::leisure_sport_class(tags),
                    classify::opening_hours_fraction(tags.get("opening_hours").map(|s| s.as_str())),
                    tsv_tag(tags, "name"),
                );
            }
            FeatureType::Industrial | FeatureType::WindTurbine => {
                let src_type: u8 = if matches!(ftype, FeatureType::WindTurbine) {
                    10
                }
                // wind_turbine
                else {
                    site_type_from_tags(tags)
                };
                let _ = write!(
                    w,
                    "\t{}\t{}\t{}\t{}\t{}",
                    src_type,
                    site_subtype_from_tags(tags),
                    tsv_tag(tags, "name"),
                    tags.get("height")
                        .and_then(|s| parse_height(s))
                        .unwrap_or(0.0),
                    parse_power_kw(tags.get("generator:output:electricity").map(|s| s.as_str())),
                );
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
                let _ = write!(
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
                );
            }
            _ => {}
        }

        // Snapped ring as grid text (empty when the feature is a point).
        let _ = write!(w, "\t{}", encode_ring_text(&snapped));

        let _ = writeln!(w);
    }

    /// Emit a function-POI node for the finalize footprint join. Format:
    /// `sq, gx, gy, class`. Consumed ONLY by finalize to reclassify the
    /// `building=yes` the POI sits inside; never a final arrow.
    /// `class` is the settlement class from [`poi_class`]; rows whose tags don't
    /// resolve are not emitted (caller checks first).
    pub fn emit_poi(&mut self, square: Square, clat: f64, clon: f64, class: u8) {
        let bucket = self.bucket(square);
        let (gx, gy) = lonlat_to_grid(clon, clat);
        let w = &mut self.get_writer(FeatureType::Poi.name(), bucket).writer;
        let _ = writeln!(w, "{}\t{}\t{}\t{}", spill_key(square), gx, gy, class);
    }

    pub fn flush_all(&mut self) -> Result<()> {
        for bf in self.writers.values_mut() {
            bf.writer.flush()?;
        }
        Ok(())
    }
}

/// Classify building type from all relevant OSM tags, not just building=*.
/// Priority: amenity/shop/healthcare/tourism > building tag.
/// WHY: OSM mappers often tag building=yes with function in amenity/shop.
///   Without this, a hospital tagged building=yes + amenity=hospital
///   would be classified as residential and get wrong emission profile.
///
/// `farm_auxiliary` co-tagged with a livestock signal is rescued from SILENT
/// back to farm (class 8) — many auxiliaries are sheds (→ SILENT) but a few are
/// barns. Function POIs reuse [`poi_class`] (shared with the finalize join).
fn building_type_from_tags(tags: &Tags) -> u8 {
    let get = |k: &str| tags.get(k).map(|s| s.as_str());
    // A SPECIFIC structural `building=*` (warehouse, stadium, train_station, …)
    // describes the whole envelope and BEATS an amenity POI tagged inside it: a
    // `building=warehouse` + `amenity=bar` is a warehouse with a staff bar, not a
    // 100 dB "restaurant" the size of the warehouse. Generic envelopes
    // (yes/commercial/retail/house/…) are NOT structural and defer to the POI
    // below, which gives them their function (a `building=yes` + `amenity=cafe`
    // IS a café). Fixes Strahov Stadium / airport terminals / breweries that
    // were stamped restaurant_bar on a 10⁵ m² footprint.
    if let Some(b) = get("building") {
        if building_overrides_poi(b) {
            return building_type(b);
        }
    }
    if let Some(c) = poi_class(
        get("amenity"),
        get("shop"),
        get("healthcare"),
        get("tourism"),
    ) {
        return c;
    }
    // farm_auxiliary + livestock → real farm building, not a silent shed.
    if get("building") == Some("farm_auxiliary")
        && (tags.contains_key("animal") || get("livestock").is_some())
    {
        return 8;
    }
    // Fallback to building tag
    tags.get("building").map(|s| building_type(s)).unwrap_or(0)
}

/// A `building=*` value naming a large / specific STRUCTURE whose emission is
/// set by the whole envelope, not by an `amenity` POI tagged inside it (a
/// stadium / warehouse / station / church with a bar is NOT a bar). Generic
/// envelopes (`yes`, `commercial`, `retail`, `office`, `residential`, `house`,
/// `apartments`) are deliberately ABSENT — they defer to the POI that gives them
/// their function.
fn building_overrides_poi(b: &str) -> bool {
    matches!(
        b,
        "warehouse"
            | "industrial"
            | "manufacture"
            | "stadium"
            | "grandstand"
            | "sports_hall"
            | "sports_centre"
            | "train_station"
            | "transportation"
            | "hospital"
            | "school"
            | "university"
            | "college"
            | "kindergarten"
            | "church"
            | "cathedral"
            | "chapel"
            | "mosque"
            | "synagogue"
            | "temple"
            | "monastery"
            | "hangar"
            | "farm"
            | "barn"
            | "stable"
            | "cowshed"
    )
}

/// The emission class implied by a function POI (`amenity`/`shop`/`healthcare`/
/// `tourism`), or `None` if none of the values are noise-relevant. Shared by the
/// way-level `building_type_from_tags` and the finalize POI-in-footprint join so
/// a standalone `amenity=hospital` node and a `building=yes + amenity=hospital`
/// way classify identically. Returns the settlement class u8.
/// [`poi_class`] resolved from a node/way `Tags` map.
pub fn poi_class_from_tags(tags: &Tags) -> Option<u8> {
    poi_class(
        tags.get("amenity").map(|s| s.as_str()),
        tags.get("shop").map(|s| s.as_str()),
        tags.get("healthcare").map(|s| s.as_str()),
        tags.get("tourism").map(|s| s.as_str()),
    )
}

pub fn poi_class(
    amenity: Option<&str>,
    shop: Option<&str>,
    healthcare: Option<&str>,
    tourism: Option<&str>,
) -> Option<u8> {
    if let Some(a) = amenity {
        match a {
            "school" | "kindergarten" | "university" | "college" | "library" => return Some(3),
            "hospital" | "clinic" | "doctors" | "dentist" | "pharmacy" => return Some(4),
            "place_of_worship" => return Some(5),
            "restaurant" | "bar" | "pub" | "cafe" | "fast_food" | "food_court" | "ice_cream"
            | "nightclub" => return Some(ids::SETTLEMENT_HOSPITALITY),
            "fuel" | "car_wash" => return Some(1), // small-commercial placeholder (PROP-MEAS)
            "parking" | "parking_space" => return Some(7),
            "fire_station" | "police" | "townhall" | "courthouse" | "post_office" => {
                return Some(9)
            }
            _ => {}
        }
    }
    if let Some(s) = shop {
        match s {
            "supermarket" | "convenience" | "wholesale" | "greengrocer" | "butcher" | "bakery"
            | "deli" | "frozen_food" | "mall" | "department_store" => {
                return Some(ids::SETTLEMENT_FOOD_RETAIL)
            }
            "" => {}
            _ => return Some(1), // any other shop = commercial
        }
    }
    if let Some(h) = healthcare {
        if matches!(h, "hospital" | "clinic") {
            return Some(4);
        }
    }
    if let Some(t) = tourism {
        if matches!(t, "hotel" | "hostel" | "motel" | "guest_house") {
            return Some(6);
        }
    }
    None
}

/// Map a raw `building=*` value to a settlement emission class (the u8s in
/// [`ids`], owned by the future `noise-compute` transfer). Single-family
/// houses → HOUSE (garden + heat pump), `building=supermarket` → FOOD_RETAIL,
/// restaurant/cafe buildings → HOSPITALITY, the silent tail (sheds, roofs,
/// huts — ~18 M objects that wrongly radiated residential) → SILENT.
/// `yes`/`""`/unknown stay 0 = residential-apartments.
fn building_type(val: &str) -> u8 {
    match val {
        // Apartments / generic residential keep type 0 (no garden term).
        "residential" | "apartments" | "dormitory" => 0,
        // Single-family houses get the HOUSE class (garden + heat pump).
        "house" | "detached" | "semidetached_house" | "semidetached" | "terrace" | "bungalow"
        | "houseboat" | "cabin" => ids::SETTLEMENT_HOUSE,
        // Retail buildings — malls, shops, supermarkets — are SHOPPING activity
        // (car park + deliveries + refrigeration), the FOOD_RETAIL profile, NOT
        // the office-HVAC commercial one.
        "supermarket" | "retail" => ids::SETTLEMENT_FOOD_RETAIL,
        "restaurant" | "cafe" | "pub" | "bar" | "fast_food" => ids::SETTLEMENT_HOSPITALITY,
        // Transport hubs: HVAC + concourse activity, commercial-grade plant.
        "commercial" | "office" | "kiosk" | "train_station" | "transportation" => 1,
        // data_center: 24/7 chiller plant, closer to a works than an office.
        "industrial" | "warehouse" | "manufacture" | "workshop" | "data_center" => 2,
        "school" | "university" | "college" | "kindergarten" | "education" => 3,
        "hospital" | "clinic" => 4,
        "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" | "monastery"
        | "religious" | "wayside_shrine" | "presbytery" | "shrine" => 5,
        "hotel" | "hostel" | "motel" => 6,
        "garage" | "garages" | "carport" | "parking" => 7,
        "farm" | "barn" | "stable" | "sty" | "cowshed" => 8,
        // Large semi-open sports structures: occasional crowds, mostly empty
        // concrete — a moderate public-grade level, NOT a footprint-scaled
        // restaurant. Civic / cultural / tourist public buildings — moderate
        // public-grade level.
        "public" | "civic" | "government" | "government_office" | "cultural" | "fire_station"
        | "castle" | "museum" | "hall" | "funeral_hall" | "historic" | "stadium" | "grandstand"
        | "sports_hall" | "sports_centre" => 9,
        // SILENT tail: uninhabited / unheated / infra structures.
        // `farm_auxiliary` is ambiguous (many are sheds) → SILENT unless the
        // livestock check above overrides it to farm.
        "shed" | "roof" | "hut" | "outbuilding" | "greenhouse" | "static_caravan"
        | "carport_roof" | "ruins" | "ruin" | "construction" | "collapsed" | "service"
        | "allotment_house" | "boathouse" | "bunker" | "tent" | "container" | "storage_tank"
        | "silo" | "hangar" | "conservatory" | "ger" | "farm_auxiliary" | "transformer_tower"
        | "water_tower" | "no" | "bridge" | "tower" | "toilets" | "elevator" | "tech_cab"
        | "guardhouse" | "gatehouse" | "pavilion" | "abandoned" | "stairs" | "staircase"
        | "chimney" | "demolished" | "forestry" | "signal_box" | "security_booth" | "shelter"
        | "proposed" | "ship" => ids::SETTLEMENT_SILENT,
        "yes" | "" => 0, // default to residential-apartments
        _ => 0,
    }
}

fn building_use(val: &str) -> u8 {
    match val {
        "residential" => 0,
        "commercial" | "retail" | "office" => 1,
        "industrial" => 2,
        _ => 0,
    }
}

fn site_type_from_tags(tags: &Tags) -> u8 {
    if let Some(lu) = tags.get("landuse") {
        match lu.as_str() {
            "industrial" => return 0,
            "quarry" => return 1,
            "farmyard" => return 2,
            _ => {}
        }
    }
    if let Some(mm) = tags.get("man_made") {
        match mm.as_str() {
            "works" => return 3,
            "wastewater_plant" => return 4,
            _ => {}
        }
    }
    0
}

/// Classify industrial site subtype from OSM `industrial=*` and `product=*` tags.
/// Values: 0 unknown (93 dB) · 1 warehouse (75) · 2 factory (95) · 3 mine (99) ·
/// 4 chemical (90) · 5 cement (100) · 6 metal (100) · 7 food (88) · 8 wood (90) ·
/// 9 waste (93) · 10 farm (70) · 11 office (60) · 12 port (92).
fn site_subtype_from_tags(tags: &Tags) -> u8 {
    // Check industrial=* tag (most specific)
    if let Some(ind) = tags.get("industrial") {
        match ind.as_str() {
            "warehouse" | "depot" | "logistics" => return 1,
            "factory" | "manufacturing" => return 2,
            "mine" | "quarry" | "open_pit" => return 3,
            "oil" | "chemical" | "refinery" | "gas" => return 4,
            "cement" | "glass" | "brickyard" | "ceramics" => return 5,
            "steelmaking" | "smelting" | "foundry" | "metal" | "aluminium" | "iron" => return 6,
            "brewery" | "winery" | "distillery" | "bakery" | "food" | "slaughterhouse"
            | "sugar" => return 7,
            "sawmill" | "timber" | "lumber" | "paper" | "pulp" | "woodworking" => return 8,
            "scrap_yard" | "recycling" | "waste" | "landfill" => return 9,
            "farm" | "agriculture" | "horticulture" | "greenhouse" => return 10,
            "port" | "shipyard" | "boatyard" | "dock" => return 12,
            _ => {}
        }
    }
    // Check product=* tag
    if let Some(prod) = tags.get("product") {
        let p = prod.to_lowercase();
        if p.contains("cement")
            || p.contains("concrete")
            || p.contains("brick")
            || p.contains("glass")
            || p.contains("ceramic")
            || p.contains("tile")
        {
            return 5;
        }
        if p.contains("steel")
            || p.contains("iron")
            || p.contains("alumin")
            || p.contains("copper")
            || p.contains("metal")
            || p.contains("zinc")
        {
            return 6;
        }
        if p.contains("chemical")
            || p.contains("petrol")
            || p.contains("oil")
            || p.contains("fuel")
            || p.contains("plastic")
            || p.contains("fertiliz")
        {
            return 4;
        }
        if p.contains("food")
            || p.contains("sugar")
            || p.contains("beer")
            || p.contains("wine")
            || p.contains("flour")
            || p.contains("dairy")
            || p.contains("meat")
        {
            return 7;
        }
        if p.contains("wood")
            || p.contains("paper")
            || p.contains("timber")
            || p.contains("lumber")
            || p.contains("pulp")
        {
            return 8;
        }
    }
    // NOTE: wastewater_plant intentionally NOT classified — it has a dedicated
    // source_type=4 profile (89 dB, 24/7) which is more accurate than waste subtype=9 (93 dB).
    if let Some(mm) = tags.get("man_made") {
        if mm.as_str() == "works" {
            return 2;
        }
    }
    if let Some(lu) = tags.get("landuse") {
        match lu.as_str() {
            "farmyard" | "farmland" => return 10,
            "quarry" => return 3,
            _ => {}
        }
    }
    if tags.get("office").is_some() {
        return 11;
    }
    0 // unknown — will use default 93 dB
}

/// OSM canonical form is a bare number or "N m" (metres). Strip at most ONE
/// metre suffix — `trim_end_matches` would strip repeatedly and turn
/// "2000mm" into 2000 metres; unsupported units now fail the parse and fall
/// back to the caller's default instead of misparsing.
fn parse_height(val: &str) -> Option<f32> {
    let val = val.trim_end();
    val.strip_suffix('m').unwrap_or(val).trim_end().parse().ok()
}

fn parse_power_kw(val: Option<&str>) -> f32 {
    let v = val.unwrap_or("0");
    if let Some(mw) = v.strip_suffix(" MW").or_else(|| v.strip_suffix("MW")) {
        mw.parse::<f32>().unwrap_or(0.0) * 1000.0
    } else if let Some(kw) = v.strip_suffix(" kW").or_else(|| v.strip_suffix("kW")) {
        kw.parse::<f32>().unwrap_or(0.0)
    } else if let Some(w) = v.strip_suffix(" W").or_else(|| v.strip_suffix("W")) {
        w.parse::<f32>().unwrap_or(0.0) / 1000.0
    } else {
        v.parse::<f32>().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod settlement_class_tests {
    use super::*;
    use crate::ids as st;

    #[test]
    fn building_type_splits_house_from_apartments() {
        assert_eq!(building_type("apartments"), 0);
        assert_eq!(building_type("residential"), 0);
        assert_eq!(building_type("yes"), 0);
        assert_eq!(building_type("house"), st::SETTLEMENT_HOUSE);
        assert_eq!(building_type("detached"), st::SETTLEMENT_HOUSE);
        assert_eq!(building_type("terrace"), st::SETTLEMENT_HOUSE);
    }

    #[test]
    fn building_type_food_retail_and_hospitality() {
        assert_eq!(building_type("supermarket"), st::SETTLEMENT_FOOD_RETAIL);
        assert_eq!(building_type("retail"), st::SETTLEMENT_FOOD_RETAIL);
        assert_eq!(building_type("restaurant"), st::SETTLEMENT_HOSPITALITY);
        assert_eq!(building_type("pub"), st::SETTLEMENT_HOSPITALITY);
        assert_eq!(building_type("commercial"), 1);
        assert_eq!(building_type("office"), 1);
    }

    #[test]
    fn silent_tail_routed_out_of_residential() {
        for v in [
            "shed",
            "roof",
            "hut",
            "greenhouse",
            "ruins",
            "construction",
            "carport_roof",
        ] {
            assert_eq!(
                building_type(v),
                st::SETTLEMENT_SILENT,
                "{v} must be SILENT"
            );
        }
        assert_eq!(building_type("some_unknown_value"), 0);
    }

    #[test]
    fn poi_class_priorities() {
        assert_eq!(poi_class(Some("hospital"), None, None, None), Some(4));
        assert_eq!(poi_class(Some("school"), None, None, None), Some(3));
        assert_eq!(
            poi_class(Some("cafe"), None, None, None),
            Some(st::SETTLEMENT_HOSPITALITY)
        );
        assert_eq!(
            poi_class(None, Some("supermarket"), None, None),
            Some(st::SETTLEMENT_FOOD_RETAIL)
        );
        assert_eq!(
            poi_class(None, Some("convenience"), None, None),
            Some(st::SETTLEMENT_FOOD_RETAIL)
        );
        assert_eq!(poi_class(None, Some("clothes"), None, None), Some(1));
        assert_eq!(poi_class(None, None, Some("hospital"), None), Some(4));
        assert_eq!(poi_class(None, None, None, Some("hotel")), Some(6));
        assert_eq!(poi_class(Some("bench"), None, None, None), None);
        assert_eq!(poi_class(None, None, None, None), None);
    }

    #[test]
    fn farm_auxiliary_silent_unless_livestock() {
        let mut shed = Tags::new();
        shed.insert("building".into(), "farm_auxiliary".into());
        assert_eq!(building_type_from_tags(&shed), st::SETTLEMENT_SILENT);
        let mut barn = Tags::new();
        barn.insert("building".into(), "farm_auxiliary".into());
        barn.insert("animal".into(), "cattle".into());
        assert_eq!(building_type_from_tags(&barn), 8);
    }

    #[test]
    fn building_type_from_tags_poi_overrides_yes() {
        let mut t = Tags::new();
        t.insert("building".into(), "yes".into());
        t.insert("amenity".into(), "hospital".into());
        assert_eq!(building_type_from_tags(&t), 4);
    }

    #[test]
    fn parse_height_strips_one_metre_suffix() {
        assert_eq!(parse_height("4 m"), Some(4.0));
        assert_eq!(parse_height("4m"), Some(4.0));
        assert_eq!(parse_height("4.5"), Some(4.5));
        assert_eq!(parse_height("tall"), None);
        assert_eq!(parse_height("2000mm"), None);
        assert_eq!(parse_height("6 ft"), None);
        assert_eq!(parse_height("10 m "), Some(10.0));
    }

    #[test]
    fn spill_key_roundtrip() {
        let sq = Square { x: 276, y: 173 };
        assert_eq!(spill_key(sq), 173 * 512 + 276);
        assert_eq!(square_from_spill_key(173 * 512 + 276), Some(sq));
        assert_eq!(square_from_spill_key(512 * 512), None);
    }

    #[test]
    fn ring_text_roundtrip() {
        let ring = vec![(100, 200), (300, 400), (500, 600)];
        let text = encode_ring_text(&ring);
        assert_eq!(parse_ring_text(&text), Some(ring));
        assert_eq!(parse_ring_text(""), None);
        assert_eq!(parse_ring_text("1,2"), None);
        assert_eq!(parse_ring_text("a,b;c,d;e,f"), None);
    }

    #[test]
    fn tsv_text_cannot_create_columns_or_records() {
        assert_eq!(TsvText("A\tB\r\nŽluťoučký").to_string(), "A B  Žluťoučký");
    }

    #[test]
    fn road_ref_falls_back_to_old_ref_only_when_empty() {
        let mut renumbered = Tags::new();
        renumbered.insert("highway".into(), "trunk".into());
        renumbered.insert("int_ref".into(), "E 55;E 65".into());
        renumbered.insert("old_ref".into(), "D1".into());
        assert_eq!(tsv_road_ref(&renumbered).to_string(), "D1");
        let mut current = Tags::new();
        current.insert("ref".into(), "D1".into());
        current.insert("old_ref".into(), "D0".into());
        assert_eq!(tsv_road_ref(&current).to_string(), "D1");
        let mut unmarked = Tags::new();
        unmarked.insert("highway".into(), "residential".into());
        assert_eq!(tsv_road_ref(&unmarked).to_string(), "");
        let mut empty_ref = Tags::new();
        empty_ref.insert("ref".into(), "".into());
        empty_ref.insert("old_ref".into(), "D1".into());
        assert_eq!(tsv_road_ref(&empty_ref).to_string(), "D1");
    }
}

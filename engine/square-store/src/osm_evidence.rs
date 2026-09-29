//! Strict readers for retained OSM tags, whole-way extents and control-point incidences.
//!
//! The second half reads the power/leisure evidence the W7 source models
//! consume: unit-bearing tag values (`plant:output:electricity`, `rating`,
//! `voltage`), indoor flags, leisure line identity, and the two per-square
//! spatial joins (transformers inside a substation polygon, raceway lines
//! inside a motorsport polygon). Every value has an untagged fallback, so a
//! missing column or an unparseable blob reads as untagged — never an error.

use crate::grid_cols::*;
use arrow::{array::Array, record_batch::RecordBatch};
use std::collections::BTreeMap;

pub struct ControlPoint<'a> {
    pub node_id: i64,
    pub grid: (i32, i32),
    pub way_id: Option<i64>,
    pub family: &'a str,
    pub vertex_index: Option<u32>,
    pub way_m: Option<f64>,
    pub tags_json: &'a str,
}

pub fn control_points(batch: &RecordBatch) -> Result<Vec<ControlPoint<'_>>, String> {
    crate::osm_contract::validate(&batch.schema(), "transport_nodes")?;
    let id = col_i64(batch, "osm_id").ok_or("missing control node id")?;
    let x = col_i32(batch, "gx").ok_or("missing control gx")?;
    let y = col_i32(batch, "gy").ok_or("missing control gy")?;
    let way = col_i64(batch, "way_id").ok_or("missing control way")?;
    let family = col_str(batch, "family").ok_or("missing control family")?;
    let vertex = col_u32(batch, "vertex_index").ok_or("missing control vertex")?;
    let metres = col_f64(batch, "way_m").ok_or("missing control metres")?;
    let tags = col_str(batch, "osm_tags").ok_or("missing control tags")?;
    if [
        id.null_count(),
        x.null_count(),
        y.null_count(),
        family.null_count(),
        tags.null_count(),
    ]
    .iter()
    .any(|n| *n != 0)
    {
        return Err("null control point identity, coordinates or tags".into());
    }
    (0..batch.num_rows())
        .map(|row| {
            let linked = !way.is_null(row);
            if linked == vertex.is_null(row)
                || (linked && !matches!(family.value(row), "roads" | "railways"))
                || (!linked && (!family.value(row).is_empty() || !metres.is_null(row)))
                || (!metres.is_null(row)
                    && (!metres.value(row).is_finite() || metres.value(row) < 0.0))
            {
                return Err("invalid control point incidence".into());
            }
            Ok(ControlPoint {
                node_id: id.value(row),
                grid: (x.value(row), y.value(row)),
                way_id: linked.then(|| way.value(row)),
                family: family.value(row),
                vertex_index: linked.then(|| vertex.value(row)),
                way_m: (!metres.is_null(row)).then(|| metres.value(row)),
                tags_json: tags.value(row),
            })
        })
        .collect()
}

pub fn tags<'a>(batch: &'a RecordBatch, family: &str, row: usize) -> Result<&'a str, String> {
    crate::osm_contract::validate(&batch.schema(), family)?;
    let tags = col_str(batch, "osm_tags").ok_or("missing retained OSM tags")?;
    if row >= tags.len() || tags.is_null(row) {
        return Err("missing retained OSM row".into());
    }
    Ok(tags.value(row))
}

/// Retained tags of one industrial/leisure row as a map. A missing `osm_tags`
/// column, a null row or an unparseable blob reads as untagged (empty): the
/// extractor always writes valid JSON, and every consumer below treats
/// untagged as a defined fallback, so one corrupt blob must not fail a popup.
pub fn optional_tags(batch: &RecordBatch, row: usize) -> BTreeMap<String, String> {
    let raw = col_str(batch, "osm_tags")
        .filter(|tags| row < tags.len() && !tags.is_null(row))
        .map(|tags| tags.value(row))
        .unwrap_or("{}");
    serde_json::from_str(raw).unwrap_or_default()
}

/// Parse `<number>[ <unit>]` with `default_scale` for a bare number
/// (`None` = a bare number carries no documented unit and is rejected).
/// The comma is a decimal separator (European mappers dominate power detail;
/// thousand separators are explicitly discouraged on OSM). A leading `2x`
/// counts repeated units (`2x630kVA` = 1260 kVA). Returns `None` for unknown
/// units, `yes`, empty text and non-finite/negative values.
fn scaled_value(text: &str, default_scale: Option<f64>, units: &[(&str, f64)]) -> Option<f64> {
    let compact = text.trim().replace(',', ".");
    let (repeat, compact) = match compact.split_once(['x', 'X']) {
        Some((count, rest)) if !count.trim().is_empty() && !rest.trim().is_empty() => {
            match count.trim().parse::<f64>() {
                Ok(count) if count.is_finite() && count > 0.0 => {
                    (count, rest.trim().to_string())
                }
                _ => return None,
            }
        }
        _ => (1.0, compact),
    };
    let split = compact
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E')))
        .unwrap_or(compact.len());
    let number: f64 = compact[..split].trim().parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = compact[split..].trim().to_ascii_lowercase();
    let scale = if unit.is_empty() {
        default_scale?
    } else {
        units
            .iter()
            .find(|(name, _)| unit == *name)
            .map(|(_, scale)| *scale)?
    };
    Some(number * scale * repeat)
}

/// Largest finite value of a `;`-separated tag list (`/` splits rating
/// alternatives too: `25/33 MVA` states ONAN/ONAF cooling stages).
fn max_list_value(text: &str, parse: impl Fn(&str) -> Option<f64>, slash: bool) -> Option<f64> {
    text.split(';')
        .flat_map(|part| {
            if slash {
                part.split('/').collect::<Vec<_>>()
            } else {
                vec![part]
            }
        })
        .filter_map(parse)
        .fold(None, |best: Option<f64>, value| {
            Some(best.map_or(value, |best| best.max(value)))
        })
}

/// Solar nameplate MW from `plant:output:electricity` (`4 MW`, `900 kW`,
/// `yes` when unknown). A bare number is MW
/// ([OSM wiki](https://wiki.openstreetmap.org/wiki/Wind_farms): "number MW").
/// Multi-values take the loudest, never the sum (a repeated value in two
/// units must not double-count).
pub fn plant_output_mw(tags: &BTreeMap<String, String>) -> Option<f64> {
    const UNITS: &[(&str, f64)] = &[
        ("w", 1e-6),
        ("kw", 1e-3),
        ("mw", 1.0),
        ("gw", 1e3),
        ("va", 1e-6),
        ("kva", 1e-3),
        ("mva", 1.0),
        ("gva", 1e3),
    ];
    max_list_value(
        tags.get("plant:output:electricity")?,
        |part| scaled_value(part, Some(1.0), UNITS),
        false,
    )
    .filter(|mw| *mw > 0.0)
}

/// Transformer nameplate MVA from `rating` (`450 MVA`, `50 kVA`). A bare
/// number has no documented unit and reads as unknown (the class median then
/// applies — a better guess than a unit gamble either way).
pub fn transformer_rating_mva(tags: &BTreeMap<String, String>) -> Option<f64> {
    const UNITS: &[(&str, f64)] = &[
        ("va", 1e-6),
        ("kva", 1e-3),
        ("mva", 1.0),
        ("gva", 1e3),
        ("w", 1e-6),
        ("kw", 1e-3),
        ("mw", 1.0),
        ("gw", 1e3),
    ];
    max_list_value(tags.get("rating")?, |part| scaled_value(part, None, UNITS), true)
        .filter(|mva| *mva > 0.0)
}

/// Highest substation voltage in kV from `voltage` (`400000;110000`,
/// `400 kV`). A bare number is volts (documented OSM practice).
pub fn max_voltage_kv(tags: &BTreeMap<String, String>) -> Option<f64> {
    const UNITS: &[(&str, f64)] = &[("v", 1e-3), ("kv", 1.0), ("mv", 1e3)];
    max_list_value(tags.get("voltage")?, |part| scaled_value(part, Some(1e-3), UNITS), false)
        .filter(|kv| *kv > 0.0)
}

/// True when the row's own `transformer=*` tag states auto-transformer
/// architecture. `transformer=auto` is deprecated in favour of
/// `windings:auto=yes`
/// ([wiki](https://wiki.openstreetmap.org/wiki/Tag:transformer%3Dauto)), which
/// the extractor does not retain — so most autotransformers stay invisible
/// and this test only catches the legacy tagging. See the open issue in the
/// integration report.
pub fn is_autotransformer_tag(tags: &BTreeMap<String, String>) -> bool {
    let value: String = tags
        .get("transformer")
        .map(|value| {
            value
                .to_ascii_lowercase()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect()
        })
        .unwrap_or_default();
    matches!(value.as_str(), "auto" | "autotransformer" | "autotransformator")
}

/// True when a leisure row is roofed: any `building=*` except an explicit
/// `building=no`, or any `indoor=*` except an explicit `indoor=no`. Such rows
/// stay silent — the building footprint carries the emission.
pub fn tags_indicate_indoor(tags: &BTreeMap<String, String>) -> bool {
    tags.get("building").is_some_and(|value| value != "no")
        || tags.get("indoor").is_some_and(|value| value != "no")
}

/// True when a leisure row is a raceway/track line (`geometry_kind = 2`).
/// A missing column reads as an area row (served v3 rows are all areas).
pub fn row_is_leisure_line(batch: &RecordBatch, row: usize) -> bool {
    col_u8(batch, "geometry_kind")
        .filter(|kinds| row < kinds.len() && !kinds.is_null(row))
        .is_some_and(|kinds| kinds.value(row) == 2)
}

/// One class-15 transformer row: centroid cell plus its parsed evidence.
pub struct TransformerUnit {
    pub gx: i32,
    pub gy: i32,
    pub rating_mva: Option<f64>,
    pub auto: bool,
    pub voltage_kv: Option<f64>,
}

/// Every transformer unit of the loaded squares' industrial batches (the
/// facility join reads this once, lazily, only when a substation row is
/// admitted). Rows without a centroid are skipped, never an error.
pub fn transformer_units(batches: &[RecordBatch]) -> Vec<TransformerUnit> {
    let mut units = Vec::new();
    for batch in batches {
        let (Some(kinds), Some(cgx), Some(cgy)) = (
            col_u8(batch, "source_type"),
            col_i32(batch, "centroid_gx"),
            col_i32(batch, "centroid_gy"),
        ) else {
            continue;
        };
        for row in 0..batch.num_rows() {
            if kinds.is_null(row) || kinds.value(row) != 15 {
                continue;
            }
            if cgx.is_null(row) || cgy.is_null(row) {
                continue;
            }
            let tags = optional_tags(batch, row);
            units.push(TransformerUnit {
                gx: cgx.value(row),
                gy: cgy.value(row),
                rating_mva: transformer_rating_mva(&tags),
                auto: is_autotransformer_tag(&tags),
                voltage_kv: max_voltage_kv(&tags),
            });
        }
    }
    units
}

/// What a substation polygon contains: summed transformer ratings plus the
/// classification evidence (an autotransformer unit, the highest contained
/// voltage). Containment is centroid-in-polygon on the z30 grid.
pub struct SubstationFeed {
    pub rated_mva_sum: f64,
    pub rated_count: usize,
    pub has_autotransformer: bool,
    pub max_contained_voltage_kv: Option<f64>,
}

/// True when a substation row is a GAS-network station, not electrical
/// plant: it carries no transformer hum and stays silent until a gas model
/// claims it. Gas values on the 2026-08 planet (`gas` 185, `valve` 8,
/// `compression` 3, `valve_group` 1): rare, but each would otherwise hum.
/// Electricity values (`transmission`, `distribution`, `converter`, …) emit.
pub fn is_gas_substation(tags: &BTreeMap<String, String>) -> bool {
    matches!(
        tags.get("substation").map(String::as_str),
        Some("gas" | "valve_group" | "valve" | "compression")
    )
}

/// Substation MVA plus its fallback class from the row's own tags and its
/// facility's joined transformer feed. The MVA is the joined `rating` sum
/// when at least one transformer in ANY part of the facility carries one;
/// else the row's own `rating` (the full facility nameplate, identical on
/// every part — the only evidence a node substation can carry); else the
/// class median applies. One rating truth — tag or joined units, never
/// both. An autotransformer unit (own tag or contained) makes it auto (2,
/// architecture rather than class); else the retained `substation` tag
/// decides: transmission is main (1), minor_distribution at distribution
/// voltage (below 60 kV — MV/LV kiosks, not the rare mistagged 115 kV
/// stations) is minor (4); else ≥ 220 kV highest voltage makes it main (1);
/// else distribution (3). Classes are shared by convention with
/// `noise-compute::emission::industrial::substation_class_mva`, like the
/// `source_type` ids the extractor writes as raw numbers.
pub fn substation_power(
    own_tags: &BTreeMap<String, String>,
    feed: &SubstationFeed,
) -> (Option<f64>, u8) {
    let mva = (feed.rated_count > 0 && feed.rated_mva_sum > 0.0)
        .then_some(feed.rated_mva_sum)
        .or_else(|| transformer_rating_mva(own_tags));
    let auto = is_autotransformer_tag(own_tags) || feed.has_autotransformer;
    let kv = max_voltage_kv(own_tags)
        .into_iter()
        .chain(feed.max_contained_voltage_kv)
        .fold(0.0f64, f64::max);
    let tag: Vec<String> = own_tags
        .get("substation")
        .map(|value| {
            value
                .split(';')
                .map(|token| token.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let class = if auto {
        2
    } else if tag.iter().any(|token| token == "transmission") {
        1
    } else if tag.iter().any(|token| token == "minor_distribution") && kv < 60.0 {
        4
    } else if kv >= 220.0 {
        1
    } else {
        3
    };
    (mva, class)
}

/// Builder-computed area fraction of one multipolygon facility part: the
/// extractor writes it on every closed part of a multi-part industrial
/// relation, so readers evaluate the facility's sound power once and share
/// it by area. Not an OSM tag — the `qm:` namespace marks extractor
/// measurement, never mapper input.
pub const QM_FACILITY_SHARE: &str = "qm:facility_share";

/// One part's area share of its facility's single acoustic power: the stored
/// fraction when it parses inside (0, 1], else the whole facility (single
/// parts and pre-share extracts carry no key; garbage must not silence or
/// NaN a source).
pub fn facility_share(tags: &BTreeMap<String, String>) -> f64 {
    tags.get(QM_FACILITY_SHARE)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|share| *share > 0.0 && *share <= 1.0)
        .unwrap_or(1.0)
}

/// Join the loaded squares' transformers against a whole substation
/// facility: a unit inside ANY part polygon counts once toward the
/// facility's single rating truth. An overlap between parts still counts
/// each unit once (one containment pass, not one sum per part).
pub fn substation_facility_feed(
    units: &[TransformerUnit],
    parts: &[grid::poly::PreparedRing],
) -> SubstationFeed {
    let mut feed = SubstationFeed {
        rated_mva_sum: 0.0,
        rated_count: 0,
        has_autotransformer: false,
        max_contained_voltage_kv: None,
    };
    for unit in units {
        if !parts.iter().any(|part| part.contains(unit.gx, unit.gy)) {
            continue;
        }
        if let Some(rating) = unit.rating_mva {
            feed.rated_mva_sum += rating;
            feed.rated_count += 1;
        }
        feed.has_autotransformer |= unit.auto;
        feed.max_contained_voltage_kv = feed
            .max_contained_voltage_kv
            .into_iter()
            .chain(unit.voltage_kv)
            .fold(None, |best: Option<f64>, value| {
                Some(best.map_or(value, |best| best.max(value)))
            });
    }
    feed
}

/// Join the square's transformers against one substation polygon. A point
/// substation (no polygon) contains nothing and falls back to its own tags.
pub fn substation_feed(units: &[TransformerUnit], polygon: &[(i32, i32)]) -> SubstationFeed {
    let prepared = grid::poly::PreparedRing::new(polygon);
    substation_facility_feed(units, prepared.as_slice())
}

/// Substation facility parts of loaded industrial batches, keyed by (OSM
/// kind, OSM id): every closed part of a multipolygon relation shares its
/// relation id, so the facility's transformer feed is the union over its
/// parts while each part emits its [`QM_FACILITY_SHARE`] of the one
/// facility power. Single-part rows group alone; node rows (no polygon)
/// contribute no parts and read their own tags. Built once per
/// query/scene, lazily, only when a substation row is admitted. Rows
/// without a decodable ring or id are skipped, never an error.
#[derive(Default)]
pub struct SubstationFacilities {
    parts: BTreeMap<(String, i64), Vec<grid::poly::PreparedRing>>,
}

impl SubstationFacilities {
    pub fn build(batches: &[RecordBatch]) -> Self {
        let mut facilities = Self::default();
        facilities.extend(batches);
        facilities
    }

    pub fn extend(&mut self, batches: &[RecordBatch]) {
        for batch in batches {
            let (Some(kinds), Some(geom)) =
                (col_u8(batch, "source_type"), col_binary(batch, "geom"))
            else {
                continue;
            };
            let ids = col_i64(batch, "osm_id");
            let obj_kinds = col_str(batch, "osm_kind");
            for row in 0..batch.num_rows() {
                if kinds.is_null(row) || kinds.value(row) != 14 {
                    continue;
                }
                let id = ids
                    .filter(|ids| !ids.is_null(row))
                    .map(|ids| ids.value(row))
                    .unwrap_or(0);
                if id == 0 || geom.is_null(row) {
                    continue;
                }
                let kind = obj_kinds
                    .filter(|kinds| !kinds.is_null(row))
                    .map(|kinds| kinds.value(row))
                    .unwrap_or("");
                let Some(prepared) = decode_geom(Some(geom.value(row)))
                    .and_then(|ring| grid::poly::PreparedRing::new(&ring))
                else {
                    continue;
                };
                self.parts
                    .entry((kind.to_string(), id))
                    .or_default()
                    .push(prepared);
            }
        }
    }

    /// The transformer feed for one substation row: the union over its
    /// facility's indexed parts, else the row's own polygon (node rows and
    /// rows from batches outside the index read their own tags).
    pub fn feed(
        &self,
        kind: &str,
        osm_id: i64,
        own_polygon: &[(i32, i32)],
        units: &[TransformerUnit],
    ) -> SubstationFeed {
        match self.parts.get(&(kind.to_string(), osm_id)) {
            Some(parts) => substation_facility_feed(units, parts),
            None => substation_feed(units, own_polygon),
        }
    }
}

/// First and last grid cell of a leisure chain (`None` under two points).
pub fn chain_ends(chain: &[(i32, i32)]) -> Option<((i32, i32), (i32, i32))> {
    (chain.len() >= 2).then(|| (chain[0], chain[chain.len() - 1]))
}

/// Chain length in metres: the windowed `flat_dist` sum the extractor stores
/// as `length_m` (`write_leisure.rs`), reused when the column is absent.
pub fn leisure_chain_length_m(chain: &[(i32, i32)]) -> f64 {
    chain
        .windows(2)
        .map(|pair| {
            let a = grid_cell_lonlat(pair[0].0, pair[0].1);
            let b = grid_cell_lonlat(pair[1].0, pair[1].1);
            grid::geo::flat_dist(a.1, a.0, b.1, b.0)
        })
        .sum()
}

/// Measurable length of a leisure line row in metres: the stored `length_m`
/// when finite and positive, else the chain sum (0 when neither measures —
/// such rows keep one full total, current behaviour).
pub fn leisure_line_length_m(stored_m: Option<f32>, chain: &[(i32, i32)]) -> f64 {
    stored_m
        .filter(|length| length.is_finite() && *length > 0.0)
        .map(f64::from)
        .unwrap_or_else(|| leisure_chain_length_m(chain))
}

/// One class-10 raceway line row of a square: centroid (polygon silence +
/// containment grouping), chain ends (connectivity grouping) and measurable
/// length (the energy share). A line without a centroid still groups by its
/// ends; a line without ends or length stays a venue of its own.
struct VenueLine {
    centroid: Option<(i32, i32)>,
    ends: Option<((i32, i32), (i32, i32))>,
    length_m: f64,
}

/// Motorsport venues of one square's leisure batches: each raceway line's
/// share of its venue's single formula total. A circuit stored as N ways is
/// one venue — fragments that touch (shared chain endpoint) or sit in one
/// class-10 polygon share the total by chain length instead of each radiating
/// it (+10·lg N). Lines are contained by centroid: a raceway inside its
/// facility polygon always qualifies; a line merely crossing one keeps both
/// emitting. Read once per square, lazily, only when a class-10 row is
/// admitted.
#[derive(Default)]
pub struct MotorsportVenues {
    lines: Vec<VenueLine>,
    share: Vec<f64>,
}

impl MotorsportVenues {
    /// Index every class-10 line row of the square and union fragments into
    /// venues: shared chain endpoints, then one containing polygon.
    pub fn build(batches: &[RecordBatch]) -> Self {
        let mut lines = Vec::new();
        let mut polygons = Vec::new();
        for batch in batches {
            // Without the sport column no row can join; every other absent
            // column degrades per row below (unmeasurable lines stay solo).
            if col_u8(batch, "sport").is_none() {
                continue;
            }
            Self::collect_batch(batch, &mut lines, &mut polygons);
        }
        Self::union_and_share(lines, &polygons)
    }

    fn collect_batch(
        batch: &RecordBatch,
        lines: &mut Vec<VenueLine>,
        polygons: &mut Vec<grid::poly::PreparedRing>,
    ) {
        let sports = col_u8(batch, "sport").expect("checked by caller");
        let cgx = col_i32(batch, "centroid_gx");
        let cgy = col_i32(batch, "centroid_gy");
        let geom = col_binary(batch, "geom");
        let length = col_f32(batch, "length_m");
        for row in 0..batch.num_rows() {
            if sports.is_null(row) || sports.value(row) != 10 {
                continue;
            }
            let chain = geom
                .filter(|g| !g.is_null(row))
                .and_then(|g| decode_geom(Some(g.value(row))))
                .unwrap_or_default();
            if !row_is_leisure_line(batch, row) {
                if let Some(prepared) = grid::poly::PreparedRing::new(&chain) {
                    polygons.push(prepared);
                }
                continue;
            }
            let centroid = match (cgx, cgy) {
                (Some(x), Some(y)) if !x.is_null(row) && !y.is_null(row) => {
                    Some((x.value(row), y.value(row)))
                }
                _ => None,
            };
            let stored = length
                .filter(|l| !l.is_null(row))
                .map(|l| l.value(row));
            lines.push(VenueLine {
                centroid,
                ends: chain_ends(&chain),
                length_m: leisure_line_length_m(stored, &chain),
            });
        }
    }

    /// Union measurable fragments into venues and precompute each line's
    /// length share of its venue total (1.0 when ungrouped).
    fn union_and_share(lines: Vec<VenueLine>, polygons: &[grid::poly::PreparedRing]) -> Self {
        let mut parent: Vec<usize> = (0..lines.len()).collect();
        fn find(parent: &mut [usize], mut line: usize) -> usize {
            while parent[line] != line {
                parent[line] = parent[parent[line]];
                line = parent[line];
            }
            line
        }
        fn union(parent: &mut [usize], a: usize, b: usize) {
            let root = find(parent, a.min(b));
            let other = find(parent, a.max(b));
            parent[other] = root;
        }
        // Touching chains are one circuit (shared endpoint cells).
        let mut ends: BTreeMap<(i32, i32), usize> = BTreeMap::new();
        for (index, line) in lines.iter().enumerate() {
            if line.length_m <= 0.0 {
                continue;
            }
            let Some((first, last)) = line.ends else {
                continue;
            };
            for end in [first, last] {
                if let Some(seen) = ends.insert(end, index) {
                    union(&mut parent, seen, index);
                }
            }
        }
        // Fragments in one facility polygon are one venue even apart.
        for polygon in polygons {
            let mut contained: Option<usize> = None;
            for (index, line) in lines.iter().enumerate() {
                if line.length_m <= 0.0 {
                    continue;
                }
                let Some((gx, gy)) = line.centroid else {
                    continue;
                };
                if !polygon.contains(gx, gy) {
                    continue;
                }
                if let Some(first) = contained {
                    union(&mut parent, first, index);
                } else {
                    contained = Some(index);
                }
            }
        }
        let mut total = vec![0.0; lines.len()];
        for (index, line) in lines.iter().enumerate() {
            total[find(&mut parent, index)] += line.length_m;
        }
        let share = lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let venue = total[find(&mut parent, index)];
                if line.length_m > 0.0 && venue > 0.0 {
                    (line.length_m / venue).min(1.0)
                } else {
                    1.0
                }
            })
            .collect();
        Self { lines, share }
    }

    /// Energy share of one venue total for a line with these chain ends and
    /// measurable length: its length over its venue's total chain length.
    /// Unknown or ungrouped lines keep 1.0 (current behaviour). Ends identify
    /// the venue — chains sharing both ends are unioned, so any twin resolves
    /// to the same group.
    pub fn line_share(&self, ends: Option<((i32, i32), (i32, i32))>, length_m: f64) -> f64 {
        if length_m <= 0.0 || !length_m.is_finite() {
            return 1.0;
        }
        let Some(ends) = ends else {
            return 1.0;
        };
        self.lines
            .iter()
            .position(|line| line.ends == Some(ends) && line.length_m > 0.0)
            .map(|index| self.share[index])
            .unwrap_or(1.0)
    }

    /// True when a motorsport polygon encloses a raceway line — the polygon
    /// then goes silent and the lines carry the emission.
    pub fn encloses_line(&self, polygon: &[(i32, i32)]) -> bool {
        let Some(prepared) = grid::poly::PreparedRing::new(polygon) else {
            return false;
        };
        self.lines.iter().any(|line| {
            line.centroid
                .is_some_and(|(gx, gy)| prepared.contains(gx, gy))
        })
    }
}

/// True when a class-13 row's tags claim the plant itself (`power=plant`);
/// anything else class-13 (generators, untyped solar) is a silence candidate.
pub fn tags_is_solar_plant(tags: &BTreeMap<String, String>) -> bool {
    tags.get("power").map(String::as_str) == Some("plant")
}

/// Every class-13 solar plant polygon of the loaded squares' industrial
/// batches, prepared for containment queries (the generator-silence join
/// reads this once, lazily, only when a class-13 non-plant row is admitted).
/// Rows without a decodable ring are skipped, never an error.
pub fn solar_plants(batches: &[RecordBatch]) -> Vec<grid::poly::PreparedRing> {
    let mut plants = Vec::new();
    for batch in batches {
        let (Some(kinds), Some(geom)) = (
            col_u8(batch, "source_type"),
            col_binary(batch, "geom"),
        ) else {
            continue;
        };
        for row in 0..batch.num_rows() {
            if kinds.is_null(row) || kinds.value(row) != 13 {
                continue;
            }
            if !tags_is_solar_plant(&optional_tags(batch, row)) {
                continue;
            }
            if geom.is_null(row) {
                continue;
            }
            if let Some(prepared) =
                decode_geom(Some(geom.value(row))).and_then(|ring| grid::poly::PreparedRing::new(&ring))
            {
                plants.push(prepared);
            }
        }
    }
    plants
}

/// True when a class-13 generator row's centroid falls inside a solar plant
/// polygon — the plant (nameplate, else its own area × density) owns the
/// emission and the contained generator stays silent, as a contained
/// transformer defers to its substation.
pub fn inside_solar_plant(plants: &[grid::poly::PreparedRing], gx: i32, gy: i32) -> bool {
    plants.iter().any(|plant| plant.contains(gx, gy))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn facility_share_defaults_to_whole_and_rejects_garbage() {
        assert_eq!(facility_share(&tags(&[])), 1.0);
        assert_eq!(facility_share(&tags(&[(QM_FACILITY_SHARE, "0.5")])), 0.5);
        assert_eq!(facility_share(&tags(&[(QM_FACILITY_SHARE, "1")])), 1.0);
        for garbage in ["0", "-0.5", "1.5", "abc", "", "NaN", "inf"] {
            assert_eq!(
                facility_share(&tags(&[(QM_FACILITY_SHARE, garbage)])),
                1.0,
                "{garbage:?}"
            );
        }
    }

    #[test]
    fn facility_feed_unions_rated_units_across_parts_once() {
        use arrow::array::{
            ArrayRef, BinaryArray, Int32Array, Int64Array, StringArray, UInt8Array,
        };
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;
        // Two parts of relation 7 (plus an unrelated substation way and a
        // transformer row): the 100 MVA unit in part A is the facility
        // truth for BOTH parts, counted once even where parts overlap.
        let ring = |points: &[(i32, i32)]| grid::poly::encode_grid_poly(points);
        let part_a = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        let part_b = ring(&[(50, 50), (150, 50), (150, 150), (50, 150)]);
        let other = ring(&[(5000, 5000), (5100, 5000), (5100, 5100), (5000, 5100)]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("source_type", DataType::UInt8, false),
            Field::new("osm_id", DataType::Int64, false),
            Field::new("osm_kind", DataType::Utf8, true),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("geom", DataType::Binary, true),
            Field::new("osm_tags", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(UInt8Array::from(vec![14, 14, 14, 15])) as ArrayRef,
                Arc::new(Int64Array::from(vec![7, 7, 9, 11])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some("relation"),
                    Some("relation"),
                    Some("way"),
                    Some("node"),
                ])) as ArrayRef,
                Arc::new(Int32Array::from(vec![50, 100, 5050, 70])) as ArrayRef,
                Arc::new(Int32Array::from(vec![50, 100, 5050, 60])) as ArrayRef,
                Arc::new(BinaryArray::from(vec![
                    Some(part_a.as_slice()),
                    Some(part_b.as_slice()),
                    Some(other.as_slice()),
                    None,
                ])) as ArrayRef,
                Arc::new(StringArray::from(vec![
                    Some(r#"{"rating":"100 MVA"}"#),
                    Some(r#"{"rating":"100 MVA"}"#),
                    Some(r#"{"rating":"10 MVA"}"#),
                    Some(r#"{"rating":"100 MVA"}"#),
                ])) as ArrayRef,
            ],
        )
        .unwrap();
        let batches = std::slice::from_ref(&batch);
        let facilities = SubstationFacilities::build(batches);
        let units = transformer_units(batches);
        assert_eq!(units.len(), 1);
        // The unit at (70, 60) sits in the parts' overlap: one count.
        let feed_a = facilities.feed("relation", 7, &[], &units);
        assert_eq!(feed_a.rated_count, 1);
        assert_eq!(feed_a.rated_mva_sum, 100.0);
        // Part B reads the same union (its own polygon is empty here, so a
        // per-part feed would find nothing): one truth, never tag + units.
        let feed_b = facilities.feed("relation", 7, &[], &units);
        assert_eq!((feed_b.rated_mva_sum, feed_b.rated_count), (100.0, 1));
        let (mva, _) = substation_power(&tags(&[("rating", "100 MVA")]), &feed_b);
        assert_eq!(mva, Some(100.0));
        // The unrelated way groups alone and reads its own tags.
        let feed_other = facilities.feed(
            "way",
            9,
            &[(5000, 5000), (5100, 5000), (5100, 5100), (5000, 5100)],
            &units,
        );
        assert_eq!(feed_other.rated_count, 0);
        // Unknown rows fall back to their own polygon, not an empty union.
        let feed_fallback =
            facilities.feed("way", 404, &[(0, 0), (100, 0), (100, 100), (0, 100)], &units);
        assert_eq!(feed_fallback.rated_count, 1);
    }

    #[test]
    fn power_values_parse_units_lists_and_bare_numbers() {
        let output = |value: &str| plant_output_mw(&tags(&[("plant:output:electricity", value)]));
        assert_eq!(output("4 MW"), Some(4.0));
        assert_eq!(output("2.112MW"), Some(2.112));
        assert_eq!(output("900 kW"), Some(0.9));
        assert_eq!(output("1.2 GW"), Some(1200.0));
        assert_eq!(output("5000000 W"), Some(5.0));
        assert_eq!(output("2,5 MW"), Some(2.5));
        assert_eq!(output("4"), Some(4.0)); // bare number is MW (wiki)
        assert_eq!(output("4 MW;4000 kW"), Some(4.0)); // loudest, never summed
        assert_eq!(output("2 MW;900 kW"), Some(2.0));
        for unknown in ["", "yes", "unknown", "4 TW", "4MMW", "-3 MW", "nan MW"] {
            assert_eq!(output(unknown), None, "{unknown:?}");
        }
        let rating = |value: &str| transformer_rating_mva(&tags(&[("rating", value)]));
        assert_eq!(rating("450 MVA"), Some(450.0));
        assert_eq!(rating("50 kVA"), Some(0.05));
        assert_eq!(rating("630kVA"), Some(0.63));
        assert_eq!(rating("25/33 MVA"), Some(33.0)); // ONAN/ONAF cooling stages
        assert_eq!(rating("16 MVA; 25 MVA"), Some(25.0));
        assert_eq!(rating("2x630kVA"), Some(1.26)); // repeated units
        assert_eq!(rating("2 X 1 MVA"), Some(2.0));
        assert_eq!(rating("630"), None); // bare number: no documented unit
        assert_eq!(rating("yes"), None);
        let voltage = |value: &str| max_voltage_kv(&tags(&[("voltage", value)]));
        assert_eq!(voltage("400000"), Some(400.0)); // bare number is volts
        assert_eq!(voltage("400000;110000"), Some(400.0));
        assert_eq!(voltage("400 kV"), Some(400.0));
        assert_eq!(voltage("22kV"), Some(22.0));
        assert_eq!(voltage("high"), None);
    }

    #[test]
    fn substation_power_prefers_join_then_own_rating_then_median() {
        let feed = |sum: f64, count: usize| SubstationFeed {
            rated_mva_sum: sum,
            rated_count: count,
            has_autotransformer: false,
            max_contained_voltage_kv: None,
        };
        // Joined ratings win over everything, with the class still derived.
        let (mva, class) = substation_power(
            &tags(&[("voltage", "400000"), ("rating", "126 MVA")]),
            &feed(65.0, 2),
        );
        assert_eq!((mva, class), (Some(65.0), 1));
        // No join: the row's own rating (a node station's only evidence).
        let (mva, class) = substation_power(&tags(&[("rating", "450 MVA")]), &feed(0.0, 0));
        assert_eq!((mva, class), (Some(450.0), 3));
        // Neither: the voltage class median path (main at 220 kV).
        let (mva, class) = substation_power(&tags(&[("voltage", "220000")]), &feed(0.0, 0));
        assert_eq!((mva, class), (None, 1));
        // An autotransformer anywhere makes it auto, even at 400 kV.
        let (mva, class) = substation_power(
            &tags(&[("voltage", "400000")]),
            &SubstationFeed {
                rated_mva_sum: 0.0,
                rated_count: 0,
                has_autotransformer: true,
                max_contained_voltage_kv: None,
            },
        );
        assert_eq!((mva, class), (None, 2));
        // Untagged: distribution.
        assert_eq!(substation_power(&tags(&[]), &feed(0.0, 0)), (None, 3));
    }

    #[test]
    fn substation_class_comes_from_the_substation_tag() {
        // 45,534 of 86,804 substation rows over 120 canary reach squares are
        // minor_distribution kiosks; 45,425 carry no rating, so they took the
        // 2 MVA distribution median — +7 to +14 dB over plausible kiosk MVA
        // (the 109 explicitly rated ones run 100 kVA–2 MVA, mode 400 kVA).
        let feed = |kv: Option<f64>| SubstationFeed {
            rated_mva_sum: 0.0,
            rated_count: 0,
            has_autotransformer: false,
            max_contained_voltage_kv: kv,
        };
        let minor = tags(&[("substation", "minor_distribution")]);
        assert_eq!(substation_power(&minor, &feed(None)), (None, 4));
        let minor_mv = tags(&[("substation", "minor_distribution"), ("voltage", "20000;400")]);
        assert_eq!(substation_power(&minor_mv, &feed(None)), (None, 4));
        // A 115 kV "minor_distribution" is a mistagged sub-transmission
        // station (41 in the canary), not a kiosk: voltage rules still apply.
        let minor_hv = tags(&[("substation", "minor_distribution"), ("voltage", "115000")]);
        assert_eq!(substation_power(&minor_hv, &feed(None)), (None, 3));
        // An explicit transmission tag wins over sub-220 kV voltage.
        let tx = tags(&[("substation", "transmission"), ("voltage", "110000")]);
        assert_eq!(substation_power(&tx, &feed(None)), (None, 1));
        // Multi-values: transmission wins; mixed minor stays minor.
        let mixed = tags(&[("substation", "transmission;distribution")]);
        assert_eq!(substation_power(&mixed, &feed(None)), (None, 1));
        let mixed_minor = tags(&[("substation", "yes;minor_distribution")]);
        assert_eq!(substation_power(&mixed_minor, &feed(None)), (None, 4));
        // Autotransformer architecture still wins over the tag.
        assert_eq!(
            substation_power(
                &tags(&[("substation", "transmission"), ("transformer", "auto")]),
                &feed(None)
            ),
            (None, 2)
        );
    }

    #[test]
    fn gas_substations_are_not_electrical_plant() {
        assert!(is_gas_substation(&tags(&[("substation", "gas")])));
        assert!(is_gas_substation(&tags(&[("substation", "valve_group")])));
        assert!(is_gas_substation(&tags(&[("substation", "compression")])));
        assert!(!is_gas_substation(&tags(&[("substation", "transmission")])));
        assert!(!is_gas_substation(&tags(&[("substation", "minor_distribution")])));
        assert!(!is_gas_substation(&tags(&[])));
    }

    #[test]
    fn autotransformer_indoor_and_line_flags() {
        assert!(is_autotransformer_tag(&tags(&[("transformer", "auto")])));
        assert!(is_autotransformer_tag(&tags(&[("transformer", "autotransformer")])));
        assert!(!is_autotransformer_tag(&tags(&[("transformer", "distribution")])));
        assert!(!is_autotransformer_tag(&tags(&[])));
        assert!(tags_indicate_indoor(&tags(&[("building", "yes")])));
        assert!(tags_indicate_indoor(&tags(&[("indoor", "yes")])));
        assert!(tags_indicate_indoor(&tags(&[("indoor", "room")])));
        assert!(!tags_indicate_indoor(&tags(&[("building", "no")])));
        assert!(!tags_indicate_indoor(&tags(&[("indoor", "no")])));
        assert!(!tags_indicate_indoor(&tags(&[])));
    }

    #[test]
    fn contained_solar_generators_defer_to_their_plant() {
        use arrow::array::{ArrayRef, BinaryArray, Int32Array, StringArray, UInt8Array};
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;
        let ring = grid::poly::encode_grid_poly(&[(0, 0), (1000, 0), (1000, 1000), (0, 1000)]);
        let plant_tags = r#"{"power":"plant","plant:source":"solar"}"#;
        let gen_tags = r#"{"power":"generator","generator:source":"solar"}"#;
        let schema = Arc::new(Schema::new(vec![
            Field::new("source_type", DataType::UInt8, false),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("geom", DataType::Binary, true),
            Field::new("osm_tags", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(UInt8Array::from(vec![13, 13, 13, 14])) as ArrayRef,
                Arc::new(Int32Array::from(vec![500, 500, 5000, 500])) as ArrayRef,
                Arc::new(Int32Array::from(vec![500, 500, 5000, 500])) as ArrayRef,
                Arc::new(BinaryArray::from(vec![Some(ring.as_slice()), None, None, None]))
                    as ArrayRef,
                Arc::new(StringArray::from(vec![plant_tags, gen_tags, gen_tags, "{}"]))
                    as ArrayRef,
            ],
        )
        .unwrap();
        // Only the `power=plant` ring indexes; the substation row is ignored.
        let plants = solar_plants(std::slice::from_ref(&batch));
        assert_eq!(plants.len(), 1);
        assert!(tags_is_solar_plant(&optional_tags(&batch, 0)));
        assert!(!tags_is_solar_plant(&optional_tags(&batch, 1)));
        // The generator inside the plant defers; the one outside still emits.
        assert!(inside_solar_plant(&plants, 500, 500));
        assert!(!inside_solar_plant(&plants, 5000, 5000));
    }

    #[test]
    fn raceway_fragments_share_one_venue_total_by_length() {
        use arrow::array::{ArrayRef, BinaryArray, Float32Array, Int32Array, UInt8Array};
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;
        let ring = |points: &[(i32, i32)]| grid::poly::encode_grid_poly(points);
        let facility =
            ring(&[(-1000, -1000), (1000, -1000), (1000, 1000), (-1000, 1000)]);
        let frag_a = ring(&[(0, 0), (100, 0)]);
        let frag_b = ring(&[(100, 0), (300, 0)]);
        let lone = ring(&[(5000, 5000), (5100, 5000)]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("sport", DataType::UInt8, false),
            Field::new("geometry_kind", DataType::UInt8, false),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            Field::new("geom", DataType::Binary, true),
            Field::new("length_m", DataType::Float32, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(UInt8Array::from(vec![10, 10, 10, 10, 10])) as ArrayRef,
                Arc::new(UInt8Array::from(vec![1, 2, 2, 2, 2])) as ArrayRef,
                Arc::new(Int32Array::from(vec![0, 50, 200, 5050, 9000])) as ArrayRef,
                Arc::new(Int32Array::from(vec![0, 0, 0, 5000, 9000])) as ArrayRef,
                Arc::new(BinaryArray::from(vec![
                    Some(facility.as_slice()),
                    Some(frag_a.as_slice()),
                    Some(frag_b.as_slice()),
                    Some(lone.as_slice()),
                    None,
                ])) as ArrayRef,
                Arc::new(Float32Array::from(vec![
                    None,
                    Some(100.0),
                    Some(200.0),
                    Some(100.0),
                    None,
                ])) as ArrayRef,
            ],
        )
        .unwrap();
        let venues = MotorsportVenues::build(std::slice::from_ref(&batch));
        // A and B touch and share the facility polygon: one venue, 1/3 + 2/3.
        let share_a = venues.line_share(chain_ends(&[(0, 0), (100, 0)]), 100.0);
        let share_b = venues.line_share(chain_ends(&[(100, 0), (300, 0)]), 200.0);
        assert!((share_a - 1.0 / 3.0).abs() < 1e-9, "{share_a}");
        assert!((share_b - 2.0 / 3.0).abs() < 1e-9, "{share_b}");
        // The lone line, the geometry-less row and unknown chains keep one
        // full total each (current behaviour).
        assert_eq!(
            venues.line_share(chain_ends(&[(5000, 5000), (5100, 5000)]), 100.0),
            1.0
        );
        assert_eq!(venues.line_share(None, 0.0), 1.0);
        assert_eq!(venues.line_share(Some(((9, 9), (8, 8))), 5.0), 1.0);
        // The facility polygon still goes silent: it encloses raceway lines.
        assert!(venues.encloses_line(&[
            (-1000, -1000),
            (1000, -1000),
            (1000, 1000),
            (-1000, 1000)
        ]));
        assert!(!venues.encloses_line(&[
            (4000, 4000),
            (4500, 4000),
            (4500, 4500),
            (4000, 4000)
        ]));
    }

    #[test]
    fn missing_tags_column_reads_as_untagged() {
        use arrow::array::Int32Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;
        let schema = Arc::new(Schema::new(vec![Field::new(
            "centroid_gx",
            DataType::Int32,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Int32Array::from(vec![1])) as arrow::array::ArrayRef],
        )
        .unwrap();
        assert!(optional_tags(&batch, 0).is_empty());
        assert!(!row_is_leisure_line(&batch, 0));
        assert!(transformer_units(std::slice::from_ref(&batch)).is_empty());
        let venues = MotorsportVenues::build(std::slice::from_ref(&batch));
        assert!(!venues.encloses_line(&[(0, 0), (1, 0), (1, 1)]));
        assert_eq!(venues.line_share(Some(((0, 0), (1, 1))), 10.0), 1.0);
    }
}

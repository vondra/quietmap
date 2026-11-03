//! Spill bucket writer.
//!
//! Features are hashed by square id % num_buckets and appended to intermediate files.
//! Each bucket is TSV (simple during development — Arrow IPC in finalize step).
//!
//! First column is always the numeric square id (`y * 512 + x`); finalize sorts
//! on it. Coordinates are already snapped z30 grid cells — the spill carries
//! ints, never floats or WKB. Rings encode as `gx,gy;gx,gy;…` (empty = none).

use crate::osm::classify::{self, FeatureType, Tags};
use crate::osm::geometry::{Square, lonlat_to_grid};
use crate::osm::ids;
use crate::osm::microsegment;
use anyhow::Result;
use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Transient row-major partition key for spill/sort: `y * 512 + x`, fits u32.
/// Scratch-only (spill text + bucketing) — never the persistent Morton square id.
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
    if out.len() < 3 { None } else { Some(out) }
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
/// for an older census). `int_ref` is deliberately not consulted: census
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

/// Leading columns of every segment row: `sq osm_id seg_idx s_gx s_gy e_gx e_gy length_m`.
pub fn write_segment_row_prefix(
    out: &mut impl Write,
    square: Square,
    osm_id: i64,
    seg_idx: i16,
    seg: &([f64; 2], [f64; 2], f32),
) -> std::io::Result<()> {
    let (sgx, sgy) = lonlat_to_grid(seg.0[1], seg.0[0]);
    let (egx, egy) = lonlat_to_grid(seg.1[1], seg.1[0]);
    write!(
        out,
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
        spill_key(square),
        osm_id,
        seg_idx,
        sgx,
        sgy,
        egx,
        egy,
        seg.2
    )
}

pub struct Spiller {
    dir: PathBuf,
    num_buckets: usize,
    /// (feature_type_name, bucket_idx) → writer
    writers: HashMap<(String, usize), BufWriter<File>>,
    selected: HashSet<u32>,
    speeds: crate::osm::speed_rules::SpeedRules,
}

impl Spiller {
    pub fn new(
        dir: &Path,
        num_buckets: usize,
        speeds: crate::osm::speed_rules::SpeedRules,
        squares: &[crate::dev4::Square],
    ) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Spiller {
            dir: dir.to_path_buf(),
            num_buckets,
            writers: HashMap::new(),
            selected: squares
                .iter()
                .map(|square| square.y * 512 + square.x)
                .collect(),
            speeds,
        })
    }

    fn selects(&self, square: Square) -> bool {
        self.selected.is_empty() || self.selected.contains(&spill_key(square))
    }

    fn get_writer(&mut self, ftype: &str, bucket: usize) -> Result<&mut BufWriter<File>> {
        Ok(match self.writers.entry((ftype.to_string(), bucket)) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let path = self.dir.join(format!("{ftype}_{bucket:03}.tsv"));
                entry.insert(BufWriter::with_capacity(1 << 20, File::create(path)?))
            }
        })
    }

    pub fn emit_control_point(
        &mut self,
        node: &crate::osm::model_nodes::ModelNode,
        link: Option<(i64, &str, usize, Option<f64>)>,
    ) -> Result<()> {
        let square = crate::osm::geometry::square_of(node.lat, node.lon);
        if !self.selects(square) {
            return Ok(());
        }
        let bucket = self.bucket(square);
        let w = self.get_writer("transport_nodes", bucket)?;
        let (gx, gy) = lonlat_to_grid(node.lon, node.lat);
        write!(w, "{}\t{}\t{gx}\t{gy}\t", spill_key(square), node.id)?;
        if let Some((way, family, vertex, metres)) = link {
            write!(
                w,
                "{way}\t{family}\t{vertex}\t{}",
                metres.map(|v| v.to_string()).unwrap_or_default()
            )?;
        } else {
            write!(w, "\t\t\t")?;
        }
        writeln!(
            w,
            "\t{}",
            tags_json(node.control.as_ref().expect("control node"))?
        )?;
        Ok(())
    }

    fn bucket(&self, square: Square) -> usize {
        spill_key(square) as usize % self.num_buckets
    }

    /// Emit a function-POI node for the finalize footprint join. Format:
    /// `sq, gx, gy, class`. Consumed ONLY by finalize to reclassify the
    /// `building=yes` the POI sits inside; never a final arrow.
    /// `class` is the settlement class from [`poi_class`]; rows whose tags don't
    /// resolve are not emitted (caller checks first).
    pub fn emit_poi(&mut self, square: Square, clat: f64, clon: f64, class: u8) -> Result<()> {
        if !self.selects(square) {
            return Ok(());
        }
        let bucket = self.bucket(square);
        let (gx, gy) = lonlat_to_grid(clon, clat);
        let w = self.get_writer(FeatureType::Poi.name(), bucket)?;
        writeln!(w, "{}\t{}\t{}\t{}", spill_key(square), gx, gy, class)?;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        for writer in self.writers.values_mut() {
            writer.flush()?;
        }
        Ok(())
    }
}

/// Stable JSON escapes arbitrary tag text without losing tabs or line breaks.
pub(crate) fn tags_json(tags: &Tags) -> Result<String> {
    Ok(serde_json::to_string(
        &tags.iter().collect::<std::collections::BTreeMap<_, _>>(),
    )?)
}

#[cfg(test)]
#[path = "../spill_settlement_class_tests.rs"]
mod settlement_class_tests;
#[cfg(test)]
#[path = "../spill_site_type_tests.rs"]
mod site_type_tests;

mod building_tags;
mod polygons;
mod segments;
mod site_tags;
pub(crate) use building_tags::{GROUND_HOLDS_A_BUILDING, GROUND_IS_A_FUNCTIONAL_AREA};
use building_tags::{building_type_from_tags, building_use, ground_state};
pub use building_tags::{poi_class, poi_class_from_tags};
use site_tags::{parse_height, parse_power_kw, site_subtype_from_tags, site_type_from_tags};

#[cfg(test)]
use building_tags::{GROUND_IS_BELOW_A_BUILDING, building_type};

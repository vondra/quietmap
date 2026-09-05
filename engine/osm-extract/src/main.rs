//! osm-extract: Extract noise-relevant features from OSM PBF into z9-partitioned Arrow IPC.
//!
//! Pipeline:
//!   Pass 0: Scan relations → manifest of multipolygon members
//!   Pass 1: Stream nodes → mmap'd coordinate cache
//!   Pass 2: Stream ways + relations → classify, resolve, microsegment, assemble multipolygons
//!   Spill to `--num-buckets` square-disjoint buckets, then finalize → per-square Arrow IPC
//!
//! Coordinates leave the spill already snapped to the global int32 z30 grid;
//! the final arrows carry integer geometry (`*_gx/gy`, `geom` binaries).

mod classify;
mod finalize;
mod ids;
mod microsegment;
mod node_cache;
mod poi_join;
mod relations;
mod spill;

use anyhow::Result;
use clap::Parser;
use osmpbf::{Element, ElementReader};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "osm-extract")]
struct Cli {
    #[arg(short, long)]
    input: PathBuf,
    #[arg(short, long, default_value = "out-sq")]
    output: PathBuf,
    #[arg(long, default_value = "/tmp/osm_nodes.cache")]
    node_cache: PathBuf,
    #[arg(long, default_value = "/tmp/osm_spill")]
    spill_dir: PathBuf,
    /// Spill partitions. Buckets are disjoint by square, so finalize
    /// parallelizes one rayon task per (source, bucket).
    #[arg(long, default_value_t = 256)]
    num_buckets: usize,
    /// Skip extraction, only run finalize on existing spill data.
    #[arg(long)]
    finalize_only: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let t0 = Instant::now();
    eprintln!("=== osm-extract ===");

    if cli.finalize_only {
        eprintln!("  Finalize-only mode (skipping extraction)");
        eprintln!("  Spill dir: {}", cli.spill_dir.display());
        eprintln!("  Output: {}", cli.output.display());

        let t_fin = Instant::now();
        let square_count = finalize::finalize(&cli.spill_dir, &cli.output, cli.num_buckets)?;
        eprintln!(
            "  {} square dirs in {:.1}s",
            square_count,
            t_fin.elapsed().as_secs_f64()
        );

        // Don't clean up spill (user may want to re-run)
        eprintln!("\n=== Done: {:.1}s ===", t0.elapsed().as_secs_f64());
        return Ok(());
    }

    eprintln!("  Input:  {}", cli.input.display());

    // ── Pass 0: Scan relations ──
    eprintln!("\n── Pass 0: Scan relations ──");
    let manifest = relations::scan_relations(&cli.input)?;
    eprintln!("  {:.1}s", t0.elapsed().as_secs_f64());

    // ── Pass 1: Node coordinate cache ──
    eprintln!("\n── Pass 1: Node cache ──");
    let t1 = Instant::now();
    let cache = node_cache::NodeCache::build(&cli.input, &cli.node_cache)?;
    eprintln!(
        "  {} nodes in {:.1}s",
        cache.count(),
        t1.elapsed().as_secs_f64()
    );

    // ── Pass 2: Extract features ──
    eprintln!("\n── Pass 2: Extract → spill ──");
    let t2 = Instant::now();
    // Start from a clean spill dir: a stale partition from an earlier run (especially
    // a different num_buckets) would let finalize place the same square in two parallel
    // units and race on its output file. (--finalize_only returns earlier, never here.)
    std::fs::remove_dir_all(&cli.spill_dir).ok();
    let mut spiller = spill::Spiller::new(&cli.spill_dir, cli.num_buckets)?;
    let mut assembler = relations::RelationAssembler::new(&manifest);

    let reader = ElementReader::from_path(&cli.input)?;
    let mut ways_total = 0u64;
    let mut features_total = 0u64;
    let mut rels_assembled = 0u64;
    let mut antimeridian_rings_omitted = 0u64;
    // Observability: noise-relevant ways that classified to None and vanished
    // (a functional AREA with no building wrapper). Reported after Pass 2.
    let mut fallthrough_tags: HashMap<String, u64> = HashMap::new();

    reader.for_each(|element| {
        match element {
            Element::Way(way) => {
                ways_total += 1;
                if ways_total.is_multiple_of(2_000_000) {
                    eprintln!(
                        "  {:.1}M ways, {:.1}M features, {} rels assembled...",
                        ways_total as f64 / 1e6,
                        features_total as f64 / 1e6,
                        rels_assembled
                    );
                }

                // Resolve coordinates
                let coords: Vec<[f64; 2]> = way.refs().filter_map(|nid| cache.get(nid)).collect();

                // Check if this way is a member of any multipolygon relation
                let is_relation_member = manifest.way_to_relations.contains_key(&way.id());

                if is_relation_member && !coords.is_empty() {
                    // Cache geometry for relation assembly
                    let completed = assembler.add_way(way.id(), coords.clone(), &manifest);
                    for rel_id in completed {
                        if let Some((ring, tags, ftype)) = assembler.assemble(rel_id, &manifest) {
                            let extracted_tags = match ftype {
                                classify::FeatureType::Building => {
                                    let mut t = classify::Tags::new();
                                    for (k, v) in &tags {
                                        // Copy amenity/shop/healthcare/tourism/leisure from relation tags.
                                        // WHY: Large buildings (hospitals, schools, malls) are often multipolygon
                                        // relations. Without these tags, building_type_from_tags() can't classify
                                        // them correctly — a hospital gets type 0 (residential) instead of 4.
                                        if matches!(
                                            k.as_str(),
                                            "building"
                                                | "building:use"
                                                | "height"
                                                | "building:levels"
                                                | "name"
                                                | "addr:street"
                                                | "addr:housenumber"
                                                | "amenity"
                                                | "shop"
                                                | "healthcare"
                                                | "tourism"
                                                | "leisure"
                                                | "animal"
                                                | "livestock"
                                                | "opening_hours"
                                        ) {
                                            t.insert(k.clone(), v.clone());
                                        }
                                    }
                                    // NOTE: a relation with no `building` tag is a
                                    // FUNCTIONAL AREA (shop=mall / amenity=hospital
                                    // multipolygon). Leave the building tag ABSENT so
                                    // `building_type_from_tags` classifies it by
                                    // function (poi_class) and the spill flags it as an
                                    // area-source for finalize overlap-suppression.
                                    t
                                }
                                classify::FeatureType::Industrial => {
                                    let mut t = classify::Tags::new();
                                    for (k, v) in &tags {
                                        // Copy operator/product/industrial from relation tags.
                                        // WHY: Large industrial complexes are multipolygon relations.
                                        // These tags enable NACE sector matching for emission profiles.
                                        if matches!(
                                            k.as_str(),
                                            "landuse"
                                                | "man_made"
                                                | "name"
                                                | "operator"
                                                | "product"
                                                | "industrial"
                                        ) {
                                            t.insert(k.clone(), v.clone());
                                        }
                                    }
                                    t
                                }
                                classify::FeatureType::AirportArea => {
                                    let mut t = classify::Tags::new();
                                    for (k, v) in &tags {
                                        if matches!(
                                            k.as_str(),
                                            "aeroway"
                                                | "name"
                                                | "ref"
                                                | "local_ref"
                                                | "icao"
                                                | "iata"
                                                | "operator"
                                                | "surface"
                                                | "width"
                                                | "access"
                                                | "aerodrome"
                                                | "aerodrome:type"
                                                | "amenity"
                                        ) {
                                            t.insert(k.clone(), v.clone());
                                        }
                                    }
                                    t
                                }
                                _ => tags.clone(),
                            };

                            let (clat, clon) = centroid(&ring);
                            let square = grid::square_of(clat, clon);
                            let safe_ring = ring_for_spill(&ring, &mut antimeridian_rings_omitted);
                            spiller.emit_polygon(
                                &ftype,
                                square,
                                rel_id,
                                clat,
                                clon,
                                &extracted_tags,
                                safe_ring,
                            );
                            features_total += 1;
                            rels_assembled += 1;
                        }
                        assembler.cleanup(rel_id, &manifest);
                    }
                }

                // Also process the way itself if it has its own relevant tags
                // (a way can be both a relation member AND a standalone feature,
                //  but usually relation members don't have building= tags themselves)
                let way_class = classify::classify_way(&way);
                // Observability: a standalone way that vanished (None) yet carried a
                // noise-relevant functional tag (a mall / hospital / school AREA with
                // no building wrapper) — record it so the gap is never silent.
                if way_class.is_none() && !is_relation_member {
                    if let Some(reason) = classify::fallthrough_reason(&way) {
                        *fallthrough_tags.entry(reason).or_insert(0) += 1;
                    }
                }
                if let Some(mut ftype) = way_class {
                    // Skip if this way is an outer member of a polygon relation;
                    // the relation's assembled multipolygon already covers it.
                    // AirportLine ways inside an aeroway=aerodrome multipolygon
                    // would otherwise be re-emitted as perimeter fragments.
                    if is_relation_member
                        && matches!(
                            ftype,
                            classify::FeatureType::Building
                                | classify::FeatureType::Industrial
                                | classify::FeatureType::AirportArea
                                | classify::FeatureType::AirportLine
                        )
                    {
                        return;
                    }

                    // Closed-ring runway/airstrip ways are geometrically
                    // polygons (a runway drawn as a perimeter way rather
                    // than a multipolygon). Reroute to AirportArea for a
                    // proper polygon source instead of perimeter fragments.
                    //
                    // taxiway/stopway closed-rings are typically turnaround
                    // LOOPS where the ring itself is the taxi path; keeping
                    // them as AirportLine preserves the downstream leg snap.
                    if matches!(ftype, classify::FeatureType::AirportLine)
                        && coords.len() >= 3
                        && (coords[0][0] - coords.last().unwrap()[0]).abs() < 1e-7
                        && (coords[0][1] - coords.last().unwrap()[1]).abs() < 1e-7
                        && matches!(
                            way.tags().find(|(k, _)| *k == "aeroway").map(|(_, v)| v),
                            Some("runway") | Some("airstrip")
                        )
                    {
                        ftype = classify::FeatureType::AirportArea;
                    }

                    if ftype.is_linear() && coords.len() < 2 {
                        return;
                    }
                    if coords.is_empty() {
                        return;
                    }

                    let tags = classify::extract_way_tags(&way, &ftype);

                    if ftype.is_linear() {
                        let max_len = 250.0;
                        let segs = microsegment::split(&coords, max_len);
                        for (idx, seg) in segs.iter().enumerate() {
                            let mid_lat = (seg.0[0] + seg.1[0]) / 2.0;
                            let mid_lon = grid::geo::wrapped_longitude_midpoint(seg.0[1], seg.1[1]);
                            let square = grid::square_of(mid_lat, mid_lon);
                            spiller.emit_segment(&ftype, square, way.id(), idx as i16, seg, &tags);
                            features_total += 1;
                        }
                    } else {
                        let (clat, clon) = centroid(&coords);
                        let square = grid::square_of(clat, clon);
                        let ring = ring_for_spill(&coords, &mut antimeridian_rings_omitted);
                        spiller.emit_polygon(&ftype, square, way.id(), clat, clon, &tags, ring);
                        features_total += 1;
                    }
                }
            }
            Element::Node(node) => {
                features_total += emit_node(
                    &mut spiller,
                    classify::is_wind_turbine_node(&node),
                    || classify::extract_turbine_tags_node(&node),
                    classify::FeatureType::WindTurbine,
                    node.id(),
                    node.lat(),
                    node.lon(),
                );
                features_total += emit_node(
                    &mut spiller,
                    classify::is_airport_node(&node),
                    || classify::extract_airport_tags_node(&node),
                    classify::FeatureType::AirportArea,
                    node.id(),
                    node.lat(),
                    node.lon(),
                );
                if let Some(kind) = classify::node_kind_node(&node) {
                    let tags = classify::extract_node_settlement_tags_node(&node);
                    features_total += emit_settlement_node(
                        &mut spiller,
                        kind,
                        node.id(),
                        node.lat(),
                        node.lon(),
                        &tags,
                    );
                }
            }
            Element::DenseNode(node) => {
                features_total += emit_node(
                    &mut spiller,
                    classify::is_wind_turbine_dense(&node),
                    || classify::extract_turbine_tags_dense(&node),
                    classify::FeatureType::WindTurbine,
                    node.id(),
                    node.lat(),
                    node.lon(),
                );
                features_total += emit_node(
                    &mut spiller,
                    classify::is_airport_dense(&node),
                    || classify::extract_airport_tags_dense(&node),
                    classify::FeatureType::AirportArea,
                    node.id(),
                    node.lat(),
                    node.lon(),
                );
                if let Some(kind) = classify::node_kind_dense(&node) {
                    let tags = classify::extract_node_settlement_tags_dense(&node);
                    features_total += emit_settlement_node(
                        &mut spiller,
                        kind,
                        node.id(),
                        node.lat(),
                        node.lon(),
                        &tags,
                    );
                }
            }
            _ => {}
        }
    })?;

    spiller.flush_all()?;
    eprintln!(
        "  {:.1}M ways → {:.1}M features ({} multipolygon rels) in {:.1}s",
        ways_total as f64 / 1e6,
        features_total as f64 / 1e6,
        rels_assembled,
        t2.elapsed().as_secs_f64()
    );

    // Classification blind spots — the two SILENT failure modes made visible so a
    // gap is never invisible (a new unmapped tag, a vanished functional area).
    eprintln!("\n  ── Classification blind spots ──");
    report_top(
        "  building=* → residential DEFAULT (unmapped tag)",
        &spiller.audit.default_residential,
        12,
    );
    report_top(
        "  functional AREA vanished (no building tag → routing fall-through)",
        &fallthrough_tags,
        12,
    );
    eprintln!("  antimeridian polygon rings omitted (centroid-only): {antimeridian_rings_omitted}");

    // Finalize opens its own readers and writers; close all spill writers first.
    drop(spiller);

    // Free node cache before finalize (saves ~64 GB disk for planet)
    drop(cache);
    if cli.node_cache.exists() {
        eprintln!("  Deleting node cache to free disk...");
        std::fs::remove_file(&cli.node_cache).ok();
    }

    // ── Finalize ──
    eprintln!("\n── Finalize ──");
    let t3 = Instant::now();
    let square_count = finalize::finalize(&cli.spill_dir, &cli.output, cli.num_buckets)?;
    eprintln!(
        "  {} square dirs in {:.1}s",
        square_count,
        t3.elapsed().as_secs_f64()
    );

    // node cache already deleted after Pass 2
    std::fs::remove_dir_all(&cli.spill_dir).ok();
    eprintln!("\n=== Done: {:.1}s ===", t0.elapsed().as_secs_f64());
    Ok(())
}

/// Extract tags and spill only classified turbine/airport nodes, returning the count.
fn emit_node(
    spiller: &mut spill::Spiller,
    kind_match: bool,
    extract_tags: impl FnOnce() -> classify::Tags,
    ftype: classify::FeatureType,
    osm_id: i64,
    lat: f64,
    lon: f64,
) -> u64 {
    if !kind_match {
        return 0;
    }
    let tags = extract_tags();
    let square = grid::square_of(lat, lon);
    spiller.emit_polygon(&ftype, square, osm_id, lat, lon, &tags, None);
    1
}

/// Print the top-N entries of a blind-spot counter (descending), or nothing if
/// empty. Single source for both classification-gap reports.
fn report_top(label: &str, counts: &HashMap<String, u64>, top_n: usize) {
    if counts.is_empty() {
        return;
    }
    let total: u64 = counts.values().sum();
    let mut v: Vec<(&String, &u64)> = counts.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let top: Vec<String> = v
        .iter()
        .take(top_n)
        .map(|(k, c)| format!("{k}={c}"))
        .collect();
    eprintln!(
        "{label}: {total} total, {} distinct\n      {}",
        counts.len(),
        top.join("  ")
    );
}

/// Spill one settlement NODE: a `Leisure` node becomes a point leisure source
/// (centroid only, no ring → default area), a `Poi` node spills to the
/// finalize footprint-join file. Returns 1 if a row was written, else 0 (e.g.
/// a POI whose tags don't resolve to a class).
fn emit_settlement_node(
    spiller: &mut spill::Spiller,
    kind: classify::FeatureType,
    osm_id: i64,
    lat: f64,
    lon: f64,
    tags: &classify::Tags,
) -> u64 {
    let square = grid::square_of(lat, lon);
    match kind {
        classify::FeatureType::Leisure => {
            spiller.emit_polygon(&kind, square, osm_id, lat, lon, tags, None);
            1
        }
        classify::FeatureType::Poi => match spill::poi_class_from_tags(tags) {
            Some(class) => {
                spiller.emit_poi(square, lat, lon, class);
                1
            }
            None => 0,
        },
        _ => 0,
    }
}

fn centroid(coords: &[[f64; 2]]) -> (f64, f64) {
    let coords = if coords.len() > 1 && coords.first() == coords.last() {
        &coords[..coords.len() - 1]
    } else {
        coords
    };
    let n = coords.len() as f64;
    let reference_lon = coords[0][1];
    (
        coords.iter().map(|c| c[0]).sum::<f64>() / n,
        grid::geo::normalize_longitude(
            reference_lon
                + coords
                    .iter()
                    .map(|c| grid::geo::wrapped_longitude_delta(reference_lon, c[1]))
                    .sum::<f64>()
                    / n,
        ),
    )
}

/// Canonical longitudes make a dateline-crossing ring look almost world-wide
/// to current polygon consumers. Preserve its correct centroid but omit the
/// unsafe ring until the shared polygon format can represent wrapped geometry.
fn ring_for_spill<'a>(
    coords: &'a [[f64; 2]],
    antimeridian_rings_omitted: &mut u64,
) -> Option<&'a [[f64; 2]]> {
    if coords.len() < 3 {
        return None;
    }
    let (min_lon, max_lon) = coords.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(min_lon, max_lon), coord| (min_lon.min(coord[1]), max_lon.max(coord[1])),
    );
    if max_lon - min_lon > 180.0 {
        *antimeridian_rings_omitted += 1;
        None
    } else {
        Some(coords)
    }
}

#[cfg(test)]
mod tests {
    use super::{centroid, classify, emit_node, ring_for_spill, spill};

    #[test]
    fn point_source_tags_are_extracted_only_when_the_node_is_kept() {
        let directory =
            std::env::temp_dir().join(format!("osm-node-tag-laziness-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let mut spiller = spill::Spiller::new(&directory, 1).unwrap();
        for kind_match in [false, true] {
            let mut extracted = 0;
            let written = emit_node(
                &mut spiller,
                kind_match,
                || {
                    extracted += 1;
                    classify::Tags::from([("name".into(), "Kept turbine".into())])
                },
                classify::FeatureType::WindTurbine,
                42,
                50.0,
                14.0,
            );
            assert_eq!(extracted, u64::from(kind_match));
            assert_eq!(written, u64::from(kind_match));
        }
        spiller.flush_all().unwrap();
        let row = std::fs::read_to_string(directory.join("industrial_000.tsv")).unwrap();
        assert_eq!(row.lines().count(), 1);
        assert!(row.contains("\t42\t"));
        assert!(row.contains("\tKept turbine\t"));
        drop(spiller);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn antimeridian_ring_keeps_its_centroid_but_not_unsafe_geometry() {
        let ring = [
            [10.0, 179.99],
            [10.0, -179.99],
            [10.01, -179.99],
            [10.01, 179.99],
            [10.0, 179.99],
        ];
        let (lat, lon) = centroid(&ring);
        assert!((lat - 10.005).abs() < 1e-12);
        assert!((lon + 180.0).abs() < 1e-12, "lon={lon}");
        let mut omitted = 0;
        assert!(ring_for_spill(&ring, &mut omitted).is_none());
        assert_eq!(omitted, 1);
    }
}

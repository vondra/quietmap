//! Multi-pass OSM extraction, original transport topology and country joins from admitted inputs.

mod classify;
mod contracts;
mod finalize;
mod geography;
mod geography_output;
mod geometry;
mod ids;
mod junctions;
mod microsegment;
mod model_nodes;
mod node_cache;
mod output_paths;
mod pass2;
mod poi_join;
pub(crate) mod region_input;
mod relations;
mod speed_rules;
mod spill;
mod transport;

use anyhow::{Result, anyhow, ensure};
use std::path::Path;

const SPILL_BUCKETS: usize = 32;

pub fn build(
    inputs: &crate::year::YearInputs,
    squares: &[crate::dev4::Square],
    output: &Path,
    scratch: &Path,
) -> Result<()> {
    let input = inputs
        .required("prepare-osm")
        .map_err(|error| anyhow!(error))?;
    let speeds = speed_rules::SpeedRules::read(
        inputs
            .optional("prepare-osm-speeds")
            .map_err(|error| anyhow!(error))?,
    )?;
    let regions = geography::Regions::read(
        inputs
            .required("prepare-regions")
            .map_err(|error| anyhow!(error))?,
    )?;
    output_paths::validate(output, scratch)?;
    ensure!(
        !squares.is_empty()
            && squares
                .iter()
                .all(|square| square.x < 512 && square.y < 512),
        "invalid or empty z9 selection"
    );
    let mut squares = squares.to_vec();
    squares.sort_unstable();
    squares.dedup();
    std::fs::create_dir(scratch)?;
    let cache_path = scratch.join("nodes");
    let spill_path = scratch.join("spill");
    eprintln!("osm: relation and junction census");
    let (manifest, junctions, needed) = relations::scan_relations_and_junctions(input)?;
    eprintln!("osm: selected node coordinates");
    let cache = node_cache::NodeCache::build(input, &cache_path, &needed)?;
    eprintln!("osm: {} selected node coordinates", cache.count());
    drop(needed);
    let mut spiller = spill::Spiller::new(&spill_path, SPILL_BUCKETS, speeds, &squares)?;
    let mut transport = transport::TransportSpill::new(&spill_path)?;
    eprintln!("osm: extraction and topology");
    let stats = pass2::extract_features(
        input,
        &cache,
        &manifest,
        &junctions,
        &mut spiller,
        &mut transport,
    )?;
    transport.finish()?;
    spiller.finish()?;
    drop(spiller);
    drop(cache);
    drop(junctions);
    std::fs::remove_file(cache_path)?;
    eprintln!(
        "osm: {} ways, {} features, {} relations, {} antimeridian rings omitted; finalizing",
        stats.ways_total,
        stats.features_total,
        stats.rels_assembled,
        stats.antimeridian_rings_omitted
    );
    std::fs::create_dir(output)?;
    finalize::finalize(&spill_path, output, SPILL_BUCKETS, &squares)?;
    geography::bake(&regions, &squares, output)?;
    std::fs::remove_dir_all(scratch)?;
    Ok(())
}

#[cfg(test)]
mod pbf_fixture;
#[cfg(test)]
mod topology_repeat_tests;
#[cfg(test)]
mod topology_test_helpers;
#[cfg(test)]
mod topology_tests;

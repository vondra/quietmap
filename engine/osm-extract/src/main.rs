//! Extract OSM features into z9 Arrow files and retain original transport connectivity for enrichment.

mod classify;
mod finalize;
mod ids;
mod junctions;
mod microsegment;
mod node_cache;
mod pass2;
mod poi_join;
mod relations;
mod spill;
mod transport;

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
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
}

fn main() -> Result<()> {
    let mut cli = Cli::parse();
    validate_paths(&mut cli)?;
    let input_identity = spill_input_identity(&cli.input, std::env::var("QM_OSM_ONLY").ok())?;
    let t0 = Instant::now();
    eprintln!("=== osm-extract ===");

    // A complete spill left by an extract whose finalize failed is finalized
    // again from the spill; the planet is read only for a partial or absent one.
    if spill::is_complete(&cli.spill_dir, cli.num_buckets, &input_identity)
        && transport::TransportWriter::is_complete(&cli.spill_dir)
    {
        eprintln!(
            "  Complete spill in {} ({} buckets): finalizing from it, not from the planet",
            cli.spill_dir.display(),
            cli.num_buckets
        );
        eprintln!("  Output: {}", cli.output.display());

        finalize_and_cleanup(&cli)?;

        eprintln!("\n=== Done: {:.1}s ===", t0.elapsed().as_secs_f64());
        return Ok(());
    }

    eprintln!("  Input:  {}", cli.input.display());

    eprintln!("\n── Pass 0: Scan relations ──");
    let (manifest, junctions, needed_nodes) = relations::scan_relations_and_junctions(&cli.input)?;
    eprintln!("  {:.1}s", t0.elapsed().as_secs_f64());

    eprintln!("\n── Pass 1: Node cache ──");
    let t1 = Instant::now();
    std::fs::create_dir_all(cli.node_cache.parent().unwrap())?;
    let cache = node_cache::NodeCache::build(&cli.input, &cli.node_cache, &needed_nodes)?;
    eprintln!(
        "  {} nodes in {:.1}s",
        cache.count(),
        t1.elapsed().as_secs_f64()
    );
    drop(needed_nodes);

    eprintln!("\n── Pass 2: Extract → spill ──");
    let t2 = Instant::now();
    // Start from a clean spill dir: a stale partition from an earlier run (especially
    // a different num_buckets) would let finalize place the same square in two parallel
    // units and race on its output file — and the completion marker would then bless it.
    match std::fs::remove_dir_all(&cli.spill_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => anyhow::bail!(
            "cannot clear stale spill {}: {error}",
            cli.spill_dir.display()
        ),
    }
    let mut spiller = spill::Spiller::new(&cli.spill_dir, cli.num_buckets)?;
    let mut transport = transport::TransportWriter::new(&cli.spill_dir)?;

    let pass2 = pass2::extract_features(
        &cli.input,
        &cache,
        &manifest,
        &junctions,
        &mut spiller,
        &mut transport,
    )?;

    transport.finish()?;
    anyhow::ensure!(
        spill_input_identity(&cli.input, std::env::var("QM_OSM_ONLY").ok())? == input_identity,
        "OSM input changed during extraction; partial spill retained"
    );
    spiller.complete(&input_identity)?;
    eprintln!(
        "  {:.1}M ways → {:.1}M features ({} multipolygon rels) in {:.1}s",
        pass2.ways_total as f64 / 1e6,
        pass2.features_total as f64 / 1e6,
        pass2.rels_assembled,
        t2.elapsed().as_secs_f64()
    );

    eprintln!("\n  ── Classification blind spots ──");
    report_top(
        "  building=* → residential DEFAULT (unmapped tag)",
        &spiller.audit.default_residential,
        12,
    );
    report_top(
        "  functional AREA vanished (no building tag → routing fall-through)",
        &pass2.fallthrough_tags,
        12,
    );
    eprintln!(
        "  antimeridian polygon rings omitted (centroid-only): {}",
        pass2.antimeridian_rings_omitted
    );

    drop(spiller);
    drop(junctions);
    drop(cache);

    finalize_and_cleanup(&cli)?;
    eprintln!("\n=== Done: {:.1}s ===", t0.elapsed().as_secs_f64());
    Ok(())
}

fn spill_input_identity(input: &Path, layers: Option<String>) -> Result<String> {
    let stat = input.metadata()?;
    Ok(serde_json::json!({
        "input": input.canonicalize()?, "layers": layers,
        "device": stat.dev(), "inode": stat.ino(), "bytes": stat.len(),
        "mtime": [stat.mtime(), stat.mtime_nsec()],
        "ctime": [stat.ctime(), stat.ctime_nsec()]
    })
    .to_string())
}

fn finalize_and_cleanup(cli: &Cli) -> Result<()> {
    match std::fs::remove_file(&cli.node_cache) {
        Ok(()) => eprintln!("  Deleted node cache to free disk"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("remove node cache"),
    }
    eprintln!("\n── Finalize ──");
    let started = Instant::now();
    let square_count = finalize::finalize(&cli.spill_dir, &cli.output, cli.num_buckets)?;
    transport::TransportWriter::publish(&cli.spill_dir, &cli.output)?;
    eprintln!(
        "  {} square dirs in {:.1}s",
        square_count,
        started.elapsed().as_secs_f64()
    );
    std::fs::remove_dir_all(&cli.spill_dir).context("remove completed spill")?;
    Ok(())
}

/// Resolve existing symlinks before processing `..`, including ancestors of new paths.
fn resolved_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::env::current_dir()?.join(path);
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                resolved.push(component.as_os_str());
                match resolved.canonicalize() {
                    Ok(canonical) => resolved = canonical,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        // A dangling symlink is not a missing output component we may create.
                        if std::fs::symlink_metadata(&resolved).is_ok() {
                            return Err(error)
                                .with_context(|| format!("resolve {}", resolved.display()));
                        }
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("resolve {}", resolved.display()))
                    }
                }
            }
        }
    }
    Ok(resolved)
}

fn validate_paths(cli: &mut Cli) -> Result<()> {
    cli.input = resolved_path(&cli.input)?;
    cli.output = resolved_path(&cli.output)?;
    cli.node_cache = resolved_path(&cli.node_cache)?;
    cli.spill_dir = resolved_path(&cli.spill_dir)?;
    let transport = resolved_path(&transport::output_path(&cli.output)?)?;
    let mut copying = transport::output_path(&cli.output)?.into_os_string();
    copying.push(".copying");
    let copying = resolved_path(Path::new(&copying))?;
    let paths = [
        ("input", &cli.input),
        ("output", &cli.output),
        ("node cache", &cli.node_cache),
        ("spill", &cli.spill_dir),
        ("transport output", &transport),
        ("transport staging", &copying),
    ];
    for (index, (left_role, left)) in paths.iter().enumerate() {
        for (right_role, right) in &paths[index + 1..] {
            let same_file = match (std::fs::metadata(left), std::fs::metadata(right)) {
                (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
                _ => false,
            };
            if left.starts_with(right) || right.starts_with(left) || same_file {
                bail!(
                    "overlapping {left_role} and {right_role}: {} / {}",
                    left.display(),
                    right.display()
                );
            }
        }
    }
    Ok(())
}

/// Print the top-N entries of a blind-spot counter (descending), or nothing if
/// empty. Single source for both classification-gap reports.
fn report_top(label: &str, counts: &std::collections::HashMap<String, u64>, top_n: usize) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn input_and_scratch_roles_cannot_alias_or_contain_each_other() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "osm-path-roles-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&root)?;
        let input = root.join("planet.pbf");
        std::fs::write(&input, b"irreplaceable planet")?;
        let original = spill_input_identity(&input, None)?;
        assert_ne!(
            original,
            spill_input_identity(&input, Some("roads,railways".into()))?
        );
        let replacement = root.join("replacement.pbf");
        std::fs::write(&replacement, b"a different planet")?;
        assert_ne!(original, spill_input_identity(&replacement, None)?);
        let make_cli = || Cli {
            input: input.clone(),
            output: root.join("new/output"),
            node_cache: root.join("cache/nodes"),
            spill_dir: root.join("spill"),
            num_buckets: 1,
        };
        let mut valid = make_cli();
        validate_paths(&mut valid)?;
        assert!(!valid.output.exists());
        let hardlink = root.join("hardlink");
        std::fs::hard_link(&input, &hardlink)?;
        let linked = root.join("linked");
        symlink(&input, &linked)?;
        for cache in [
            input.clone(),
            hardlink,
            linked,
            valid.spill_dir.join("nodes"),
            valid.output.join("nodes"),
            transport::output_path(&valid.output)?,
        ] {
            let mut cli = make_cli();
            cli.node_cache = cache;
            assert!(validate_paths(&mut cli).is_err());
        }
        let alias = root.join("alias");
        symlink(&root, &alias)?;
        for (spill, output) in [
            (root.clone(), valid.output.clone()),
            (valid.spill_dir.clone(), valid.spill_dir.join("output")),
            (valid.output.join("spill"), valid.output.clone()),
            (root.join("future"), alias.join("future/output")),
            (root.join("future"), alias.join("new/../future/output")),
        ] {
            let mut cli = make_cli();
            cli.spill_dir = spill;
            cli.output = output;
            assert!(validate_paths(&mut cli).is_err());
        }
        assert_eq!(std::fs::read(&input)?, b"irreplaceable planet");
        assert!(!valid.output.exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}

//! Sort only selected square buckets and propagate every scratch read failure.

use super::*;

/// Sort one spill bucket by square id, group consecutive rows, and write one
/// `{source}.arrow` per square directory. Returns the square directories it created.
///
/// `sort` groups on disk; its stdout streams straight into the encoder — no
/// intermediate sorted copy, so peak scratch stays at the spill size. `-S`
/// bounds per-sort RAM and `--parallel=1` keeps each sort single-threaded
/// (parallelism is across buckets). A nonzero sort exit aborts finalize.
pub(super) fn finalize_bucket(
    source: &str,
    bucket: usize,
    spill_dir: &Path,
    output_dir: &Path,
    aliases: &NodeAliases,
    join_stats: &JoinStats,
    selected: &HashSet<u32>,
) -> Result<HashSet<String>> {
    let mut square_dirs = HashSet::new();
    let path = spill_dir.join(format!("{source}_{bucket:03}.tsv"));

    // The buildings unit consumes its bucket's POI nodes for the footprint
    // join. A POI inside a building shares its square → its bucket, so the
    // join is fully local to this unit (no cross-bucket lookup).
    let poi_index = if source == "buildings" {
        load_poi_bucket(spill_dir, bucket)?
    } else {
        PoiIndex::default()
    };

    let mut child = std::process::Command::new("sort")
        .args([
            "-t\t",
            "-k1,1n",
            "-T",
            spill_dir.to_str().unwrap_or("/tmp"),
            "-S",
            "1G",
            "--parallel=1",
        ])
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let reader = BufReader::with_capacity(1 << 20, child.stdout.take().expect("sort stdout piped"));
    let mut current_id: Option<u32> = None;
    let mut current_rows: Vec<Vec<String>> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let sq_id: u32 = line.split('\t').next().unwrap_or("").parse()?;
        if !selected.is_empty() && !selected.contains(&sq_id) {
            continue;
        }

        let fields: Vec<String> = line.split('\t').map(str::to_owned).collect();
        if Some(sq_id) != current_id && !current_rows.is_empty() {
            if let Some(id) = current_id {
                square_dirs.insert(flush_square(
                    source,
                    id,
                    &current_rows,
                    output_dir,
                    aliases,
                    &poi_index,
                    join_stats,
                )?);
            }
            current_rows.clear();
        }
        current_id = Some(sq_id);
        current_rows.push(fields);
    }
    if !current_rows.is_empty()
        && let Some(id) = current_id
    {
        square_dirs.insert(flush_square(
            source,
            id,
            &current_rows,
            output_dir,
            aliases,
            &poi_index,
            join_stats,
        )?);
    }

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("sort failed for {source} bucket {bucket} ({status})");
    }

    eprintln!("    {source} [{bucket:03}]: {} squares", square_dirs.len());
    Ok(square_dirs)
}

/// Load `poi_<bucket>.tsv` (unsorted is fine — `PoiIndex` keys by square).
/// Absent when a region has no function POIs → an empty index (no-op join).
fn load_poi_bucket(spill_dir: &Path, bucket: usize) -> Result<PoiIndex> {
    let path = spill_dir.join(format!("poi_{bucket:03}.tsv"));
    if !path.exists() {
        return Ok(PoiIndex::default());
    }
    let reader = BufReader::with_capacity(1 << 20, File::open(&path)?);
    Ok(PoiIndex::from_lines(
        reader
            .lines()
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter(),
    ))
}

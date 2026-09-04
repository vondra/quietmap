//! Validate measured shard extents and route every ground hit to its line owner.
use super::*;
use crate::extent::Extent;
use std::path::PathBuf;

pub(super) struct GroundWork {
    pub owner: u64,
    pub inputs: Vec<PathBuf>,
    pub candidates: Vec<u64>,
}

pub(super) fn ground_work_plan(
    input_root: &Path,
    prepared_root: &Path,
    scope: Option<&ScopeBbox>,
) -> Result<Vec<GroundWork>> {
    anyhow::ensure!(
        input_root.is_dir(),
        "ground shard directory missing: {}",
        input_root.display()
    );
    let inputs = crate::shuffle::list_square_shards(input_root, "ground.arrow", None)?;
    let mut lines = Vec::new();
    let mut line_ids = HashSet::new();
    let mut broadphase: HashMap<u64, Vec<usize>> = HashMap::new();
    for (owner, directory) in crate::spatial::square_directories(prepared_root)? {
        anyhow::ensure!(!(scope.is_some_and(|scope| !scope.contains_square(owner)) && directory.join("airport_traffic.arrow").try_exists()?),
            "scoped Stage 2C cannot replace a global airport summary while out-of-scope traffic exists at {}; regional flight IDs cannot reconstruct the global movement union; use an isolated prepared YEAR tree", directory.display());
        let cache = SquareCache::load(prepared_root, owner, &[])?;
        let mut extent = Extent::empty(owner);
        for line in cache.lines {
            let id = (line.osm_id, line.segment_idx);
            anyhow::ensure!(
                line_ids.insert(id),
                "airport microsegment {id:?} has multiple prepared owners"
            );
            extent.include(line.start_lat, line.start_lon);
            extent.include(line.end_lat, line.end_lon);
        }
        if extent.is_empty() {
            continue;
        }
        let extent = extent.padded(AIRPORT_LINE_SNAP_BUFFER_M);
        for square in extent.squares() {
            broadphase.entry(square).or_default().push(lines.len());
        }
        lines.push((owner, extent));
    }
    let mut by_owner: HashMap<u64, (Vec<PathBuf>, HashSet<u64>)> = HashMap::new();
    // Every shard is validated, including empty IPC files and the last batch;
    // no previously deployed output is removed until this pass completes.
    for (owner, path) in inputs {
        let mut extent = Extent::empty(owner);
        crate::arrow_io::for_each_segment_batch(&path, |segments| {
            for seg in segments {
                anyhow::ensure!(
                    seg.phase == Phase::Ground
                        && seg.flight_id != 0
                        && seg.veh_kind <= 1
                        && (seg.veh_kind == 0 || usize::from(seg.gse_class) < NUM_GSE_CLASSES)
                        && seg.speed_kt >= 0.0
                        && seg.length_m > 0.0,
                    "invalid ground phase, identity, vehicle class, speed or length in {}",
                    path.display()
                );
                extent.include(seg.start_lat, seg.start_lon);
                extent.include(seg.end_lat, seg.end_lon);
            }
            Ok(())
        })
        .with_context(|| format!("validate {}", path.display()))?;
        let mut candidates = HashSet::new();
        for square in extent.squares() {
            if let Some(indices) = broadphase.get(&square) {
                for &idx in indices {
                    if extent.intersects(lines[idx].1) {
                        candidates.insert(lines[idx].0);
                    }
                }
            }
        }
        for &target in &candidates {
            if scope.is_some_and(|scope| !scope.contains_square(target)) {
                continue;
            }
            let work = by_owner.entry(target).or_default();
            work.0.push(path.clone());
            // The projection must include adjacent owners before overlap
            // normalization, even though this worker emits only its own lines.
            work.1.extend(candidates.iter().copied());
        }
    }
    let mut plan: Vec<_> = by_owner
        .into_iter()
        .map(|(owner, (inputs, candidates))| {
            let mut candidates: Vec<_> = candidates.into_iter().collect();
            candidates.sort_unstable();
            GroundWork {
                owner,
                inputs,
                candidates,
            }
        })
        .collect();
    plan.sort_unstable_by_key(|work| work.owner);
    Ok(plan)
}

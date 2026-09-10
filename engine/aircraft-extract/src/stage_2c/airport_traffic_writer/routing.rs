//! Validate shard extents and charge routed support before any traffic output mutation.
use super::*;
use crate::extent::Extent;
use std::mem::size_of;
use std::path::PathBuf;

pub struct GroundTrafficWork {
    pub owner: u64,
    pub inputs: Vec<PathBuf>,
    pub candidates: Vec<u64>,
    pub allocation: u64,
    pub input_rows: u64,
    pub input_bytes: u64,
    pub cached_lines: usize,
    pub owned_lines: usize,
    pub maximum_counter_rows: usize,
    pub maximum_airport_key_bytes: usize,
}

impl GroundTrafficWork {
    pub fn indexed_allocation(&self) -> Result<u64> {
        self.allocation
            .checked_add(AirportLineIndex::allocation_allowance(self.cached_lines)?)
            .context("indexed ground worker allowance overflow")
    }
}

#[derive(Default)]
struct RoutedGroundInputs {
    inputs: Vec<PathBuf>,
    candidates: HashSet<u64>,
    largest_input_allocation: u64,
    input_rows: u64,
    input_bytes: u64,
}

pub fn plan_ground_traffic(
    input_root: &Path,
    prepared_root: &Path,
    scope: Option<&ScopeBbox>,
    index: &crate::airport_index::AerodromeIndex,
) -> Result<Vec<GroundTrafficWork>> {
    anyhow::ensure!(
        input_root.is_dir(),
        "ground shard directory missing: {}",
        input_root.display()
    );
    let inputs = crate::shuffle::list_square_shards(input_root, "ground.arrow", None)?;
    let limit = crate::memory::available_memory_bytes();
    // Runtime, schemas, input directory inventories and allocator fragmentation.
    let mut budget = AllocationBudget::new(limit, (1 << 30) + index.allocation_allowance())?;
    let empty_index = crate::airport_index::AerodromeIndex::build(&[]);
    let mut lines = Vec::new();
    let mut line_ids = HashSet::new();
    let mut broadphase: HashMap<u64, Vec<usize>> = HashMap::new();
    // bytes, line count, maximum CounterKey rows, maximum actual/fallback key bytes.
    let mut cache_allocations = HashMap::new();
    for (owner, directory) in crate::spatial::square_directories(prepared_root)? {
        anyhow::ensure!(!(scope.is_some_and(|scope| !scope.contains_square(owner)) && directory.join("airport_traffic.arrow").try_exists()?),
            "scoped Stage 2C cannot replace a global airport summary while out-of-scope traffic exists at {}; regional flight IDs cannot reconstruct the global movement union; use an isolated prepared YEAR tree", directory.display());
        let mut file_bytes = 0u64;
        for name in [
            "airport_lines.arrow",
            crate::synth_airport_io::SYNTH_LINES_FILE,
        ] {
            let path = directory.join(name);
            if path.try_exists()? {
                file_bytes = file_bytes
                    .checked_add(path.metadata()?.len())
                    .context("airport source size overflow")?;
            }
        }
        AllocationBudget::new(
            limit,
            budget
                .reserved()
                .checked_add(
                    file_bytes
                        .checked_mul(8)
                        .context("airport decode allowance overflow")?,
                )
                .context("airport cache allowance overflow")?,
        )?;
        let cache = SquareCache::load(prepared_root, owner, &empty_index)?;
        let cache_bytes = file_bytes as u128 * 8
            + cache.lines.len() as u128
                * (8 * size_of::<AirportLineSegment>()
                    + 4 * index.maximum_airport_key_bytes()
                    + 4 * size_of::<((u64, u16), usize)>()
                    + 4 * size_of::<String>()
                    + 3 * size_of::<crate::stage_2c::airport_traffic::LegIntersection>())
                    as u128;
        let maximum_counter_rows = cache
            .lines
            .iter()
            .try_fold(0usize, |total, line| {
                let directions = usize::from(
                    ops_kind_from_aeroway(line.aeroway_type) == Some(GROUND_OPS_KIND_RUNWAY_ROLL),
                ) + 1;
                total.checked_add(
                    3 * (noise_compute::emission::profiles_generated::NUM_CLASSES * directions
                        + NUM_GSE_CLASSES),
                )
            })
            .context("ground counter row bound overflow")?;
        let maximum_key_bytes = cache
            .airport_keys
            .iter()
            .map(String::len)
            .max()
            .unwrap_or(0)
            .max(index.maximum_airport_key_bytes());
        cache_allocations.insert(
            owner,
            (
                u64::try_from(cache_bytes).context("airport cache size overflow")?,
                cache.lines.len(),
                maximum_counter_rows,
                maximum_key_bytes,
            ),
        );
        let mut extent = Extent::empty(owner);
        for line in cache.lines {
            let id = (line.osm_id, line.segment_idx);
            budget.reserve_hash_entry::<(u64, u16), ()>(line_ids.len())?;
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
            if !broadphase.contains_key(&square) {
                budget.reserve_hash_entry::<u64, Vec<usize>>(broadphase.len())?;
            }
            let indices = broadphase.entry(square).or_default();
            budget.reserve_vec_entry::<usize>(indices.len())?;
            indices.push(lines.len());
        }
        budget.reserve_vec_entry::<(u64, Extent)>(lines.len())?;
        lines.push((owner, extent));
    }
    let mut by_owner: HashMap<u64, RoutedGroundInputs> = HashMap::new();
    for (owner, path) in inputs {
        let file_bytes = path.metadata()?.len();
        let validation = file_bytes
            .checked_mul(4)
            .and_then(|n| {
                n.checked_add(
                    (2 * crate::arrow_io::SEGMENT_READ_CHUNK_ROWS * size_of::<FlightSegment>())
                        as u64,
                )
            })
            .context("ground input allowance overflow")?;
        AllocationBudget::new(
            limit,
            budget
                .reserved()
                .checked_add(validation)
                .context("ground validation allowance overflow")?,
        )?;
        let mut extent = Extent::empty(owner);
        let mut decoded_batch = 0usize;
        let mut input_rows = 0u64;
        crate::arrow_io::for_each_segment_batch(&path, |segments| {
            input_rows = input_rows
                .checked_add(segments.len() as u64)
                .context("ground row count overflow")?;
            decoded_batch = decoded_batch.max(
                segments.capacity() * size_of::<FlightSegment>()
                    + segments
                        .iter()
                        .map(|s| s.callsign.capacity())
                        .sum::<usize>(),
            );
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
        let input_allocation = file_bytes
            .checked_mul(4)
            .and_then(|n| n.checked_add(2 * decoded_batch as u64))
            .context("ground input allowance overflow")?;
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
            if !by_owner.contains_key(&target) {
                budget.reserve_hash_entry::<u64, RoutedGroundInputs>(by_owner.len())?;
            }
            let work = by_owner.entry(target).or_default();
            budget.reserve_vec_entry::<PathBuf>(work.inputs.len())?;
            budget.reserve(2 * path.as_os_str().as_encoded_bytes().len() as u64)?;
            work.inputs.push(path.clone());
            // Include adjacent owners before normalization; emit this owner's hits only.
            for candidate in &candidates {
                if !work.candidates.contains(candidate) {
                    budget.reserve_hash_entry::<u64, ()>(work.candidates.len())?;
                    work.candidates.insert(*candidate);
                }
            }
            work.largest_input_allocation = work.largest_input_allocation.max(input_allocation);
            work.input_rows = work
                .input_rows
                .checked_add(input_rows)
                .context("routed ground row count overflow")?;
            work.input_bytes = work
                .input_bytes
                .checked_add(file_bytes)
                .context("routed ground byte count overflow")?;
        }
    }
    let plan_allocation = budget.reserved();
    let mut plan = Vec::new();
    for (owner, work) in by_owner {
        let RoutedGroundInputs {
            inputs,
            candidates,
            largest_input_allocation: input_allocation,
            input_rows,
            input_bytes,
        } = work;
        let mut candidates: Vec<_> = candidates.into_iter().collect();
        candidates.sort_unstable();
        let cache = candidates
            .iter()
            .try_fold(0u64, |n, owner| n.checked_add(cache_allocations[owner].0))
            .context("ground cache sum overflow")?;
        let allocation = plan_allocation
            .checked_add(input_allocation)
            .and_then(|n| n.checked_add(cache))
            .context("ground worker allowance overflow")?;
        let cached_lines = candidates
            .iter()
            .try_fold(0usize, |n, id| n.checked_add(cache_allocations[id].1))
            .context("ground cached line count overflow")?;
        let (_, owned_lines, maximum_counter_rows, maximum_airport_key_bytes) =
            cache_allocations[&owner];
        plan.push(GroundTrafficWork {
            owner,
            inputs,
            candidates,
            allocation,
            input_rows,
            input_bytes,
            cached_lines,
            owned_lines,
            maximum_counter_rows,
            maximum_airport_key_bytes,
        });
    }
    plan.sort_unstable_by_key(|work| work.owner);
    Ok(plan)
}

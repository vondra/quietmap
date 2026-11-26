//! The painted pixels' loudness on the card (kernels/distribution.cuh, kernels/loudness.cuh): its
//! tables written from the Rust ones (the map's resolution, Kurze's quantiles at the nodes, the
//! groups of hours, ISO 532-1), the pixels' packed sounds in (`paint::sone::PixelSound::pack`),
//! their Nden out.

use crate::device::{Gpu, ensure_device, trim_device};
use crate::stream::PinnedBuf;
use crate::weather_runs::mixture_blocks;
use cudarc::driver::{CudaDevice, CudaSlice, LaunchConfig};
use paint::exact::{LAYERS, PIXELS_PER_LOUDNESS_BATCH};
use paint::sone::{PACKED_LINE_FLOATS, map_hour_groups};
use physics::bands::{PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS};
use physics::loudness::{A0, DCB, DLL, LTQ, RAP, RNS, SILENT_DB, THIRD_OCTAVES, USL, ZUP};
use physics::percentile::{
    AtProbabilities, LAMBDA_MIN, QUANTILES, ROWS, SIMULATED_LAMBDA_MAX, STEPS_PER_DECADE,
    probability,
};
use popup::distribution::{LEVEL_MIN_DB, SPAN_DB, node_probabilities};
use popup::loudness::{CURVE_HIGH_DB, CURVE_LOW_DB};
use popup::percentiles::{
    HELD_EXCEEDED, HELD_SHARE, HOLDS_MEAN_LAMBDA, PASSES_HOLD_MEAN, Resolution, WEATHER_STATES,
};
use std::sync::{Arc, Mutex, PoisonError};

const DISTRIBUTION: &str = include_str!("../kernels/distribution.cuh");
const LOUDNESS: &str = include_str!("../kernels/loudness.cuh");

/// Floats a pixel's answer takes: per layer and in total.
pub const LOUDNESS_FLOATS: usize = LAYERS + 1;

/// The loudness kernels' source, their tables first.
pub(crate) fn loudness_source() -> String {
    format!("{}{DISTRIBUTION}{LOUDNESS}", loudness_constants())
}

/// The map's level bins (`popup::distribution`'s span at the map's resolution).
fn bins() -> usize {
    (SPAN_DB / Resolution::MAP.bin_db).round() as usize
}

/// The bytes of one weather state's warp in a mixtures block (kernels/distribution.cuh
/// LoudWarp, checked against the card's when the module loads): a layer's distribution in two
/// buffers (a bin's two floats each, their silence and range) and a line's values (each value, and
/// its shares and weighted shares from it, and their count).
pub(crate) fn loud_warp_bytes() -> usize {
    let values = node_probabilities().len().max(bins() + 1);
    2 * (12 + 8 * bins()) + 4 * (3 * values + 3)
}

/// A pixel's blocks on the card: each period's groups of hours (as when some line follows a
/// profile), its weather states a warp each.
fn period_groups() -> Vec<(usize, usize)> {
    let hours = map_hour_groups();
    (0..PERIODS)
        .flat_map(|period| (0..hours.groups[period].len()).map(move |group| (period, group)))
        .collect()
}

/// The floats of a pixel's mixtures on the card: per period and curve (each layer's and the
/// place's) and bin two.
fn mixture_floats() -> usize {
    PERIODS * (LAYERS + 1) * bins() * 2
}

/// The kernels' tables at the map's resolution, in global memory (a warp's lanes read different
/// entries, which constant memory serves one at a time: a quarter less time for the same pixels);
/// f32's Debug is the shortest decimal that reads back as it.
fn loudness_constants() -> String {
    let resolution = Resolution::MAP;
    let floats = |name: &str, values: &[f64]| {
        let values: Vec<String> = values.iter().map(|&v| format!("{:?}f", v as f32)).collect();
        format!(
            "__device__ const float {name}[{}] = {{{}}};\n",
            values.len(),
            values.join(", ")
        )
    };
    let ints = |name: &str, values: &[usize]| {
        let values: Vec<String> = values.iter().map(usize::to_string).collect();
        format!(
            "__device__ const int {name}[{}] = {{{}}};\n",
            values.len(),
            values.join(", ")
        )
    };
    // Kurze's quantiles at the nodes, then at the table's last quantile and at the held test's.
    let nodes = node_probabilities();
    let probabilities: Vec<f64> = (nodes.iter().map(|&(p, _)| p))
        .chain([probability(QUANTILES - 1), 1.0 - HELD_EXCEEDED])
        .collect();
    let at = AtProbabilities::new(&probabilities);
    let (mut normal, mut ln_p, mut tail, mut node_rows) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (n, l, r) in at.parts() {
        normal.push(n);
        ln_p.push(l);
        tail.push(usize::from(r.is_none()));
        node_rows.push(r.map_or([0.0; ROWS], |r| *r));
    }
    // Row-major: a row's entries of every node together, as a warp's lanes read them.
    let row_nodes = node_rows.len();
    let rows: Vec<f64> = (0..ROWS)
        .flat_map(|row| node_rows.iter().map(move |node| node[row]))
        .collect();
    let shares: Vec<f64> = nodes.iter().map(|&(_, share)| share).collect();
    // The map's groups of hours and profile flow factors, shared with `paint::sone`.
    let hours = map_hour_groups();
    let groups_max = hours.groups_max;
    let group_slots = &hours.group_slots;
    let factor = &hours.factors;
    let group_counts: Vec<usize> = hours.groups.iter().map(Vec::len).collect();
    let defines = [
        format!("#define LOUD_BIN_DB {:?}f\n", resolution.bin_db as f32),
        format!("#define LOUD_BINS {}\n", bins()),
        format!("#define LOUD_PERIOD_GROUPS {}\n", period_groups().len()),
        format!("#define LOUD_LEVEL_MIN_DB ({:?}f)\n", LEVEL_MIN_DB as f32),
        format!("#define LOUD_STATES {}\n", WEATHER_STATES),
        format!("#define LOUD_NODES {}\n", nodes.len()),
        format!("#define LOUD_TOP_NODE {}\n", nodes.len()),
        format!("#define LOUD_HELD_NODE {}\n", nodes.len() + 1),
        format!("#define LOUD_ROWS_COUNT {ROWS}\n"),
        format!("#define LOUD_ROW_NODES {row_nodes}\n"),
        format!("#define LOUD_LAMBDA_MIN {:?}f\n", LAMBDA_MIN as f32),
        format!(
            "#define LOUD_STEPS_PER_DECADE {:?}f\n",
            STEPS_PER_DECADE as f32
        ),
        format!(
            "#define LOUD_SIMULATED_LAMBDA_MAX {:?}f\n",
            SIMULATED_LAMBDA_MAX as f32
        ),
        format!(
            "#define LOUD_HOLDS_MEAN_LAMBDA {:?}f\n",
            HOLDS_MEAN_LAMBDA as f32
        ),
        format!("#define LOUD_HELD_SHARE {:?}f\n", HELD_SHARE as f32),
        format!(
            "#define LOUD_PASSES_HOLD_MEAN {:?}f\n",
            PASSES_HOLD_MEAN as f32
        ),
        // The share of the time under which the card ends a stream's passes in progress.
        "#define LOUD_PASS_TAIL 1e-9f\n".to_string(),
        format!("#define LOUD_LAYERS {LAYERS}\n"),
        format!("#define LOUD_LINE_FLOATS {PACKED_LINE_FLOATS}\n"),
        format!("#define LOUD_THIRDS {THIRD_OCTAVES}\n"),
        format!("#define LOUD_CURVE_LOW_DB ({:?}f)\n", CURVE_LOW_DB as f32),
        format!(
            "#define LOUD_CURVE_STEPS {}\n",
            (CURVE_HIGH_DB - CURVE_LOW_DB) as usize
        ),
        format!("#define LOUD_GROUPS_MAX {groups_max}\n"),
        format!("#define LOUD_SILENT_DB ({:?}f)\n", SILENT_DB as f32),
    ]
    .concat();
    let period_groups = period_groups();
    let period_group_part = |part: fn(&(usize, usize)) -> usize| -> Vec<usize> {
        period_groups.iter().map(part).collect()
    };
    let dll: Vec<f64> = DLL.iter().flatten().copied().collect();
    let usl: Vec<f64> = USL.iter().flatten().copied().collect();
    [
        defines,
        floats("LOUD_NORMAL", &normal),
        floats("LOUD_LN_P", &ln_p),
        ints("LOUD_TAIL", &tail),
        floats("LOUD_ROWS", &rows),
        floats("LOUD_SHARES", &shares),
        ints("LOUD_GROUPS", &group_counts),
        ints("LOUD_GROUP_SLOTS", group_slots),
        floats("LOUD_FACTOR", factor),
        ints(
            "LOUD_PERIOD_GROUP_PERIOD",
            &period_group_part(|period_group| period_group.0),
        ),
        ints(
            "LOUD_PERIOD_GROUP_GROUP",
            &period_group_part(|period_group| period_group.1),
        ),
        floats("LOUD_PERIOD_HOURS", &PERIOD_HOURS),
        floats("LOUD_PENALTY_DB", &PERIOD_PENALTY_DB),
        floats("LOUD_RAP", &RAP),
        floats("LOUD_DLL", &dll),
        floats("LOUD_LTQ", &LTQ),
        floats("LOUD_A0", &A0),
        floats("LOUD_DCB", &DCB),
        floats("LOUD_ZUP", &ZUP),
        floats("LOUD_RNS", &RNS),
        floats("LOUD_USL", &usl),
    ]
    .concat()
}

/// Bounds on retained loudness slot buffers: at most one slot per concurrent turn on the card,
/// and packed float inputs trimmed back to ~64 MB when an extreme chunk spikes higher.
const MAX_RETAINED_LOUDNESS_SLOTS: usize = crate::turns::EVALUATIONS_ON_THE_CARD;
const MAX_RETAINED_INPUT_FLOATS: usize = 16_000_000;
const MAX_RETAINED_TASKS: usize = 8 * PIXELS_PER_LOUDNESS_BATCH;

/// Reusable device and pinned host staging buffers for one concurrent `pixel_loudness` launch,
/// avoiding per-batch allocations and pageable-memory DMA stalls.
pub(crate) struct LoudnessSlot {
    input: Option<CudaSlice<f32>>,
    offsets: Option<CudaSlice<u64>>,
    tasks: Option<CudaSlice<u32>>,
    runs: Option<CudaSlice<u8>>,
    mixtures: Option<CudaSlice<f32>>,
    ndens: Option<CudaSlice<f32>>,
    host_input: PinnedBuf<f32>,
    host_offsets: PinnedBuf<u64>,
    host_tasks: PinnedBuf<u32>,
    host_runs: PinnedBuf<u8>,
    host_ndens: PinnedBuf<f32>,
}

impl LoudnessSlot {
    fn new(device: &Arc<CudaDevice>) -> Self {
        Self {
            input: None,
            offsets: None,
            tasks: None,
            runs: None,
            mixtures: None,
            ndens: None,
            host_input: PinnedBuf::new(device),
            host_offsets: PinnedBuf::new(device),
            host_tasks: PinnedBuf::new(device),
            host_runs: PinnedBuf::new(device),
            host_ndens: PinnedBuf::new(device),
        }
    }

    fn trim_oversized(&mut self) {
        let max_runs = MAX_RETAINED_TASKS * 2 * WEATHER_STATES;
        let max_mix = mixture_floats() * PIXELS_PER_LOUDNESS_BATCH;
        let max_ndens = LOUDNESS_FLOATS * PIXELS_PER_LOUDNESS_BATCH;
        trim_device(&mut self.input, MAX_RETAINED_INPUT_FLOATS);
        trim_device(&mut self.offsets, PIXELS_PER_LOUDNESS_BATCH);
        trim_device(&mut self.tasks, MAX_RETAINED_TASKS);
        trim_device(&mut self.runs, max_runs);
        trim_device(&mut self.mixtures, max_mix);
        trim_device(&mut self.ndens, max_ndens);
        self.host_input.trim(MAX_RETAINED_INPUT_FLOATS);
        self.host_offsets.trim(PIXELS_PER_LOUDNESS_BATCH);
        self.host_tasks.trim(MAX_RETAINED_TASKS);
        self.host_runs.trim(max_runs);
        self.host_ndens.trim(max_ndens);
    }
}

struct LoudnessSlotGuard<'a> {
    pool: &'a Mutex<Vec<LoudnessSlot>>,
    slot: Option<LoudnessSlot>,
}

impl Drop for LoudnessSlotGuard<'_> {
    fn drop(&mut self) {
        if let Some(mut slot) = self.slot.take() {
            slot.trim_oversized();
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            if pool.len() < MAX_RETAINED_LOUDNESS_SLOTS {
                pool.push(slot);
            }
        }
    }
}

impl Gpu {
    /// Each pixel's Nden per layer and in total (sone) from its packed sound: `input` all the
    /// pixels' floats, `offsets` where each pixel's begin.
    pub fn pixel_loudness(
        &self,
        input: &[f32],
        offsets: &[u64],
    ) -> Result<Vec<[f32; LOUDNESS_FLOATS]>, String> {
        let count = offsets.len();
        if count == 0 {
            return Ok(Vec::new());
        }
        let device = &self.device;
        let (spans, tasks, runs) = mixture_blocks(input, offsets, &period_groups());
        // Bound concurrent loudness launches on the card so mixtures VRAM stays bounded across
        // concurrent squares and passes.
        self.loudness_turns.in_turn(|| {
            let slot = self
                .loudness_slots
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop();
            let mut guard = LoudnessSlotGuard {
                pool: &self.loudness_slots,
                slot: Some(slot.unwrap_or_else(|| LoudnessSlot::new(device))),
            };
            let slot = guard.slot.as_mut().expect("loudness slot present");
            let mix_len = mixture_floats() * count;
            let nden_len = LOUDNESS_FLOATS * count;
            let device_input = ensure_device(device, &mut slot.input, input.len())?;
            let device_offsets = ensure_device(device, &mut slot.offsets, count)?;
            let device_tasks = ensure_device(device, &mut slot.tasks, tasks.len())?;
            let device_runs = ensure_device(device, &mut slot.runs, runs.len())?;
            let mixtures = ensure_device(device, &mut slot.mixtures, mix_len)?;
            let ndens = ensure_device(device, &mut slot.ndens, nden_len)?;
            let kernel = |name: &str| {
                device
                    .get_func("loudness", name)
                    .ok_or(format!("kernel {name} missing"))
            };
            let host_input = &mut slot.host_input;
            let host_offsets = &mut slot.host_offsets;
            let host_tasks = &mut slot.host_tasks;
            let host_runs = &mut slot.host_runs;
            let host_ndens = &mut slot.host_ndens;
            host_input.copy_from_slice(input)?;
            host_offsets.copy_from_slice(offsets)?;
            host_tasks.copy_from_slice(&tasks)?;
            host_runs.copy_from_slice(&runs)?;
            host_ndens.reserve(nden_len)?;
            // On a stream of their own, forked once their buffers are grown: the pairs' kernels of
            // another pass run beside them on the card.
            let stream = self.fork()?;
            // SAFETY: `stream` is never leaked and drops before the pinned buffers and slot guard.
            unsafe {
                stream.copy_prefix(host_input.as_slice(input.len()), device_input)?;
                stream.copy_prefix(host_offsets.as_slice(count), device_offsets)?;
                stream.copy_prefix(host_tasks.as_slice(tasks.len()), device_tasks)?;
                stream.copy_prefix(host_runs.as_slice(runs.len()), device_runs)?;
            }
            stream.zero_prefix(mixtures, mix_len)?;
            let pixels_config = LaunchConfig {
                grid_dim: (count as u32, 1, 1),
                block_dim: (32, 1, 1),
                shared_mem_bytes: 0,
            };
            // SAFETY: the kernels read each pixel's floats from its offset (the host packed them as
            // kernels/distribution.cuh lays them out); the first adds into `mixture_floats()`
            // zeroed floats a pixel, the second reads them and writes LOUDNESS_FLOATS a pixel;
            // every device buffer outlives `stream`.
            let state_bytes = loud_warp_bytes() + 4;
            for (states, &(first, blocks)) in spans.iter().enumerate().filter(|(_, (_, n))| *n > 0)
            {
                let config = LaunchConfig {
                    grid_dim: (blocks as u32, 1, 1),
                    block_dim: (states as u32 * 32, 1, 1),
                    shared_mem_bytes: (states * state_bytes) as u32,
                };
                unsafe {
                    stream.launch(
                        kernel("pixel_mixtures")?,
                        config,
                        (
                            &*device_input,
                            &*device_offsets,
                            &*device_tasks,
                            &*device_runs,
                            first as u32,
                            &*mixtures,
                        ),
                    )?;
                }
            }
            unsafe {
                stream.launch(
                    kernel("pixel_ndens")?,
                    pixels_config,
                    (
                        &*device_input,
                        &*device_offsets,
                        &*mixtures,
                        count as u32,
                        &*ndens,
                    ),
                )?;
            }
            stream.read_back_into(ndens, host_ndens.as_mut_slice(nden_len))?;
            stream.finish()?;
            Ok(host_ndens
                .as_slice(nden_len)
                .chunks_exact(LOUDNESS_FLOATS)
                .map(|pixel| std::array::from_fn(|k| pixel[k]))
                .collect())
        })
    }
}

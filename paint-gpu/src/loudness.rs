//! The painted pixels' loudness on the card (kernels/distribution.cuh, kernels/loudness.cuh): its
//! tables written from the Rust ones (the map's resolution, Kurze's quantiles at the nodes, the
//! groups of hours, ISO 532-1), the pixels' packed sounds in (`paint::sone::PixelSound::pack`),
//! their Nden out.

use crate::device::{Gpu, driver};
use cudarc::driver::{LaunchAsync, LaunchConfig};
use paint::exact::LAYERS;
use physics::bands::{PERIOD_HOURS, PERIOD_PENALTY_DB, PERIODS};
use physics::loudness::{A0, DCB, DLL, LTQ, RAP, RNS, SILENT_DB, THIRD_OCTAVES, USL, ZUP};
use physics::percentile::{
    AtProbabilities, LAMBDA_MIN, QUANTILES, ROWS, SIMULATED_LAMBDA_MAX, STEPS_PER_DECADE,
    probability,
};
use popup::distribution::{LEVEL_MIN_DB, SPAN_DB, node_probabilities};
use popup::loudness::{CURVE_HIGH_DB, CURVE_LOW_DB};
use popup::percentiles::{
    HELD_EXCEEDED, HELD_SHARE, HOLDS_MEAN_LAMBDA, Resolution, hour_factor, hour_groups,
};

const DISTRIBUTION: &str = include_str!("../kernels/distribution.cuh");
const LOUDNESS: &str = include_str!("../kernels/loudness.cuh");

/// Floats a pixel's answer takes: per layer and in total.
pub const LOUDNESS_FLOATS: usize = LAYERS + 1;

/// The hourly profiles (`popup::percentiles` HOURLY_SHARES's rows).
const PROFILES: usize = 3;

/// Warps of a block of the mixtures' kernel, a task each (4 x 8 KB of shared memory).
const TASK_WARPS: u32 = 4;

/// The loudness kernels' source, their tables first.
pub(crate) fn loudness_source() -> String {
    format!("{}{DISTRIBUTION}{LOUDNESS}", loudness_constants())
}

/// The map's level bins (`popup::distribution`'s span at the map's resolution).
fn bins() -> usize {
    (SPAN_DB / Resolution::MAP.bin_db).round() as usize
}

/// A pixel's tasks on the card: each period's groups of hours (as when some line follows a
/// profile) and weather states.
fn tasks() -> Vec<(usize, usize, usize)> {
    let resolution = Resolution::MAP;
    (0..PERIODS)
        .flat_map(|period| {
            let groups = hour_groups(true, period, resolution.hour_groups).0.len();
            (0..groups).flat_map(move |group| {
                (0..resolution.weather_states).map(move |state| (period, group, state))
            })
        })
        .collect()
}

/// The floats of a pixel's mixtures on the card: per period and curve (each layer's and the
/// place's) and bin two.
fn mixture_floats() -> usize {
    PERIODS * (LAYERS + 1) * bins() * 2
}

/// The kernels' tables at the map's resolution, in global memory (a warp's lanes read different
/// entries, which constant memory serves one at a time: 0.40 -> 0.29 s for 16,384 pixels on a V100);
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
    let (mut normal, mut ln_p, mut tail, mut rows) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (n, l, r) in at.parts() {
        normal.push(n);
        ln_p.push(l);
        tail.push(usize::from(r.is_none()));
        rows.extend(r.map_or([0.0; ROWS], |r| *r));
    }
    let shares: Vec<f64> = nodes.iter().map(|&(_, share)| share).collect();
    // The map's groups of hours (as when some line follows a profile): their hours and each
    // profile's mean flow over them against the period's.
    let groups: Vec<(Vec<Vec<usize>>, usize)> = (0..PERIODS)
        .map(|period| hour_groups(true, period, resolution.hour_groups))
        .collect();
    let groups_max = groups.iter().map(|(g, _)| g.len()).max().unwrap_or(1);
    let mut group_slots = vec![0; PERIODS * groups_max];
    let mut factor = vec![1.0; PROFILES * PERIODS * groups_max];
    for (period, (slots, _)) in groups.iter().enumerate() {
        for (group, slots) in slots.iter().enumerate() {
            group_slots[period * groups_max + group] = slots.len();
            for profile in 0..PROFILES {
                factor[(profile * PERIODS + period) * groups_max + group] = (slots
                    .iter()
                    .map(|&slot| hour_factor(profile, period, slot))
                    .sum::<f64>())
                    / slots.len() as f64;
            }
        }
    }
    let defines = [
        format!("#define LOUD_BIN_DB {:?}f\n", resolution.bin_db as f32),
        format!("#define LOUD_BINS {}\n", bins()),
        format!("#define LOUD_TASKS {}\n", tasks().len()),
        format!("#define LOUD_TASK_WARPS {TASK_WARPS}\n"),
        format!("#define LOUD_LEVEL_MIN_DB ({:?}f)\n", LEVEL_MIN_DB as f32),
        format!("#define LOUD_STATES {}\n", resolution.weather_states),
        format!("#define LOUD_NODES {}\n", nodes.len()),
        format!("#define LOUD_TOP_NODE {}\n", nodes.len()),
        format!("#define LOUD_HELD_NODE {}\n", nodes.len() + 1),
        format!("#define LOUD_ROWS_COUNT {ROWS}\n"),
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
        format!("#define LOUD_LAYERS {LAYERS}\n"),
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
    let tasks = tasks();
    let task_part = |part: fn(&(usize, usize, usize)) -> usize| -> Vec<usize> {
        tasks.iter().map(part).collect()
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
        ints(
            "LOUD_GROUPS",
            &groups.iter().map(|(g, _)| g.len()).collect::<Vec<_>>(),
        ),
        ints("LOUD_GROUP_SLOTS", &group_slots),
        floats("LOUD_FACTOR", &factor),
        ints("LOUD_TASK_PERIOD", &task_part(|task| task.0)),
        ints("LOUD_TASK_GROUP", &task_part(|task| task.1)),
        ints("LOUD_TASK_STATE", &task_part(|task| task.2)),
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
        let input = driver(device.htod_sync_copy(input))?;
        let offsets = driver(device.htod_sync_copy(offsets))?;
        let mixtures = driver(device.alloc_zeros::<f32>(mixture_floats() * count))?;
        let ndens = driver(device.alloc_zeros::<f32>(LOUDNESS_FLOATS * count))?;
        let kernel = |name: &str| {
            device
                .get_func("loudness", name)
                .ok_or(format!("kernel {name} missing"))
        };
        let task_count = (count * tasks().len()) as u32;
        let tasks_config = LaunchConfig {
            grid_dim: (task_count.div_ceil(TASK_WARPS), 1, 1),
            block_dim: (TASK_WARPS * 32, 1, 1),
            shared_mem_bytes: 0,
        };
        let pixels_config = LaunchConfig {
            grid_dim: (count as u32, 1, 1),
            block_dim: (32, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the kernels read each pixel's floats from its offset (the host packed them as
        // kernels/distribution.cuh lays them out); the first adds into `mixture_floats()` zeroed
        // floats a pixel, the second reads them and writes LOUDNESS_FLOATS a pixel.
        driver(unsafe {
            kernel("pixel_mixtures")?
                .launch(tasks_config, (&input, &offsets, task_count, &mixtures))
        })?;
        driver(unsafe {
            kernel("pixel_ndens")?.launch(
                pixels_config,
                (&input, &offsets, &mixtures, count as u32, &ndens),
            )
        })?;
        let flat = driver(device.dtoh_sync_copy(&ndens))?;
        Ok(flat
            .chunks_exact(LOUDNESS_FLOATS)
            .map(|pixel| std::array::from_fn(|k| pixel[k]))
            .collect())
    }
}

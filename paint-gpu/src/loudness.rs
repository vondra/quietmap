//! The painted pixels' loudness on the card (kernels/distribution.cuh, kernels/loudness.cuh): its
//! tables written from the Rust ones (the map's resolution, Kurze's quantiles at the nodes, the
//! groups of hours, ISO 532-1), the pixels' packed sounds in (`paint::sone::PixelSound::pack`),
//! their Nden out.

use crate::device::{Gpu, driver, read_back};
use crate::weather_runs::mixture_blocks;
use cudarc::driver::{LaunchAsync, LaunchConfig};
use paint::exact::LAYERS;
use paint::sone::PACKED_LINE_FLOATS;
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
    hour_factor, hour_groups,
};

const DISTRIBUTION: &str = include_str!("../kernels/distribution.cuh");
const LOUDNESS: &str = include_str!("../kernels/loudness.cuh");

/// Floats a pixel's answer takes: per layer and in total.
pub const LOUDNESS_FLOATS: usize = LAYERS + 1;

/// The hourly profiles (`popup::percentiles` HOURLY_SHARES's rows).
const PROFILES: usize = 3;

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
    let resolution = Resolution::MAP;
    (0..PERIODS)
        .flat_map(|period| {
            let groups = hour_groups(true, period, resolution.hour_groups).0.len();
            (0..groups).map(move |group| (period, group))
        })
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
        ints(
            "LOUD_GROUPS",
            &groups.iter().map(|(g, _)| g.len()).collect::<Vec<_>>(),
        ),
        ints("LOUD_GROUP_SLOTS", &group_slots),
        floats("LOUD_FACTOR", &factor),
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
        let (spans, tasks, runs) = mixture_blocks(
            offsets.iter().map(|&at| &input[at as usize..]),
            &period_groups(),
        );
        let input = driver(device.htod_sync_copy(input))?;
        let offsets = driver(device.htod_sync_copy(offsets))?;
        let tasks = driver(device.htod_sync_copy(&tasks))?;
        let runs = driver(device.htod_sync_copy(&runs))?;
        let mixtures = driver(device.alloc_zeros::<f32>(mixture_floats() * count))?;
        let ndens = driver(device.alloc_zeros::<f32>(LOUDNESS_FLOATS * count))?;
        let kernel = |name: &str| {
            device
                .get_func("loudness", name)
                .ok_or(format!("kernel {name} missing"))
        };
        // On a stream of their own (once the buffers above are zeroed): the pairs' kernels of
        // another pass run beside them on the card.
        let stream = self.fork()?;
        let pixels_config = LaunchConfig {
            grid_dim: (count as u32, 1, 1),
            block_dim: (32, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the kernels read each pixel's floats from its offset (the host packed them as
        // kernels/distribution.cuh lays them out); the first adds into `mixture_floats()` zeroed
        // floats a pixel, the second reads them and writes LOUDNESS_FLOATS a pixel.
        let state_bytes = loud_warp_bytes() + 4;
        for (states, &(first, blocks)) in spans.iter().enumerate().filter(|(_, (_, n))| *n > 0) {
            let config = LaunchConfig {
                grid_dim: (blocks as u32, 1, 1),
                block_dim: (states as u32 * 32, 1, 1),
                shared_mem_bytes: (states * state_bytes) as u32,
            };
            driver(unsafe {
                kernel("pixel_mixtures")?.launch_on_stream(
                    &stream,
                    config,
                    (&input, &offsets, &tasks, &runs, first as u32, &mixtures),
                )
            })?;
        }
        driver(unsafe {
            kernel("pixel_ndens")?.launch_on_stream(
                &stream,
                pixels_config,
                (&input, &offsets, &mixtures, count as u32, &ndens),
            )
        })?;
        let flat = read_back(&stream, &ndens)?;
        Ok(flat
            .chunks_exact(LOUDNESS_FLOATS)
            .map(|pixel| std::array::from_fn(|k| pixel[k]))
            .collect())
    }
}

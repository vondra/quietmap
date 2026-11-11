//! The painted pixels' loudness on the card (kernels/distribution.cuh, kernels/loudness.cuh): its
//! tables written from the Rust ones (the map's resolution, Kurze's quantiles at the nodes, the
//! groups of hours, ISO 532-1), the pixels' packed sounds in (`paint::sone::PixelSound::pack`),
//! their Nden out.

use crate::device::{Gpu, driver, read_back};
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
    HELD_EXCEEDED, HELD_SHARE, HOLDS_MEAN_LAMBDA, Resolution, WEATHER_STATES, hour_factor,
    hour_groups,
};

const DISTRIBUTION: &str = include_str!("../kernels/distribution.cuh");
const LOUDNESS: &str = include_str!("../kernels/loudness.cuh");

/// Floats a pixel's answer takes: per layer and in total.
pub const LOUDNESS_FLOATS: usize = LAYERS + 1;

/// The hourly profiles (`popup::percentiles` HOURLY_SHARES's rows).
const PROFILES: usize = 3;

/// The loudness kernels' source, their tables first, the mixtures' blocks of `block_states` states.
pub(crate) fn loudness_source(block_states: usize) -> String {
    format!(
        "{}{DISTRIBUTION}{LOUDNESS}",
        loudness_constants(block_states)
    )
}

/// The map's level bins (`popup::distribution`'s span at the map's resolution).
fn bins() -> usize {
    (SPAN_DB / Resolution::MAP.bin_db).round() as usize
}

/// Weather states a block of the mixtures' kernel takes on a card whose multiprocessors hold
/// `shared_bytes` of shared memory, a warp each: all ten (a line's values made once for all) where
/// two such blocks fit (compute capability 7.0 and 12.0, about 96 KB); two elsewhere, which keeps
/// more warps resident (on 7.5, 64 KB, ten a block painted 2-3 % slower than a task a warp, two
/// 2-3 % faster).
pub(crate) fn block_states(shared_bytes: usize) -> usize {
    // A state's warp holds four distributions (a bin's two floats each, their silence and range)
    // and a line's values (each value, and its shares and weighted shares from it), and its bit.
    let values = node_probabilities().len().max(bins() + 1);
    let warp_bytes = 4 * (12 + 8 * bins()) + 4 * (1 + 3 * values + 2) + 4;
    if 2 * WEATHER_STATES * warp_bytes <= shared_bytes {
        WEATHER_STATES
    } else {
        2
    }
}

/// A pixel's blocks on the card: each period's groups of hours (as when some line follows a
/// profile), a block's weather states ([`block_states`]) a warp each.
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
fn loudness_constants(block_states: usize) -> String {
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
        format!("#define LOUD_BLOCK_STATES {block_states}\n"),
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
        let input = driver(device.htod_sync_copy(input))?;
        let offsets = driver(device.htod_sync_copy(offsets))?;
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
        let mixtures_config = LaunchConfig {
            grid_dim: (
                (count * period_groups().len() * (WEATHER_STATES / self.loudness_block_states))
                    as u32,
                1,
                1,
            ),
            block_dim: (self.loudness_block_states as u32 * 32, 1, 1),
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
            kernel("pixel_mixtures")?.launch_on_stream(
                &stream,
                mixtures_config,
                (&input, &offsets, &mixtures),
            )
        })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten states a block where two blocks of ten fit (96 KB and 100 KB multiprocessors), two in a
    /// 64 KB one; every count divides the states.
    #[test]
    fn a_block_takes_every_state_where_two_blocks_fit() {
        assert_eq!(block_states(96 * 1024), WEATHER_STATES);
        assert_eq!(block_states(100 * 1024), WEATHER_STATES);
        assert_eq!(block_states(64 * 1024), 2);
        assert_eq!(WEATHER_STATES % block_states(64 * 1024), 0);
    }
}

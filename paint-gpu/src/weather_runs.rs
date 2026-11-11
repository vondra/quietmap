//! The mixtures' blocks, one a pixel's period and group of hours with a warp each of the period's
//! distinct weather states, read from the pixels' packed floats (kernels/distribution.cuh's layout).

use paint::exact::LAYERS;
use physics::bands::PERIODS;
use popup::percentiles::WEATHER_STATES;

/// Floats a packed line takes: energy per period, its mean in each state per period, lambda per
/// period, its profile, its duty per period.
const LINE_FLOATS: usize = PERIODS + PERIODS * WEATHER_STATES + PERIODS + 1 + PERIODS;

/// A pixel's packed weather: per period the first state of each run of states in which every line
/// of every layer has the same mean (as the card reads them), and whether a line follows the hours.
fn pixel_runs(pixel: &[f32]) -> ([Vec<usize>; PERIODS], bool) {
    let mut lines = Vec::new();
    let mut at = 0;
    for _ in 0..LAYERS {
        let count = pixel[at + PERIODS] as usize;
        let start = at + PERIODS + 1;
        lines.extend(
            (0..count).map(|i| &pixel[start + i * LINE_FLOATS..start + (i + 1) * LINE_FLOATS]),
        );
        at = start + count * LINE_FLOATS;
    }
    let profiled = (lines.iter()).any(|line| line[2 * PERIODS + PERIODS * WEATHER_STATES] >= 0.0);
    let heads = std::array::from_fn(|period| {
        let mean = |line: &[f32], state: usize| line[PERIODS + period * WEATHER_STATES + state];
        (0..WEATHER_STATES)
            .filter(|&state| {
                state == 0
                    || lines
                        .iter()
                        .any(|line| mean(line, state) != mean(line, state - 1))
            })
            .collect()
    });
    (heads, profiled)
}

/// The blocks of `pixels` (each its packed floats) by their number of states: per count n (1 to
/// WEATHER_STATES) where its blocks begin in `tasks` and `runs` and how many; `tasks` each block's
/// pixel * period groups + its index in `period_groups`; `runs` per block its first states, then the
/// runs' lengths (WEATHER_STATES bytes each, unused zero). A pixel none of whose lines follows the
/// hours has one group a period.
pub(crate) fn mixture_blocks<'a>(
    pixels: impl Iterator<Item = &'a [f32]>,
    period_groups: &[(usize, usize)],
) -> (Vec<(usize, usize)>, Vec<u32>, Vec<u8>) {
    let mut by_states: Vec<Vec<(u32, [u8; 2 * WEATHER_STATES])>> =
        vec![Vec::new(); WEATHER_STATES + 1];
    for (pixel, floats) in pixels.enumerate() {
        let (heads, profiled) = pixel_runs(floats);
        for (k, &(period, group)) in period_groups.iter().enumerate() {
            if group > 0 && !profiled {
                continue;
            }
            let heads = &heads[period];
            let mut run = [0u8; 2 * WEATHER_STATES];
            for (i, &head) in heads.iter().enumerate() {
                let end = heads.get(i + 1).copied().unwrap_or(WEATHER_STATES);
                run[i] = head as u8;
                run[WEATHER_STATES + i] = (end - head) as u8;
            }
            by_states[heads.len()].push(((pixel * period_groups.len() + k) as u32, run));
        }
    }
    let (mut spans, mut tasks, mut runs) = (Vec::new(), Vec::new(), Vec::new());
    for blocks in &by_states {
        spans.push((tasks.len(), blocks.len()));
        for (task, run) in blocks {
            tasks.push(*task);
            runs.extend_from_slice(run);
        }
    }
    (spans, tasks, runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packed pixel of one line in the first layer (`means` its ten state means every period,
    /// `profile` -1 for none), the other layers empty.
    fn pixel(means: [f32; WEATHER_STATES], profile: f32) -> Vec<f32> {
        let mut floats = vec![0.0; PERIODS];
        floats.push(1.0);
        floats.extend([1.0; PERIODS]);
        for _ in 0..PERIODS {
            floats.extend(means);
        }
        floats.extend([1.0; PERIODS]);
        floats.push(profile);
        floats.extend([f32::NAN; PERIODS]);
        for _ in 1..LAYERS {
            floats.extend([0.0; PERIODS]);
            floats.push(0.0);
        }
        floats
    }

    /// Equal consecutive states share a block's warp with their run's length; a pixel without
    /// hourly profiles takes only each period's first group of hours.
    #[test]
    fn a_block_takes_a_warp_for_each_run_of_equal_states() {
        let groups = [(0, 0), (0, 1), (1, 0), (2, 0), (2, 1)];
        let runs = pixel([3.0, 3.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0, 0.5], -1.0);
        let steady = pixel([1.0; WEATHER_STATES], 0.0);
        let pixels = [runs.as_slice(), steady.as_slice()];
        let (spans, tasks, bytes) = mixture_blocks(pixels.into_iter(), &groups);
        // The first pixel: four runs in each of its three periods' first groups.
        assert_eq!(spans[4], (5, 3));
        assert_eq!(&tasks[5..8], &[0, 2, 3]);
        assert_eq!(
            &bytes[5 * 20..5 * 20 + 20],
            &[0, 2, 5, 9, 0, 0, 0, 0, 0, 0, 2, 3, 4, 1, 0, 0, 0, 0, 0, 0]
        );
        // The second, profiled: one state in every group of hours.
        assert_eq!(spans[1], (0, 5));
        assert_eq!(&tasks[0..5], &[5, 6, 7, 8, 9]);
        assert_eq!(bytes[10], 10);
    }
}

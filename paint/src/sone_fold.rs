//! Map-resolution line folding, weather-state merging and shared hourly-profile group tables.

use physics::bands::PERIODS;
use physics::percentile::AtProbabilities;
use popup::loudness::PlacedLayer;
use popup::percentiles::{
    HELD_EXCEEDED, HOLDS_MEAN_LAMBDA, Line, PASSES_HOLD_MEAN, Resolution, WEATHER_STATES,
    hour_factor, hour_groups, merge_close_states,
};
use std::sync::OnceLock;

/// Per-worker scratch reused across pixels when folding lines at `Resolution::MAP`.
#[derive(Default)]
pub(crate) struct FoldScratch {
    duty_candidates: Vec<(f64, usize)>,
    keep: Vec<bool>,
}

/// Hourly traffic profiles (`popup::percentiles::hour_factor` rows).
pub const MAP_PROFILES: usize = 3;
/// Upper bound on hour groups per period at `Resolution::MAP` (currently `[4, 1, 3]`), checked once
/// at initialization so per-pixel folding stays stack-allocated.
const MAX_MAP_HOUR_GROUPS: usize = 4;

/// Map-resolution groups of hours per period and each hourly profile's mean flow factor per
/// `(profile, period, group)`, shared by map-resolution line folding (`paint::sone`) and the
/// card's loudness tables (`paint_gpu::loudness`).
pub struct MapHourGroups {
    pub groups: [Vec<Vec<usize>>; PERIODS],
    pub groups_max: usize,
    pub group_slots: Vec<usize>,
    /// Row-major `[(profile * PERIODS + period) * groups_max + group]`.
    pub factors: Vec<f64>,
}

impl MapHourGroups {
    pub fn factor(&self, profile: usize, period: usize, group: usize) -> f64 {
        self.factors[(profile * PERIODS + period) * self.groups_max + group]
    }
}

/// Shared map-resolution hour groups and mean profile factors, computed once from
/// `popup::percentiles::{hour_groups, hour_factor}`.
pub fn map_hour_groups() -> &'static MapHourGroups {
    static HOURS: OnceLock<MapHourGroups> = OnceLock::new();
    HOURS.get_or_init(|| {
        let groups: [Vec<Vec<usize>>; PERIODS] =
            std::array::from_fn(|period| hour_groups(true, period, Resolution::MAP.hour_groups).0);
        let groups_max = groups.iter().map(Vec::len).max().unwrap_or(1);
        assert!(
            groups_max <= MAX_MAP_HOUR_GROUPS,
            "Resolution::MAP hour groups ({groups_max}) exceed MAX_MAP_HOUR_GROUPS"
        );
        let mut group_slots = vec![0; PERIODS * groups_max];
        let mut factors = vec![1.0; MAP_PROFILES * PERIODS * groups_max];
        for (period, slots_by_group) in groups.iter().enumerate() {
            for (group, slots) in slots_by_group.iter().enumerate() {
                group_slots[period * groups_max + group] = slots.len();
                for profile in 0..MAP_PROFILES {
                    factors[(profile * PERIODS + period) * groups_max + group] = slots
                        .iter()
                        .map(|&slot| hour_factor(profile, period, slot))
                        .sum::<f64>()
                        / slots.len() as f64;
                }
            }
        }
        MapHourGroups {
            groups,
            groups_max,
            group_slots,
            factors,
        }
    })
}

fn map_exceeded_table() -> &'static AtProbabilities {
    static EXCEEDED: OnceLock<AtProbabilities> = OnceLock::new();
    EXCEEDED.get_or_init(|| AtProbabilities::new(&[1.0 - HELD_EXCEEDED]))
}

/// The map's weather states apart only where some source's mean differs by more than this between
/// them (`merge_close_states`): `1.5 dB = 10^(1.5 / 10)`. Chosen below one 2 dB map loudness bin
/// (`Resolution::MAP.bin_db`, widened from 1.0 dB); the combined release kept 0.194 % of visible
/// Nden cells > 1 phon against the earlier painter and 97.68 % of visible popup clicks within
/// 1 phon in the 54-square world sample (2026-11-26).
pub(crate) const MAP_WEATHER_SPREAD: f64 = 1.412_537_544_622_754_4; // 10^(1.5 / 10)

/// Maximum ON-peak share of a layer's energy for a weak `duty` line to be eligible for steady
/// folding (`0.12 -> 10 log10(1.12) = 0.49 dB` single-source on-peak ratio, below one 2 dB map
/// bin). Chosen together with [`DUTY_FOLD_STD_SHARE`] to collapse minor intermittent sources; the
/// combined release kept visible Nden > 1 phon at 0.194 % and popup clicks within 1 phon at
/// 97.68 % in the 54-square world sample (2026-11-26).
const DUTY_FOLD_PEAK_SHARE: f64 = 0.12;
/// Chosen maximum combined standard-deviation share of a layer's energy across all folded `duty`
/// lines in a period and weather state (`0.08 -> 10 log10(1.08) = 0.33 dB` combined RMS
/// fluctuation budget, preventing many individually weak duty lines from accumulating noticeable
/// variance).
const DUTY_FOLD_STD_SHARE: f64 = 0.08;
/// Chosen per-line peak share of a layer's non-duty/non-passing mean energy (`held`) under which
/// a sparse Kurze line's 1-in-1000 peak (`HELD_EXCEEDED`) folds into the layer's steady component
/// for the map (raised from `HELD_SHARE = 0.01`). For a single line beside steady energy alone,
/// `0.05` corresponds to `10 log10(1.05) = 0.21 dB`, but this is not a strict tail bound because
/// `held` sums all non-duty/non-passing lines (including other potentially moving Kurze lines)
/// and folded lines are not jointly budgeted. The combined release kept 97.68 % of visible popup
/// clicks within 1 phon and 99.97 % within 3 phon in the 54-square world sample (2026-11-26).
const KURZE_FOLD_HELD_SHARE: f64 = 0.05;

/// Applies the map's single folding and weather-merging rule in place, shared by both the cores
/// (`ndens`) and the card (`pack`): folds non-moving pass streams, always-held and sparse Kurze
/// lines (`KURZE_FOLD_HELD_SHARE`), and variance-bounded weak `duty` lines
/// (`DUTY_FOLD_PEAK_SHARE`, `DUTY_FOLD_STD_SHARE`) per hourly profile, then merges consecutive
/// weather states staying within [`MAP_WEATHER_SPREAD`] across all kept lines and collapses
/// weather-invariant unprofiled steady sums back into each layer's steady floor.
pub(crate) fn fold_placed_for_map(placed: &mut [PlacedLayer], scratch: &mut FoldScratch) {
    let hours = map_hour_groups();
    let exceeded = map_exceeded_table();
    let any_profiled = placed
        .iter()
        .any(|(lines, _)| lines.iter().any(|line| line.parts().profile.is_some()));
    for (lines, steady) in placed.iter_mut() {
        if lines.is_empty() {
            continue;
        }
        let mut has_kurze = false;
        let mut has_duty = false;
        for line in lines.iter() {
            let p = line.parts();
            if p.duty.is_some() {
                has_duty = true;
            } else if p.passing.is_none() && p.lambda.iter().any(|&lam| lam.is_finite()) {
                has_kurze = true;
            }
        }
        // Per-(period, group, state) sum of steady and non-duty/non-passing line means (used as
        // the denominator for sparse Kurze line folding when any Kurze line exists).
        let mut held = [[[0.0f64; WEATHER_STATES]; MAX_MAP_HOUR_GROUPS]; PERIODS];
        if has_kurze {
            for (p, period_held) in held.iter_mut().enumerate() {
                let ng = if any_profiled {
                    hours.groups[p].len()
                } else {
                    1
                };
                for group_held in period_held.iter_mut().take(ng) {
                    group_held.fill(steady[p]);
                }
            }
            for line in lines.iter() {
                let parts = line.parts();
                if parts.duty.is_some() || parts.passing.is_some() {
                    continue;
                }
                for (p, period_held) in held.iter_mut().enumerate() {
                    if parts.energy[p] <= 0.0 {
                        continue;
                    }
                    let ng = if any_profiled {
                        hours.groups[p].len()
                    } else {
                        1
                    };
                    for (g, group_held) in period_held.iter_mut().enumerate().take(ng) {
                        let factor = match (any_profiled, parts.profile) {
                            (true, Some(prof)) => hours.factor(prof, p, g),
                            _ => 1.0,
                        };
                        for (dst, &mean) in group_held.iter_mut().zip(&parts.means[p]) {
                            let m = mean * factor;
                            if m > 0.0 {
                                *dst += m;
                            }
                        }
                    }
                }
            }
        }
        // Per-(period, state) total layer energy when any `duty` line exists.
        let mut layer_total = [[0.0f64; WEATHER_STATES]; PERIODS];
        if has_duty {
            for (p, dst) in layer_total.iter_mut().enumerate() {
                dst.fill(steady[p]);
            }
            for line in lines.iter() {
                let parts = line.parts();
                for (p, dst_period) in layer_total.iter_mut().enumerate() {
                    if parts.energy[p] <= 0.0 {
                        continue;
                    }
                    for (dst, &mean) in dst_period.iter_mut().zip(&parts.means[p]) {
                        if mean > 0.0 {
                            *dst += mean;
                        }
                    }
                }
            }
        }
        let mut unprof_means = [[0.0f64; WEATHER_STATES]; PERIODS];
        let mut has_unprof = false;
        let mut prof_means = [[[0.0f64; WEATHER_STATES]; PERIODS]; MAP_PROFILES];
        let mut has_prof = [false; MAP_PROFILES];
        scratch.keep.clear();
        scratch.keep.resize(lines.len(), false);
        scratch.duty_candidates.clear();
        for (idx, line) in lines.iter().enumerate() {
            let parts = line.parts();
            if let Some(duty) = parts.duty {
                let mut moves = false;
                let mut eligible = true;
                let mut max_var_ratio = 0.0f64;
                for p in 0..PERIODS {
                    if parts.energy[p] <= 0.0 || duty[p] <= 0.0 {
                        continue;
                    }
                    let d = duty[p];
                    let var_scale = (1.0 - d).max(0.0) / d;
                    for (s, &mean) in parts.means[p].iter().enumerate() {
                        if mean <= 0.0 {
                            continue;
                        }
                        moves = true;
                        let total = layer_total[p][s];
                        if total <= 0.0 || mean / d > DUTY_FOLD_PEAK_SHARE * total {
                            eligible = false;
                        } else {
                            let r = mean / total;
                            max_var_ratio = max_var_ratio.max(r * r * var_scale);
                        }
                    }
                }
                if moves {
                    if eligible {
                        scratch.duty_candidates.push((max_var_ratio, idx));
                    } else {
                        scratch.keep[idx] = true;
                    }
                }
                continue;
            }
            if let Some(passing) = parts.passing {
                let moves = (0..PERIODS).any(|p| {
                    parts.energy[p] > 0.0
                        && passing[p] > 0.0
                        && passing[p] < PASSES_HOLD_MEAN
                        && parts.means[p].iter().any(|&m| m > 0.0)
                });
                if moves {
                    scratch.keep[idx] = true;
                } else {
                    for (p, dst_period) in unprof_means.iter_mut().enumerate() {
                        if parts.energy[p] <= 0.0 {
                            continue;
                        }
                        for (dst, &mean) in dst_period.iter_mut().zip(&parts.means[p]) {
                            if mean > 0.0 {
                                *dst += mean;
                                has_unprof = true;
                            }
                        }
                    }
                }
                continue;
            }
            // Steady or Kurze line: check whether it is guaranteed held in every (p, g, s).
            let mut always_held = true;
            'check: for (p, period_held) in held.iter().enumerate() {
                if parts.energy[p] <= 0.0 {
                    continue;
                }
                let ng = if any_profiled {
                    hours.groups[p].len()
                } else {
                    1
                };
                for (g, group_held) in period_held.iter().enumerate().take(ng) {
                    let factor = match (any_profiled, parts.profile) {
                        (true, Some(prof)) => hours.factor(prof, p, g),
                        _ => 1.0,
                    };
                    let lam = parts.lambda[p] * factor;
                    if lam >= HOLDS_MEAN_LAMBDA {
                        continue;
                    }
                    let exc = exceeded.relative_intensities(lam).next().unwrap_or(1.0);
                    for (&h, &mean) in group_held.iter().zip(&parts.means[p]) {
                        let m = mean * factor;
                        if m > 0.0
                            && (m * exc >= KURZE_FOLD_HELD_SHARE * h || !(m * exc).is_finite())
                        {
                            always_held = false;
                            break 'check;
                        }
                    }
                }
            }
            if always_held {
                for p in 0..PERIODS {
                    if parts.energy[p] <= 0.0 {
                        continue;
                    }
                    let target = match parts.profile {
                        Some(prof) => {
                            has_prof[prof] = true;
                            &mut prof_means[prof][p]
                        }
                        None => {
                            has_unprof = true;
                            &mut unprof_means[p]
                        }
                    };
                    for (dst, &mean) in target.iter_mut().zip(&parts.means[p]) {
                        if mean > 0.0 {
                            *dst += mean;
                        }
                    }
                }
            } else {
                scratch.keep[idx] = true;
            }
        }
        if !scratch.duty_candidates.is_empty() {
            scratch
                .duty_candidates
                .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let mut folded_var = [[0.0f64; WEATHER_STATES]; PERIODS];
            let budget_sq = DUTY_FOLD_STD_SHARE * DUTY_FOLD_STD_SHARE;
            for &(_, idx) in &scratch.duty_candidates {
                let parts = lines[idx].parts();
                let duty = parts.duty.expect("duty candidate");
                let fits = (0..PERIODS).all(|p| {
                    if parts.energy[p] <= 0.0 || duty[p] <= 0.0 {
                        return true;
                    }
                    let var_scale = (1.0 - duty[p]).max(0.0) / duty[p];
                    (0..WEATHER_STATES).all(|s| {
                        let m = parts.means[p][s];
                        if m <= 0.0 {
                            return true;
                        }
                        let total = layer_total[p][s];
                        folded_var[p][s] + m * m * var_scale <= budget_sq * total * total
                    })
                });
                if fits {
                    for p in 0..PERIODS {
                        if parts.energy[p] <= 0.0 || duty[p] <= 0.0 {
                            continue;
                        }
                        let var_scale = (1.0 - duty[p]).max(0.0) / duty[p];
                        for s in 0..WEATHER_STATES {
                            let m = parts.means[p][s];
                            if m > 0.0 {
                                folded_var[p][s] += m * m * var_scale;
                                unprof_means[p][s] += m;
                                has_unprof = true;
                            }
                        }
                    }
                } else {
                    scratch.keep[idx] = true;
                }
            }
        }
        // Compact kept lines in place and prepend folded steady sums without cloning `Line`s or
        // allocating a replacement vector (each added steady line replaces at least one folded
        // line, so the existing capacity suffices).
        let mut write = 0usize;
        for read in 0..lines.len() {
            if scratch.keep[read] {
                if write != read {
                    lines.swap(write, read);
                }
                write += 1;
            }
        }
        lines.truncate(write);
        let prefix = usize::from(has_unprof) + has_prof.iter().filter(|&&b| b).count();
        if prefix > 0 {
            if has_unprof {
                lines.push(Line::steady(unprof_means, None));
            }
            for (prof, &present) in has_prof.iter().enumerate() {
                if present {
                    lines.push(Line::steady(prof_means[prof], Some(prof)));
                }
            }
            lines.rotate_right(prefix);
        }
    }
    merge_close_states(placed, MAP_WEATHER_SPREAD);
    // Collapse an unprofiled steady line whose weather states all merged to one value back into
    // the layer's steady floor so both cores and card avoid an extra steady line.
    for (layer_lines, steady) in placed.iter_mut() {
        if let Some(first) = layer_lines.first() {
            let parts = first.parts();
            if parts.profile.is_none()
                && parts.duty.is_none()
                && parts.passing.is_none()
                && parts.lambda.iter().all(|&lam| !lam.is_finite())
                && parts
                    .means
                    .iter()
                    .all(|period| period.iter().all(|&m| m as f32 == period[0] as f32))
            {
                for (dst, src) in steady.iter_mut().zip(&parts.means) {
                    *dst += src.iter().sum::<f64>() / src.len() as f64;
                }
                layer_lines.remove(0);
            }
        }
    }
}

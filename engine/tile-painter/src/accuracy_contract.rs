//! The accuracy contract of `docs/dev/accuracy-contract.md` (owner, 2026-08-10) as a
//! scorer: two HM3 tiles in, a PASS/FAIL verdict out, so a measurement produces a
//! verdict instead of a table someone has to interpret.
//!
//! Error is `candidate − reference` per cell, banded by the REFERENCE cell's level —
//! not the candidate's and not `max(ref, cand)`; that ambiguity is closed. The rungs
//! are CUMULATIVE (each counts every cell above its threshold, so they nest) and their
//! budgets are fractions of the WHOLE tile, which is how the owner's worked example
//! reads: 20 % of 262,144 = 52,429 cells.
//!
//! Wave 1's amplitude rungs are GUIDANCE — in the draft wave speed wins and wave 2
//! repairs the accuracy, so an overshoot is reported, not failed. Three things are hard
//! in BOTH waves because wave 2 cannot repair them: qualifying presence flips,
//! systematic one-sided bias, and physics deleted rather than approximated. Only the
//! first two are visible in two tiles; the third is a code-review gate.

use crate::wire_hm3::{dequantise_lden, NO_DATA};

/// Level at which the palette starts painting: `frontend/src/lib/heatmap-palette.ts:24`,
/// `STOPS[0].db = 30`. Below it the map draws nothing, so error there is only ever a
/// presence question. Pinned here so the scorer and the palette cannot drift apart
/// silently — if that stop moves, this constant moves with it.
pub const PAINT_FLOOR_DB: f64 = 30.0;

/// How far clear of [`PAINT_FLOOR_DB`] a reference cell must sit before a flip counts.
/// A 0.5 dB quantum straddling a hard threshold flips on rounding alone, so a
/// zero-flip gate without this clearance fails the unmodified kernel itself.
pub const FLIP_CLEARANCE_DB: f64 = 1.0;

/// Hard bound on systematic one-sided bias (owner, 2026-08-10), measured as the signed
/// mean over PAINTED cells — reference ≥30 dB — in one tile.
///
/// Why bias needs a bound of its OWN, separate from the amplitude ladder: a uniform dB
/// offset passes through energy averaging **unchanged**. `build-pyramid` averages
/// energy, so a uniform `+δ` scales every cell by `10^(δ/10)`, scales the mean by the
/// same factor, and comes back out as `L + δ`. The offset therefore reaches EVERY
/// overview zoom at full strength, while the ladder — which only ever sees a small
/// per-cell magnitude — cannot detect it. Reach, not size, is what makes it dangerous.
///
/// At 0.25 dB (wave 2) the offset can move a cell by at most ONE storage step, and only
/// cells already within 0.25 dB of a byte boundary: a one-step shade change on a
/// minority of cells at every zoom, which reads as a slight uniform tint and never as
/// structure. At 0.5 dB (wave 1) it can move EVERY cell by one step — the most a draft
/// wave may cost a viewer.
///
/// These are CHOSEN bounds calibrated to the storage quantum, not derived from a
/// perception study, and the owner may move them. What is not negotiable is that a bias
/// bound exists and is reported for every configuration.
pub const MAX_SIGNED_MEAN_DB_WAVE_TWO: f64 = 0.25;
/// See [`MAX_SIGNED_MEAN_DB_WAVE_TWO`] — the draft wave's looser bound.
pub const MAX_SIGNED_MEAN_DB_WAVE_ONE: f64 = 0.5;

/// Presence flips carry the SAME allowance as the top amplitude rung — 0.01 %, ≈26
/// cells on a full tile — in both waves (owner ruling 2026-08-10). A flip is a real
/// defect, but a handful of them is not a different CLASS of wrongness from a handful
/// of large amplitude errors, and the ladder is the owner's own calibration of how much
/// visible error a wave may carry. Zero tolerance here would have made 3 flipped cells
/// stricter than 26 cells ten times worse in amplitude.
pub const FLIP_MAX_FRACTION: f64 = 0.0001;

/// Distinct byte differences an HM3 pair can show: cells are `0..=254` (255 is
/// `NO_DATA`), so `|Δbyte|` lands in `0..=254`.
const ABS_DIFF_BUCKETS: usize = 255;

/// One cumulative rung: at most `max_fraction` of the tile's cells may exceed
/// `over_db` in the ≥30 dB band.
#[derive(Clone, Copy, Debug)]
pub struct Rung {
    pub over_db: f64,
    pub max_fraction: f64,
}

/// Which of the two heatmap contracts to score against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    /// Draft z12: double wave 2's amplitudes, tiers are guidance.
    One,
    /// Accurate (+ z13 city tier): the popup's physics, every tier hard.
    Two,
}

impl Wave {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "1" => Some(Wave::One),
            "2" => Some(Wave::Two),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Wave::One => "1 (draft)",
            Wave::Two => "2 (accurate)",
        }
    }

    /// Flat cap on the invisible band, where nothing is painted.
    pub fn quiet_band_max_db(self) -> f64 {
        match self {
            Wave::One => 20.0,
            Wave::Two => 10.0,
        }
    }

    /// The cumulative ladder. The last rung is the ceiling: wave 2 forbids anything
    /// above 6 dB outright, wave 1 allows 0.01 % of cells above 12 dB with NO upper
    /// bound — which is why a draft candidate is decided by that COUNT, not by its max.
    pub fn rungs(self) -> [Rung; 4] {
        let rung = |over_db, max_fraction| Rung {
            over_db,
            max_fraction,
        };
        match self {
            Wave::Two => [
                rung(0.5, 0.20),
                rung(1.0, 0.01),
                rung(3.0, 0.0001),
                rung(6.0, 0.0),
            ],
            Wave::One => [
                rung(1.0, 0.20),
                rung(2.0, 0.01),
                rung(6.0, 0.0001),
                rung(12.0, 0.0001),
            ],
        }
    }

    /// Wave 2 fails on amplitude; wave 1 reports the overshoot and passes.
    pub fn amplitude_is_binding(self) -> bool {
        self == Wave::Two
    }

    /// Bound on the signed mean over painted cells. Unlike the amplitude tiers this
    /// does NOT soften for the draft — it widens to exactly one storage step, and then
    /// binds.
    pub fn max_signed_mean_db(self) -> f64 {
        match self {
            Wave::One => MAX_SIGNED_MEAN_DB_WAVE_ONE,
            Wave::Two => MAX_SIGNED_MEAN_DB_WAVE_TWO,
        }
    }
}

/// Which reference the candidate was measured against. Label only — the ladder is the
/// same — but it must be stated, because a clean marginal beside a failing absolute
/// says the pre-existing fork is the blocker, not the change under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scoring {
    /// vs the popup/CPU reference. This is the contract.
    Absolute,
    /// vs the unmodified kernel. Isolates what this change itself did.
    Marginal,
}

impl Scoring {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "absolute" => Some(Scoring::Absolute),
            "marginal" => Some(Scoring::Marginal),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scoring::Absolute => "absolute",
            Scoring::Marginal => "marginal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// Wave 1 only: amplitude over a rung, which is guidance, not grounds for
    /// rejection.
    PassWithOvershoot,
    Fail,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::PassWithOvershoot => "PASS_WITH_OVERSHOOT",
            Verdict::Fail => "FAIL",
        }
    }
}

/// Error accounting for one level band. Both bands are accumulated identically so the
/// contract's per-band numbers and the whole-tile statistics line read off the SAME
/// pass — no second, quietly divergent accounting.
#[derive(Clone, Debug)]
pub struct Band {
    /// Cells present in both tiles that fall in this band.
    pub cells: usize,
    pub abs_sum_db: f64,
    pub signed_sum_db: f64,
    pub max_abs_db: f64,
    /// Cells that moved at all, and how many of those got louder — a mean of 0.03 dB
    /// that is 50/50 and one that is 97 % one way are different products.
    pub moved: usize,
    pub cand_louder: usize,
    /// `|Δbyte|` histogram. Errors are exact multiples of the 0.5 dB quantum, so 255
    /// buckets answer "how many cells exceed X dB" for EVERY threshold exactly, and
    /// both waves' ladders read off one pass.
    abs_diff_hist: [u32; ABS_DIFF_BUCKETS],
}

impl Band {
    fn new() -> Self {
        Band {
            cells: 0,
            abs_sum_db: 0.0,
            signed_sum_db: 0.0,
            max_abs_db: 0.0,
            moved: 0,
            cand_louder: 0,
            abs_diff_hist: [0; ABS_DIFF_BUCKETS],
        }
    }

    fn add(&mut self, delta_bytes: i16) {
        let signed = f64::from(delta_bytes) * 0.5;
        self.cells += 1;
        self.abs_sum_db += signed.abs();
        self.signed_sum_db += signed;
        self.max_abs_db = self.max_abs_db.max(signed.abs());
        if delta_bytes != 0 {
            self.moved += 1;
            self.cand_louder += usize::from(delta_bytes > 0);
        }
        self.abs_diff_hist[usize::from(delta_bytes.unsigned_abs())] += 1;
    }

    /// Cells whose error exceeds `over_db`. Exact: the smallest byte step that can
    /// exceed a threshold is `floor(over_db × 2) + 1`.
    pub fn cells_over(&self, over_db: f64) -> usize {
        let first_bucket = (over_db * 2.0).floor() as usize + 1;
        if first_bucket >= ABS_DIFF_BUCKETS {
            return 0;
        }
        self.abs_diff_hist[first_bucket..]
            .iter()
            .map(|&n| n as usize)
            .sum()
    }

    pub fn signed_mean_db(&self) -> f64 {
        if self.cells > 0 {
            self.signed_sum_db / self.cells as f64
        } else {
            0.0
        }
    }

    pub fn cand_louder_pct(&self) -> f64 {
        if self.moved > 0 {
            100.0 * self.cand_louder as f64 / self.moved as f64
        } else {
            0.0
        }
    }
}

/// Everything the contract needs to know about one pair of tiles.
#[derive(Clone, Debug)]
pub struct Score {
    /// Every cell in the tile — the denominator for every rung budget.
    pub cells: usize,
    /// Reference ≥30 dB: the painted band, where the ladder applies.
    pub loud: Band,
    /// Reference <30 dB: invisible, so only a flat cap applies.
    pub quiet: Band,
    /// Cells where exactly one side is `NO_DATA`, for the statistics line.
    pub presence_changed: usize,
    /// Flips across the 30 dB paint edge where the reference sits ≥1 dB clear of it.
    pub qualifying_flips: usize,
    /// Silence painted as noise.
    pub flips_newly_painted: usize,
    /// Audible content that vanished.
    pub flips_newly_silent: usize,
}

/// Is this cell painted on the map? `NO_DATA` is silence, and so is anything under the
/// palette floor.
fn is_painted(byte: u8) -> bool {
    byte != NO_DATA && dequantise_lden(byte) >= PAINT_FLOOR_DB
}

/// Budget in whole cells, rounded to nearest — the rule that reproduces the owner's
/// worked example (20 % of 262,144 = 52,429).
pub fn allowance(cells: usize, fraction: f64) -> usize {
    (cells as f64 * fraction).round() as usize
}

/// Score a candidate tile against a reference tile. Both are dense `u8 × 0.5 dB` HM3
/// cell arrays of equal length.
pub fn score(reference: &[u8], candidate: &[u8]) -> Score {
    let mut s = Score {
        cells: reference.len(),
        loud: Band::new(),
        quiet: Band::new(),
        presence_changed: 0,
        qualifying_flips: 0,
        flips_newly_painted: 0,
        flips_newly_silent: 0,
    };

    for (&r, &c) in reference.iter().zip(candidate.iter()) {
        // Presence is judged at the PAINT floor, not at the NO_DATA sentinel: what a
        // visitor sees flip is colour appearing or disappearing, not a byte.
        let r_painted = is_painted(r);
        if r_painted != is_painted(c) {
            // An absent reference is fully clear below the edge, so painting noise
            // over silence always counts.
            // The clearance exemption is for ROUNDING noise across the threshold, so it
            // requires both cells to actually have a value near the edge. If either
            // side is absent the difference is structural, not rounding, and always
            // counts — otherwise a candidate that drops every near-edge cell to
            // NO_DATA erases painted content for free.
            let qualifies = r == NO_DATA
                || c == NO_DATA
                || (dequantise_lden(r) - PAINT_FLOOR_DB).abs() >= FLIP_CLEARANCE_DB;
            if qualifies {
                s.qualifying_flips += 1;
                if r_painted {
                    s.flips_newly_silent += 1;
                } else {
                    s.flips_newly_painted += 1;
                }
            }
        }

        if (r == NO_DATA) != (c == NO_DATA) {
            s.presence_changed += 1;
            continue;
        }
        if r == NO_DATA {
            continue;
        }
        let delta_bytes = i16::from(c) - i16::from(r);
        if r_painted {
            s.loud.add(delta_bytes);
        } else {
            s.quiet.add(delta_bytes);
        }
    }
    s
}

impl Score {
    /// Cells compared in either band — the statistics line's `both`.
    pub fn compared(&self) -> usize {
        self.loud.cells + self.quiet.cells
    }

    pub fn mean_abs_db(&self) -> f64 {
        let n = self.compared();
        if n > 0 {
            (self.loud.abs_sum_db + self.quiet.abs_sum_db) / n as f64
        } else {
            0.0
        }
    }

    pub fn max_abs_db(&self) -> f64 {
        self.loud.max_abs_db.max(self.quiet.max_abs_db)
    }

    pub fn signed_mean_db(&self) -> f64 {
        let n = self.compared();
        if n > 0 {
            (self.loud.signed_sum_db + self.quiet.signed_sum_db) / n as f64
        } else {
            0.0
        }
    }

    pub fn moved(&self) -> usize {
        self.loud.moved + self.quiet.moved
    }

    pub fn cand_louder_pct(&self) -> f64 {
        let moved = self.moved();
        if moved > 0 {
            100.0 * (self.loud.cand_louder + self.quiet.cand_louder) as f64 / moved as f64
        } else {
            0.0
        }
    }

    /// Cells over each rung of `wave`, in the painted band.
    pub fn count_rungs(&self, wave: Wave) -> [usize; 4] {
        let rungs = wave.rungs();
        [
            self.loud.cells_over(rungs[0].over_db),
            self.loud.cells_over(rungs[1].over_db),
            self.loud.cells_over(rungs[2].over_db),
            self.loud.cells_over(rungs[3].over_db),
        ]
    }

    /// The two gates that hold in BOTH waves and are visible in two tiles. (The third,
    /// physics deleted rather than approximated, is a code-review gate.)
    pub fn hard_gates_hold(&self, wave: Wave) -> bool {
        !self.flips_over_budget() && !self.bias_over_budget(wave)
    }

    /// Systematic lean beyond what this wave allows — see
    /// [`MAX_SIGNED_MEAN_DB_WAVE_TWO`] for why this is gated apart from the ladder.
    pub fn bias_over_budget(&self, wave: Wave) -> bool {
        self.loud.signed_mean_db().abs() > wave.max_signed_mean_db()
    }

    /// Cells that may flip across the paint edge before the tile fails — the same
    /// 0.01 % the top amplitude rung gets.
    pub fn flip_allowance(&self) -> usize {
        allowance(self.cells, FLIP_MAX_FRACTION)
    }

    pub fn flips_over_budget(&self) -> bool {
        self.qualifying_flips > self.flip_allowance()
    }

    /// Rung indices whose budget is exceeded.
    pub fn amplitude_overshoots(&self, wave: Wave) -> Vec<usize> {
        let counts = self.count_rungs(wave);
        wave.rungs()
            .iter()
            .enumerate()
            .filter(|(i, rung)| counts[*i] > allowance(self.cells, rung.max_fraction))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn quiet_band_over(&self, wave: Wave) -> bool {
        self.quiet.max_abs_db > wave.quiet_band_max_db()
    }

    pub fn verdict(&self, wave: Wave) -> Verdict {
        if !self.hard_gates_hold(wave) {
            return Verdict::Fail;
        }
        let amplitude_clean =
            self.amplitude_overshoots(wave).is_empty() && !self.quiet_band_over(wave);
        if amplitude_clean {
            Verdict::Pass
        } else if wave.amplitude_is_binding() {
            Verdict::Fail
        } else {
            Verdict::PassWithOvershoot
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire_hm3::quantise_lden;

    /// One full z12 HM3 tile: `grid::TILE_PX`² = 512², the denominator in the owner's
    /// worked example.
    const CELLS: usize = 262_144;
    const REF_DB: f64 = 60.0;

    /// A tile of [`CELLS`] cells all at [`REF_DB`], scored as the ≥30 dB band.
    fn reference_tile() -> Vec<u8> {
        vec![quantise_lden(REF_DB); CELLS]
    }

    /// A candidate built from disjoint groups of `(count, signed error in dB)`, applied
    /// from the start of the tile, so each cell's error is exactly the one asked for.
    fn candidate_with(groups: &[(usize, f64)]) -> Vec<u8> {
        let mut tile = reference_tile();
        let mut at = 0usize;
        for &(count, delta_db) in groups {
            for cell in tile.iter_mut().skip(at).take(count) {
                *cell = quantise_lden(REF_DB + delta_db);
            }
            at += count;
        }
        tile
    }

    /// `n` cells over `over_db`, split evenly in sign so the ladder is under test and
    /// the bias gate stays quiet.
    fn sign_split_over(n: usize, over_db: f64) -> Vec<u8> {
        let delta = over_db + 0.5;
        candidate_with(&[(n / 2, delta), (n - n / 2, -delta)])
    }

    fn rung_index(wave: Wave, over_db: f64) -> usize {
        wave.rungs()
            .iter()
            .position(|r| r.over_db == over_db)
            .expect("rung exists")
    }

    #[test]
    fn the_worked_example_allowances_match_the_contract() {
        // docs/dev/accuracy-contract.md §6: 262,144 cells → 52,429 / 2,621 / 26.
        assert_eq!(allowance(CELLS, 0.20), 52_429);
        assert_eq!(allowance(CELLS, 0.01), 2_621);
        assert_eq!(allowance(CELLS, 0.0001), 26);
    }

    #[test]
    fn an_identical_tile_passes_both_waves() {
        let reference = reference_tile();
        let s = score(&reference, &reference);
        assert_eq!(s.loud.cells, CELLS);
        assert_eq!(s.qualifying_flips, 0);
        assert_eq!(s.verdict(Wave::Two), Verdict::Pass);
        assert_eq!(s.verdict(Wave::One), Verdict::Pass);
    }

    #[test]
    fn half_a_db_is_the_quantum_and_does_not_exceed_the_baseline() {
        // 0.5 dB is ONE byte step, so it is AT wave 2's baseline, not over it —
        // invisible by construction, which is the whole argument for that number.
        let reference = reference_tile();
        let candidate = candidate_with(&[(CELLS / 2, 0.5), (CELLS / 2, -0.5)]);
        let s = score(&reference, &candidate);
        assert_eq!(s.loud.max_abs_db, 0.5);
        assert_eq!(s.loud.cells_over(0.5), 0, "0.5 dB does not EXCEED baseline");
        assert_eq!(s.loud.moved, CELLS);
        assert_eq!(s.verdict(Wave::Two), Verdict::Pass);
    }

    #[test]
    fn each_wave_two_rung_boundary_is_pinned() {
        let reference = reference_tile();
        for (over_db, fraction) in [(0.5, 0.20), (1.0, 0.01), (3.0, 0.0001)] {
            let allowed = allowance(CELLS, fraction);
            let index = rung_index(Wave::Two, over_db);

            // Exactly at the allowance: the budget is inclusive, so this passes.
            let s = score(&reference, &sign_split_over(allowed, over_db));
            assert_eq!(s.loud.cells_over(over_db), allowed, "rung {over_db} dB");
            assert!(
                s.amplitude_overshoots(Wave::Two).is_empty(),
                "{allowed} cells over {over_db} dB is exactly the budget"
            );
            assert_eq!(s.verdict(Wave::Two), Verdict::Pass);

            // One cell more, and wave 2 fails on that rung and no other.
            let s = score(&reference, &sign_split_over(allowed + 1, over_db));
            assert_eq!(s.loud.cells_over(over_db), allowed + 1);
            assert_eq!(s.amplitude_overshoots(Wave::Two), vec![index]);
            assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
        }
    }

    #[test]
    fn wave_two_ceiling_is_hard_and_wave_one_has_no_ceiling_at_all() {
        let reference = reference_tile();

        // One cell 7 dB out: over wave 2's 6 dB ceiling, well inside wave 1's ladder.
        let s = score(&reference, &candidate_with(&[(1, 7.0)]));
        assert_eq!(s.loud.cells_over(6.0), 1);
        assert_eq!(s.amplitude_overshoots(Wave::Two), vec![3]);
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail, "6 dB ceiling is hard");
        assert_eq!(s.loud.cells_over(12.0), 0);
        assert_eq!(s.verdict(Wave::One), Verdict::Pass);

        // Wave 1 tolerates an unbounded tail — but only 26 cells of it.
        let s = score(&reference, &candidate_with(&[(26, 40.0)]));
        assert_eq!(s.loud.cells_over(12.0), 26);
        assert!(s.loud.max_abs_db > 12.0, "no upper bound in wave 1");
        assert!(s.amplitude_overshoots(Wave::One).is_empty());
        assert_eq!(s.verdict(Wave::One), Verdict::Pass);

        // The 27th trips it. Both top rungs move together: >12 dB is NESTED inside
        // >6 dB and they share the 0.01 % allowance, so the count above 12 dB can
        // never fail on its own — it is reported because it decides a draft
        // configuration, not because it gates independently.
        let s = score(&reference, &candidate_with(&[(27, 40.0)]));
        assert_eq!(s.loud.cells_over(12.0), 27);
        assert_eq!(s.amplitude_overshoots(Wave::One), vec![2, 3]);
        assert_eq!(s.verdict(Wave::One), Verdict::PassWithOvershoot);
    }

    #[test]
    fn wave_one_amplitude_overshoot_reports_but_does_not_reject() {
        // In the draft wave speed wins and wave 2 repairs the accuracy, so a fast
        // configuration is not discarded over a marginal amplitude overshoot.
        let reference = reference_tile();
        let over_first_rung = allowance(CELLS, 0.20) + 10_000;
        let s = score(&reference, &sign_split_over(over_first_rung, 1.0));
        assert_eq!(s.amplitude_overshoots(Wave::One), vec![0]);
        assert!(s.hard_gates_hold(Wave::Two));
        assert_eq!(s.verdict(Wave::One), Verdict::PassWithOvershoot);
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
    }

    #[test]
    fn a_flip_counts_only_when_the_reference_is_clear_of_the_edge() {
        // Reference at 30.5 dB is inside the 1 dB clearance: edge noise, not a flip.
        let s = score(&[quantise_lden(30.5); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(s.qualifying_flips, 0, "edge noise is not a flip");

        // Reference at 31 dB is exactly 1 dB clear: audible content vanishing.
        let s = score(&[quantise_lden(31.0); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(s.flips_newly_silent, 4);
        assert_eq!(s.verdict(Wave::One), Verdict::Fail, "hard in both waves");
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);

        // Silence painted as noise: an absent reference is fully clear below the edge.
        let s = score(&[NO_DATA; 4], &[quantise_lden(45.0); 4]);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(s.flips_newly_painted, 4);
        assert_eq!(s.presence_changed, 4);
        assert_eq!(s.loud.cells, 0, "no reference value to compare against");
        // A 4-cell tile's 0.01 % allowance rounds to 0, so 4 flips is over budget.
        assert_eq!(s.flip_allowance(), 0);
        assert_eq!(s.verdict(Wave::One), Verdict::Fail);
    }

    #[test]
    fn an_absent_candidate_always_counts_however_close_the_reference_sits() {
        // The clearance exemption is for rounding across the edge, not for content
        // that disappears. Without this, a tile whose painted cells all sit in the
        // 29-31 dB window could be erased wholesale to NO_DATA and score a clean PASS:
        // no flip would qualify, and the presence mismatch skips amplitude and bias.
        let reference = vec![quantise_lden(30.5); CELLS];
        let s = score(&reference, &[NO_DATA; CELLS]);
        assert_eq!(s.qualifying_flips, CELLS, "a vanished tile is not free");
        assert_eq!(s.flips_newly_silent, CELLS);
        assert_eq!(s.loud.cells, 0, "nothing to compare — hence the flip gate");
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
        assert_eq!(s.verdict(Wave::One), Verdict::Fail);

        // Same in the other direction: silence filled in with near-edge paint.
        let s = score(&[NO_DATA; CELLS], &vec![quantise_lden(30.5); CELLS]);
        assert_eq!(s.qualifying_flips, CELLS);
        assert_eq!(s.flips_newly_painted, CELLS);
        assert_eq!(s.verdict(Wave::One), Verdict::Fail);

        // But a genuine rounding crossing, both sides present, is still exempt.
        let s = score(&[quantise_lden(30.5); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(s.qualifying_flips, 0);
    }

    #[test]
    fn flips_carry_the_top_rungs_allowance_in_both_waves() {
        // Owner ruling 2026-08-10: a handful of flips is not a different CLASS of
        // wrongness from a handful of large amplitude errors, so flips get the same
        // 0.01 % the top rung gets. Zero tolerance made 3 flipped cells stricter than
        // 26 cells ten times worse in amplitude.
        let mut reference = reference_tile();
        for cell in reference.iter_mut().take(64) {
            // Exactly 1 dB clear of the edge, so a flip here qualifies.
            *cell = quantise_lden(31.0);
        }
        let flip_n = |n: usize| {
            let mut candidate = reference.clone();
            for cell in candidate.iter_mut().take(n) {
                *cell = quantise_lden(29.5);
            }
            candidate
        };

        let s = score(&reference, &flip_n(26));
        assert_eq!(s.qualifying_flips, 26);
        assert_eq!(s.flip_allowance(), 26);
        assert!(!s.flips_over_budget(), "at the allowance, not over it");
        assert!(s.hard_gates_hold(Wave::Two));
        assert_eq!(s.verdict(Wave::Two), Verdict::Pass);
        assert_eq!(s.verdict(Wave::One), Verdict::Pass);

        let s = score(&reference, &flip_n(27));
        assert_eq!(s.qualifying_flips, 27);
        assert!(s.flips_over_budget());
        // Amplitudes are still spotless — only the flip budget fails the tile.
        assert!(s.amplitude_overshoots(Wave::Two).is_empty());
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
        assert_eq!(s.verdict(Wave::One), Verdict::Fail, "hard in both waves");
    }

    #[test]
    fn the_invisible_band_has_its_own_flat_cap() {
        // Reference below the paint floor: 12 dB of error is fine for wave 1 and not
        // for wave 2, and neither flips anything — the candidate stays unpainted.
        let s = score(&[quantise_lden(10.0); 16], &[quantise_lden(22.0); 16]);
        assert_eq!(s.quiet.cells, 16);
        assert_eq!(s.loud.cells, 0);
        assert_eq!(s.qualifying_flips, 0);
        assert_eq!(s.quiet.max_abs_db, 12.0);
        assert!(s.quiet_band_over(Wave::Two));
        assert!(!s.quiet_band_over(Wave::One));
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
        assert_eq!(s.verdict(Wave::One), Verdict::Pass);

        // Over even wave 1's cap, and still invisible on both sides (nothing crosses
        // the paint floor, so no flip). The sub-30 dB cap is an amplitude tier like any
        // other, so wave 1 REPORTS it and wave 2 fails on it.
        let s = score(&[quantise_lden(5.0); 16], &[quantise_lden(27.0); 16]);
        assert_eq!(s.quiet.max_abs_db, 22.0);
        assert_eq!(s.qualifying_flips, 0);
        assert!(s.quiet_band_over(Wave::One));
        assert_eq!(s.verdict(Wave::One), Verdict::PassWithOvershoot);
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
    }

    #[test]
    fn a_one_sided_lean_is_gated_apart_from_the_ladder_and_per_wave() {
        // 60 % of cells one step louder: max 0.5 dB is inside wave 2's baseline, so the
        // ladder is spotless. A uniform dB offset passes through energy averaging
        // UNCHANGED, so it reaches every overview zoom at full strength — the ladder
        // cannot see it and the bias bound is what catches it.
        let reference = reference_tile();
        let s = score(&reference, &candidate_with(&[(CELLS * 3 / 5, 0.5)]));
        assert_eq!(s.loud.max_abs_db, 0.5);
        assert!(s.amplitude_overshoots(Wave::Two).is_empty());
        assert!(s.loud.cand_louder_pct() > 99.0);
        let lean = s.loud.signed_mean_db();
        assert!((0.25..0.5).contains(&lean), "~0.3 dB lean, got {lean}");
        assert!(s.bias_over_budget(Wave::Two));
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
        // The draft wave allows a whole storage step of lean, so 0.3 dB is fine there.
        assert!(!s.bias_over_budget(Wave::One));
        assert_eq!(s.verdict(Wave::One), Verdict::Pass);

        // 60 % of cells a full 1.0 dB louder: nothing EXCEEDS wave 1's 1 dB baseline,
        // so its ladder is clean too and only the 0.6 dB lean fails the tile. Bias is
        // the one bound that does not soften into guidance for the draft.
        let s = score(&reference, &candidate_with(&[(CELLS * 3 / 5, 1.0)]));
        assert!(s.amplitude_overshoots(Wave::One).is_empty());
        assert!(s.loud.signed_mean_db() > 0.5);
        assert!(s.bias_over_budget(Wave::One));
        assert_eq!(
            s.verdict(Wave::One),
            Verdict::Fail,
            "bias binds in the draft"
        );
    }

    #[test]
    fn dobris_and_praha_score_as_the_contract_records_them() {
        let reference = reference_tile();

        // §7: Dobris rail 576 cells >0.5 dB (0.22 %), max 0.9704 dB → PASSES wave 2.
        // The recorded 0.9704 dB is the RAW pre-quantisation max; on the wire it is one
        // 1.0 dB step, which is what the ladder actually sees.
        let s = score(&reference, &candidate_with(&[(576, 1.0)]));
        assert_eq!(s.loud.cells_over(0.5), 576);
        assert_eq!(
            s.loud.cells_over(1.0),
            0,
            "1.0 dB does not exceed that rung"
        );
        assert!(
            s.hard_gates_hold(Wave::Two),
            "576 leaning cells in 262,144 is no bias"
        );
        assert_eq!(s.verdict(Wave::Two), Verdict::Pass);

        // §7: Praha rail 4,120 cells >1 dB (1.57 %, budget 1 %), max 16.5 dB → FAILS
        // wave 2 on the second rung AND on the hard 6 dB ceiling. 99 % candidate-louder
        // at this count is still only a 0.024 dB mean, so the bias gate is not what
        // fails here — the amplitudes are.
        let s = score(&reference, &candidate_with(&[(1, 16.5), (4_119, 1.5)]));
        assert_eq!(s.loud.cells_over(1.0), 4_120);
        assert!(s.loud.cells_over(1.0) > allowance(CELLS, 0.01));
        assert_eq!(s.loud.max_abs_db, 16.5);
        assert_eq!(s.loud.cells_over(6.0), 1, "over the hard ceiling");
        assert!(s.hard_gates_hold(Wave::Two));
        assert_eq!(s.amplitude_overshoots(Wave::Two), vec![1, 3]);
        assert_eq!(s.verdict(Wave::Two), Verdict::Fail);
    }
}

//! The accuracy contract enforced by this scorer: the complete ordered HM3 benchmark
//! in, one PASS/FAIL verdict out. A single
//! pair remains diagnostic and cannot qualify a release.
//!
//! Error is `candidate − reference` per cell, banded by the REFERENCE cell's level —
//! not the candidate's and not `max(ref, cand)`; that ambiguity is closed. The rungs
//! are CUMULATIVE (each counts every cell above its threshold, so they nest) and their
//! legacy single-pair budgets are fractions of the WHOLE tile. Release amplitude and
//! presence allowances use the reference's PAINTED cells across the whole benchmark.
//! Signed bias averages the subset with a numeric candidate value; `NO_DATA` has no dB
//! delta and is charged independently to the presence gate.
//!
//! Only three benchmark-wide families gate either wave: aggregate amplitude (the
//! painted ladder, Wave 2's unified extreme tail, and below-paint numeric drift),
//! aggregate presence-change area, and aggregate signed bias. Every per-row quantity
//! remains diagnostic; physics deleted rather than approximated is the separate
//! code-review gate.

use crate::wire_hm3::{dequantise_lden, NO_DATA};

/// Level at which the palette starts painting: `frontend/src/lib/heatmap-palette.ts:24`,
/// `STOPS[0].db = 30`. Below it the palette draws nothing, but hover still exposes the
/// numeric HM3 byte. Pinned here so the scorer and the palette cannot drift apart
/// silently — if that stop moves, this constant moves with it.
pub const PAINT_FLOOR_DB: f64 = 30.0;

/// How far clear of [`PAINT_FLOOR_DB`] either numeric side must sit before a crossing
/// counts. A 0.5 dB quantum straddling a hard threshold flips on rounding alone, so a
/// zero-clearance gate fails the unmodified kernel itself. `NO_DATA` is structural and
/// always qualifies when it changes painted state.
pub const FLIP_CLEARANCE_DB: f64 = 1.0;

/// Hard bound on systematic one-sided bias (owner, 2026-08-12), measured as the signed
/// mean over PAINTED cells — reference ≥30 dB — across the full benchmark.
///
/// Why bias needs a bound of its OWN, separate from the amplitude ladder: a uniform dB
/// offset passes through energy averaging **unchanged**. `build-pyramid` averages
/// energy, so a uniform `+δ` scales every cell by `10^(δ/10)`, scales the mean by the
/// same factor, and comes back out as `L + δ`. The offset therefore reaches EVERY
/// overview zoom at full strength, while the ladder — which only ever sees a small
/// per-cell magnitude — cannot detect it. Reach, not size, is what makes it dangerous.
///
/// At 0.5 dB the offset can move every cell by one storage step — the most either
/// heatmap wave may cost a viewer.
///
/// These are CHOSEN bounds calibrated to the storage quantum, not derived from a
/// perception study, and the owner may move them. What is not negotiable is that a bias
/// bound exists and is reported for every configuration.
pub const MAX_AGGREGATE_SIGNED_MEAN_DB: f64 = 0.5;

/// Rows above this qualifying presence-change share are rendered for human inspection
/// before paint. This is a diagnostic trigger, never a numeric release gate.
pub const ROW_EYEBALL_PRESENCE_FRACTION: f64 = 0.05;

/// Rows above this absolute signed bias are rendered for human inspection before paint.
/// This is a diagnostic trigger, never a numeric release gate.
pub const ROW_EYEBALL_SIGNED_MEAN_DB: f64 = 2.0;

/// Wave 2's shared extreme-tail boundary. Painted amplitude errors and numeric
/// paint-state flips above this magnitude consume one cell from the same union budget.
pub const WAVE_TWO_UNIFIED_TAIL_OVER_DB: f64 = 6.0;

/// Wave 2's shared extreme-tail budget (0.001 % of reference-painted cells).
pub const WAVE_TWO_UNIFIED_TAIL_FRACTION: f64 = 0.00001;

/// Wave 1 aggregate paint-state/existence changes allowed (1.5 % of
/// reference-painted cells). One-step corrected-epoch anchoring, 2026-08-13:
/// five completion-attested arms clustered at 1.389138-1.404197 % after structural
/// `NO_DATA` changes were admitted; 90-92 % of that structural population was an
/// unpainted 0-13 dB GPU value where the popup reference had `NO_DATA`. The 1.5 %
/// bound separates that measured class from the pinned 25 % mass-fill attack.
pub const WAVE_ONE_PRESENCE_FRACTION: f64 = 0.015;

/// Wave 2 aggregate paint-state changes allowed (0.25 % of reference-painted cells).
pub const WAVE_TWO_PRESENCE_FRACTION: f64 = 0.0025;

/// Wave 1 maximum numeric drift below the paint threshold.
pub const WAVE_ONE_QUIET_MAX_DB: f64 = 20.0;

/// Wave 2 maximum numeric drift below the paint threshold.
pub const WAVE_TWO_QUIET_MAX_DB: f64 = 10.0;

// Owner-approved consolidated ladder (rulings #3--#6, 2026-08-13). These stay named
// because the one scheduled corrected-epoch anchoring must be a constants-only diff;
// after anchoring, any further movement requires another owner ruling.
const WAVE_ONE_BASELINE_OVER_DB: f64 = 1.0;
const WAVE_ONE_BASELINE_FRACTION: f64 = 0.20;
const WAVE_ONE_MIDDLE_OVER_DB: f64 = 2.0;
const WAVE_ONE_MIDDLE_FRACTION: f64 = 0.01;
const WAVE_ONE_TAIL_OVER_DB: f64 = 6.0;
const WAVE_ONE_TAIL_FRACTION: f64 = 0.0001;

const WAVE_TWO_BASELINE_OVER_DB: f64 = 0.5;
const WAVE_TWO_BASELINE_FRACTION: f64 = 0.20;
const WAVE_TWO_MIDDLE_OVER_DB: f64 = 1.0;
const WAVE_TWO_MIDDLE_FRACTION: f64 = 0.01;
const WAVE_TWO_HIGH_OVER_DB: f64 = 3.0;
const WAVE_TWO_HIGH_FRACTION: f64 = 0.0001;

/// Retained count-only diagnostic above the old Wave 1 magnitude ceiling.
pub const DIAGNOSTIC_EXTREME_OVER_DB: f64 = 12.0;

/// Diagnostic per-row comparison against the aggregate amplitude and presence rates.
/// It no longer gates either wave; retained so reviewers can see localized debt.
pub const ROW_ANTI_DILUTION_MULTIPLIER: f64 = 3.0;

/// Distinct byte differences an HM3 pair can show: cells are `0..=254` (255 is
/// `NO_DATA`), so `|Δbyte|` lands in `0..=254`.
const ABS_DIFF_BUCKETS: usize = 255;

/// One cumulative rung: at most `max_fraction` of the benchmark's aggregate
/// reference-painted population may exceed `over_db`. Single-row mode retains its
/// historical whole-tile denominator as diagnostics only.
#[derive(Clone, Copy, Debug)]
pub struct Rung {
    pub over_db: f64,
    pub max_fraction: f64,
    population: RungPopulation,
}

impl Rung {
    /// Population counted by this rung, made explicit in scorer output so the Wave 2
    /// union tail cannot be mistaken for a painted-only amplitude count.
    pub fn population_label(self) -> &'static str {
        match self.population {
            RungPopulation::PaintedAmplitude => "painted_amplitude",
            RungPopulation::WaveTwoUnifiedTail => "painted_or_extreme_presence",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum RungPopulation {
    PaintedAmplitude,
    WaveTwoUnifiedTail,
}

const WAVE_ONE_RUNGS: [Rung; 3] = [
    Rung {
        over_db: WAVE_ONE_BASELINE_OVER_DB,
        max_fraction: WAVE_ONE_BASELINE_FRACTION,
        population: RungPopulation::PaintedAmplitude,
    },
    Rung {
        over_db: WAVE_ONE_MIDDLE_OVER_DB,
        max_fraction: WAVE_ONE_MIDDLE_FRACTION,
        population: RungPopulation::PaintedAmplitude,
    },
    Rung {
        over_db: WAVE_ONE_TAIL_OVER_DB,
        max_fraction: WAVE_ONE_TAIL_FRACTION,
        population: RungPopulation::PaintedAmplitude,
    },
];

const WAVE_TWO_RUNGS: [Rung; 4] = [
    Rung {
        over_db: WAVE_TWO_BASELINE_OVER_DB,
        max_fraction: WAVE_TWO_BASELINE_FRACTION,
        population: RungPopulation::PaintedAmplitude,
    },
    Rung {
        over_db: WAVE_TWO_MIDDLE_OVER_DB,
        max_fraction: WAVE_TWO_MIDDLE_FRACTION,
        population: RungPopulation::PaintedAmplitude,
    },
    Rung {
        over_db: WAVE_TWO_HIGH_OVER_DB,
        max_fraction: WAVE_TWO_HIGH_FRACTION,
        population: RungPopulation::PaintedAmplitude,
    },
    Rung {
        over_db: WAVE_TWO_UNIFIED_TAIL_OVER_DB,
        max_fraction: WAVE_TWO_UNIFIED_TAIL_FRACTION,
        population: RungPopulation::WaveTwoUnifiedTail,
    },
];

/// Which of the two heatmap contracts to score against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    /// Draft z12: double wave 2's amplitudes.
    One,
    /// Accurate z13: the popup's physics, every aggregate painted-cell tier hard.
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

    /// Exact `(tile, layer)` rows in the fixed complete benchmark for this wave.
    /// qexp is the mandatory trust boundary that separately attests the ordered key
    /// set; this count prevents a probe or historical subset from being mislabeled as
    /// a release verdict by the scorer alone.
    pub fn benchmark_rows(self) -> usize {
        match self {
            Wave::One => 250,
            Wave::Two => 980,
        }
    }

    /// Hard cap on numeric drift below the paint threshold. HM3 bytes remain visible
    /// through the hover tooltip even when the heatmap palette draws no colour
    /// (`frontend/src/lib/stay-noise.ts:48`, owner ruling #4, 2026-08-13).
    pub fn quiet_band_max_db(self) -> f64 {
        match self {
            Wave::One => WAVE_ONE_QUIET_MAX_DB,
            Wave::Two => WAVE_TWO_QUIET_MAX_DB,
        }
    }

    /// Cumulative painted-cell ladder. Wave 1's last rung has no magnitude ceiling.
    /// Wave 2 separately unions its `>6 dB` painted cells with extreme numeric
    /// paint-state flips so an overlapping cell consumes the shared tail pool once.
    pub fn rungs(self) -> &'static [Rung] {
        match self {
            Wave::One => &WAVE_ONE_RUNGS,
            Wave::Two => &WAVE_TWO_RUNGS,
        }
    }

    /// Aggregate qualifying paint-state changes allowed for this wave.
    pub fn max_presence_fraction(self) -> f64 {
        match self {
            Wave::One => WAVE_ONE_PRESENCE_FRACTION,
            Wave::Two => WAVE_TWO_PRESENCE_FRACTION,
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
    Fail,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
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
    /// Every cell in the tile — the legacy single-row diagnostic denominator.
    /// Release rungs use `reference_painted_cells` across the benchmark.
    pub cells: usize,
    /// Every reference cell that is actually painted, including one whose candidate
    /// disappeared. Aggregate scoring uses this as its denominator; `loud.cells`
    /// excludes a reference/candidate `NO_DATA` mismatch because no amplitude exists
    /// to subtract.
    pub reference_painted_cells: usize,
    /// Reference ≥30 dB: the painted band, where the ladder applies.
    pub loud: Band,
    /// Reference <30 dB: unpainted by the palette but visible through numeric hover,
    /// so a wave-specific maximum-drift cap applies.
    pub quiet: Band,
    /// Cells where exactly one side is `NO_DATA`, for the statistics line.
    pub presence_changed: usize,
    /// Per-cell union governed by the aggregate presence-area gate: a qualifying
    /// paint-state flip OR a structural `NO_DATA` mismatch. An overlap counts once.
    pub gated_presence_changes: usize,
    /// Every visible crossing of the 30 dB paint edge, including the near-edge
    /// rounding band that is deliberately exempt from the hard flip gate.
    pub paint_edge_crossings: usize,
    /// Flips across the 30 dB paint edge where either numeric side sits ≥1 dB clear of
    /// it, or where a `NO_DATA` transition changes painted state.
    pub qualifying_flips: usize,
    /// Silence painted as noise.
    pub flips_newly_painted: usize,
    /// Audible content that vanished.
    pub flips_newly_silent: usize,
    /// Cell-level union used only by Wave 2: numeric painted amplitude `>6 dB` OR a
    /// qualifying numeric paint-state flip `>6 dB`. An overlap is counted once.
    pub wave_two_unified_tail: usize,
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
    assert_eq!(
        reference.len(),
        candidate.len(),
        "reference and candidate grids must be the same size"
    );
    let mut s = Score {
        cells: reference.len(),
        reference_painted_cells: 0,
        loud: Band::new(),
        quiet: Band::new(),
        presence_changed: 0,
        gated_presence_changes: 0,
        paint_edge_crossings: 0,
        qualifying_flips: 0,
        flips_newly_painted: 0,
        flips_newly_silent: 0,
        wave_two_unified_tail: 0,
    };

    for (&r, &c) in reference.iter().zip(candidate.iter()) {
        // The paint-state part is judged at the palette floor. Structural
        // NO_DATA mismatches join it below in the per-cell presence union because
        // hover exposes whether a numeric value exists even below that floor.
        let r_painted = is_painted(r);
        s.reference_painted_cells += usize::from(r_painted);
        let c_painted = is_painted(c);
        let mut qualifying_flip = false;
        if r_painted != c_painted {
            s.paint_edge_crossings += 1;
            // An absent reference is fully clear below the edge, so painting noise
            // over silence always counts.
            // The clearance exemption is for ROUNDING noise across the threshold, so it
            // requires both cells to actually have a value near the edge. If either
            // side is absent the difference is structural, not rounding, and always
            // counts — otherwise a candidate that drops every near-edge cell to
            // NO_DATA erases painted content for free.
            let qualifies = r == NO_DATA
                || c == NO_DATA
                || (dequantise_lden(r) - PAINT_FLOOR_DB).abs() >= FLIP_CLEARANCE_DB
                || (dequantise_lden(c) - PAINT_FLOOR_DB).abs() >= FLIP_CLEARANCE_DB;
            if qualifies {
                qualifying_flip = true;
                s.qualifying_flips += 1;
                if r_painted {
                    s.flips_newly_silent += 1;
                } else {
                    s.flips_newly_painted += 1;
                }
            }
        }

        let no_data_changed = (r == NO_DATA) != (c == NO_DATA);
        s.gated_presence_changes += usize::from(qualifying_flip || no_data_changed);
        if no_data_changed {
            s.presence_changed += 1;
            continue;
        }
        if r == NO_DATA {
            continue;
        }
        let delta_bytes = i16::from(c) - i16::from(r);
        let abs_delta_db = f64::from(delta_bytes.unsigned_abs()) * 0.5;
        if abs_delta_db > WAVE_TWO_UNIFIED_TAIL_OVER_DB && (r_painted || qualifying_flip) {
            s.wave_two_unified_tail += 1;
        }
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

    /// Cells over each rung of `wave`. Every rung uses painted-band amplitude except
    /// Wave 2's final rung, which is the approved union of painted amplitude and
    /// extreme numeric paint-state flips.
    pub fn count_rungs(&self, wave: Wave) -> Vec<usize> {
        wave.rungs()
            .iter()
            .map(|rung| match rung.population {
                RungPopulation::PaintedAmplitude => self.loud.cells_over(rung.over_db),
                RungPopulation::WaveTwoUnifiedTail => self.wave_two_unified_tail,
            })
            .collect()
    }

    pub fn presence_allowance(&self, wave: Wave) -> usize {
        allowance(self.reference_painted_cells, wave.max_presence_fraction())
    }

    pub fn presence_over_budget(&self, wave: Wave) -> bool {
        self.gated_presence_changes > self.presence_allowance(wave)
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
}

/// Painted-weighted verdict over a fixed benchmark's `(tile, layer)` rows.
///
/// Amplitude counts and qualifying presence-change area are summed against all
/// reference-painted cells. Signed bias is summed over numerically comparable painted
/// cells because `NO_DATA` has no dB delta; the presence gate accounts for those cells.
/// Gates apply only after the entire benchmark is combined. Every per-row rate is a
/// printed diagnostic.
pub struct AggregateScore<'a> {
    rows: &'a [Score],
}

impl<'a> AggregateScore<'a> {
    pub fn new(rows: &'a [Score]) -> Self {
        assert!(!rows.is_empty(), "aggregate score needs at least one row");
        AggregateScore { rows }
    }

    pub fn rows(&self) -> &'a [Score] {
        self.rows
    }

    pub fn painted_cells(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.reference_painted_cells)
            .sum()
    }

    pub fn count_rungs(&self, wave: Wave) -> Vec<usize> {
        let mut counts = vec![0usize; wave.rungs().len()];
        for row in self.rows {
            for (total, count) in counts.iter_mut().zip(row.count_rungs(wave)) {
                *total += count;
            }
        }
        counts
    }

    pub fn amplitude_overshoots(&self, wave: Wave) -> Vec<usize> {
        let counts = self.count_rungs(wave);
        wave.rungs()
            .iter()
            .enumerate()
            .filter(|(i, rung)| counts[*i] > allowance(self.painted_cells(), rung.max_fraction))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn row_anti_dilution_allowance(&self, row: usize, rung: Rung) -> usize {
        allowance(
            self.rows[row].reference_painted_cells,
            rung.max_fraction * ROW_ANTI_DILUTION_MULTIPLIER,
        )
    }

    /// `(row index, rung index)` pairs above the former 3x local diagnostic reference.
    pub fn row_amplitude_overshoots(&self, wave: Wave) -> Vec<(usize, usize)> {
        let rungs = wave.rungs();
        let mut overshoots = Vec::new();
        for (row_index, row) in self.rows.iter().enumerate() {
            let counts = row.count_rungs(wave);
            for (rung_index, rung) in rungs.iter().copied().enumerate() {
                if counts[rung_index] > self.row_anti_dilution_allowance(row_index, rung) {
                    overshoots.push((row_index, rung_index));
                }
            }
        }
        overshoots
    }

    pub fn loud_max_abs_db(&self) -> f64 {
        self.rows
            .iter()
            .map(|row| row.loud.max_abs_db)
            .fold(0.0, f64::max)
    }

    pub fn quiet_max_abs_db(&self) -> f64 {
        self.rows
            .iter()
            .map(|row| row.quiet.max_abs_db)
            .fold(0.0, f64::max)
    }

    pub fn quiet_band_over(&self, wave: Wave) -> bool {
        self.rows.iter().any(|row| row.quiet_band_over(wave))
    }

    pub fn signed_mean_db(&self) -> f64 {
        let cells: usize = self.rows.iter().map(|row| row.loud.cells).sum();
        if cells == 0 {
            0.0
        } else {
            self.rows
                .iter()
                .map(|row| row.loud.signed_sum_db)
                .sum::<f64>()
                / cells as f64
        }
    }

    pub fn cand_louder_pct(&self) -> f64 {
        let moved: usize = self.rows.iter().map(|row| row.loud.moved).sum();
        if moved == 0 {
            0.0
        } else {
            100.0
                * self
                    .rows
                    .iter()
                    .map(|row| row.loud.cand_louder)
                    .sum::<usize>() as f64
                / moved as f64
        }
    }

    pub fn qualifying_flips(&self) -> usize {
        self.rows.iter().map(|row| row.qualifying_flips).sum()
    }

    pub fn flips_newly_painted(&self) -> usize {
        self.rows.iter().map(|row| row.flips_newly_painted).sum()
    }

    pub fn flips_newly_silent(&self) -> usize {
        self.rows.iter().map(|row| row.flips_newly_silent).sum()
    }

    pub fn presence_changed(&self) -> usize {
        self.rows.iter().map(|row| row.presence_changed).sum()
    }

    pub fn gated_presence_changes(&self) -> usize {
        self.rows.iter().map(|row| row.gated_presence_changes).sum()
    }

    pub fn paint_edge_crossings(&self) -> usize {
        self.rows.iter().map(|row| row.paint_edge_crossings).sum()
    }

    pub fn presence_allowance(&self, wave: Wave) -> usize {
        allowance(self.painted_cells(), wave.max_presence_fraction())
    }

    pub fn presence_over_budget(&self, wave: Wave) -> bool {
        self.gated_presence_changes() > self.presence_allowance(wave)
    }

    pub fn row_presence_diagnostic_allowance(&self, row: usize, wave: Wave) -> usize {
        allowance(
            self.rows[row].reference_painted_cells,
            wave.max_presence_fraction() * ROW_ANTI_DILUTION_MULTIPLIER,
        )
    }

    /// Row indices whose gated presence area exceeds the former 3x diagnostic reference.
    pub fn row_presence_diagnostic_overshoots(&self, wave: Wave) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(row_index, row)| {
                row.gated_presence_changes
                    > self.row_presence_diagnostic_allowance(*row_index, wave)
            })
            .map(|(row_index, _)| row_index)
            .collect()
    }

    pub fn row_presence_fraction(&self, row: usize) -> f64 {
        let painted = self.rows[row].reference_painted_cells;
        if painted == 0 {
            if self.rows[row].gated_presence_changes == 0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            self.rows[row].gated_presence_changes as f64 / painted as f64
        }
    }

    pub fn row_presence_eyeball_rows(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&row| self.row_presence_fraction(row) > ROW_EYEBALL_PRESENCE_FRACTION)
            .collect()
    }

    pub fn row_bias_eyeball_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.loud.signed_mean_db().abs() > ROW_EYEBALL_SIGNED_MEAN_DB)
            .map(|(row, _)| row)
            .collect()
    }

    pub fn bias_over_budget(&self) -> bool {
        self.signed_mean_db().abs() > MAX_AGGREGATE_SIGNED_MEAN_DB
    }

    pub fn wave_two_unified_tail(&self) -> usize {
        self.rows.iter().map(|row| row.wave_two_unified_tail).sum()
    }

    /// Count beyond the former 12 dB Wave 1 ceiling. It remains visible evidence but
    /// has no independent allowance; the cumulative >6 dB tail owns the verdict.
    pub fn diagnostic_extreme_count(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.loud.cells_over(DIAGNOSTIC_EXTREME_OVER_DB))
            .sum()
    }

    fn hard_gates_hold(&self, wave: Wave) -> bool {
        !self.presence_over_budget(wave)
            && self.amplitude_overshoots(wave).is_empty()
            && !self.quiet_band_over(wave)
            && !self.bias_over_budget()
    }

    fn computed_verdict(&self, wave: Wave) -> Verdict {
        if self.hard_gates_hold(wave) {
            Verdict::Pass
        } else {
            Verdict::Fail
        }
    }

    /// Release verdict only for the fixed complete benchmark row count. Exact ordered
    /// key identity is qexp's separate responsibility.
    pub fn verdict(&self, wave: Wave) -> Option<Verdict> {
        self.verdict_for_expected_rows(wave, wave.benchmark_rows())
    }

    /// Release verdict for a caller-attested complete benchmark row count.
    ///
    /// The normal contract remains [`Self::verdict`], pinned to Wave 1's 250 rows
    /// and Wave 2's 980 rows. A separately named release scope may own a different
    /// fixed row count; it must pass that count explicitly so a short or overfull
    /// workset can never receive a verdict.
    pub fn verdict_for_expected_rows(&self, wave: Wave, expected_rows: usize) -> Option<Verdict> {
        (self.rows.len() == expected_rows).then(|| self.computed_verdict(wave))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::TILE_PX;
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

    fn aggregate_verdict(row: &Score, wave: Wave) -> Verdict {
        AggregateScore::new(std::slice::from_ref(row)).computed_verdict(wave)
    }

    #[test]
    fn legacy_single_pair_allowances_remain_stable() {
        // Single-pair diagnostic mode retains its whole-tile denominator; aggregate
        // release mode is separately pinned below with a painted-cell denominator.
        assert_eq!(allowance(CELLS, 0.20), 52_429);
        assert_eq!(allowance(CELLS, 0.01), 2_621);
        assert_eq!(allowance(CELLS, 0.0001), 26);
        assert_eq!(allowance(CELLS, 0.00001), 3);
        assert_eq!(Wave::One.benchmark_rows(), 250);
        assert_eq!(Wave::Two.benchmark_rows(), 980);

        let reference = reference_tile();
        let clean = score(&reference, &reference);
        let probe = [clean.clone()];
        assert_eq!(AggregateScore::new(&probe).verdict(Wave::One), None);
        let per_layer = vec![clean.clone(); 125];
        assert_eq!(
            AggregateScore::new(&per_layer).verdict_for_expected_rows(Wave::One, 125),
            Some(Verdict::Pass)
        );
        assert_eq!(
            AggregateScore::new(&per_layer[..124]).verdict_for_expected_rows(Wave::One, 125),
            None
        );
        let mut overfull = per_layer.clone();
        overfull.push(clean.clone());
        assert_eq!(
            AggregateScore::new(&overfull).verdict_for_expected_rows(Wave::One, 125),
            None
        );
        let wave_one = vec![clean.clone(); Wave::One.benchmark_rows()];
        assert_eq!(
            AggregateScore::new(&wave_one).verdict(Wave::One),
            Some(Verdict::Pass)
        );
        assert_eq!(
            AggregateScore::new(&wave_one).verdict(Wave::Two),
            None,
            "a complete Wave-1 workset is only diagnostic under the Wave-2 ladder"
        );
        let wave_two = vec![clean; Wave::Two.benchmark_rows()];
        assert_eq!(
            AggregateScore::new(&wave_two).verdict(Wave::Two),
            Some(Verdict::Pass)
        );
        assert_eq!(
            AggregateScore::new(&wave_two).verdict(Wave::One),
            None,
            "a complete Wave-2 workset is only diagnostic under the Wave-1 ladder"
        );
    }

    #[test]
    fn half_a_db_is_the_quantum_and_does_not_exceed_the_baseline() {
        // 0.5 dB is ONE byte step, so it is AT Wave 2's baseline, not over it. No
        // tighter non-zero byte-domain allowance would be representable.
        let reference = reference_tile();
        let candidate = candidate_with(&[(CELLS / 2, 0.5), (CELLS / 2, -0.5)]);
        let s = score(&reference, &candidate);
        assert_eq!(s.loud.max_abs_db, 0.5);
        assert_eq!(s.loud.cells_over(0.5), 0, "0.5 dB does not EXCEED baseline");
        assert_eq!(s.loud.moved, CELLS);
        assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Pass);
    }

    #[test]
    fn each_wave_two_rung_boundary_is_pinned() {
        let reference = reference_tile();
        for (over_db, fraction) in [
            (WAVE_TWO_BASELINE_OVER_DB, WAVE_TWO_BASELINE_FRACTION),
            (WAVE_TWO_MIDDLE_OVER_DB, WAVE_TWO_MIDDLE_FRACTION),
            (WAVE_TWO_HIGH_OVER_DB, WAVE_TWO_HIGH_FRACTION),
            (
                WAVE_TWO_UNIFIED_TAIL_OVER_DB,
                WAVE_TWO_UNIFIED_TAIL_FRACTION,
            ),
        ] {
            let allowed = allowance(CELLS, fraction);
            let index = rung_index(Wave::Two, over_db);

            // Exactly at the allowance: the budget is inclusive, so this passes.
            let s = score(&reference, &sign_split_over(allowed, over_db));
            assert_eq!(s.loud.cells_over(over_db), allowed, "rung {over_db} dB");
            assert!(
                s.amplitude_overshoots(Wave::Two).is_empty(),
                "{allowed} cells over {over_db} dB is exactly the budget"
            );
            assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Pass);

            // One cell more, and wave 2 fails on that rung and no other.
            let s = score(&reference, &sign_split_over(allowed + 1, over_db));
            assert_eq!(s.loud.cells_over(over_db), allowed + 1);
            assert_eq!(s.amplitude_overshoots(Wave::Two), vec![index]);
            assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Fail);
        }
    }

    #[test]
    fn each_wave_one_rung_boundary_is_pinned() {
        let reference = reference_tile();
        for (over_db, fraction) in [
            (WAVE_ONE_BASELINE_OVER_DB, WAVE_ONE_BASELINE_FRACTION),
            (WAVE_ONE_MIDDLE_OVER_DB, WAVE_ONE_MIDDLE_FRACTION),
            (WAVE_ONE_TAIL_OVER_DB, WAVE_ONE_TAIL_FRACTION),
        ] {
            let allowed = allowance(CELLS, fraction);
            let index = rung_index(Wave::One, over_db);

            let s = score(&reference, &sign_split_over(allowed, over_db));
            assert_eq!(s.loud.cells_over(over_db), allowed, "rung {over_db} dB");
            assert!(s.amplitude_overshoots(Wave::One).is_empty());
            assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Pass);

            let s = score(&reference, &sign_split_over(allowed + 1, over_db));
            assert_eq!(s.amplitude_overshoots(Wave::One), vec![index]);
            assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);
        }
    }

    #[test]
    fn wave_two_shares_its_tail_and_wave_one_has_no_height_ceiling() {
        let reference = reference_tile();
        let mut union_reference = reference_tile();
        union_reference[0] = quantise_lden(29.0);
        let mut union_candidate = union_reference.clone();
        union_candidate[0] = quantise_lden(36.0);
        let union_allowance = allowance(CELLS - 1, WAVE_TWO_UNIFIED_TAIL_FRACTION);
        assert!(union_allowance > 0);
        for (offset, cell) in union_candidate
            .iter_mut()
            .enumerate()
            .skip(1)
            .take(union_allowance - 1)
        {
            *cell = quantise_lden(if offset % 2 == 0 { 67.0 } else { 53.0 });
        }
        let at_union_limit = score(&union_reference, &union_candidate);
        assert_eq!(at_union_limit.wave_two_unified_tail, union_allowance);
        assert_eq!(aggregate_verdict(&at_union_limit, Wave::Two), Verdict::Pass);

        union_candidate[union_allowance] = quantise_lden(67.0);
        let over_union_limit = score(&union_reference, &union_candidate);
        assert_eq!(over_union_limit.wave_two_unified_tail, union_allowance + 1);
        assert_eq!(over_union_limit.amplitude_overshoots(Wave::Two), vec![3]);
        assert_eq!(
            aggregate_verdict(&over_union_limit, Wave::Two),
            Verdict::Fail
        );

        // Wave 1 has no height cap; only the >6 dB count gates.
        let allowed = allowance(CELLS, WAVE_ONE_TAIL_FRACTION);
        let s = score(&reference, &candidate_with(&[(allowed, 40.0)]));
        assert_eq!(s.loud.cells_over(DIAGNOSTIC_EXTREME_OVER_DB), allowed);
        assert!(s.loud.max_abs_db > 12.0, "no upper bound in wave 1");
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Pass);

        let s = score(&reference, &candidate_with(&[(allowed + 1, 40.0)]));
        assert_eq!(s.amplitude_overshoots(Wave::One), vec![2]);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);
    }

    #[test]
    fn wave_two_unified_tail_counts_numeric_flips_once_and_excludes_no_data() {
        let reference = [
            quantise_lden(60.0),
            quantise_lden(60.0),
            quantise_lden(20.0),
            quantise_lden(60.0),
            NO_DATA,
        ];
        let candidate = [
            quantise_lden(67.0), // painted amplitude only
            quantise_lden(20.0), // painted amplitude plus a qualifying flip
            quantise_lden(40.0), // newly painted numeric flip
            NO_DATA,             // no numeric delta: presence only
            quantise_lden(60.0), // no numeric delta: presence only
        ];
        let s = score(&reference, &candidate);
        assert_eq!(s.loud.cells_over(WAVE_TWO_UNIFIED_TAIL_OVER_DB), 2);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(s.presence_changed, 2);
        assert_eq!(s.gated_presence_changes, 4, "overlaps consume one cell");
        assert_eq!(s.wave_two_unified_tail, 3, "the overlap consumes one cell");
        assert_eq!(s.count_rungs(Wave::Two)[3], 3);
    }

    #[test]
    fn a_numeric_crossing_is_exempt_only_inside_the_rounding_band() {
        // A targeted 30.0 → 29.5 dB remap crosses the visible edge but remains
        // inside the rounding exemption. Keep that crossing observable even though
        // it correctly does not become a hard-gate flip by itself.
        let s = score(&[quantise_lden(30.0); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(s.paint_edge_crossings, 4);
        assert_eq!(s.qualifying_flips, 0);

        // Reference at 30.5 dB is inside the 1 dB clearance: edge noise, not a flip.
        let s = score(&[quantise_lden(30.5); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(
            s.paint_edge_crossings, 4,
            "the visible contour still changed"
        );
        assert_eq!(s.qualifying_flips, 0, "edge noise is not a flip");

        // The rounding exemption requires BOTH numeric values to remain strictly
        // inside (29, 31). A large change cannot hide behind a near-edge reference.
        let s = score(&[quantise_lden(29.5); 4], &[quantise_lden(60.0); 4]);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);

        let s = score(&[quantise_lden(29.5); 4], &[quantise_lden(31.0); 4]);
        assert_eq!(s.qualifying_flips, 4, "the candidate boundary is inclusive");

        let s = score(&[quantise_lden(29.0); 4], &[quantise_lden(30.5); 4]);
        assert_eq!(s.qualifying_flips, 4, "the reference boundary is inclusive");

        let s = score(&[quantise_lden(30.5); 4], &[quantise_lden(10.0); 4]);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);

        // Reference at 31 dB is exactly 1 dB clear: audible content vanishing.
        let s = score(&[quantise_lden(31.0); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(s.paint_edge_crossings, 4);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(s.flips_newly_silent, 4);
        assert_eq!(
            aggregate_verdict(&s, Wave::One),
            Verdict::Fail,
            "the aggregate presence area fails"
        );

        // Silence painted as noise: an absent reference is fully clear below the edge.
        let s = score(&[NO_DATA; 4], &[quantise_lden(45.0); 4]);
        assert_eq!(s.paint_edge_crossings, 4);
        assert_eq!(s.qualifying_flips, 4);
        assert_eq!(s.flips_newly_painted, 4);
        assert_eq!(s.presence_changed, 4);
        assert_eq!(s.loud.cells, 0, "no reference value to compare against");
        assert_eq!(
            aggregate_verdict(&s, Wave::One),
            Verdict::Fail,
            "zero painted denominator makes every structural appearance exceed the area cap"
        );
    }

    #[test]
    fn an_absent_candidate_always_counts_however_close_the_reference_sits() {
        // The clearance exemption is for rounding across the edge, not for content
        // that disappears. Without this, a tile whose painted cells all sit in the
        // 29-31 dB window could be erased wholesale to NO_DATA and score a clean PASS:
        // no flip would qualify, and the presence mismatch skips amplitude and bias.
        let reference = vec![quantise_lden(30.5); CELLS];
        let s = score(&reference, &[NO_DATA; CELLS]);
        assert_eq!(s.paint_edge_crossings, CELLS);
        assert_eq!(s.qualifying_flips, CELLS, "a vanished tile is not free");
        assert_eq!(s.flips_newly_silent, CELLS);
        assert_eq!(s.loud.cells, 0, "nothing to compare — hence the flip gate");
        assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Fail);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);

        // Same in the other direction: silence filled in with near-edge paint.
        let s = score(&[NO_DATA; CELLS], &vec![quantise_lden(30.5); CELLS]);
        assert_eq!(s.qualifying_flips, CELLS);
        assert_eq!(s.flips_newly_painted, CELLS);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);

        // But a genuine rounding crossing, both sides present, is still exempt.
        let s = score(&[quantise_lden(30.5); 4], &[quantise_lden(29.5); 4]);
        assert_eq!(s.qualifying_flips, 0);
    }

    #[test]
    fn no_data_mismatches_below_paint_are_presence_governed() {
        for (reference, candidate) in [
            ([NO_DATA; 4], [quantise_lden(20.0); 4]),
            ([quantise_lden(20.0); 4], [NO_DATA; 4]),
        ] {
            let s = score(&reference, &candidate);
            assert_eq!(s.paint_edge_crossings, 0, "neither side is painted");
            assert_eq!(s.qualifying_flips, 0, "no paint-state flip occurred");
            assert_eq!(s.presence_changed, 4);
            assert_eq!(s.gated_presence_changes, 4);
            assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);
            assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Fail);
        }
    }

    #[test]
    fn presence_area_rejects_half_tile_checkerboard() {
        // The review's half-tile checkerboard erasure is far beyond the area cap.
        let reference = reference_tile();
        let mut checkerboard = reference.clone();
        for y in 0..TILE_PX {
            for x in 0..TILE_PX {
                if (x + y) % 2 == 0 {
                    checkerboard[y * TILE_PX + x] = NO_DATA;
                }
            }
        }
        let s = score(&reference, &checkerboard);
        assert_eq!(s.qualifying_flips, CELLS / 2);
        assert!(s.presence_over_budget(Wave::One));
        assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Fail);
    }

    #[test]
    fn presence_area_has_exact_wave_and_diagnostic_row_boundaries() {
        let with_absent_flips = |count: usize| {
            let reference = reference_tile();
            let mut candidate = reference.clone();
            for cell in candidate.iter_mut().take(count) {
                *cell = NO_DATA;
            }
            score(&reference, &candidate)
        };

        for (wave, allowed) in [(Wave::One, 3_932), (Wave::Two, 655)] {
            let at_rows = [with_absent_flips(allowed)];
            let at = AggregateScore::new(&at_rows);
            assert_eq!(at.presence_allowance(wave), allowed);
            assert!(!at.presence_over_budget(wave));
            assert_eq!(at.computed_verdict(wave), Verdict::Pass);

            let over_rows = [with_absent_flips(allowed + 1)];
            let over = AggregateScore::new(&over_rows);
            assert!(over.presence_over_budget(wave));
            assert_eq!(over.computed_verdict(wave), Verdict::Fail);
        }

        // Four equal painted rows put 11,797 changes below the aggregate 1.5%
        // allowance (15,729). The local 3x (4.5%) reference is diagnostic:
        // exactly 11,796 is on it and the next cell appears in rows_over, but neither
        // changes the aggregate verdict.
        let clean_reference = reference_tile();
        let clean = score(&clean_reference, &clean_reference);
        let rows_at = [
            with_absent_flips(11_796),
            clean.clone(),
            clean.clone(),
            clean.clone(),
        ];
        let aggregate_at = AggregateScore::new(&rows_at);
        assert_eq!(
            aggregate_at.row_presence_diagnostic_allowance(0, Wave::One),
            11_796
        );
        assert!(aggregate_at
            .row_presence_diagnostic_overshoots(Wave::One)
            .is_empty());
        assert_eq!(aggregate_at.computed_verdict(Wave::One), Verdict::Pass);

        let rows_over = [
            with_absent_flips(11_797),
            clean.clone(),
            clean.clone(),
            clean.clone(),
        ];
        let aggregate_over = AggregateScore::new(&rows_over);
        assert!(!aggregate_over.presence_over_budget(Wave::One));
        assert_eq!(
            aggregate_over.row_presence_diagnostic_overshoots(Wave::One),
            vec![0]
        );
        assert_eq!(aggregate_over.computed_verdict(Wave::One), Verdict::Pass);
        assert!(aggregate_over.row_presence_eyeball_rows().is_empty());

        let rows_for_eyeball = [
            with_absent_flips(13_108),
            clean.clone(),
            clean.clone(),
            clean,
        ];
        let aggregate_for_eyeball = AggregateScore::new(&rows_for_eyeball);
        assert_eq!(aggregate_for_eyeball.row_presence_eyeball_rows(), vec![0]);
    }

    #[test]
    fn below_paint_numeric_drift_caps_are_hard_and_inclusive() {
        // Twelve dB is visible in hover and exceeds only Wave 2's 10 dB cap.
        let s = score(&[quantise_lden(10.0); 16], &[quantise_lden(22.0); 16]);
        assert_eq!(s.quiet.cells, 16);
        assert_eq!(s.quiet.max_abs_db, 12.0);
        assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Fail);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Pass);

        let s = score(&[quantise_lden(5.0); 16], &[quantise_lden(27.0); 16]);
        assert_eq!(s.quiet.max_abs_db, 22.0);
        assert_eq!(aggregate_verdict(&s, Wave::One), Verdict::Fail);
        assert_eq!(aggregate_verdict(&s, Wave::Two), Verdict::Fail);

        // The caps are inclusive at exactly 20/10 dB.
        let wave_one = score(&[quantise_lden(5.0); 16], &[quantise_lden(25.0); 16]);
        assert_eq!(aggregate_verdict(&wave_one, Wave::One), Verdict::Pass);
        let wave_two = score(&[quantise_lden(10.0); 16], &[quantise_lden(20.0); 16]);
        assert_eq!(aggregate_verdict(&wave_two, Wave::Two), Verdict::Pass);
    }

    #[test]
    fn aggregate_bias_is_half_a_db_in_both_waves() {
        let reference = reference_tile();

        // The aggregate limit is inclusive at exactly one storage quantum.
        let s = score(&reference, &candidate_with(&[(CELLS, 0.5)]));
        let rows = [s];
        let aggregate = AggregateScore::new(&rows);
        assert_eq!(aggregate.signed_mean_db(), 0.5);
        assert!(!aggregate.bias_over_budget());
        assert_eq!(aggregate.computed_verdict(Wave::Two), Verdict::Pass);

        // 60 % of cells one step louder: max 0.5 dB is inside wave 2's baseline, so the
        // ladder is spotless. A uniform dB offset passes through energy averaging
        // UNCHANGED, so it reaches every overview zoom at full strength — the ladder
        // cannot see it and the bias bound is what catches it.
        let s = score(&reference, &candidate_with(&[(CELLS * 3 / 5, 0.5)]));
        assert_eq!(s.loud.max_abs_db, 0.5);
        assert!(s.amplitude_overshoots(Wave::Two).is_empty());
        assert!(s.loud.cand_louder_pct() > 99.0);
        let lean = s.loud.signed_mean_db();
        assert!((0.25..0.5).contains(&lean), "~0.3 dB lean, got {lean}");
        let rows = [s];
        let aggregate = AggregateScore::new(&rows);
        assert!(!aggregate.bias_over_budget());
        assert_eq!(aggregate.computed_verdict(Wave::Two), Verdict::Pass);
        assert_eq!(aggregate.computed_verdict(Wave::One), Verdict::Pass);

        // 60 % of cells a full 1.0 dB louder: nothing EXCEEDS wave 1's 1 dB baseline,
        // so its ladder is clean too and only the 0.6 dB aggregate lean fails.
        let s = score(&reference, &candidate_with(&[(CELLS * 3 / 5, 1.0)]));
        assert!(s.amplitude_overshoots(Wave::One).is_empty());
        assert!(s.loud.signed_mean_db() > 0.5);
        let rows = [s];
        let aggregate = AggregateScore::new(&rows);
        assert!(aggregate.bias_over_budget());
        assert_eq!(aggregate.computed_verdict(Wave::One), Verdict::Fail);
        assert_eq!(aggregate.computed_verdict(Wave::Two), Verdict::Fail);

        // A small row can exceed the human-inspection trigger while the aggregate
        // amplitude and bias stay clean. That row is named but does not gate.
        let large_reference = vec![quantise_lden(60.0); 10_000];
        let large = score(&large_reference, &large_reference);
        let small_reference = vec![quantise_lden(60.0); 100];
        let small_candidate = vec![quantise_lden(63.0); 100];
        let small = score(&small_reference, &small_candidate);
        let rows = [large, small];
        let aggregate = AggregateScore::new(&rows);
        assert_eq!(aggregate.row_bias_eyeball_rows(), vec![1]);
        assert_eq!(aggregate.computed_verdict(Wave::One), Verdict::Pass);
    }

    #[test]
    fn row_eyeball_thresholds_are_strict_diagnostics() {
        let reference = vec![quantise_lden(31.0); 100];
        let with_flips = |count: usize| {
            let mut candidate = reference.clone();
            for cell in candidate.iter_mut().take(count) {
                *cell = quantise_lden(29.5);
            }
            score(&reference, &candidate)
        };

        let at_presence_rows = [with_flips(5)];
        let at_presence = AggregateScore::new(&at_presence_rows);
        assert_eq!(at_presence.row_presence_fraction(0), 0.05);
        assert!(at_presence.row_presence_eyeball_rows().is_empty());

        let over_presence_rows = [with_flips(6)];
        let over_presence = AggregateScore::new(&over_presence_rows);
        assert_eq!(over_presence.row_presence_eyeball_rows(), vec![0]);

        let bias_reference = vec![quantise_lden(60.0); 100];
        let at_bias = score(&bias_reference, &[quantise_lden(62.0); 100]);
        let at_bias_rows = [at_bias];
        assert!(AggregateScore::new(&at_bias_rows)
            .row_bias_eyeball_rows()
            .is_empty());

        let over_bias = score(&bias_reference, &[quantise_lden(62.5); 100]);
        let over_bias_rows = [over_bias];
        assert_eq!(
            AggregateScore::new(&over_bias_rows).row_bias_eyeball_rows(),
            vec![0]
        );
    }

    #[test]
    fn aggregate_uses_painted_cells_and_reports_three_times_row_debt() {
        let large_reference = vec![quantise_lden(60.0); 900];
        let large = score(&large_reference, &large_reference);

        let small_reference = vec![quantise_lden(60.0); 100];
        let mut small_candidate = small_reference.clone();
        // Seventy cells exceed wave 2's 0.5 dB baseline, split in sign so this tests
        // amplitude accounting rather than the independent bias gate.
        for cell in small_candidate.iter_mut().take(35) {
            *cell = quantise_lden(61.0);
        }
        for cell in small_candidate.iter_mut().skip(35).take(35) {
            *cell = quantise_lden(59.0);
        }
        let small = score(&small_reference, &small_candidate);
        let rows = [large, small];
        let aggregate = AggregateScore::new(&rows);

        assert_eq!(aggregate.painted_cells(), 1_000);
        assert_eq!(aggregate.count_rungs(Wave::Two)[0], 70);
        assert!(
            aggregate.amplitude_overshoots(Wave::Two).is_empty(),
            "70/1000 is inside the aggregate 20% allowance"
        );
        assert_eq!(
            aggregate.row_anti_dilution_allowance(1, Wave::Two.rungs()[0]),
            60
        );
        assert_eq!(aggregate.row_amplitude_overshoots(Wave::Two), vec![(1, 0)]);
        assert_eq!(aggregate.computed_verdict(Wave::Two), Verdict::Pass);

        let mut draft_candidate = small_reference.clone();
        for cell in draft_candidate.iter_mut().take(35) {
            *cell = quantise_lden(62.0);
        }
        for cell in draft_candidate.iter_mut().skip(35).take(35) {
            *cell = quantise_lden(58.0);
        }
        let draft_small = score(&small_reference, &draft_candidate);
        let draft_rows = [rows[0].clone(), draft_small];
        let draft_aggregate = AggregateScore::new(&draft_rows);
        assert_eq!(
            draft_aggregate.row_amplitude_overshoots(Wave::One),
            vec![(1, 0)]
        );
        assert_eq!(draft_aggregate.computed_verdict(Wave::One), Verdict::Pass);

        // Nearest-cell rounding at zero remains useful in the diagnostic: the tiny row
        // is named, while one tail cell stays inside the aggregate allowance.
        let tiny_reference = vec![quantise_lden(60.0); 1_666];
        let mut tiny_candidate = tiny_reference.clone();
        tiny_candidate[0] = quantise_lden(73.0);
        let tiny = score(&tiny_reference, &tiny_candidate);
        let aggregate_clean_reference = vec![quantise_lden(60.0); 10_000];
        let aggregate_clean = score(&aggregate_clean_reference, &aggregate_clean_reference);
        let tiny_rows = [aggregate_clean, tiny];
        let tiny_aggregate = AggregateScore::new(&tiny_rows);
        assert_eq!(
            tiny_aggregate.row_anti_dilution_allowance(1, Wave::One.rungs()[2]),
            0
        );
        assert_eq!(
            tiny_aggregate.row_amplitude_overshoots(Wave::One),
            vec![(1, 2)]
        );
        assert_eq!(tiny_aggregate.computed_verdict(Wave::One), Verdict::Pass);

        // NO_DATA is transparent, not painted denominator ballast.
        let reference = [quantise_lden(60.0), quantise_lden(31.0), NO_DATA, NO_DATA];
        let silent_row = score(&reference, &reference);
        let rows = [silent_row];
        assert_eq!(AggregateScore::new(&rows).painted_cells(), 2);
    }
}

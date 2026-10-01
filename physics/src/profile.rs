//! The sampled ground under one ray: one bilateral cadence (a 10 m probe at each end, three
//! steps each of 30.7, 61.4 and 122.8 m from both ends, then 245.6 m through the middle), dense
//! where obstacles near the source and receiver diffract most. Terrain, ground factor and forest
//! cover are all read at these positions.

/// Base step of the cadence (m): one arc-second of latitude as dev4 measured it (110,540 m per
/// degree / 3600), the terrain lattice spacing; a sampling cadence, not a distance of geometry.
pub const CADENCE_STEP_M: f64 = 110_540.0 / 3600.0;
/// Near-endpoint probe offset (m): catches berms 5-15 m from a road that would fall between the
/// end and the first regular sample.
pub const NEAR_OFFSET_M: f64 = 10.0;

/// The ground under one ray, sampled at fractions `t` of its horizontal length (0 and 1 included).
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub horizontal_m: f64,
    pub t: Vec<f64>,
    /// Bare-earth altitude (m).
    pub ground_m: Vec<f64>,
    /// CNOSSOS G in [0, 1].
    pub ground_factor: Vec<f64>,
}

impl Profile {
    /// Empties the samples and sets the cadence for a ray of `horizontal_m`.
    pub fn reset(&mut self, horizontal_m: f64) {
        self.horizontal_m = horizontal_m;
        fill_t_values(horizontal_m, &mut self.t);
        self.ground_m.clear();
        self.ground_factor.clear();
    }
}

/// The cadence of a ray of `dist_m` as fractions of its length. Paths up to ten steps are
/// sampled uniformly; the 10 m probes are left out below 30 m, where they would crowd the middle.
pub fn fill_t_values(dist_m: f64, buf: &mut Vec<f64>) {
    buf.clear();

    // Near-endpoint probe at 10m — only emitted when there's room (≥3×NEAR_OFFSET
    // so the probe doesn't collapse toward the midpoint). Skipped for paths
    // shorter than 30m.
    let emit_near = dist_m >= 3.0 * NEAR_OFFSET_M;
    let near_t = NEAR_OFFSET_M / dist_m;

    if dist_m <= CADENCE_STEP_M * 10.0 {
        // Short path: uniform stepping + optional 10m probe at each end.
        let n = (dist_m / CADENCE_STEP_M).ceil().max(3.0) as usize;
        buf.push(0.0);
        if emit_near {
            buf.push(near_t);
        }
        for i in 1..n.saturating_sub(1) {
            let t = i as f64 / (n - 1) as f64;
            // Skip uniform sample if it's within 3m of a near-endpoint probe.
            if emit_near
                && ((t - near_t).abs() * dist_m < 3.0 || ((1.0 - t) - near_t).abs() * dist_m < 3.0)
            {
                continue;
            }
            buf.push(t);
        }
        if emit_near {
            buf.push(1.0 - near_t);
        }
        buf.push(1.0);
        buf.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        return;
    }

    buf.push(0.0);
    if emit_near {
        buf.push(near_t);
    }

    let levels = [
        CADENCE_STEP_M,
        CADENCE_STEP_M * 2.0,
        CADENCE_STEP_M * 4.0,
        CADENCE_STEP_M * 8.0,
    ];
    let reps = 3usize;

    // Forward from source — ramp starts *after* the 10m near-probe so the
    // first ramp sample lands at 10 + 30 = 40m (vs. 30m before), which costs
    // nothing: we already have a sample at 10m.
    let mut pos = if emit_near { NEAR_OFFSET_M } else { 0.0 };
    'fwd: for &step in &levels {
        for _ in 0..reps {
            pos += step;
            if pos >= dist_m * 0.5 {
                break 'fwd;
            }
            buf.push(pos / dist_m);
        }
    }
    let fwd_end = pos.min(dist_m * 0.5) / dist_m;

    // Fill the middle (everything past both ramps) at the coarsest step.
    let coarse = levels[levels.len() - 1].min(dist_m * 0.25);
    // Backward ramp start as a t fraction (so the middle fill stops there).
    let mut bpos = if emit_near { NEAR_OFFSET_M } else { 0.0 };
    'bw: for &step in &levels {
        for _ in 0..reps {
            let next = bpos + step;
            if next >= dist_m * 0.5 {
                break 'bw;
            }
            bpos = next;
        }
    }
    let bwd_start = (1.0 - bpos / dist_m).max(1.0 - dist_m * 0.5 / dist_m);
    let mut mid = fwd_end;
    while mid < bwd_start - 0.0001 {
        mid += coarse / dist_m;
        if mid < bwd_start - 1e-9 {
            buf.push(mid);
        }
    }

    // Backward from receiver (mirror of forward, reversed).
    let mut back_count = 0usize;
    pos = if emit_near { NEAR_OFFSET_M } else { 0.0 };
    'back: for &step in &levels {
        for _ in 0..reps {
            pos += step;
            if pos >= dist_m * 0.5 {
                break 'back;
            }
            buf.push(1.0 - pos / dist_m);
            back_count += 1;
        }
    }
    let back_start = buf.len() - back_count;
    buf[back_start..].reverse();

    if emit_near {
        buf.push(1.0 - near_t);
    }
    buf.push(1.0);
    buf.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_short_path_uniform() {
        let mut buf = Vec::new();
        fill_t_values(200.0, &mut buf);
        assert!(buf.first().copied().unwrap() == 0.0);
        assert!((*buf.last().unwrap() - 1.0).abs() < 1e-9);
        assert!(buf.len() >= 3);
        // Monotonic + first sample at 0, last at 1.
        for w in buf.windows(2) {
            assert!(w[1] > w[0], "non-monotonic: {:?}", buf);
        }
        // With 10 m near-probes, first gap is ≤ 10 m + tolerance.
        let first_gap_m = (buf[1] - buf[0]) * 200.0;
        assert!(
            first_gap_m <= NEAR_OFFSET_M + 1.0,
            "first gap ≤ 10m, got {first_gap_m}"
        );
    }

    #[test]
    fn fill_long_path_bilateral() {
        let mut buf = Vec::new();
        fill_t_values(5000.0, &mut buf);
        assert_eq!(buf[0], 0.0);
        assert!((*buf.last().unwrap() - 1.0).abs() < 1e-9);
        // First gap is the 10m near-probe.
        let first_gap = (buf[1] - buf[0]) * 5000.0;
        assert!(
            (first_gap - NEAR_OFFSET_M).abs() < 1.0,
            "first gap should be 10m near-probe, got {first_gap}"
        );
        // Last gap is symmetric.
        let last_gap = (*buf.last().unwrap() - buf[buf.len() - 2]) * 5000.0;
        assert!(
            (last_gap - NEAR_OFFSET_M).abs() < 1.0,
            "last gap should be 10m, got {last_gap}"
        );
        // Second gap (10m probe → first ramp sample) should be ≈ CADENCE_STEP_M (30m).
        let second_gap = (buf[2] - buf[1]) * 5000.0;
        assert!(
            (second_gap - CADENCE_STEP_M).abs() < 1.0,
            "second gap should be ~30m, got {second_gap}"
        );
    }

    #[test]
    fn near_probe_at_10m_from_both_ends() {
        for dist in &[50.0, 100.0, 300.0, 1000.0, 10_000.0] {
            let mut buf = Vec::new();
            fill_t_values(*dist, &mut buf);
            let first_probe_m = buf[1] * dist;
            let last_probe_m = (1.0 - buf[buf.len() - 2]) * dist;
            assert!(
                (first_probe_m - NEAR_OFFSET_M).abs() < 1.0,
                "D={dist}: near-source probe at {first_probe_m}m"
            );
            assert!(
                (last_probe_m - NEAR_OFFSET_M).abs() < 1.0,
                "D={dist}: near-receiver probe at {last_probe_m}m"
            );
        }
    }

    #[test]
    fn very_short_path_skips_near_probe() {
        // D < 3×NEAR_OFFSET (30 m) → no 10m probe, just uniform.
        let mut buf = Vec::new();
        fill_t_values(25.0, &mut buf);
        assert_eq!(buf[0], 0.0);
        assert!((*buf.last().unwrap() - 1.0).abs() < 1e-9);
        // No 10m probe at t ≈ 0.4 (10/25).
        let has_near = buf.iter().any(|&t| ((t * 25.0) - 10.0).abs() < 0.5);
        assert!(!has_near, "D=25m should not emit 10m probe, got {:?}", buf);
    }

    /// A 10 km ray takes about 56 samples: the cost of every ray scales with this count.
    #[test]
    fn a_ten_kilometre_ray_takes_under_sixty_four_samples() {
        let mut buf = Vec::new();
        fill_t_values(10_000.0, &mut buf);
        assert!((50..64).contains(&buf.len()), "{}", buf.len());
    }
}

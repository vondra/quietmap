//! Wall-clock progress emitters for long-running pipeline stages.
//!
//! Three helpers, used together by each stage:
//!
//! - [`started`] / [`finished`] — phase boundary markers. One line per
//!   phase begin and end.
//! - [`Milestone`] — atomic counter that emits `+N <unit> (total M)`
//!   every time the running total crosses a multiple of `every`.
//!   Lock-free; safe to call from `par_iter` closures.
//! - [`ts`] — wall-clock prefix `[YYYY-MM-DD HH:MM:SS]` (UTC) prepended
//!   to every line. Operators read these logs days later via
//!   `logs/aircraft-extract-latest.log`; without timestamps you can't
//!   tell when a particular line was written.
//!
//! Format is plain text by design — the reader (operator, monitoring
//! script) infers rate / ETA from the timestamps. The emitter only
//! reports facts: what started, what's been processed, what finished.
//!
//! UTC is the convention because the server runs 24/7 across DST
//! boundaries; local time would be ambiguous in archived logs.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;

/// Wall-clock prefix `[YYYY-MM-DD HH:MM:SS]` (UTC). Cheap enough to
/// call per heartbeat: chrono `Utc::now()` reads `CLOCK_REALTIME` once.
pub fn ts() -> String {
    Utc::now().format("[%Y-%m-%d %H:%M:%S]").to_string()
}

/// Phase boundary — "starting" marker. Use at the top of each stage /
/// sub-phase. `what` should name the unit count or scope, not the
/// stage itself (the label carries that): e.g. `started("shuffle/passA",
/// "365 day shards")` → `[ts] [shuffle/passA] start: 365 day shards`.
pub fn started(label: &str, what: &str) {
    eprintln!("{} [{label}] start: {what}", ts());
}

/// Phase boundary — "done" marker. `summary` should report the
/// emission count and any anomaly summary (e.g. "170 z9s empty").
pub fn finished(label: &str, summary: &str) {
    eprintln!("{} [{label}] done: {summary}", ts());
}

/// Atomic milestone counter. Emits one line whenever the running total
/// crosses a multiple of `every` — e.g. `Milestone::new("shuffle/passA",
/// "segments", 1_000_000)` produces `+1M segments (total 1M)`,
/// `+1M segments (total 2M)`, …
///
/// `add` is lock-free and safe across `par_iter` worker threads.
/// Trailing remainder below `every` is not emitted automatically —
/// stages typically follow the loop with a [`finished`] line that
/// reports the exact final total.
pub struct Milestone {
    label: String,
    unit: &'static str,
    every: u64,
    total: AtomicU64,
}

impl Milestone {
    pub fn new(label: impl Into<String>, unit: &'static str, every: u64) -> Self {
        Self {
            label: label.into(),
            unit,
            every,
            total: AtomicU64::new(0),
        }
    }

    /// Add `n` to the counter; emits zero or one log line.
    ///
    /// Logs when the running total crosses one or more thresholds in
    /// this call. To keep output bounded we emit a single line for the
    /// highest crossed multiple, not one per crossing — a single
    /// worker dumping a million-segment day shouldn't produce 1000
    /// near-identical lines.
    pub fn add(&self, n: u64) {
        if n == 0 {
            return;
        }
        let before = self.total.fetch_add(n, Ordering::Relaxed);
        let after = before + n;
        let crossed = after / self.every > before / self.every;
        if !crossed {
            return;
        }
        let marker = (after / self.every) * self.every;
        eprintln!(
            "{} [{label}] +{step} {unit} (total {marker})",
            ts(),
            label = self.label,
            step = human(self.every),
            unit = self.unit,
            marker = human(marker),
        );
    }

    /// Final running total. Stages emit this on the `finished` line.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// Human-readable integer with `K` / `M` / `B` suffix for the
/// milestone log lines. `1_000_000 → "1M"`, `1_500_000 → "1.5M"`,
/// `1_234 → "1234"`.
pub fn human(n: u64) -> String {
    const K: u64 = 1_000;
    const M: u64 = 1_000_000;
    const B: u64 = 1_000_000_000;
    if n >= B {
        format_scaled(n, B, "B")
    } else if n >= M {
        format_scaled(n, M, "M")
    } else if n >= K * 10 {
        format_scaled(n, K, "K")
    } else {
        n.to_string()
    }
}

fn format_scaled(n: u64, scale: u64, suffix: &str) -> String {
    let whole = n / scale;
    let frac = (n % scale) / (scale / 10);
    if frac == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{frac}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn human_formatting() {
        assert_eq!(human(0), "0");
        assert_eq!(human(999), "999");
        assert_eq!(human(9_999), "9999");
        assert_eq!(human(10_000), "10K");
        assert_eq!(human(1_500_000), "1.5M");
        assert_eq!(human(2_000_000), "2M");
        assert_eq!(human(1_234_567_000), "1.2B");
    }

    #[test]
    fn milestone_emits_on_threshold_cross() {
        let m = Milestone::new("test", "items", 1_000);
        m.add(500); // no log: total 500 < 1000
        m.add(400); // no log: total 900
        m.add(200); // log: total 1100 crossed 1000
        assert_eq!(m.total(), 1_100);
    }

    #[test]
    fn milestone_large_add_emits_one_line_not_many() {
        let m = Milestone::new("test", "items", 1_000);
        m.add(5_000); // total 5000 crosses 5 multiples — must emit one line, not 5.
        assert_eq!(m.total(), 5_000);
    }

    #[test]
    fn milestone_zero_add_is_silent() {
        let m = Milestone::new("test", "items", 1_000);
        m.add(0);
        assert_eq!(m.total(), 0);
    }

    #[test]
    fn milestone_is_thread_safe() {
        // Hammer add() from many threads. The atomicity contract is
        // that the FINAL total equals the sum of all adds, regardless
        // of how the threshold crossings interleave.
        let m = Arc::new(Milestone::new("test", "items", 100));
        let n_threads = 8;
        let per_thread = 1_000_u64;
        let handles: Vec<_> = (0..n_threads)
            .map(|_| {
                let m = Arc::clone(&m);
                std::thread::spawn(move || {
                    for _ in 0..per_thread {
                        m.add(1);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.total(), n_threads as u64 * per_thread);
    }

    #[test]
    fn ts_format_is_iso_seconds() {
        let s = ts();
        // [YYYY-MM-DD HH:MM:SS] = 21 chars including brackets.
        assert_eq!(s.len(), 21);
        assert!(s.starts_with('['));
        assert!(s.ends_with(']'));
        assert_eq!(&s[5..6], "-");
        assert_eq!(&s[8..9], "-");
        assert_eq!(&s[11..12], " ");
        assert_eq!(&s[14..15], ":");
        assert_eq!(&s[17..18], ":");
    }
}

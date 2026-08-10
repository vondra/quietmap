//! Measurement census for the tile sampling rule and the receiver skyline —
//! the counters behind `docs/dev/gpu-gather-redesign-2026-08-09.md` §8 task 0
//! (milestone zero), kept as a permanent instrument because task 9 of that plan
//! decomposes any future benchmark gap with exactly these numbers.
//!
//! Everything is OFF unless `QM_TILE_CENSUS=1`: the hot paths pay one
//! `OnceLock` bool read, and only census runs pay the relaxed atomics. The
//! report is a single line the harness greps (`TILECENSUS …`), printed by the
//! painter binaries at the end of a run, in the same generic `k=v` shape as
//! `layer-stats` (box_timing.py parses tokens, never positions).
//!
//! `QM_ARC_NEED_CLIP_M=<metres>` additionally CLIPS every skyline gather need
//! (an output-changing DEV lever, census runs only): with the eligibility gate
//! of SPEC §3.5e all needs should already be near-field, and the clipped run
//! measures the merged-arc demand a bounded-radius per-receiver skyline (plan
//! §6, increment D) would hold.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;

/// Log-ish histogram cap for per-receiver merged-arc counts (linear bins, 1
/// arc per bin, everything past the cap in the last bin — rail's measured max
/// was 827, Praha is the open question this sizes).
const MERGED_BINS: usize = 4096;
/// Need-radius histogram: 64 bins × 250 m = 0..16 km, tail clamped.
const NEED_BINS: usize = 64;
const NEED_BIN_M: f64 = 250.0;

static PAIRS_WALKED: AtomicU64 = AtomicU64::new(0);
static PAIRS_QUAD: AtomicU64 = AtomicU64::new(0);
static PAIRS_ESCALATED: AtomicU64 = AtomicU64::new(0);
static BUCKETS: AtomicU64 = AtomicU64::new(0);
static BUCKETS_ESCALATED: AtomicU64 = AtomicU64::new(0);
static SKY_RECEIVERS: AtomicU64 = AtomicU64::new(0);
static SKY_MERGED_SUM: AtomicU64 = AtomicU64::new(0);
static SKY_MERGED_MAX: AtomicU64 = AtomicU64::new(0);
static RCV_QUAD: AtomicU64 = AtomicU64::new(0);
static RCV_ESCALATED: AtomicU64 = AtomicU64::new(0);

fn merged_hist() -> &'static [AtomicU64; MERGED_BINS] {
    static H: OnceLock<Box<[AtomicU64; MERGED_BINS]>> = OnceLock::new();
    H.get_or_init(|| {
        Box::new(std::array::from_fn::<AtomicU64, MERGED_BINS, _>(|_| {
            AtomicU64::new(0)
        }))
    })
}

fn need_hist() -> &'static [AtomicU64; NEED_BINS] {
    static H: OnceLock<Box<[AtomicU64; NEED_BINS]>> = OnceLock::new();
    H.get_or_init(|| {
        Box::new(std::array::from_fn::<AtomicU64, NEED_BINS, _>(|_| {
            AtomicU64::new(0)
        }))
    })
}

/// `QM_TILE_CENSUS=1` — the one gate for every counter in this module.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("QM_TILE_CENSUS").is_ok_and(|v| v == "1"))
}

/// `QM_ARC_NEED_CLIP_M` — skyline gather-need clip in metres (∞ when unset).
pub fn need_clip_m() -> f64 {
    static V: OnceLock<f64> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("QM_ARC_NEED_CLIP_M")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(f64::INFINITY)
    })
}

/// One (source, receiver) pair the exact walk computed (post byte-stop).
#[inline]
pub fn pair_walked() {
    if enabled() {
        PAIRS_WALKED.fetch_add(1, Relaxed);
    }
}

/// One nondegenerate pair that ran the §3.5d uniform quadrature: `buckets`
/// rays marched, of which `escalated` ran the §3.5e nested arc query. The
/// eligibility fraction of the gather redesign is `pairs_escalated /
/// pairs_quad` (a pair is ELIGIBLE when ≥1 of its buckets clears the 3° gate).
#[inline]
pub fn quad_pair(buckets: u64, escalated: u64) {
    if !enabled() {
        return;
    }
    PAIRS_QUAD.fetch_add(1, Relaxed);
    BUCKETS.fetch_add(buckets, Relaxed);
    if escalated > 0 {
        PAIRS_ESCALATED.fetch_add(1, Relaxed);
        BUCKETS_ESCALATED.fetch_add(escalated, Relaxed);
    }
}

/// One receiver leaving the walk (skyline reset after ≥1 quadrature pair):
/// whether any of its pairs escalated — the fraction of receivers increment D
/// must build a skyline for at all.
#[inline]
pub fn receiver_done(any_quad: bool, any_escalated: bool) {
    if !enabled() {
        return;
    }
    if any_quad {
        RCV_QUAD.fetch_add(1, Relaxed);
    }
    if any_escalated {
        RCV_ESCALATED.fetch_add(1, Relaxed);
    }
}

/// One receiver whose skyline was BUILT: its final merged-arc count and the
/// maximum gather need it ever asked for. Flushed from `ArcSkyline::reset`.
#[inline]
pub fn skyline_receiver(merged: usize, need_max_m: f64) {
    if !enabled() {
        return;
    }
    SKY_RECEIVERS.fetch_add(1, Relaxed);
    SKY_MERGED_SUM.fetch_add(merged as u64, Relaxed);
    SKY_MERGED_MAX.fetch_max(merged as u64, Relaxed);
    merged_hist()[merged.min(MERGED_BINS - 1)].fetch_add(1, Relaxed);
    let bin = ((need_max_m / NEED_BIN_M) as usize).min(NEED_BINS - 1);
    need_hist()[bin].fetch_add(1, Relaxed);
}

fn percentile(hist: &[AtomicU64], total: u64, q: f64) -> usize {
    let target = (total as f64 * q).ceil() as u64;
    let mut seen = 0u64;
    for (i, b) in hist.iter().enumerate() {
        seen += b.load(Relaxed);
        if seen >= target {
            return i;
        }
    }
    hist.len() - 1
}

/// The census line, `None` when the census is off or nothing was counted.
pub fn report() -> Option<String> {
    if !enabled() {
        return None;
    }
    let walked = PAIRS_WALKED.load(Relaxed);
    let quad = PAIRS_QUAD.load(Relaxed);
    if walked == 0 && quad == 0 {
        return None;
    }
    let esc = PAIRS_ESCALATED.load(Relaxed);
    let rcv = SKY_RECEIVERS.load(Relaxed);
    let mh = merged_hist();
    let nh = need_hist();
    let clip = need_clip_m();
    Some(format!(
        "TILECENSUS pairs_walked={walked} pairs_quad={quad} pairs_escalated={esc} \
         eligible_frac={:.6} buckets={} buckets_escalated={} rcv_quad={} rcv_escalated={} \
         sky_receivers={rcv} merged_mean={:.1} merged_p50={} merged_p95={} merged_p99={} \
         merged_max={} need_p95_m={:.0} need_max_bin_m={:.0} need_clip_m={}",
        if quad > 0 {
            esc as f64 / quad as f64
        } else {
            0.0
        },
        BUCKETS.load(Relaxed),
        BUCKETS_ESCALATED.load(Relaxed),
        RCV_QUAD.load(Relaxed),
        RCV_ESCALATED.load(Relaxed),
        if rcv > 0 {
            SKY_MERGED_SUM.load(Relaxed) as f64 / rcv as f64
        } else {
            0.0
        },
        percentile(&mh[..], rcv, 0.50),
        percentile(&mh[..], rcv, 0.95),
        percentile(&mh[..], rcv, 0.99),
        SKY_MERGED_MAX.load(Relaxed),
        percentile(&nh[..], rcv, 0.95) as f64 * NEED_BIN_M,
        percentile(&nh[..], rcv, 1.0) as f64 * NEED_BIN_M,
        if clip.is_finite() {
            format!("{clip:.0}")
        } else {
            "inf".into()
        },
    ))
}

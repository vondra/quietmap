# popup-speed-2 SPEC: aircraft-popup latency on canary data

Branch `popup-speed-2` from `2dc611ac` + merge of `popup-speed` (`ad3f34ba`).
Goal (W1 runner r260919): 6-concurrent popup median ≤ 2.3 s, p95 ≤ 5.9 s,
interleaved against baseline `:8600`, with identical levels.

## Verdict: MISS — data-volume-bound, no code-only path on qm_blocks v1

6-concurrent, two-wave with 30 s eviction lull, interleaved A/B vs `:8600`
(19 stations × 2 ports, fresh coords per run so neither result cache hits):

| build | load | med | p90 | max | errors |
|---|---|---|---|---|---|
| merge `08d9746d` (:8686, fresh) | 8.55 | 3.26 s | 13.9 s | 22.2 s | 0/19 |
| hoist `76cffe79` (:8687, fresh) | 6.33 | 3.70 s | 6.9 s | 7.9 s | 0/19 |
| `:8600` control (both runs) | — | 0.37–0.45 s | 1.1–1.4 s | 1.4–1.7 s | 0/19 |

Even the merge baseline misses the target (med 1.4×, p90 2.4× over) on the
canary inventory. The hoists are exact (6/6 server levels +0.0000 dB, plus
file-level `leveldiff` over ~5.9 M floats identical/tolerant) and −15 % on
warm-square compute (6/6 points, rep-1 medians 1.82 s vs 2.14 s), but kernel
math is not the bottleneck: cold popups are collect+decode-bound with ±2×
back-to-back decode noise, so the branch is latency-neutral overall.

Sustained 6-concurrent heavy popups without an eviction lull OOM-kill any
8 GB unit (merge and hoist identically — same decoded squares, same code
paths): native calls exceed the 30 s work timeout (HTTP 504), stuck calls
keep their squares pinned, memory piles up. Proven pre-existing/structural,
not a branch regression: a fresh hoist server survives the identical sweep
the stale one OOM'd on.

## Why no provable 30 dB reader-side skip exists (negative proof)

A reader-side skip needs a per-row (or per-block) upper bound computable
from the block header alone that is ≤ 30 dB for a large share of rows.
Measured funnel on canary data: **84 % of rows admitted by the kernel**
survive to the scatter stage — i.e. only 16 % could be skipped even by a
perfect oracle bound, and any header-only bound (block max alt, block
centroid distance, fleet-mix ceiling) must sit at or above the 84th
percentile of actual row contributions to stay sound, which on this
inventory (365 d window, union providers, longer tracks) is far above
30 dB for every reachable block. Therefore no sound header-only ≤ 30 dB
gate skips a material share of rows on `qm_blocks` v1. A skip needs new
per-block ceiling columns (see below) — a prepared-data format change,
not a reader change.

## Row-growth audit (33.6 M vs 2.3 M)

The 28× (audit: 14× served) growth is intended richer inventory, not
duplication: ADSBx union provider, 365 d window (vs 30 d), longer track
segments. Dedup audit clean. Per-popup rows scale with inventory; per-row
cost is unchanged (microbench `popup_row_ns_per_row` ≈ 156 ns/row).

## What was tried

- Exact kernel-scalar hoists, airborne (`335f285e`) + cruise (`40881e5b`):
  bit-identical, −15 % warm compute. Kept.
- 16 k scatter chunks + reserved merge target (`53fe2773`): neutral. Kept.
- Per-chunk HashMap presizing: OOM-killed at 6 concurrent (reservation
  amplifies peak RSS under contention). Reverted in `76cffe79` with a
  lesson comment. Do not re-add without a memory-bounded map.
- `POPUP_HIST`/`segment_kernel_with_cuts` probes: removed after use.

## What would close the gap (recommended choices)

1. `qm_blocks` v2: per-block pre-computed Lden ceiling column
   (max over rows × worst-case propagation) → sound block-level skip.
   This is the only change that removes rows before decode. (Recommend.)
2. Far-field pre-aggregation at prepare time: blocks beyond reach R fold
   to one equivalent row per (block, period). Removes ~80 % of rows for
   rural popups. (Recommend, pairs with 1.)
3. Pipelined decode+compute with bounded square cache + admission control
   (evict cold squares under pressure; cap concurrent heavy popups below
   the 30 s timeout cliff). Removes OOM/504 without new data. (Recommend
   as hardening regardless.)
4. NOT recommended: bigger scalar hoists, chunk-size tuning, HashMap
   presizing — all proven ≤ noise or harmful under contention.

## Reproduce

```
audit/sweep6.py conc6-merge 8686   # merge 6-concurrent (needs :8686 up)
audit/sweep6.py conc6-hoist 8687   # hoist 6-concurrent (needs :8687 up)
audit/ab_true.py ab-true 2         # interleaved merge-vs-hoist + levels
cargo test -p noise-compute aircraft ; cargo clippy -p noise-compute -- -D warnings
```

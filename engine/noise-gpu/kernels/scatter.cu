// Surface scatter GPU kernels (grows into the full rail kernel per
// docs/dev/gpu-ground-hybrid-plan.md). Ports of the noise-compute CPU path,
// each validated GPU-vs-CPU before the next piece is added. The hot path is already
// fp32 — ray-march rc lerp, bilinear_elev_rc, flc, base/atm_km, fexp; the remaining
// f64 is precision-critical and STAYS (fp32 there flips the LoS gate / reach cull or
// regresses speed — two measured NO-GOs, docs/dev/road-rail-gpu-ledger.md): the reach
// cull, LoS diffraction gate + δ*/fit_plane cancellation, profiles, cadence, budgets.
// Args are packed into a few buffers (cudarc's tuple launch caps at ~12 args).

#define NB 8
// TPX (tile side in receiver px) and OBST_META_STRIDE are INJECTED by build.rs
// from raster_reader::TILE_PX and noise_gpu::META_STRIDE. They decide how far
// this kernel writes into `out` and how it strides the obstacle metas, so a
// hand-copied value that drifts is a silent out-of-bounds device write. The
// fallbacks exist only for a bare `nvcc kernels/scatter.cu` syntax check.
#ifndef TPX
#define TPX 512
#endif
#define M_LAT 110540.0          // M_PER_DEG_LAT
#define M_LON_EQ 111320.0       // M_PER_DEG_LON_EQ
#define LN10 2.302585092994046
#define PI_D 3.141592653589793

__constant__ double A_W[NB]       = {-26.2,-16.1,-8.6,-3.2,0.0,1.2,1.0,-1.1};
__constant__ double ALPHA_ATM[NB] = {0.1,0.4,1.0,1.9,3.7,8.7,22.0,58.4};
__constant__ double BAND_FREQ[NB] = {63.0,125.0,250.0,500.0,1000.0,2000.0,4000.0,8000.0};
__constant__ double GROUND_CF[NB] = {-1.5,-0.7,1.5,2.5,2.0,1.3,0.7,0.2};
// CNOSSOS-EU 2015/996 (2.5.15) hard-ground floor, INJECTED by build.rs from
// noise-compute's GROUND_HARD_FLOOR_DB. This fallback exists only for a bare
// `nvcc kernels/scatter.cu` syntax check — do not hand-edit it into agreement.
#ifndef GROUND_HARD_FLOOR_DB
#define GROUND_HARD_FLOOR_DB -3.0
#endif
// Parenthesised aliases: the -D value arrives unbracketed, so every USE must
// bracket it or `x * GROUND_HARD_FLOOR_DB` parses as `x * -3.0` inside a wider
// expression.
#define GHF_D ((double)(GROUND_HARD_FLOOR_DB))
#define GHF_F ((float)(GROUND_HARD_FLOOR_DB))
// Largest ground GAIN any band can reach for any G in [0,1] — mirrors
// constants::GROUND_GAIN_UB_DB. `A_gr(G) = max(CF*G,0) + FLOOR*(1-G) >= FLOOR`
// with equality at G = 0 in EVERY band, so the deepest gain is the floor
// itself. The old per-band `max(-CF[i],0)` (1.5/0.7/0…) was the bound only
// while A_gr bottomed out at 0 over hard ground; keeping it would make the
// energy-budget skip's `ub >= exact` false and drop audible sources silently.
#define GROUND_GAIN_UB_F ((float)(-(GROUND_HARD_FLOOR_DB)))

// THE ground term, mirroring iso9613::ground_atten_db expression-for-expression:
// `A_gr[i] = max(CF[i]*G, 0) + FLOOR*(1-G)`. The max() is the (2.5.18) lower
// bound, which REPLACES the CF table's negative low-frequency lobe instead of
// stacking on it; at G = 0 every band lands on the -3 dB the standard states
// verbatim. f64 arm for the arc kernel, f32 arm for the scatter hot loop —
// same expression, the kernel's usual two precisions.
__device__ __forceinline__ double ground_atten_d(int i, double g) {
    return fmax(GROUND_CF[i] * g, 0.0) + GHF_D * (1.0 - g);
}
__device__ __forceinline__ float ground_atten_f(int i, float g) {
    return fmaxf((float)GROUND_CF[i] * g, 0.0f) + GHF_F * (1.0f - g);
}
__constant__ double ALPHA_VEG[NB] = {0.01,0.015,0.02,0.025,0.03,0.04,0.045,0.06};
__constant__ double MAX_VEG[NB]   = {2.0,3.0,4.0,5.0,6.0,8.0,9.0,12.0};
// Lden period weights = 12/4/8-hour × 0/5/10-dB penalty (10^(p/10)); the shared
// /24 cancels in the skip ratio (scatter_line::LDEN_WEIGHTS).
__constant__ double LDEN_W[3]     = {12.0, 12.649110640673518, 80.0};  // 4·√10
#define UB_SAFETY 1.0001         // inflate UB past fast_exp non-monotonicity
#define SOS 340.0                // SPEED_OF_SOUND
#define SINGLE_DIFF_CAP 20.0     // ISO 9613-2 §7.3 single-edge cap
// CNOSSOS favourable/homogeneous mixing — compile-time mirrors of constants.rs
// (P_FAV, FAVOURABLE_MIXING, FAV_RAY_CURVATURE_*). Flip FAVOURABLE_MIXING to 1
// IN THE SAME commit as the CPU const + rebuild the PTX + re-run e2-full
// CPU≡GPU parity (the plan's G6 gate) — a one-sided flip silently forks the
// physics between lanes.
#define P_FAV 0.5
#define FAVOURABLE_MIXING 1
#define FAV_GAMMA_MIN 1000.0
#define FAV_GAMMA_PER_DSR 8.0
#define CELL_M (110540.0/3600.0) // mirror of path_profile::CELL_M (M_PER_DEG_LAT/3600)
#define NEAR_OFFSET_M 10.0       // near-endpoint probe
#define MAXT 80                  // per-thread profile capacity (fill_t ≤~58 @10km)
// SURFACE-HEATMAP coarse-middle cadence — MUST match the CPU defaults in
// scatter_line.rs (SHADOW_MID_STRIDE / SHADOW_SRC_ZONE_M / SHADOW_RX_ZONE_M) so
// the GPU line lane stays bit-parity with the CPU scatter. The dense 10/30/60/120
// m ramp is kept only within the near-end zone; the far field is coarse-stepped.
// Set stride 1 = exact. NOTE: the GPU has no env knobs — these are the
// compile-time mirror of the CPU env DEFAULTS. If the CPU knobs are tuned,
// re-sync here + rebuild the PTX (then re-check CPU≡GPU parity).
#define SHADOW_MID_STRIDE 3
#define SHADOW_SRC_ZONE_M 600.0
#define SHADOW_RX_ZONE_M 600.0
// scatter_band::EXACT_CADENCE_MAX_DIST_M (fix-pack Fix 5): at or below this the
// heatmap samples where the POPUP samples — the coarse middle is off, whatever
// the zones say. Mirror of the CPU `cadence_for_ray` gate; without it the two
// lanes fork on every 307–400 m ray.
#define EXACT_CADENCE_MAX_DIST_M 400.0

// ---- Arc-clipped line screening (CNOSSOS fix-pack Fix 1) — compile-time
// mirrors of propagation/arc_screening.rs. A 250 m microsegment subtends 136°
// at 50 m, so the cp ray's single screening verdict cannot stand for the whole
// segment: the blocked sub-intervals of its angular span get their own
// evaluation and the ground/barrier term is energy-averaged over the fan.
// Signed-δ reject floor for a prune that runs BEFORE δ* is known. maekawa_bands
// silences band i when δ ≤ λ_i/4 − δ* OR δ ≤ −λ_i/20, so all bands are silent
// only below min_i max(λ_i/4 − δ*, −λ_i/20). That floor DROPS as δ* rises, so a
// prune with no δ* in hand must use the δ*→∞ limit, which is min_i(−λ_i/20) =
// −λ/20 at the LOWEST band, 63 Hz: −0.269841 m at SOS = 340. (The 8 kHz value is
// the binding one only at δ* → 0, where the floor is +0.010625 m; using it as a
// δ*-free constant would skip near misses that still screen — verified against
// diffraction::maekawa_bands, e.g. δ = −0.002125 with δ* = 0.1 still yields
// 4.39 dB at 1 kHz.)
//
// ONE definition for both signs. The magnitude is INJECTED by build.rs, which
// mirrors noise-compute's `PENUMBRA_DELTA_FLOOR_M = -SPEED_OF_SOUND/63/20`
// expression rather than its value; this fallback exists only for a bare
// `nvcc kernels/scatter.cu` syntax check — do not hand-edit it into agreement.
// `340.0 / 63.0 / 20.0` had FIVE hand copies (twice in this file), which is the
// same failure mode that lost the ground term at nine call sites.
#ifndef ARC_PENUMBRA_FLOOR_M
#define ARC_PENUMBRA_FLOOR_M 2.698412698412699e-1
#endif
#define ARC_DELTA_REJECT (-(ARC_PENUMBRA_FLOOR_M))
// INJECTED by build.rs from noise-compute's DEGENERATE_SPAN_RAD — this fallback
// exists only for a bare nvcc invocation. Do not hand-edit it into agreement.
#ifndef ARC_DEGENERATE_SPAN
#define ARC_DEGENERATE_SPAN 1e-4
#endif
// Likewise from CP_AZIMUTH_EPS — the tolerance deciding which interval OWNS the
// characteristic-point ray. Injected for the same reason: it is one number that
// has to mean the same thing on both lanes.
#ifndef ARC_CP_EPS
#define ARC_CP_EPS 1e-9
#endif
#define ARC_ESCALATE_SPAN 0.26     // ESCALATE_SPAN_RAD
#define ARC_ESCALATE_MAX_PARTS 9   // ESCALATE_MAX_PARTS
// Blocked-interval capacity per (segment, receiver) pair. GPU-ONLY bound: the
// CPU grows a Vec, a thread cannot.
//
// WARNING, 2026-08-09: the sizing premise below is now WEAKER than when it was
// written and the demand study further down was measured under the OLD regime.
// It assumed an interval survives only if a footprint's angular hull overlaps the
// span AND a ray to its centre crosses it — i.e. it was sized against the set
// that had already passed the confirmation ray and the δ prefilter. Both are gone
// (see `arc_screening`'s "Why the blocked test is geometry only"), so what enters
// this capped list is now EVERY span-clipped edge, admission runs only after
// collection, and the peaks in the demand study are under-estimates. The two
// tiles measured after the change reported dropped_arcs = 0, so it did not bind
// there, but the bound has NOT been re-derived for the widened set. Do that
// before trusting this number on denser geometry than rail 2206/1391.
//
// The old premise, kept because the shape of the argument still applies to the
// admitted set: an interval matters only if it is one of the buildings actually
// standing in the fan between receiver and segment. Overflow drops the EXTRA
// footprints (the intervals already collected still average), which degrades
// continuously toward less screening rather than snapping back to the cp
// verdict's full-segment shadow.
// Overridable (-DARC_MAX_IV=N via NOISE_GPU_DEFINES) so the cap's PHYSICS cost
// can be A/B-measured against the CPU's uncapped Vec: an urban triangle bbox
// holds far more than 24 footprints, and the CPU keeps every one of them.
#ifndef ARC_MAX_IV
#define ARC_MAX_IV 24
#endif
// f64 slots per per-cell index in the flat `metas` array — injected by build.rs
// from noise_gpu::META_STRIDE (a drift here walks a neighbouring index's grid).
#ifndef OBST_META_STRIDE
#define OBST_META_STRIDE 19
#endif
// f32 slots per footprint in foot_box (the bbox) — injected like the rest. The
// height + convexity slots went with the -DARC_EDGE_UNION switch on 2026-08-09:
// the hull emission it selected is not a cheaper approximation of the per-edge
// union but different physics, because a hull arc can carry only one RANGE. Kept
// where the emission is, arc walk below.
#ifndef FOOT_BOX_STRIDE
#define FOOT_BOX_STRIDE 4
#endif
// ---- Footprint-CSR arc walk (TRACK C 2026-08-03). -DARC_FOOTPRINT_CSR=0
// restores the edge-CSR walk, which is what the acceptance A/B compares
// against. The footprint walk visits each footprint ONCE (back-link dedup),
// rejects most of them with sign tests before any atan2, completes the hull
// from its own contiguous edge run, and unions the result straight into the
// merged list — so ARC_MAX_IV, and the physics divergence its cap caused
// against the CPU's uncapped grouping, have no counterpart here.
#ifndef ARC_FOOTPRINT_CSR
#define ARC_FOOTPRINT_CSR 1
#endif
// Merged blocked arcs per (segment, receiver). Unlike ARC_MAX_IV this bounds
// arcs, not footprints: overlapping buildings of similar height collapse into
// one, so the geometry has to fragment the fan this many ways to reach it.
// CORRECTED 2026-08-04: this comment used to claim the overflowing arc is
// "unioned into its nearest neighbour ... never dropped". The code does the
// opposite — `arc_iv_union` returns early and DROPS it, which under-states the
// blocked extent. That is the safe direction (less screening, never a phantom
// shadow), but a cap sized from the old comment would have been sized wrong.
//
// RAISED 32 -> 160, 2026-08-05, from a MEASUREMENT of the demand, not a guess.
// The (height, range) strata make the merged list ~2.5x longer than the
// height-only fuse it replaced — a near and a far footprint that used to
// collapse into one slot now hold two — so a cap that provably never bound
// before started binding hard: 1 123 557 arcs dropped on the dense rail tile
// 2206/1391 (48 692 pairs), against 2 662 over 1 201 pairs before the strata —
// 422x the drop rate, for the same tile and the same cap.
//
// DEMAND. One instrumented run per tile at cap 256 (which never bound: 0 arcs
// dropped, so every list PEAK below is the real requirement, not a clipped one),
// histogrammed per (segment, receiver) pair. 1/16 of the receivers sampled, whole
// swizzle tiles at a time. Pairs whose list peaked above:
//
//                     >32        >48       >64      >80     >96   >128   >192
//   rail  2206/1391   5 436        11        0        0       0      0      0
//   praha 2212/1387  14 602 016  3 114 726  560 078  81 660  2 971    0      0
//
// So the rail tile the e2 gate runs on needs 64, and the dense PRAHA ROAD tile
// needs more than 96 — four orders of magnitude more demand at every threshold.
// Anchoring on rail alone would have shipped a cap that silently drops arcs on
// every city in the world, which is why both were measured.
//
// 128 is the smallest probed value that binds nowhere IN THE SAMPLE, and that is
// exactly why it is not the value. The tail falls ~30x per 16 slots
// (81 660 -> 2 971 between 80 and 96), so scaling the sample back to the whole
// tile puts of order 50 pairs above 128 and of order none above 160. Sizing to
// the sample's own last non-zero bucket would ship a cap that binds on the very
// tile it was measured on. Headroom is also cheap where it is not used:
// `arc_iv_union`'s insert scan is O(niv) in the LIVE list length, not in the cap,
// so past the demand ceiling the only thing a bigger ceiling changes is the
// local-memory footprint, 32 B per slot per thread (5 KB at 160).
//
// COST, kernel time on the dense rail tile 2206/1391, three arms of one
// sequential uncontended run on the 5070 (each 512² pixels, 49 173 rail rows):
//   cap 32,  __logf key   158 304.5 ms   (as shipped, 1 123 557 arcs dropped)
//   cap 32,  fixed key    156 770.3 ms   (-1.0 %, 1 123 538 dropped)
//   cap 96,  fixed key    157 437.4 ms   (+0.4 % over cap 32, 0 dropped)
// i.e. both fixes together are inside run-to-run noise on a tile whose demand
// never exceeds 64. Praha, where a quarter of pairs carry a list longer than 32,
// is the tile that PAYS — same sampled tiles in every arm, kernel time only:
//   praha cap 32   688 533.7 ms      rail cap 32   13 598.8 ms
//   praha cap 96   792 481.1 ms      rail cap 96   13 770.0 ms   (+1.3 %)
//   praha cap 128  739 843.3 ms      rail cap 128  13 362.2 ms   (-1.7 %)
//   praha cap 160  751 768.0 ms      rail cap 160  14 991.6 ms   (+10.2 %)
// A dense city tile pays ~9 % at the shipped value for not dropping arcs. Note
// the cost is NOT monotone in the cap — 128 timed FASTER than 96 on identical
// geometry, and on the rail tile caps 96/128/160 all do the SAME work (demand
// there is 64) yet spread 12 %. Past the demand ceiling nothing changes but the
// local-memory footprint and how it lands in cache, so single-run timings in that
// band rank layouts, not algorithms, and cannot decide the ceiling. The demand
// distribution decides it; the price of getting it wrong is now loud, not silent.
//
// The CPU twin (`ArcBounds::max_arcs`/`max_intervals`) is `usize::MAX` and its
// documented overflow policy is to FUSE the closest pair, not to drop — so 32 had
// no CPU counterpart at all and every drop was a pure lane fork. A cap chosen from
// two tiles is still a bet on unseen geometry; that is what the always-on fault
// counter (`OUT_FAULT_SLOT`) and the e2 gate's hard failure on it are for. Note
// the fixture cannot catch any of this: its scenes carry 5 footprints, so no cap
// can bind there (screening_fixture.rs:1089).
#ifndef ARC_MAX_MERGED
#define ARC_MAX_MERGED 160
#endif
// Mirrors of arc_screening.rs. Two arcs may be described by ONE (range, height)
// pair only if their heights already agree — otherwise merging them invents an
// obstacle that is simultaneously near-and-low and far-and-tall, which is the
// same "phantom" the convex hull makes, one level coarser. ARC_PENUMBRA_FLOOR_M
// is the δ*-free floor (λ/20 at the LOWEST band) and is defined with
// ARC_DELTA_REJECT near the top, both injected by build.rs. There is no grazing
// prune here any more: it, and the whole δ prefilter it belonged to, are gone
// from BOTH lanes — see `arc_screening`'s "Why the blocked test is geometry
// only". ARC_FUSE_HEIGHT_TOL_M is injected too — `arc_fuse_key` FLOORS a
// height by it, so the two lanes must divide by the same f32 or an arc lands in
// the neighbouring stratum. Fallback = bare-nvcc syntax check only.
#ifndef ARC_FUSE_HEIGHT_TOL_M
#define ARC_FUSE_HEIGHT_TOL_M 3.0f
#endif
// Ratio between neighbouring RANGE strata: the other half of the same rule. A
// fusion hands `near = min` to the UNION of the fused azimuth ranges, so merging
// a near obstacle with a far one puts the far one's shadow at the near one's
// range and the admission test can no longer discard it. Keyed on height ALONE —
// as both lanes were — nearly every overlapping pair is fusible, because 84.6 %
// of Praha's shading edges carry one of three literal heights. Swept on
// `screening_fixture` scene k: 1.50/1.12 dB unstratified, 0.23/0.25 at 1.5 (v3
// and v4 to the decimal).
//
// The ratio itself is NOT defined here. `arc_fuse_key` needs only its logarithm,
// and build.rs injects that as ARC_FUSE_RANGE_RATIO_LN — computed in Rust from
// `arc_screening::ARC_FUSE_RANGE_RATIO` — because CUDA's own `logf(1.5f)` differs
// from Rust's `1.5f32.ln()` by one ulp and that ulp moves arcs between strata
// (see `arc_fuse_key`). A hand-copied `1.5f` here would have been a second,
// silent source of the same fork.
// Scanline-rasterise the (receiver, start, end) triangle instead of sweeping its
// bbox. -DARC_TRI_WALK=0 builds the bbox sweep, which — with the footprint walk
// and no cap — is the EXACT kernel every optimisation is diffed against.
#ifndef ARC_TRI_WALK
#define ARC_TRI_WALK 1
#endif
// Minimum angular span (rad) that gets the arc treatment at all. The default is
// ARC_DEGENERATE_SPAN — i.e. OFF, the exact kernel. The physics argument for
// raising it is CNOSSOS point-source validity, which is also the fix-pack's own
// K=1 rule: below ~10° a microsegment IS a point source at that receiver, so the
// cp verdict already represents it and the blocked-fraction correction has
// nothing to correct. It is a DECLARED bound, swept against the exact kernel
// with compare_hm3 — never a tuned constant.
// INVARIANT: A PHYSICS CONSTANT IS DEFAULT-ON IN EVERY LANE OR IT DOES NOT
// EXIST. This kernel once had the gate reachable only through -DARC_MIN_SPAN, so
// the shipped GPU ran arc screening on every pair the CPU skipped: the lanes
// forked silently, P4 measured a comparison that had been rigged by the
// asymmetry, and it cost 2.6x kernel time as well (267 428 -> 102 653 ms on the
// dense rail tile). The cure was to make it a DEFAULT here — and then the same
// invariant bit in the other direction and had to be paid a second time.
//
// 2026-08-04: the CPU swept this gate on its OWN lane and turned it OFF
// (`ARC_BOUNDS_DEFAULT.min_span_rad = 0.0`). At 0.01 rad dense Praha puts 25
// road and 37 rail receivers over the 1.0 dB line, maxima 7.8 / 11.4 dB, to buy
// 1.15-1.33x. The reason is that ONE number drives TWO different tests: the
// pre-gate at the bottom of this file is a genuine point-source test and is
// sound (it fires zero times), while the in-kernel test above skips whenever the
// ACTUAL span is small — which also catches a long segment seen nearly END-ON,
// where the cp ray does NOT represent it. This define stayed at 0.01 for two
// hours after the CPU moved, i.e. the invariant was violated again, silently,
// by the fix for its first violation. Found by review, not by a gate.
//
// The default is ARC_DEGENERATE_SPAN, which makes the effective threshold
// identical to the CPU's `max(DEGENERATE_SPAN_RAD, min_span_rad)` — mirroring
// the CPU EXPRESSION, not a copy of its number, so the lanes cannot drift apart
// again by editing one side. -DARC_MIN_SPAN=<x> raises it for A/B, which is the
// ONLY thing a -D lever may be used for.
#ifndef ARC_MIN_SPAN
#define ARC_MIN_SPAN ARC_DEGENERATE_SPAN
#endif
// ---- `out` LAYOUT. Mirrored by the OUT_* consts in noise-gpu/src/lib.rs, and
// the host tells the kernel how many f32 slots it actually ALLOCATED in
// `meta[13]` — so every optional region below is entered only after the buffer
// has been proven big enough for it, on the host's own word.
//
//   [0, OUT_ENERGY_SLOTS)                the three period energies per pixel
//   [OUT_FAULT_SLOT]                     ARC FAULT: blocked arcs dropped for
//                                        ARC_MAX_MERGED overflow, whole launch
//   [OUT_ARCSTAT_BASE + opix*8, +8)      per-pixel PROF_COUNTERS counters
//
// The fault slot is ONE float and is always allocated, in production too: a
// silent drop with no channel to report it on is what let ARC_MAX_MERGED
// under-screen 1.1 M arcs unnoticed. The 8-per-pixel counter block is 8 MiB at
// 512² and is NOT allocated in production.
#define OUT_ENERGY_SLOTS  (TPX * TPX * 3)
#define OUT_FAULT_SLOT    OUT_ENERGY_SLOTS
#define OUT_ARCSTAT_BASE  (OUT_ENERGY_SLOTS + 1)
#define OUT_SLOTS_PROD    (OUT_ENERGY_SLOTS + 1)
#define OUT_SLOTS_PROF    (OUT_ARCSTAT_BASE + TPX * TPX * 8)
// -DPROF_COUNTERS=1 makes the kernel report, per pixel, how many (segment,
// receiver) pairs survived the budget skip and how many of those actually took
// the arc path. The RATIO is what transfers between cells: it says whether a
// span bound is trimming pairs that cannot matter or clipping real work.
//
// The counters are a compile-time AND a runtime choice, and it takes both.
// Compile-time alone (what this was until 2026-08-05) meant a kernel built with
// -DPROF_COUNTERS=1 wrote `out[OUT_ARCSTAT_BASE + opix*8 .. +8]` unconditionally
// — fine under `e2-full`, which allocates that region, and 8 MiB of writes PAST
// THE END of the device buffer under `gpu-surface`, which allocates the energies
// alone. No bounds check exists on a device store: it is silent corruption of
// whatever cudarc handed out next. Now the writes are gated on the host having
// declared a buffer that holds them (`meta[13] >= OUT_SLOTS_PROF`), so the same
// counter-instrumented PTX is safe under both binaries and production allocates
// — and pays for — nothing but the one fault slot.
//
// PROVEN, `compute-sanitizer --tool memcheck` on the 5070, same
// -DPROF_COUNTERS=1 kernel driven by a host allocating and declaring the
// PRODUCTION buffer (i.e. the `gpu-surface` situation), rail tile 2206/1391:
//   consulting meta[13] (this code)        ERROR SUMMARY: 0 errors
//   compile-time gate alone (as shipped)   ERROR SUMMARY: 35 errors,
//                                          "Invalid __global__ ... of size 4"
// and `gpu-surface` built with the counters now writes a tile byte-identical to
// the plain build's instead of running off the end of its own allocation.
#ifndef PROF_COUNTERS
#define PROF_COUNTERS 0
#endif
// Per-RECEIVER footprint hull cache (direct-mapped, power of two; 0 = off).
// A footprint's angular hull depends only on (receiver, footprint), but the
// walk recomputes it once per SEGMENT — and a receiver sees ~321 segments on
// the dense rail tile, so the same building's ~6 edges are re-projected through
// 2 f64 atan2 each, over and over. A thread owns one receiver for its whole
// life, so the cache is valid for that life. Stored in ABSOLUTE azimuth (a
// footprint seen from outside subtends < π, so the hull is an interval that
// re-bases with one wrap_pi), which is what makes it reusable across segments
// whose span base differs.
// MEASURED AND REJECTED (frozen-source A/B, dense rail tile): 145 cells >0.5 dB
// and max 13.5 dB against the exact kernel, for ~2 % of kernel time. The stored
// (absolute start, width) pair is NOT base-invariant — the min/max hull of a
// footprint sitting near the ANTIPODE of the span base spans ~2π in that
// representation, so a hit reconstructs a different arc than a fresh
// computation would. Kept behind the switch as a documented dead end; default
// OFF.
#ifndef ARC_HULL_CACHE
#define ARC_HULL_CACHE 0
#endif
// Vector noise walls are SEGMENTS on the GPU too (fix-pack Fix 3): 6 f64 each,
// {start_lat, start_lon, end_lat, end_lon, height_m, dist_m}. Mirrors
// `noise_gpu::BARRIER_STRIDE` and `types::BARRIER_PATH_HORIZON_M`.
#define BARR_STRIDE 6
#define BARR_HORIZON_M 175.0
#define TAU_D 6.283185307179586
// geo::FLC_MIN_PERP_M — a receiver on the segment's extended line has
// d_perp = 0, where the exact line integral θ/d_perp is 0/0.
#define FLC_MIN_PERP_M 0.5

// ---- DEV-ONLY phase ablation (-DPROF_ABLATE=<bits> via NOISE_GPU_DEFINES).
// 0 = the production kernel, bit-for-bit. Each bit removes ONE phase so the
// kernel-time DELTA between two builds prices that phase; the outputs of a
// non-zero build are deliberately wrong and must never be gated or shipped.
//   1  arc screening off entirely      (pre-fix-pack cp-ray behaviour)
//   2  arc stops after the hull build  (prices the triangle-bbox area query)
//   4  arc stops after confirm+merge   (prices arc_ray_hits_id)
//   8  interval rays reuse the cp bands (prices the per-interval ray_path_bands)
//  16  cp ray skips obstacle_best_candidate (prices the cp-ray obstacle DDA)
//  32  arc returns right after the span setup   (prices the CALL, not the walk)
//  64  arc walks cells but touches no footprint (prices the cell sweep alone)
#ifndef PROF_ABLATE
#define PROF_ABLATE 0
#endif
#define ABL_ARC_OFF     (PROF_ABLATE & 1)
#define ABL_ARC_HULL    (PROF_ABLATE & 2)
#define ABL_ARC_CONFIRM (PROF_ABLATE & 4)
#define ABL_ARC_CPEVAL  (PROF_ABLATE & 8)
#define ABL_CAND_OFF    (PROF_ABLATE & 16)
#define ABL_ARC_SPAN    (PROF_ABLATE & 32)
#define ABL_ARC_CELLS   (PROF_ABLATE & 64)

// ---- raster_reader::FusedGrid::lookup_fused_rc (elevation bilinear) ----
__device__ __forceinline__ double bilinear_elev_d(
    const float* elev, int rows, int cols,
    double lat_min, double lon_min, double inv_cell_deg, double lat, double lon)
{
    double rf = (lat - lat_min) * inv_cell_deg;
    double cf = (lon - lon_min) * inv_cell_deg;
    rf = fmin(fmax(rf, 0.0), (double)(rows - 1));
    cf = fmin(fmax(cf, 0.0), (double)(cols - 1));
    int r0 = min((int)floor(rf), rows - 2);
    int c0 = min((int)floor(cf), cols - 2);
    double fr = rf - (double)r0, fc = cf - (double)c0;
    long base = (long)r0 * cols + c0;
    double e00 = elev[base], e01 = elev[base + 1];
    double e10 = elev[base + cols], e11 = elev[base + cols + 1];
    double v0 = e00 + fc * (e01 - e00);
    double v1 = e10 + fc * (e11 - e10);
    return v0 + fr * (v1 - v0);
}

// ---- noise_compute fast_exp_f64, fp32 internals (exp2f/poly on the SFU; f64
// emulation of exp is the dominant per-source cost on consumer Ada). Arg widened
// in; ~1e-6 drift vs the f64 form, validated against the baseline tile.
__device__ __forceinline__ double fexp(double xd) {
    float x = (float)fmin(fmax(xd, -87.0), 88.0);
    float n = roundf(x * 1.4426950408889634f);   // 1/ln2
    float r = x - n * 0.6931471805599453f;        // ln2
    float r2 = r * r;
    float poly = 1.0f + r + r2 * (0.5f + r * (1.0f/6.0f + r * (1.0f/24.0f + r * (1.0f/120.0f))));
    return (double)(poly * exp2f(n));             // 2^n exact for integer n
}

// ---- geo::point_to_segment_full (the fields the line scatter needs) ----
// Lon-scale `cos` → SFU `__cosf` (1 val/source; ~1e-6 rel err ≪0.5 dB). The
// projection/dot/sqrt stay f64: bisected on the box, an fp32 projection put
// `d_end` within fp32-ε of the per-source reach cull at a handful of cells →
// presence flip → 2.0 dB (>1.5 dB gate). The f64 dot-products are also where
// the only measurable p2s f64 cost lives, so fp32 there bought <1% anyway.
__device__ __forceinline__ void p2s(
    double plat, double plon, double alat, double alon, double blat, double blon,
    double* d_end, double* d_perp, double* cplat, double* cplon, double* frac)
{
    double mid = ((alat + blat) * 0.5) * (PI_D / 180.0);
    double mlon = M_LON_EQ * fmax(__cosf((float)mid), 0.01f);
    double bx = (blon - alon) * mlon, by = (blat - alat) * M_LAT;
    double px = (plon - alon) * mlon, py = (plat - alat) * M_LAT;
    double ab = bx * bx + by * by;
    double tu = (ab < 1e-10) ? 0.0 : (px * bx + py * by) / ab;
    double tc = fmin(fmax(tu, 0.0), 1.0);
    double cpx = tc * bx, cpy = tc * by;
    *d_end = sqrt((px - cpx) * (px - cpx) + (py - cpy) * (py - cpy));
    // UNCLAMPED foot ⇒ distance to the segment's INFINITE line: the finite-line
    // correction's own geometry (fix-pack C), distinct from the endpoint
    // distance the divergence term uses.
    double fx = tu * bx, fy = tu * by;
    *d_perp = sqrt((px - fx) * (px - fx) + (py - fy) * (py - fy));
    *cplat = alat + tc * (blat - alat);
    *cplon = alon + tc * (blon - alon);
    *frac = tu;
}

// Block-corner radius UPPER BOUND in the p2s metric, for ANY source latitude: the
// longitude leg uses M_LON_EQ (equatorial, cos=1) which is >= every p2s mlon
// (M_LON_EQ·cos(seg_mid)), so reach >= |centre−corner| in the per-pixel cull's OWN
// metric at all latitudes ⇒ the GPU block-bin is a conservative superset everywhere
// (no floor, no per-source recompute). +1 m guards fp ULP. Provenance: path_dist_m
// geometry with cos→1 (docs/dev/gpu-binning-plan.md). dlat in meters, dlon in degrees.
__device__ __forceinline__ double block_reach_ub(double dlat_half_m, double dlon_half_deg) {
    double dlon_m = dlon_half_deg * M_LON_EQ;
    return sqrt(dlat_half_m * dlat_half_m + dlon_m * dlon_m) + 1.0;
}

// ---- geo::finite_line_correction (fp32: ratios + atan, scale-free; logf already f32) ----
__device__ __forceinline__ float flc(float len, float dperp, float frac) {
    if (len < 0.1f || dperp < 0.1f) return 0.0f;
    float d1 = frac * len, d2 = (1.0f - frac) * len, invd = 1.0f / dperp;
    float a1 = d1 * invd, a2 = d2 * invd, prod = a1 * a2;
    float theta = (prod < 0.98f) ? atanf((a1 + a2) / (1.0f - prod)) : (atanf(a1) + atanf(a2));
    float corr = 4.342944819032518f * logf(theta / (float)PI_D);
    return fminf(corr, 0.0f);
}

// ---- geo::finite_line_correction_for_divergence (fix-pack C) — the form line
// callers must use. The kernel's line chain is −10·log10(2π·d_div) + FLC, while
// a finite straight line's exact free-field energy is ∝ θ/d_perp, so the pairing
// only closes with the d_div/d_perp ratio term. `frac` is the UNCLAMPED signed
// foot fraction: past an endpoint the subtended angle is the DIFFERENCE of the
// two end angles, not their sum, and the clamped form read such a segment
// +1.9 dB loud. Collapses to plain `flc` when the foot lies ON the segment.
__device__ __forceinline__ float flc_div(float len, float dperp_in, float frac, float ddiv) {
    if (len < 0.1f) return 0.0f;
    float dperp = fmaxf(dperp_in, (float)FLC_MIN_PERP_M);
    return flc(len, dperp, frac)
         + 4.342944819032518f * logf(fmaxf(ddiv, dperp) / dperp);
}

// ---- arc_screening helpers: the receiver-centred flat-earth frame (x east,
// y north, metres) and the angle folding `wrap_pi` runs in.
__device__ __forceinline__ double wrap_pi_d(double a) {
    double r = fmod(a, TAU_D);
    if (r > PI_D) return r - TAU_D;
    if (r <= -PI_D) return r + TAU_D;
    return r;
}

// -DARC_AZ_F32=0 restores the f64 atan2. The measured case for fp32: 91 % of the
// footprints whose hull is built are CONFIRMED blockers (clip 0.94, confirm 0.91
// on the dense rail tile), so none of this atan2 volume — ~2.6e9 calls — can be
// pruned away; it can only be made cheaper. The displacements are formed in f64
// and only the ANGLE is fp32, whose ~4e-7 rad error sits 4e-5 below the 0.01 rad
// minimum span the gate now enforces, i.e. four orders under the interval
// fractions it feeds. Bounded and gated by compare_hm3 like everything else.
// MEASURED AND REJECTED for now: worth 1.36×, but 11 cells >0.5 dB (max 1.0 dB)
// against the exact kernel on the dense rail tile — the bar is zero such cells.
// The error is not the fp32 angle itself (~4e-7 rad) but its effect at CLIP
// BOUNDARIES, where an interval flips on or off and the Maekawa cliff turns
// that into whole dB. Default OFF.
#ifndef ARC_AZ_F32
#define ARC_AZ_F32 0
#endif
__device__ __forceinline__ double arc_az(double flat, double flon, double fmlon,
                                         double lat, double lon) {
#if ARC_AZ_F32
    return (double)atan2f((float)((lat - flat) * M_LAT), (float)((lon - flon) * fmlon));
#else
    return atan2((lat - flat) * M_LAT, (lon - flon) * fmlon);
#endif
}

// arc_screening::source_point_at — the point ON the segment the receiver sees at
// `az`, by exact ray×line intersection (the interpolated endpoints at the solved
// fraction). False when the ray runs parallel to the segment.
__device__ __forceinline__ bool arc_source_point(
    double rlat, double rlon, double mlon,
    double alat, double alon, double blat, double blon, double az,
    double* olat, double* olon)
{
    double ax = (alon - rlon) * mlon, ay = (alat - rlat) * M_LAT;
    double bx = (blon - rlon) * mlon, by = (blat - rlat) * M_LAT;
    double dx = cos(az), dy = sin(az);
    double ex = bx - ax, ey = by - ay;
    double denom = dx * ey - dy * ex;
    if (fabs(denom) < 1e-12) return false;
    double u = fmin(fmax((dy * ax - dx * ay) / denom, 0.0), 1.0);
    *olat = alat + u * (blat - alat);
    *olon = alon + u * (blon - alon);
    return true;
}

// ---- raster_reader::lookup_fused_rc (elevation bilinear from row/col coords) ----
// The profile walk lerps (rf,cf) directly (build_path_profile), so this is the
// rc-form of bilinear_elev_d; returns the f32 the CPU profile stores.
__device__ __forceinline__ float bilinear_elev_rc(
    const float* elev, int rows, int cols, float rf, float cf)
{
    rf = fminf(fmaxf(rf, 0.0f), (float)(rows - 1));
    cf = fminf(fmaxf(cf, 0.0f), (float)(cols - 1));
    int r0 = min((int)floorf(rf), rows - 2);
    int c0 = min((int)floorf(cf), cols - 2);
    float fr = rf - (float)r0, fc = cf - (float)c0;
    long base = (long)r0 * cols + c0;
    float v0 = elev[base]        + fc * (elev[base + 1]        - elev[base]);
    float v1 = elev[base + cols] + fc * (elev[base + cols + 1] - elev[base + cols]);
    return v0 + fr * (v1 - v0);
}

// ---- FusedTileZ13::elevation — the production SOURCE-ground lookup. Inside the
// tile bbox: nearest inner-grid cell (latlon_to_inner_idx; DEM bilinear pre-baked
// at the TPX×TPX pixel centres). Outside: halo bilinear. scatter_band reads the
// source ground this way (NOT halo.lookup_fused), so the kernel must too.
//   bb = [north_lat, south_lat, west_lon, east_lon]
__device__ __forceinline__ double tile_elev(
    const float* inner, const float* elev, int rows, int cols,
    double lat_min, double lon_min, double inv, const double* bb,
    double lat, double lon)
{
    if (lat >= bb[1] && lat <= bb[0] && lon >= bb[2] && lon <= bb[3]) {
        double latf = (bb[0] - lat) / (bb[0] - bb[1]);
        double lonf = (lon - bb[2]) / (bb[3] - bb[2]);
        int py = (int)fmin(fmax(floor(latf * (double)TPX), 0.0), (double)(TPX - 1));
        int px = (int)fmin(fmax(floor(lonf * (double)TPX), 0.0), (double)(TPX - 1));
        return inner[py * TPX + px];
    }
    return bilinear_elev_d(elev, rows, cols, lat_min, lon_min, inv, lat, lon);
}

// ---- path_profile::fill_t_values — bilateral adaptive cadence (depends ONLY on
// dist). Fills t[0..n), returns n. Byte-port of the CPU; dedup consecutive <1e-9.
__device__ int fill_t(double dist, double* t) {
    int m = 0;
    bool near = dist >= 3.0 * NEAR_OFFSET_M;
    double near_t = NEAR_OFFSET_M / dist;
    if (dist <= CELL_M * 10.0) {
        int n = (int)ceil(dist / CELL_M); if (n < 3) n = 3;
        t[m++] = 0.0;
        if (near) t[m++] = near_t;
        for (int i = 1; i < n - 1; i++) {
            double tt = (double)i / (double)(n - 1);
            if (near && (fabs(tt - near_t) * dist < 3.0 ||
                         fabs((1.0 - tt) - near_t) * dist < 3.0)) continue;
            t[m++] = tt;
        }
        if (near) t[m++] = 1.0 - near_t;
        t[m++] = 1.0;
    } else {
        // SURFACE coarse-middle (matches CPU fill_t_values_coarse_mid): the dense
        // 10/30/60/120 m ramp is TRUNCATED at the near-end zone and the far field
        // is coarse-stepped at mstride×240 m. stride 1 ⇒ ∞ zone = exact cadence.
        // Fix-pack Fix 5: the NEAR FIELD always runs the exact popup cadence —
        // the CPU's `scatter_band::cadence_for_ray` gate, mirrored here (the two
        // builders bridge their middle from different anchors, so a ~320 m ray
        // otherwise picks up a sample the popup has not got).
        double src_zone_m = 1e30, rx_zone_m = 1e30; int mstride = 1;
        if (SHADOW_MID_STRIDE > 1 && dist > EXACT_CADENCE_MAX_DIST_M) {
            src_zone_m = SHADOW_SRC_ZONE_M; rx_zone_m = SHADOW_RX_ZONE_M; mstride = SHADOW_MID_STRIDE;
        }
        t[m++] = 0.0;
        if (near) t[m++] = near_t;
        double levels[4] = {CELL_M, CELL_M * 2.0, CELL_M * 4.0, CELL_M * 8.0};
        double pos = near ? NEAR_OFFSET_M : 0.0;
        double last_fwd = pos;  // last COMMITTED ramp sample (bridge origin)
        for (int L = 0; L < 4; L++) {
            int brk = 0;
            for (int r = 0; r < 3; r++) {
                pos += levels[L];
                if (pos >= dist * 0.5 || pos > src_zone_m) { brk = 1; break; }
                t[m++] = pos / dist; last_fwd = pos;
            }
            if (brk) break;
        }
        // Exact (mstride==1): clamp to midpoint; coarse: bridge from last pushed
        // sample (pos over-steps by one increment on the break → a >coarse hole).
        double fwd_end = (mstride > 1) ? (last_fwd / dist) : (fmin(pos, dist * 0.5) / dist);
        double coarse = fmin(levels[3] * (double)mstride, dist * 0.25);
        // Backward ramp start (where the far-field fill stops): mirror of forward.
        double bpos = near ? NEAR_OFFSET_M : 0.0;
        for (int L = 0; L < 4; L++) {
            int brk = 0;
            for (int r = 0; r < 3; r++) {
                double next = bpos + levels[L];
                if (next >= dist * 0.5 || next > rx_zone_m) { brk = 1; break; }
                bpos = next;
            }
            if (brk) break;
        }
        double bwd_start = fmax(1.0 - bpos / dist, 0.5);
        // The only dist-unbounded loop: reserve room for the ≤14 remaining
        // backward-ramp + endpoint pushes so a pathological dist can't overflow
        // t[MAXT]. Never triggers for the ≤10 km reach the cadence sees.
        double mid = fwd_end;
        while (mid < bwd_start - 0.0001 && m < MAXT - 16) {
            mid += coarse / dist;
            if (mid < bwd_start - 1e-9) t[m++] = mid;
        }
        int bstart = m, bcount = 0;
        pos = near ? NEAR_OFFSET_M : 0.0;
        for (int L = 0; L < 4; L++) {
            int brk = 0;
            for (int r = 0; r < 3; r++) {
                pos += levels[L];
                if (pos >= dist * 0.5 || pos > rx_zone_m) { brk = 1; break; }
                t[m++] = 1.0 - pos / dist; bcount++;
            }
            if (brk) break;
        }
        for (int i = 0; i < bcount / 2; i++) {
            double tmp = t[bstart + i];
            t[bstart + i] = t[bstart + bcount - 1 - i];
            t[bstart + bcount - 1 - i] = tmp;
        }
        if (near) t[m++] = 1.0 - near_t;
        t[m++] = 1.0;
    }
    int w = 0;
    for (int i = 0; i < m; i++) if (w == 0 || fabs(t[i] - t[w - 1]) >= 1e-9) t[w++] = t[i];
    return w;
}

// ---- horizon::max_delta_idx — edge of largest path-length difference δ over
// 1..n-1 among samples above the source→receiver LOS; -1 if path clear.
// Stable-δ in fp32: δ = d_sb+d_br−dsr rewritten as Σ dz²/(d+x) so the near-equal
// large distances never cancel (a naive fp32 cast drifted ~1500 cells). dsg+drg
// = dist EXACTLY via drg = dist−dsg; the LoS gate stays f64 (exact candidate
// selection); height diffs widen exactly from the f32 profile.
__device__ int mdidx(const double* t, const double* prof, int n,
                      double dist, double se, double re, double dsr) {
    int best = -1; float bestd = 0.0f;
    float distf = (float)dist;
    float dzsr = (float)(re - se);
    float third = dzsr * dzsr / (float)(dsr + dist);
    for (int i = 1; i < n - 1; i++) {
        double topd = prof[i];
        if (topd <= se + (re - se) * t[i]) continue;
        float ti = (float)t[i];
        float dsg = ti * distf, drg = distf - dsg;
        float dzsb = (float)(topd - se), dzbr = (float)(topd - re);
        float dsb = sqrtf(dsg * dsg + dzsb * dzsb);
        float dbr = sqrtf(drg * drg + dzbr * dzbr);
        float delta = dzsb * dzsb / (dsb + dsg) + dzbr * dzbr / (dbr + drg) - third;
        if (delta > bestd) { bestd = delta; best = i; }
    }
    return best;
}

// ---- diffraction::fit_plane — OLS line over samples [lo,hi] (x = (t−t_off)·dist).
__device__ void fit_plane(const double* t, const double* prof, int lo, int hi,
                          double t_off, double dist, double* a, double* b) {
    double nn = (double)(hi - lo + 1);
    double sx = 0, sz = 0, sxx = 0, sxz = 0;
    for (int i = lo; i <= hi; i++) {
        double x = (t[i] - t_off) * dist, z = prof[i];
        sx += x; sz += z; sxx += x * x; sxz += x * z;
    }
    double denom = nn * sxx - sx * sx;
    if (fabs(denom) < 1e-9) { *a = 0.0; *b = sz / nn; return; }
    *a = (nn * sxz - sx * sz) / denom;
    *b = (sz - (*a) * sx) / nn;
}

// ---- diffraction::fit_plane_with_point — fit_plane + the diffraction point D
// folded into the regression (the explicit-edge δ* fits, SPEC §3.5b: D
// joins BOTH mean planes exactly once).
__device__ void fit_plane_pt(const double* t, const double* prof, int lo, int hi,
                             double extra_t, double extra_z,
                             double t_off, double dist, double* a, double* b) {
    double sx = 0, sz = 0, sxx = 0, sxz = 0;
    double nn = 0.0;
    for (int i = lo; i < hi; i++) {
        double x = (t[i] - t_off) * dist, z = prof[i];
        sx += x; sz += z; sxx += x * x; sxz += x * z; nn += 1.0;
    }
    { // the D point
        double x = (extra_t - t_off) * dist;
        sx += x; sz += extra_z; sxx += x * x; sxz += x * extra_z; nn += 1.0;
    }
    double denom = nn * sxx - sx * sx;
    if (fabs(denom) < 1e-9) { *a = 0.0; *b = sz / nn; return; }
    *a = (nn * sxz - sx * sz) / denom;
    *b = (sz - (*a) * sx) / nn;
}

// ---- diffraction::compute_single_edge_at δ* part — explicit edge (t_e over
// LERPed bare ground), strict partitions (src side t < t_e, rcv side t > t_e),
// D in both fits. Mirrors the CPU in f64 (mod nvcc FMA contraction; the
// e2-full vector run gates the result).
__device__ double dstar_at(const double* t, const double* bare, int n, double t_e,
                           double dist, double src_h, double rcv_h) {
    double d_sg = t_e * dist, d_rg = (1.0 - t_e) * dist;
    // partition points: p_lo = first i with t[i] >= t_e; p_hi = first i with t[i] > t_e
    int p_lo = 0; while (p_lo < n && t[p_lo] < t_e) p_lo++;
    int p_hi = 0; while (p_hi < n && t[p_hi] <= t_e) p_hi++;
    // bare ground under the edge, LERPed between neighbours (i1 = clamp(p_hi,1,n-1))
    int i1 = p_hi < 1 ? 1 : (p_hi > n - 1 ? n - 1 : p_hi);
    double t0 = t[i1 - 1], t1 = t[i1];
    double frac = (t1 > t0) ? fmin(fmax((t_e - t0) / (t1 - t0), 0.0), 1.0) : 0.0;
    double d_top = bare[i1 - 1] + frac * (bare[i1] - bare[i1 - 1]);
    double a_s, b_s; fit_plane_pt(t, bare, 0, p_lo, t_e, d_top, 0.0, dist, &a_s, &b_s);
    double a_r, b_r; fit_plane_pt(t, bare, p_hi, n, t_e, d_top, t_e, dist, &a_r, &b_r);
    double plane_rcv_end = a_r * d_rg + b_r;
    double s_star = 2.0 * b_s - ((double)bare[0] + src_h);
    double r_star = 2.0 * plane_rcv_end - ((double)bare[n - 1] + rcv_h);
    double d_sd = sqrt(d_sg * d_sg + (d_top - s_star) * (d_top - s_star));
    double d_dr = sqrt(d_rg * d_rg + (r_star - d_top) * (r_star - d_top));
    double d_sr = sqrt(dist * dist + (r_star - s_star) * (r_star - s_star));
    double v = d_sd + d_dr - d_sr;
    return v > 0.0 ? v : 0.0;
}

// ---- obstacle_index::segment_intersection_t — chainage of ray×edge, t strictly
// inside (0,1), hit within the segment (u ∈ [0,1] inclusive), collinear → none.
__device__ __forceinline__ bool seg_isect_t(
    double sx, double sy, double dx, double dy,
    double x0, double y0, double x1, double y1, double* t_out)
{
    double ex = x1 - x0, ey = y1 - y0;
    double denom = dx * ey - dy * ex;
    if (denom == 0.0) return false;
    double wx = x0 - sx, wy = y0 - sy;
    double tt = (wx * ey - wy * ex) / denom;
    double u = (wx * dy - wy * dx) / denom;
    if (tt > 0.0 && tt < 1.0 && u >= 0.0 && u <= 1.0) { *t_out = tt; return true; }
    return false;
}

// ---- ObstacleSet::crossings + path_effects §5b fused for the GPU: walk every
// per-cell index's grid along the ray (the same Amanatides & Woo shape and
// clamps as the CPU walk), and for each exact crossing evaluate the candidate
// δ ON THE FLY (terrain LERPed between the ray's bare samples, LOS-gated),
// keeping only the max-δ winner — no sort, no dedup (duplicate hits evaluate
// identically, max() is idempotent), no dynamic memory. `obst` is the 14-slot
// pointer table built by `upload_obstacles` (noise-gpu/src/lib.rs) —
//   0 n_indexes  1 metas  2 starts  3 refs  4 edges  5 cell_max_h  6 edge_ids
//   7 foot_edge_starts  8 foot_box  9 cell_foot_starts  10 cell_foot_ids
//   11 cell_foot_prev  12 foot_edge_refs  13 cell_foot_cell
// — of which this walk reads 0..=5 (slot 5 == 0 ⇒ branch-and-bound pruning
// disabled). Slots 6..=13 belong to the arc-screening walks below.
// Exclusion radius: the GPU lanes are line layers (roads/rail), which pass 0
// on the CPU too — no gate here by construction.
__device__ void obstacle_best_candidate(
    const unsigned long long* obst,
    double src_lat, double src_lon, double rcv_lat, double rcv_lon,
    const double* t, const double* bare, int n,
    double dist, double se, double re, double dsr,
    int* have, double* cand_t, double* cand_top)
{
    *have = 0;
    // Signed δ: a below-LOS near miss stays in the race with a NEGATIVE δ
    // (mirror of path_effects §5b after the fix-pack penumbra landed), so the
    // running best must start below every possible value, not at 0.
    double best_delta = -1e300;
    int n_idx = (int)obst[0];
    const double* metas = (const double*)obst[1];
    const unsigned int* starts = (const unsigned int*)obst[2];
    const unsigned int* refs = (const unsigned int*)obst[3];
    const float* edges = (const float*)obst[4];
    const float* maxh = (const float*)obst[5];
    // Branch-and-bound prune (z13 plan E2): the best candidate a cell can
    // produce is bounded by the max bare elevation over THAT CELL's chainage
    // window plus the cell's max edge height. `detour(t, top)` is CONVEX in t for
    // a fixed top, so the bound is exact — but only if BOTH branches of the
    // signed δ are maximised: +detour (top above the sight line) peaks at a
    // window ENDPOINT, while −detour (below it, the penumbra branch) is concave
    // and peaks at the interior REFLECTION point t* = |h_s|/(|h_s|+|h_r|). This
    // kernel evaluates both (see `tstar` below); the Rust lane evaluated the
    // sight-line crossing instead and therefore UNDERBOUND the below-LOS case
    // until 2026-08-08. The ground term is
    // computed per cell inside the DDA below (`terr_win`); a GLOBAL profile max
    // is never taken, because in relief it made the bound useless (see there).
    for (int gi = 0; gi < n_idx; gi++) {
        const double* m = &metas[gi * OBST_META_STRIDE];
        double mlon = m[2], cell = m[3], minx = m[4], miny = m[5];
        int cols = (int)m[6], rows = (int)m[7];
        size_t soff = (size_t)m[8], roff = (size_t)m[9], eoff = (size_t)m[10];
        size_t hoff = (size_t)m[12];
        double sx = (src_lon - m[1]) * mlon, sy = (src_lat - m[0]) * M_LAT;
        double rx = (rcv_lon - m[1]) * mlon, ry = (rcv_lat - m[0]) * M_LAT;
        double dx = rx - sx, dy = ry - sy;
        // Slab reject (GPU-only perf; results identical — edges only exist
        // inside the grid): a ray whose bbox misses this index's extent
        // cannot cross any of its edges. A typical region holds 7 per-cell
        // indexes and a ray touches 1–3.
        double gx1 = minx + (double)cols * cell, gy1 = miny + (double)rows * cell;
        if (fmax(sx, rx) < minx || fmin(sx, rx) > gx1 ||
            fmax(sy, ry) < miny || fmin(sy, ry) > gy1) continue;
        double inv_cell = 1.0 / cell;
        long cx = (long)floor((sx - minx) * inv_cell); cx = cx < 0 ? 0 : (cx > cols - 1 ? cols - 1 : cx);
        long cy = (long)floor((sy - miny) * inv_cell); cy = cy < 0 ? 0 : (cy > rows - 1 ? rows - 1 : cy);
        long end_cx = (long)floor((rx - minx) * inv_cell); end_cx = end_cx < 0 ? 0 : (end_cx > cols - 1 ? cols - 1 : end_cx);
        long end_cy = (long)floor((ry - miny) * inv_cell); end_cy = end_cy < 0 ? 0 : (end_cy > rows - 1 ? rows - 1 : end_cy);
        long step_x = dx >= 0.0 ? 1 : -1, step_y = dy >= 0.0 ? 1 : -1;
        double t_delta_x = dx != 0.0 ? fabs(cell / dx) : 1e300;
        double t_delta_y = dy != 0.0 ? fabs(cell / dy) : 1e300;
        double next_xb = minx + (double)(cx + (dx >= 0.0 ? 1 : 0)) * cell;
        double next_yb = miny + (double)(cy + (dy >= 0.0 ? 1 : 0)) * cell;
        double t_max_x = dx != 0.0 ? fabs((next_xb - sx) / dx) : 1e300;
        double t_max_y = dy != 0.0 ? fabs((next_yb - sy) / dy) : 1e300;
        long guard = (long)cols + (long)rows + 4;
        double t_enter = 0.0;
        // Advancing pointer into the profile for the per-cell ground bound. The
        // DDA visits cells in increasing chainage, so this only ever moves
        // forward: the whole ray costs O(n), not O(n) per cell.
        int win_lo = 0;
        while (1) {
            size_t c = (size_t)cy * (size_t)cols + (size_t)cx;
            unsigned int lo = starts[soff + c], hi = starts[soff + c + 1];
            // Exact cell prune (maxh == NULL ⇒ pruning disabled — the host
            // writes slot 5 as 0 under NOISE_GPU_DISABLE_PRUNE=1, an
            // incident A/B lever that needs no rebuild).
            // top_bound = terr_win + cell max edge height
            // (+1e-9 m outward slack so no f64 rounding chain — e.g. the
            // terrain LERP exceeding terr_win by an ulp — can push a real
            // candidate above the bound). Validity: δ(t, top) as a function
            // of top is strictly convex with its MINIMUM exactly on the LOS
            // (both direction cosines cancel there), so every edge the exact
            // loop admits (top > los) has δ increasing in top ⇒ δ ≤ δ(top_bound).
            // In t, δ(·, top_bound) is convex ⇒ its max over the cell's
            // chainage interval sits at an endpoint. An edge shared with a
            // later cell whose crossing lies outside this interval is listed
            // (supercover) and re-found in the cell that owns the crossing,
            // so skipping here never loses it. Skips: LOS skip mirrors the
            // exact loop's `top <= los` discard; the best-candidate skip is
            // STRICT `<` — a bound that can still TIE must walk the edges,
            // because the winner rule prefers lower t among f64-equal δ.
            if (hi > lo && maxh) {
                double t_exit = fmin(fmin(t_max_x, t_max_y), 1.0);
                double t_a = fmin(fmax(t_enter, 0.0), 1.0);
                // GROUND TERM: the max bare elevation over THIS CELL's chainage
                // window, not over the whole profile. Exact for the same reason
                // the rest of the bound is: a candidate's terrain is LERPed
                // between the two profile samples bracketing its crossing
                // (path_effects §5b), so over a window it can never exceed the
                // max of the samples bracketing that window. Using the global
                // profile max instead made the bound useless in relief — the
                // Brdy hills put a 200 m hilltop into every cell's bound — and
                // MEASURED the whole prune at 2.2 % on the dense rail tile.
                while (win_lo + 1 < n && t[win_lo + 1] <= t_a) win_lo++;
                double terr_win = bare[win_lo];
                for (int i = win_lo + 1; i < n; i++) {
                    terr_win = fmax(terr_win, bare[i]);
                    if (t[i] >= t_exit) break;
                }
                double top_bound = terr_win + (double)maxh[hoff + c] + 1e-9;
                double los_min = fmin(se + (re - se) * t_a, se + (re - se) * t_exit);
                if (top_bound <= los_min) {
                    // ENTIRELY below the sight line. That is no longer a discard
                    // — such a cell can still hold a penumbra screener — so bound
                    // it instead. The best (least negative) signed δ it can offer
                    // is −min_t δ_geo(t, top_bound); δ_geo at fixed top is convex
                    // in t with its minimiser at the reflection point
                    // t* = |h_s|/(|h_s|+|h_r|), clamped to the cell's chainage
                    // interval. Exact, one divide and two sqrt.
                    double hs = top_bound - se, hr = top_bound - re;
                    double ahs = fabs(hs), ahr = fabs(hr);
                    double tstar = (ahs + ahr > 0.0) ? ahs / (ahs + ahr) : 0.5;
                    tstar = fmin(fmax(tstar, t_a), t_exit);
                    double dsg = tstar * dist, drg = dist - dsg;
                    double ub = -(sqrt(dsg * dsg + hs * hs)
                                + sqrt(drg * drg + hr * hr) - dsr);
                    // Skip only when the cell can neither reach the audibility
                    // floor nor beat the running best. The floor moved WITH the
                    // loop: it was δ = 0 while the loop discarded below-LOS
                    // candidates, and is now the δ*-free penumbra floor.
                    if (ub <= ARC_DELTA_REJECT || (*have && ub + 1e-9 < best_delta)) {
                        lo = hi;
                    }
                }
                else if (*have) {
                    double d_bound = 0.0;
                    for (int ep = 0; ep < 2; ep++) {
                        double tt = ep == 0 ? t_a : t_exit;
                        double d_sg = tt * dist, d_rg = (1.0 - tt) * dist;
                        double d = sqrt(d_sg * d_sg + (top_bound - se) * (top_bound - se))
                                 + sqrt(d_rg * d_rg + (top_bound - re) * (top_bound - re)) - dsr;
                        d_bound = fmax(d_bound, d);
                    }
                    // DELTA-space slack, not just the height bump above: the
                    // two-sqrt-minus-dsr evaluation cancels near grazing and
                    // its f64 result can land ~1e-12 m BELOW the exact loop's
                    // candidate δ (measured by direct IEEE evaluation — gg
                    // z13 impl review, Codex #4); dδ/dtop ≈ 0 there, so the
                    // 1e-9 m height slack adds ~nothing. 1e-9 m of δ dwarfs
                    // the whole rounding chain (eps·10^5 m ≈ 2e-11) and is
                    // physically nil.
                    if (d_bound + 1e-9 < best_delta) { lo = hi; }
                }
            }
            for (unsigned int k = lo; k < hi; k++) {
                const float* e = &edges[(eoff + (size_t)refs[roff + k]) * 5];
                double tt;
                if (!seg_isect_t(sx, sy, dx, dy,
                                 (double)e[0], (double)e[1], (double)e[2], (double)e[3], &tt))
                    continue;
                // path_effects §5b: terrain LERP between neighbouring bare
                // samples (p = first sample with t > tt, clamped to [1, n-1]).
                int p = 1; while (p < n - 1 && t[p] <= tt) p++;
                double t0 = t[p - 1], t1 = t[p];
                double frac = (t1 > t0) ? (tt - t0) / (t1 - t0) : 0.0;
                double terr = bare[p - 1] + frac * (bare[p] - bare[p - 1]);
                double top = terr + (double)e[4];
                double los = se + (re - se) * tt;
                // NO below-LOS discard: a 3 m wall under a 4 m receiver's sight
                // line sits ~0.44 m below it and still screens ~0.6 dB through
                // the §2.5.6(c) penumbra. Rank on the SIGNED δ so a real blocker
                // still beats every near miss (path_effects §5b).
                double sign = (top >= los) ? 1.0 : -1.0;
                double d_sg = tt * dist, d_rg = (1.0 - tt) * dist;
                double delta = sign
                             * (sqrt(d_sg * d_sg + (top - se) * (top - se))
                              + sqrt(d_rg * d_rg + (top - re) * (top - re)) - dsr);
                // Strict max + lower-t tie-break: the CPU walks candidates
                // t-sorted and keeps the FIRST max (path_effects §5b), i.e.
                // the lowest-t among f64-equal δ; DDA discovery order is not
                // t-sorted across edges, so break ties explicitly. Bounded
                // deviation vs CPU (documented, gg review 2026-07-28 #4): a
                // vertex double-hit (two edges of one ring at the same point)
                // is collapsed to one candidate by the CPU's (id,t) dedup but
                // evaluated twice here — the two δ agree to the last ulp, so
                // only an ulp-level tie can pick the other edge of the SAME
                // geometry; the bands are identical to fp32.
                if (!*have || delta > best_delta ||
                    (delta == best_delta && tt < *cand_t)) {
                    *have = 1; best_delta = delta; *cand_t = tt; *cand_top = top;
                }
            }
            if ((cx == end_cx && cy == end_cy) || guard <= 0) break;
            guard--;
            t_enter = fmin(t_max_x, t_max_y);
            if (t_max_x < t_max_y) { t_max_x += t_delta_x; cx += step_x; }
            else                   { t_max_y += t_delta_y; cy += step_y; }
            if (cx < 0 || cy < 0 || cx >= cols || cy >= rows) break;
        }
    }
}

// ---- arc_screening step 2's confirmation: does the ray src→rcv actually CROSS
// the footprint `id`? Mirrors `ObstacleSet::crossings(...).any(|c| c.id == id)`
// — matched on the id ALONE across every index, exactly as the CPU does (ids are
// dense per index, so a cross-index collision can confirm an interval on the
// wrong obstacle; it then evaluates to that ray's true screening, the honest
// answer either way). No LOS or height gate: `crossings` is pure 2D geometry.
// The walk is `obstacle_best_candidate`'s DDA with the δ machinery removed.
__device__ bool arc_ray_hits_id(
    const unsigned long long* obst,
    double src_lat, double src_lon, double rcv_lat, double rcv_lon, unsigned int id)
{
    int n_idx = (int)obst[0];
    const double* metas = (const double*)obst[1];
    const unsigned int* starts = (const unsigned int*)obst[2];
    const unsigned int* refs = (const unsigned int*)obst[3];
    const float* edges = (const float*)obst[4];
    const unsigned int* eids = (const unsigned int*)obst[6];
    for (int gi = 0; gi < n_idx; gi++) {
        const double* m = &metas[gi * OBST_META_STRIDE];
        double mlon = m[2], cell = m[3], minx = m[4], miny = m[5];
        int cols = (int)m[6], rows = (int)m[7];
        size_t soff = (size_t)m[8], roff = (size_t)m[9], eoff = (size_t)m[10];
        double sx = (src_lon - m[1]) * mlon, sy = (src_lat - m[0]) * M_LAT;
        double rx = (rcv_lon - m[1]) * mlon, ry = (rcv_lat - m[0]) * M_LAT;
        double dx = rx - sx, dy = ry - sy;
        double gx1 = minx + (double)cols * cell, gy1 = miny + (double)rows * cell;
        if (fmax(sx, rx) < minx || fmin(sx, rx) > gx1 ||
            fmax(sy, ry) < miny || fmin(sy, ry) > gy1) continue;
        double inv_cell = 1.0 / cell;
        long cx = (long)floor((sx - minx) * inv_cell); cx = cx < 0 ? 0 : (cx > cols - 1 ? cols - 1 : cx);
        long cy = (long)floor((sy - miny) * inv_cell); cy = cy < 0 ? 0 : (cy > rows - 1 ? rows - 1 : cy);
        long end_cx = (long)floor((rx - minx) * inv_cell); end_cx = end_cx < 0 ? 0 : (end_cx > cols - 1 ? cols - 1 : end_cx);
        long end_cy = (long)floor((ry - miny) * inv_cell); end_cy = end_cy < 0 ? 0 : (end_cy > rows - 1 ? rows - 1 : end_cy);
        long step_x = dx >= 0.0 ? 1 : -1, step_y = dy >= 0.0 ? 1 : -1;
        double t_delta_x = dx != 0.0 ? fabs(cell / dx) : 1e300;
        double t_delta_y = dy != 0.0 ? fabs(cell / dy) : 1e300;
        double next_xb = minx + (double)(cx + (dx >= 0.0 ? 1 : 0)) * cell;
        double next_yb = miny + (double)(cy + (dy >= 0.0 ? 1 : 0)) * cell;
        double t_max_x = dx != 0.0 ? fabs((next_xb - sx) / dx) : 1e300;
        double t_max_y = dy != 0.0 ? fabs((next_yb - sy) / dy) : 1e300;
        long guard = (long)cols + (long)rows + 4;
        while (1) {
            size_t c = (size_t)cy * (size_t)cols + (size_t)cx;
            for (unsigned int k = starts[soff + c]; k < starts[soff + c + 1]; k++) {
                unsigned int eref = refs[roff + k];
                if (eids[eoff + eref] != id) continue;
                const float* e = &edges[(eoff + eref) * 5];
                double tt;
                if (seg_isect_t(sx, sy, dx, dy,
                                (double)e[0], (double)e[1], (double)e[2], (double)e[3], &tt))
                    return true;
            }
            if ((cx == end_cx && cy == end_cy) || guard <= 0) break;
            guard--;
            if (t_max_x < t_max_y) { t_max_x += t_delta_x; cx += step_x; }
            else                   { t_max_y += t_delta_y; cy += step_y; }
            if (cx < 0 || cy < 0 || cx >= cols || cy >= rows) break;
        }
    }
    return false;
}

// ---- diffraction::compute_delta_star — CNOSSOS §2.5.6(c) Rayleigh δ* (mirror
// source/receiver across per-side mean-ground planes; OLS on BARE earth).
__device__ double dstar(const double* t, const double* prof, int n, int d_idx,
                        double dist, double src_h, double rcv_h) {
    double dsg = t[d_idx] * dist, drg = (1.0 - t[d_idx]) * dist;
    double a_s, b_s; fit_plane(t, prof, 0, d_idx, 0.0, dist, &a_s, &b_s);
    double a_r, b_r; fit_plane(t, prof, d_idx, n - 1, t[d_idx], dist, &a_r, &b_r);
    double plane_rcv_end = a_r * drg + b_r;
    double s_star = 2.0 * b_s - ((double)prof[0] + src_h);
    double r_star = 2.0 * plane_rcv_end - ((double)prof[n - 1] + rcv_h);
    double d_top = prof[d_idx];
    double d_sd = sqrt(dsg * dsg + (d_top - s_star) * (d_top - s_star));
    double d_dr = sqrt(drg * drg + (r_star - d_top) * (r_star - d_top));
    double d_sr = sqrt(dist * dist + (r_star - s_star) * (r_star - s_star));
    double v = d_sd + d_dr - d_sr;
    return v > 0.0 ? v : 0.0;
}

// ---- diffraction::maekawa_bands (single edge, cap 20). fp32.
// -DPENUMBRA=0 restores the pre-fix-pack `delta <= 0 => 0` form, for bisecting.
#ifndef PENUMBRA
#define PENUMBRA 1
#endif
// ---- diffraction::rayleigh_admits — CNOSSOS-EU 2021/1226 point (9)(c) per
// band, as a bit per band. Two scopes, both mirroring the CPU exactly:
//
//  * δ ≥ 0 is ALWAYS admitted. The amendment's sentence opens "If the direct
//    ray is not blocked", ISO/TR 17534-4 §5.9 spells it out ("If the line of
//    sight is blocked, diffraction is always calculated") and NoiseModelling's
//    `AttenuationCnossos.isValidRcrit` is the same predicate character for
//    character: `pp.delta >= 0 || (…)`. Gating a blocked ray on λ/4 − δ* was
//    this kernel's own addition and stepped 7.40–9.03 dB in one band.
//  * ONE verdict per path, taken on the HOMOGENEOUS δ and spent on both
//    meteorological states — see diffraction.rs for why (our δ* is
//    straight-geometry, so a curved δ_F must not be tested against it).
__device__ __forceinline__ unsigned int rayleigh_admits(float delta, float dstar_v) {
    unsigned int m = 0u;
    for (int i = 0; i < NB; i++) {
        float lambda = (float)(SOS / BAND_FREQ[i]);
        if (delta >= 0.0f || delta > lambda * 0.25f - dstar_v) m |= (1u << i);
    }
    return m;
}

__device__ void maek_single(float delta, unsigned int admits, float* bands) {
    for (int i = 0; i < NB; i++) bands[i] = 0.0f;
    if (!PENUMBRA && delta <= 0.0f) return;
    for (int i = 0; i < NB; i++) {
        if (!((admits >> i) & 1u)) continue;
        float lambda = (float)(SOS / BAND_FREQ[i]);
        // CNOSSOS-EU §2.5.6(c) penumbra arm, mirroring diffraction::maekawa_bands
        // exactly: a NEGATIVE path difference takes the steeper 40/λ slope and
        // reaches 0 dB at δ = −λ/20. Fix-pack Fix 2 listed this block as a site
        // and the CPU got it, but the mirror never did — and it is not a rare
        // branch: `maek_mixed` feeds this routine the FAVOURABLE-ray δ_F, which
        // is shorter than δ by construction and goes negative on every grazing
        // path, so the GPU was returning 0 dB for the favourable arm wherever
        // the CPU returned up to 10·log10(3) ≈ 4.8 dB. With P_FAV = 0.5 that is
        // a systematic several-dB divergence on ordinary geometry — the
        // GPU↔CPU gap measured with arc screening OFF on both lanes.
        float n = (delta < 0.0f) ? (3.0f + 40.0f * delta / lambda)
                                 : (3.0f + 20.0f * delta * (float)BAND_FREQ[i] / (float)SOS);
        if (n <= 1.0f) continue;   // δ < −λ/20: outside the penumbra
        bands[i] = fminf(10.0f * log10f(n), (float)SINGLE_DIFF_CAP);
    }
}

// ---- diffraction::curved_path_difference{,_near_miss} + mix_fav_hom shared
// tail: hom bands from (delta, δ*), favourable bands from the curved-ray δ_F
// over the SAME chords, then the P_FAV energy mix
// ((2.5.24)/(2.5.25)/(2.5.26)/(2.5.27)/(2.5.9)). Chords + arc difference in
// f64: the near-equal-kilometres cancellation loses ~4 digits — fatal in fp32
// (the mdidx lesson), comfortable in f64. Mix itself is dB-scale fp32.
//
// `dsa_d`/`dar_d` are the two halves of the STRAIGHT SR cut by the vertical
// through the edge (A of (2.5.27)); the caller has the station and the
// endpoints, this routine only has chords, so they come in as arguments.
__device__ void maek_mixed(float delta, float dstar_v,
                           double dsb_d, double dbr_d, double dsr,
                           double dsa_d, double dar_d, float* out) {
    // One Rayleigh verdict on the homogeneous δ, spent on both arms — mirrors
    // `diffraction::diffraction_attenuation_mixed`.
    unsigned int admits = rayleigh_admits(delta, dstar_v);
    maek_single(delta, admits, out);
    if (FAVOURABLE_MIXING) {
        double gamma = fmax(FAV_GAMMA_MIN, FAV_GAMMA_PER_DSR * dsr);
        // Both branches of the standard's own construction, chosen on the
        // STRAIGHT ray exactly as diffraction.rs chooses on `sign`:
        //   (2.5.26)  direct ray broken     δ_F =  ŜO + ÔR − ŜR
        //   (2.5.27)  direct ray clear      δ_F = 2ŜA + 2ÂR − ŜO − ÔR − ŜR
        // A is where the straight SR crosses the vertical through the edge, so
        // ŜA/ÂR are the arcs over `dsa_d`/`dar_d`. The favourable ray is
        // concave TOWARD THE GROUND and therefore stands ≈d²/8Γ ABOVE the
        // chord (~3 m over 200 m at Γ = 1600) — it does not sag — so a screen
        // under the sight line is screened LESS, never more, in this state.
        //
        // The near-miss arm used to be `(double)delta`, i.e. `−(d_SO + d_OR −
        // d_SR)`. That is not a different model, it is (2.5.27) evaluated on
        // STRAIGHT chords (A lies on SR, so d_SA + d_AR = d_SR exactly) — and
        // pairing it with a CURVED positive arm left the whole arc correction
        // as a cliff at the sight line: ~0.1 m of δ_F, 4.77 → 1.76 dB at 8 kHz,
        // a taller screen coming out louder. Both lanes now run the arc form.
        double delta_f = (delta < 0.0f)
            ? 2.0 * gamma * (2.0 * asin(dsa_d / (2.0 * gamma))
                + 2.0 * asin(dar_d / (2.0 * gamma)) - asin(dsb_d / (2.0 * gamma))
                - asin(dbr_d / (2.0 * gamma)) - asin(dsr / (2.0 * gamma)))
            : 2.0 * gamma * (asin(dsb_d / (2.0 * gamma))
                + asin(dbr_d / (2.0 * gamma)) - asin(dsr / (2.0 * gamma)));
        float fav[NB];
        maek_single((float)delta_f, admits, fav);
        for (int i = 0; i < NB; i++) {
            float e = (float)P_FAV * exp10f(-fav[i] * 0.1f)
                + (1.0f - (float)P_FAV) * exp10f(-out[i] * 0.1f);
            out[i] = -10.0f * log10f(e);
        }
    }
}

// ---- horizon::single_edge_atten + path_effects §5b/5c — the shared single-δ
// primitive with VECTOR-candidate competition. δ geometry + max-δ edge run on
// `top`; the §2.5.6(c) Rayleigh δ* OLS always on `bare`. Heights above bare
// earth (0.05 / 0.5 floors). A vector candidate (exact crossing at cand_t,
// absolute top cand_top — from obstacle_best_candidate) competes with the
// cadence sample edge ON δ (the actual selection criterion); the winner's
// bands are emitted. Candidate δ* uses the explicit-edge fits with D in both
// planes (dstar_at). Writes 8 bands. Terrain calls it with top==bare and no
// candidate; screening with top==composite, bare==elevation.
__device__ void single_edge_bands_cand(const double* t, const double* top, const double* bare,
                                       int n, double dist, double src_alt, double rcv_alt,
                                       int have_cand, double cand_t, double cand_top,
                                       float* out) {
    for (int i = 0; i < NB; i++) out[i] = 0.0f;
    double src_h = fmax(src_alt - bare[0], 0.05);
    double rcv_h = fmax(rcv_alt - bare[n - 1], 0.5);
    double se = bare[0] + src_h, re = bare[n - 1] + rcv_h;
    double dsr = sqrt(dist * dist + (re - se) * (re - se));
    int idx = mdidx(t, top, n, dist, se, re, dsr);
    bool sample_ok = idx >= 0 && top[idx] > se + (re - se) * t[idx];
    float delta_samp = 0.0f;
    double s_dsb_d = 0.0, s_dbr_d = 0.0, s_delta_d = 0.0, s_dsa_d = 0.0, s_dar_d = 0.0;
    if (sample_ok) {
        // stable-δ in fp32 (same reformulation as mdidx); δ* stays f64 (1× per edge).
        float distf = (float)dist, ti = (float)t[idx];
        float dsg = ti * distf, drg = distf - dsg;
        float dzsb = (float)(top[idx] - se), dzbr = (float)(top[idx] - re), dzsr = (float)(re - se);
        float dsb = sqrtf(dsg * dsg + dzsb * dzsb), dbr = sqrtf(drg * drg + dzbr * dzbr);
        delta_samp = dzsb * dzsb / (dsb + dsg) + dzbr * dzbr / (dbr + drg)
                   - dzsr * dzsr / (float)(dsr + dist);
        // f64 chords: reused by the favourable arc AND the winner compare —
        // the CPU compares two f64 deltas (path_effects §5c), so deciding on
        // the fp32 stable form would flip near-ties (gg review 2026-07-28 #3).
        double dsg_d = t[idx] * dist, drg_d = dist - dsg_d;
        double dzsb_d = top[idx] - se, dzbr_d = top[idx] - re;
        s_dsb_d = sqrt(dsg_d * dsg_d + dzsb_d * dzsb_d);
        s_dbr_d = sqrt(drg_d * drg_d + dzbr_d * dzbr_d);
        s_delta_d = s_dsb_d + s_dbr_d - dsr;
        // (2.5.27)'s A on the straight SR at this edge's station. `sample_ok`
        // already put the top ABOVE the sight line, so the near-miss arm is
        // unreachable here on paper — but `delta_samp` is the fp32 stable form
        // and a grazing edge can round it below zero, and a lane that then had
        // no A to work with would fall off exactly the cliff this fixes.
        double los_s = se + (re - se) * t[idx];
        s_dsa_d = sqrt(dsg_d * dsg_d + (los_s - se) * (los_s - se));
        s_dar_d = sqrt(drg_d * drg_d + (re - los_s) * (re - los_s));
    }
    if (have_cand) {
        // path_effects §5b/§5c: candidate vs cadence edge, by SIGNED δ in f64.
        // The candidate is NOT above-LOS by construction — an older comment here
        // claimed it was "LOS-gated in the DDA", but fix-pack Fix 2 (penumbra)
        // deliberately keeps below-LOS candidates in the race with a NEGATIVE δ,
        // and `obstacle_best_candidate`'s prune floor is negative
        // (ARC_DELTA_REJECT = −λ₆₃/20) precisely so they survive.
        //
        // `dsb + dbr − dsr` is POSITIVE by the triangle inequality no matter how
        // low the obstacle is — the sign is a CONVENTION carrying "does the top
        // clear the sight line", and dropping it silently promotes every near
        // miss to a real screen. Mirrors path_effects.rs:402
        //     let sign = if top >= los { 1.0 } else { -1.0 };
        // and it must also rank the race: a real blocker has to beat a near miss.
        double d_sg = cand_t * dist, d_rg = (1.0 - cand_t) * dist;
        double dsb_d = sqrt(d_sg * d_sg + (cand_top - se) * (cand_top - se));
        double dbr_d = sqrt(d_rg * d_rg + (cand_top - re) * (cand_top - re));
        double los_cand = se + (re - se) * cand_t;
        double sign_cand = (cand_top >= los_cand) ? 1.0 : -1.0;
        double delta_cand = sign_cand * (dsb_d + dbr_d - dsr);
        // A of (2.5.27): the straight SR at this candidate's station. Mirrors
        //     let d_sa = (d_sg*d_sg + (los - src_elev).powi(2)).sqrt();
        //     let d_ar = (d_rg*d_rg + (rcv_elev - los).powi(2)).sqrt();
        double dsa_d = sqrt(d_sg * d_sg + (los_cand - se) * (los_cand - se));
        double dar_d = sqrt(d_rg * d_rg + (re - los_cand) * (re - los_cand));
        if (!sample_ok || delta_cand > s_delta_d) {
            float dstar_v = (float)dstar_at(t, bare, n, cand_t, dist, src_h, rcv_h);
            maek_mixed((float)delta_cand, dstar_v, dsb_d, dbr_d, dsr, dsa_d, dar_d, out);
            return;
        }
    }
    if (!sample_ok) return;
    float dstar_v = (float)dstar(t, bare, n, idx, dist, src_h, rcv_h);
    maek_mixed(delta_samp, dstar_v, s_dsb_d, s_dbr_d, dsr, s_dsa_d, s_dar_d, out);
}

__device__ __forceinline__ void single_edge_bands(const double* t, const double* top,
                                                  const double* bare, int n, double dist,
                                                  double src_alt, double rcv_alt, float* out) {
    single_edge_bands_cand(t, top, bare, n, dist, src_alt, rcv_alt, 0, 0.0, 0.0, out);
}

// ---- path_effects::terrain_attenuation — bare-earth diffraction. Guard (short
// path) + the "any sample above LoS" hill scan, then the single-edge primitive.
__device__ void terrain_bands(const double* t, const double* prof, int n,
                              double dist, double src_alt, double rcv_alt, float* out) {
    for (int i = 0; i < NB; i++) out[i] = 0.0f;
    if (n < 3 || dist < 30.0) return;
    double dz_total = rcv_alt - src_alt;
    bool hill = false;
    for (int i = 0; i < n; i++) if (prof[i] > src_alt + dz_total * t[i]) { hill = true; break; }
    if (!hill) return;
    single_edge_bands(t, prof, prof, n, dist, src_alt, rcv_alt, out);
}

// ---- raster_reader::lookup_fused_rc categoricals: building/forest NEAREST,
// imd BILINEAR (round, clamp). `cover` is the halo packed [build,forest,imd] per cell.
__device__ __forceinline__ void cover_rc(
    const unsigned char* cover, int rows, int cols, float rf, float cf,
    unsigned char* bh, unsigned char* fr_out, unsigned char* imd_out)
{
    rf = fminf(fmaxf(rf, 0.0f), (float)(rows - 1));
    cf = fminf(fmaxf(cf, 0.0f), (float)(cols - 1));
    int r0 = min((int)floorf(rf), rows - 2);
    int c0 = min((int)floorf(cf), cols - 2);
    float fr = rf - (float)r0, fc = cf - (float)c0;
    long base = (long)r0 * cols + c0;
    long b00 = base*3, b01 = (base+1)*3, b10 = (base+cols)*3, b11 = (base+cols+1)*3;
    long near = (fr >= 0.5f) ? ((fc >= 0.5f) ? b11 : b10) : ((fc >= 0.5f) ? b01 : b00);
    *bh = cover[near]; *fr_out = cover[near + 1];
    float v0 = (float)cover[b00+2] + fc * ((float)cover[b01+2] - (float)cover[b00+2]);
    float v1 = (float)cover[b10+2] + fc * ((float)cover[b11+2] - (float)cover[b10+2]);
    float im = fminf(fmaxf(roundf(v0 + fr * (v1 - v0)), 0.0f), 255.0f);
    *imd_out = (unsigned char)im;
}

// ---- path_profile::path_integral_u8 (interval-length-weighted mean of imd).
// `fill_t` guarantees t[0]=0,t[n-1]=1 ⇒ Σ(t[i]−t[i−1])=1 ⇒ the CPU's `sum·dist /
// (total=dist)` collapses to the t-weighted mean Σ mid·(t[i]−t[i−1]); the `dist`
// and the `total` accumulator/division cancel exactly. fp32 (imd∈[0,255]).
__device__ float path_integral_imd(const double* t, const unsigned char* imd, int n) {
    if (n < 2) return 0.0f;
    float sum = 0.0f;
    for (int i = 1; i < n; i++) {
        float mid = 0.5f * ((float)imd[i - 1] + (float)imd[i]);
        sum += mid * (float)(t[i] - t[i - 1]);
    }
    return sum;
}

// ---- path_profile::vegetation_run_length — density-weighted forest depth
// (geodata-v2 2a): the ≥10 m scattered-tree gate stays on the PHYSICAL run
// extent, the accumulated depth is `len × v/100`. On binary rasters
// (v ∈ {0,100}) bit-identical to the old boolean run (100/100.0f = 1.0f
// exactly); output changes only when continuous density tiles land. fp32.
__device__ float veg_run_length(const double* t, const unsigned char* forest, int n, float dist) {
    if (n < 2) return 0.0f;
    float total = 0.0f, run_phys = 0.0f, run_w = 0.0f;
    for (int i = 1; i < n; i++) {
        float len = (float)(t[i] - t[i - 1]) * dist;
        if (forest[i] > 0) {
            run_phys += len;
            // __fmul_rn/__fadd_rn pin plain IEEE mul+add (no FMA
            // contraction): len × 1.0f is exact, so on binary rasters the
            // accumulation is bit-identical to the old `run += len`.
            run_w = __fadd_rn(run_w, __fmul_rn(len, (float)forest[i] / 100.0f));
        } else {
            if (run_phys >= 10.0f) total += run_w;
            run_phys = 0.0f; run_w = 0.0f;
        }
    }
    if (run_phys >= 10.0f) total += run_w;
    return total;
}

// ---- vegetation::vegetation_attenuation (per-band, capped). fp32.
__device__ void veg_bands(float depth, float* out) {
    for (int i = 0; i < NB; i++)
        out[i] = (depth <= 0.0f) ? 0.0f : fminf((float)ALPHA_VEG[i] * depth, (float)MAX_VEG[i]);
}

__device__ void barrier_best_candidate(
    const double* barr, int nbarr,
    double slat, double slon, double rlat, double rlon,
    const double* t, const double* bare, int n,
    double dist, double se, double re, double dsr,
    int* have, double* cand_t, double* cand_top)
{
    if (nbarr <= 0 || barr[5] > dist + BARR_HORIZON_M) return;
    double best_delta = 0.0;
    if (*have) {
        // SIGNED, like every other δ in this race — see the comment on the wall
        // δ below. Seeding the incumbent building with an UNSIGNED δ let a
        // below-LOS building enter at +|δ| and outrank a real wall: a 0.1 m
        // building 2 m from the receiver seeds +2.344 while a 6 m midpoint wall
        // scores +0.158, so the wall lost a race it wins on the CPU. Third site
        // of the same dropped-sign defect (review 2026-08-04); the other two are
        // the candidate δ and the favourable arm in `single_edge_bands_cand`.
        double d_sg = *cand_t * dist, d_rg = (1.0 - *cand_t) * dist;
        double los_inc = se + (re - se) * (*cand_t);
        best_delta = ((*cand_top >= los_inc) ? 1.0 : -1.0)
                   * (sqrt(d_sg * d_sg + (*cand_top - se) * (*cand_top - se))
                    + sqrt(d_rg * d_rg + (*cand_top - re) * (*cand_top - re)) - dsr);
    }
    double ray_mid = ((slat + rlat) * 0.5) * (PI_D / 180.0);
    double ray_mlon = M_LON_EQ * fmax(__cosf((float)ray_mid), 0.01f);
    double pdx = (rlon - slon) * ray_mlon, pdy = (rlat - slat) * M_LAT;
    for (int b = 0; b < nbarr; b++) {
        const double* w = &barr[b * BARR_STRIDE];
        if (w[5] > dist + BARR_HORIZON_M) break;
        double x0 = (w[1] - slon) * ray_mlon, y0 = (w[0] - slat) * M_LAT;
        double x1 = (w[3] - slon) * ray_mlon, y1 = (w[2] - slat) * M_LAT;
        double tt;
        if (!seg_isect_t(0.0, 0.0, pdx, pdy, x0, y0, x1, y1, &tt)) continue;
        // path_effects §5b: terrain LERPed between the neighbouring bare samples.
        int p = 1; while (p < n - 1 && t[p] <= tt) p++;
        double t0 = t[p - 1], t1 = t[p];
        double frac = (t1 > t0) ? (tt - t0) / (t1 - t0) : 0.0;
        double top = bare[p - 1] + frac * (bare[p] - bare[p - 1]) + w[4];
        double los = se + (re - se) * tt;
        double d_sg = tt * dist, d_rg = (1.0 - tt) * dist;
        // SIGNED δ, exactly as path_effects §5b ranks candidates: a wall that
        // just fails to break the sight line stays in the race with a NEGATIVE
        // δ, so a real blocker always outranks a near miss. Its dB is the Fix 2
        // penumbra, which `maek_single` now mirrors, and `obstacle_best_candidate`
        // ranks its building edges the same signed way — so a below-LOS wall is
        // both SELECTED and VOICED identically on the two lanes.
        double delta = ((top >= los) ? 1.0 : -1.0)
            * (sqrt(d_sg * d_sg + (top - se) * (top - se))
             + sqrt(d_rg * d_rg + (top - re) * (top - re)) - dsr);
        if (!*have || delta > best_delta) {
            *have = 1; best_delta = delta; *cand_t = tt; *cand_top = top;
        }
    }
}

// ---- the CLEAR-FAN arm of `ray_path_bands`: bare-earth terrain only.
//
// A clear direction has no obstacle to rank, so the crossing scan, the
// composite and the barrier race all fall away and only the elevation cadence
// is left. Identical output to `ray_path_bands`'s `terr[]` — `terrain_bands`
// reads nothing but `tprof`/`ed` — at a fraction of its cost.
__device__ void ray_terrain_bands(
    const float* elev, int rows, int cols,
    double lat_min, double lon_min, double inv,
    double slat, double slon, double salt,
    double rlat, double rlon, double ralt, double dist,
    double* tprof, double* ed, float* terr)
{
    float src_rf = (float)((slat - lat_min) * inv), src_cf = (float)((slon - lon_min) * inv);
    float d_rf = (float)((rlat - slat) * inv), d_cf = (float)((rlon - slon) * inv);
    int n = fill_t(dist, tprof);
    for (int i = 0; i < n; i++) {
        float rf = src_rf + (float)tprof[i] * d_rf, cf = src_cf + (float)tprof[i] * d_cf;
        ed[i] = (double)bilinear_elev_rc(elev, rows, cols, rf, cf);
    }
    terrain_bands(tprof, ed, n, dist, salt, ralt, terr);
}

// ---- ONE propagation ray, source→receiver: cadence march (bare elevation +
// building/forest/IMD), vector barriers, terrain diffraction, the vector
// obstacle candidate, and the screening increment. Fills the caller's profile
// scratch (`tprof/ed/comp/bld/forr/imdp`, `n` samples returned) and writes
// `terr[]`/`screen[]`.
//
// THE single ray evaluator: the characteristic-point ray and every
// arc-screening interval ray go through it, so the two cannot drift — the same
// reason `arc_screening::interval_screening` calls the production
// `terrain_attenuation`/`screening_attenuation` instead of a copy.
__device__ int ray_path_bands(
    const float* elev, const unsigned char* cover, int rows, int cols,
    double lat_min, double lon_min, double inv,
    double slat, double slon, double salt,
    double rlat, double rlon, double ralt, double dist,
    const double* barr, int nbarr, const unsigned long long* obst,
    double* tprof, double* ed, double* comp,
    unsigned char* bld, unsigned char* forr, unsigned char* imdp,
    float* terr, float* screen, int need_cover)
{
    // halo-RELATIVE ray coords (source → receiver). The per-sample ray delta
    // `d_rf` is ≲326 cells/10 km; the absolute `src_rf` is ≤~8000 cells even for a
    // 4×4-tile + 10 km batch, where fp32 ULP is ~1.5 cm ≪ the 30 m cell — so the lerp
    // runs in fp32 directly (mm–cm accurate; NOT incremental: `+=` would drift + break
    // the sample↔t[i] tie). The f64 ORIGIN subtractions (slat−lat_min, absolute
    // lat/lon) stay f64, cast once.
    float src_rf = (float)((slat - lat_min) * inv), src_cf = (float)((slon - lon_min) * inv);
    float d_rf = (float)((rlat - slat) * inv), d_cf = (float)((rlon - slon) * inv);

    // ray-march the cadence: bare elevation + building/forest/imd, in halo-RELATIVE
    // raster coords (coords computed above).
    // `need_cover` = 0 skips the building/forest/IMD sampling: an ARC INTERVAL
    // ray needs only the elevation profile, because ground G and vegetation are
    // read off the cp ray (DECISION 2026-08-03) and vector mode takes buildings
    // from the obstacle store, not the raster channel. Its three u8 grids were
    // being sampled and thrown away on every interval ray.
    int n = fill_t(dist, tprof);
    if (need_cover) {
        for (int i = 0; i < n; i++) {
            float rf = src_rf + (float)tprof[i] * d_rf, cf = src_cf + (float)tprof[i] * d_cf;
            ed[i] = (double)bilinear_elev_rc(elev, rows, cols, rf, cf);
            cover_rc(cover, rows, cols, rf, cf, &bld[i], &forr[i], &imdp[i]);
        }
    } else {
        for (int i = 0; i < n; i++) {
            float rf = src_rf + (float)tprof[i] * d_rf, cf = src_cf + (float)tprof[i] * d_cf;
            ed[i] = (double)bilinear_elev_rc(elev, rows, cols, rf, cf);
            bld[i] = 0;
        }
    }

    // path effects (bands in fp32)
    terrain_bands(tprof, ed, n, dist, salt, ralt, terr);
    for (int i = 0; i < NB; i++) screen[i] = 0.0f;
    // Vector obstacles (geodata-v2, QM_VECTOR_BUILDINGS=1): exact crossings
    // REPLACE the raster building channel (path_effects
    // replace_sample_buildings) — the composite keeps only barriers, and the
    // max-δ candidate from the obstacle grids competes with the cadence edge.
    // Exact-crossing candidates — vector building edges (geodata-v2,
    // QM_VECTOR_BUILDINGS=1, which REPLACE the raster building channel:
    // path_effects `replace_sample_buildings`) AND noise-barrier segments —
    // run ONE max-δ race against the cadence composite edge, mirroring
    // path_effects §5b/§5c. Neither kind ever enters the composite.
    bool vec_mode = obst[0] != 0ULL;
    int have_cand = 0;
    double cand_t = 0.0, cand_top = 0.0;
    if (n >= 3 && dist >= 30.0) {
        double src_h = fmax(salt - ed[0], 0.05);
        double rcv_h = fmax(ralt - ed[n - 1], 0.5);
        double se = ed[0] + src_h, re = ed[n - 1] + rcv_h;
        double dsr = sqrt(dist * dist + (re - se) * (re - se));
        if (!ABL_CAND_OFF && vec_mode)
            obstacle_best_candidate(obst, slat, slon, rlat, rlon,
                                    tprof, ed, n, dist, se, re, dsr,
                                    &have_cand, &cand_t, &cand_top);
        barrier_best_candidate(barr, nbarr, slat, slon, rlat, rlon,
                               tprof, ed, n, dist, se, re, dsr,
                               &have_cand, &cand_t, &cand_top);
    }
    // A wall over open ground has no building cell — its crossing must enable
    // the screening pass too, or walls outside towns are silently ignored.
    bool anyb = have_cand;
    if (!vec_mode)
        for (int i = 0; i < n; i++) if (bld[i] > 0) { anyb = true; break; }
    if (anyb && n >= 3 && dist >= 30.0) {
        for (int i = 0; i < n; i++) {
            // Endpoint exclusion (tprof ∈ (0,1)) — the CPU composite gate,
            // path_effects.rs §3. Buildings only: walls are candidates now.
            double bh = vec_mode ? 0.0 : (double)bld[i];
            double above = (tprof[i] > 0.0 && tprof[i] < 1.0) ? bh : 0.0;
            comp[i] = ed[i] + above;
        }
        float comb[NB];
        single_edge_bands_cand(tprof, comp, ed, n, dist, salt, ralt,
                               have_cand, cand_t, cand_top, comb);
        for (int i = 0; i < NB; i++) screen[i] = fmaxf(comb[i] - terr[i], 0.0f);
    }
    return n;
}

// ---- x-extent of the (receiver, start, end) TRIANGLE inside one horizontal
// cell row [y_lo, y_hi], in an index's local frame. False when the triangle
// misses the row entirely.
//
// The arc walk's candidate set only has to cover the triangle, not its bbox:
// every confirmation ray runs from the receiver to a point ON the segment, so it
// is a chord of the triangle, and a footprint lying ENTIRELY outside one can
// never be crossed by any of them — its hull would be built, clipped and then
// rejected. For a 250 m segment seen from 2 km the triangle is ~1/16 of its own
// bbox, and with the physics rays measured free that ratio IS the walk's cost.
__device__ __forceinline__ bool tri_row_x_span(
    double rx, double ry, double ax, double ay, double bx, double by,
    double y_lo, double y_hi, double* x_lo, double* x_hi)
{
    double lo = 1e300, hi = -1e300;
    const double vx[3] = {rx, ax, bx};
    const double vy[3] = {ry, ay, by};
    for (int i = 0; i < 3; i++) {
        if (vy[i] >= y_lo && vy[i] <= y_hi) {          // vertex inside the slab
            lo = fmin(lo, vx[i]); hi = fmax(hi, vx[i]);
        }
        int j = (i + 1) % 3;                            // edge i→j vs each boundary
        double dy = vy[j] - vy[i];
        if (dy == 0.0) continue;
        double inv_dy = 1.0 / dy;
        for (int e = 0; e < 2; e++) {
            double t = ((e == 0 ? y_lo : y_hi) - vy[i]) * inv_dy;
            if (t < 0.0 || t > 1.0) continue;
            double x = vx[i] + t * (vx[j] - vx[i]);
            lo = fmin(lo, x); hi = fmax(hi, x);
        }
    }
    if (hi < lo) return false;
    *x_lo = lo; *x_hi = hi;
    return true;
}

// ---- Clip an angular arc to the segment's span. A hull can sit one turn away
// from it (base near ±π), hence the three shifts.
__device__ __forceinline__ bool arc_clip_span(
    double a_lo, double a_hi, double lo, double hi, double* st, double* en)
{
    for (int sh = 0; sh < 3; sh++) {
        double shift = sh == 0 ? 0.0 : (sh == 1 ? TAU_D : -TAU_D);
        double s2 = fmax(a_lo + shift, lo), e2 = fmin(a_hi + shift, hi);
        if (e2 > s2) { *st = s2; *en = e2; return true; }
    }
    return false;
}

// ---- The (height, range) STRATUM an arc belongs to — `arc_screening::fuse_key`.
// Two arcs may be described by ONE (range, height) pair only inside one stratum;
// the key is INVARIANT under fusion, because `min`/`max` of two values in a band
// stay in that band, which is what lets the merged list stay sorted by it.
//
// `logf`, NOT `__logf`, over a divisor build.rs takes from the RUST const. This
// is the one place in the kernel where a float approximation does not perturb a
// VALUE, it flips a DISCRETE DECISION: the quotient goes straight into `floorf`,
// so one ulp anywhere near a stratum boundary moves the arc into the
// NEIGHBOURING range band, where it fuses with a different set of arcs (or with
// none) and the merged list the rest of the walk sees is a different list.
//
// MEASURED EXHAUSTIVELY on the 5070 (GB205, CUDA 13.3) — every f32 in
// 1e-3 .. 1e6 m, 250 679 698 values, i.e. every range this term can represent,
// each one's stratum diffed against the Rust lane's own answer for it:
//
//   __logf(near) / __logf(1.5f)            158 disagreements  (AS SHIPPED)
//     logf(near) /   logf(1.5f)            305                (numerator only)
//     logf(near) / ARC_FUSE_RANGE_RATIO_LN   9                (this line)
//
// Read the middle row twice: swapping ONLY the numerator to the accurate `logf`
// — the obvious one-line fix — makes the lanes disagree TWICE AS OFTEN. The
// divisor is why. CUDA's `logf(1.5f)` is 0x3ecf9920 while `__logf(1.5f)` and
// Rust's `1.5f32.ln()` are both 0x3ecf991f, so the shipped expression was
// accidentally consistent about the boundary ladder and an accurate numerator
// over an inaccurate divisor is not. One ulp on a divisor is not noise: it tilts
// every boundary the same way.
//
// So both halves are mirrored, and neither is a hand-copied number: the divisor
// comes from build.rs computing `ARC_FUSE_RANGE_RATIO.ln()` in Rust. On the
// numerator `logf` is libdevice's 1-ulp routine (differs from Rust's `f32::ln` in
// VALUE on 4.5 % of the range, against 50.5 % for the MUFU-based `__logf`), so
// it is the closer of the two once the divisor no longer cancels its error. The
// residual 9 disagreements are f32 values within one ulp of a 1.5^k boundary,
// where CUDA's and glibc's `logf` round the numerator apart; closing them needs a
// bit-identical `logf`, not a constant, and one ulp of range there is physically
// indistinguishable inside a stratum a factor of 1.5 wide.
// Cost: none measurable — see the ARC_MAX_MERGED note below for the A/B.
// `-use_fast_math` would silently put `__logf` back (it maps logf -> __logf), so
// it must never be added to the nvcc line in build.rs; the flags there are
// -O3 only, and `--prec-div`/`--prec-sqrt` default to true.
__device__ __forceinline__ int arc_fuse_key(float near_m, float height_m)
{
    int h = (int)floorf(fmaxf(height_m, 0.0f) / ARC_FUSE_HEIGHT_TOL_M);
    int r = (int)floorf(logf(fmaxf(near_m, 1e-3f)) / ARC_FUSE_RANGE_RATIO_LN);
    // The Rust `clamp`s, mirrored rather than argued away. `r`'s cannot bind for
    // any FINITE range — the 1e-3 floor pins r >= -18 and f32::MAX pins r <= 218
    // (ln(3.4e38)/ln 1.5) — but `h`'s CAN: it needs only a garbage 48 km height in
    // the obstacle store, and `ObstacleIndex::add_polyline` admits any height that
    // is finite and > 0. Unclamped, `h << 16` at that point overflows the int (UB)
    // and aliases two strata onto one key. Two integer min/max against that; the
    // CPU pays them too. They also make the degenerate inputs AGREE rather than
    // merely not crash: `near = inf` saturates the cast on both lanes (cvt.rzi
    // here, `as i32` there) and both then clamp to the same 16 000, and `NaN`
    // becomes 1e-3 through `fmaxf`/`max` on both.
    return (min(max(h, 0), 16000) << 16) | ((min(max(r, -16000), 16000) + 0x4000) & 0xffff);
}

// ---- Union-insert one blocked arc into a SORTED, non-overlapping list.
// The union of a set of intervals does not depend on insertion order, so the
// incremental form here yields exactly the interval set the CPU's
// sort-then-merge pass produces — the footprint walk can therefore retire each
// footprint immediately instead of parking it in a capped table. Touching
// intervals merge (`<=`), mirroring the CPU's `iv_s[i] <= iv_e[mg]`.
__device__ __forceinline__ int arc_iv_union(
    double* iv_s, double* iv_e, float* iv_near, float* iv_h,
    int* iv_key, int niv, double st, double en, float near_m, float h_m,
    int* overflow)
{
    // Sorted by `(stratum key, lo)`, so one STRATUM's arcs are contiguous and,
    // inside it, disjoint. Arcs of different strata are left side by side and
    // each faces the admission test with its OWN range — merging them would give
    // the union the nearest member's range, which is an obstacle that does not
    // exist. Inside one stratum a fusion takes the MINIMUM range and the MAXIMUM
    // height, both of which only keep the arc alive, so it can cost an
    // evaluation ray and never a lost screen.
    double s0 = st, e0 = en;
    float n0 = near_m, h0 = h_m;
    int key = arc_fuse_key(near_m, h_m);
    int i = 0;
    while (i < niv && (iv_key[i] < key || (iv_key[i] == key && iv_s[i] < s0))) i++;
    // At most ONE arc of this stratum can overlap `s0`, and it is the one
    // immediately left: the stratum's arcs are disjoint and sorted by `lo`.
    if (i > 0 && iv_key[i - 1] == key && iv_e[i - 1] >= s0) {
        i--;
        s0 = fmin(s0, iv_s[i]); e0 = fmax(e0, iv_e[i]);
        n0 = fminf(n0, iv_near[i]); h0 = fmaxf(h0, iv_h[i]);
        for (int q = i; q < niv - 1; q++) {
            iv_s[q] = iv_s[q + 1]; iv_e[q] = iv_e[q + 1];
            iv_near[q] = iv_near[q + 1]; iv_h[q] = iv_h[q + 1];
            iv_key[q] = iv_key[q + 1];
        }
        niv--;
    }
    // `i` sits inside this stratum's run; absorbing only grows `e0`, so the same
    // slot is re-tested against the wider arc and the scan needs no rewind.
    while (i < niv && iv_key[i] == key && iv_s[i] <= e0) {
        s0 = fmin(s0, iv_s[i]); e0 = fmax(e0, iv_e[i]);
        h0 = fmaxf(h0, iv_h[i]);
        n0 = fminf(n0, iv_near[i]);
        for (int q = i; q < niv - 1; q++) {
            iv_s[q] = iv_s[q + 1]; iv_e[q] = iv_e[q + 1];
            iv_near[q] = iv_near[q + 1]; iv_h[q] = iv_h[q + 1];
            iv_key[q] = iv_key[q + 1];
        }
        niv--;
    }
    // The strata make the list LONGER — a near and a far footprint that used to
    // collapse into one slot now hold two — and an overflow DROPS the arc, i.e.
    // under-screens. `arcstat[6]`/`[7]` count them, but ONLY under
    // -DPROF_COUNTERS=1; an unflagged build reads zeros and looks clean.
    //
    // ALREADY MEASURED, without a GPU (2026-08-05). `ArcBounds::max_intervals`
    // is this cap's CPU twin — same scope (per segment, receiver), same drop
    // semantics — so `QM_ARC_MAX_INTERVALS=32` against an uncapped run answers
    // it. On z12/2212/1387 (Praha, road), painting the same rows both ways:
    //
    //   height-only fuse : cap 32 is BIT-IDENTICAL to uncapped — never binds
    //   (height, range)  : cap 32 CHANGES the field — max 4.5 dB on a pixel
    //
    // So 32 was adequate for the old merge and is NOT adequate for this one:
    // the strata push the list past a ceiling it used to clear. The ceiling
    // needed also GROWS with source density — at 1/4000 of the rows cap 48 was
    // already clean, at 1/500 cap 40 still bound — so 32 must be RAISED, and
    // the value must be fixed against a production-density run, not this one.
    // The CPU twin cannot be the last word (it stops collecting on overflow,
    // where this loop keeps fusing into occupied slots, so it reads slightly
    // pessimistic); confirm the final number with `arcstat[6]`/`[7]` on GPU.
    // Note the fixture cannot catch this: its scenes carry 5 footprints, so
    // ARC_MAX_MERGED can never bind there (screening_fixture.rs:1089).
    if (niv >= ARC_MAX_MERGED) { *overflow += 1; return niv; }
    for (int q = niv; q > i; q--) {
        iv_s[q] = iv_s[q - 1]; iv_e[q] = iv_e[q - 1];
        iv_near[q] = iv_near[q - 1]; iv_h[q] = iv_h[q - 1];
        iv_key[q] = iv_key[q - 1];
    }
    iv_s[i] = s0; iv_e[i] = e0; iv_near[i] = n0; iv_h[i] = h0;
    iv_key[i] = key;
    return niv + 1;
}

// ---- arc_screening's `ground_or_barrier`: the kernel's own ISO 9613-2 §7.3.1
// term for ONE band — the barrier arm replaces the ground arm when it exists,
// `A_ground` alone stands when it does not. Averaging THIS (not the bare
// screening increment) is what keeps a blocked fraction's energy where it
// belongs: a segment 14 % of whose fan is blocked by 17 dB yields ~0.3 dB of
// average screening, which max(A_ground, …) would otherwise discard entirely.
__device__ __forceinline__ double arc_gob(double terr_b, double ground_b, double screen_db) {
    double barrier = terr_b + screen_db;
    return (barrier > 0.0) ? fmax(ground_b, barrier) : ground_b;
}

// ---- propagation::arc_screening::arc_screened_attenuation — the per-band
// effective screening of the WHOLE segment as seen from this receiver.
//
//   1. the segment's angular span at the receiver (two atan2),
//   2. the sub-intervals of that span that vector obstacles actually block —
//      one arc per obstacle EDGE at that edge's own range and height, exactly as
//      `ObstacleIndex::skyline_arcs_within` emits them, each confirmed by an
//      exact ray crossing (the per-footprint angular hull survives only as a
//      cheap reject, because one hull arc cannot carry per-edge ranges),
//   3. ONE screening evaluation per blocked interval, on the ray to the EXACT
//      source point on the segment at the interval's centre azimuth (exact
//      range, no banding), the cp evaluation reused for the interval that
//      contains the cp azimuth,
//   4. an energy average over the interval fractions of the span, taken on the
//      term the kernel actually applies — max(A_ground, A_terrain + A_screen).
//
// Terrain, ground G and vegetation stay on the caller's cp ray; the terrain of
// an interval ray is used ONLY inside that interval's own screening call, as the
// increment base. `screen` is BOTH the cp verdict in and the arc result out (all
// reads finish before the single write pass), and is left untouched whenever the
// CPU would return the cp bands: degenerate span, no hull, nothing confirmed.
__device__ void arc_screen_bands(
    const float* elev, const float* inner, const unsigned char* cover,
    int rows, int cols, double lat_min, double lon_min, double inv, const double* bb,
    double rlat, double rlon, double ralt,
    double alat, double alon, double blat, double blon, double src_height,
    double cplat, double cplon, const float* cp_terr, float ground_g,
    const double* barr, int nbarr, const unsigned long long* obst,
    double* tprof, double* ed, double* comp,
    unsigned char* bld, unsigned char* forr, unsigned char* imdp,
    float* screen, unsigned int* hc_key, double* hc_lo, double* hc_hi, float* arcstat,
    unsigned int* arc_drops)
{
    double mlon = M_LON_EQ * fmax(cos(rlat * (PI_D / 180.0)), 0.01);
    double base = arc_az(rlat, rlon, mlon, alat, alon);
    double delta = wrap_pi_d(arc_az(rlat, rlon, mlon, blat, blon) - base);
    // Below this span the segment IS a point source at this receiver: the cp ray
    // already represents it, and the interval arithmetic would divide by ~0.
    if (fabs(delta) < ARC_MIN_SPAN) return;
    // Span in the base-relative frame; a segment subtends at most π at a
    // receiver off its line, so the short arc between the endpoints IS it.
    double lo = delta < 0.0 ? delta : 0.0;
    double hi = delta < 0.0 ? 0.0 : delta;
    double span = hi - lo;
    if (ABL_ARC_SPAN) { if (span == 1.2345678e300) screen[0] += 1.0f; return; }

#if ARC_FOOTPRINT_CSR
    // ---- 1-3 FUSED, one pass per FOOTPRINT (TRACK C). Same candidate set as
    // the edge walk below — the (receiver, start, end) triangle bbox, cell
    // granular — but the unit of work is the footprint, so each is visited once,
    // completed from its own contiguous edge run, and retired. That removes the
    // per-edge linear search of a partial-hull table, the ARC_MAX_IV cap (and
    // its divergence from the CPU's uncapped grouping), and the full-grid DDA
    // the confirmation used to need.
    double iv_s[ARC_MAX_MERGED], iv_e[ARC_MAX_MERGED];
    float iv_near[ARC_MAX_MERGED], iv_h[ARC_MAX_MERGED];
    int iv_key[ARC_MAX_MERGED];
    int niv = 0, iv_overflow = 0;
    // No cp source ALTITUDE is read here any more. It existed only to build the
    // sight line the deleted δ prefilter measured against; the admission test
    // below is geometry only, and the ray march re-derives δ from the real
    // profile. Removing it also removes a `tile_elev` raster read per call.
    const double* metas = (const double*)obst[1];
    const float* edges = (const float*)obst[4];
    const unsigned int* foot_es = (const unsigned int*)obst[7];
    const float* foot_box = (const float*)obst[8];
    const unsigned int* cfs = (const unsigned int*)obst[9];
    const unsigned int* cfi = (const unsigned int*)obst[10];
    const unsigned int* cfp = (const unsigned int*)obst[11];
    const unsigned int* foot_er = (const unsigned int*)obst[12];
    const unsigned int* cfc = (const unsigned int*)obst[13];
    int n_idx = (int)obst[0];
    double q_min_lat = fmin(fmin(rlat, alat), blat), q_max_lat = fmax(fmax(rlat, alat), blat);
    double q_min_lon = fmin(fmin(rlon, alon), blon), q_max_lon = fmax(fmax(rlon, alon), blon);
    // The fan's two boundary directions, in the RECEIVER metric.
    double dlo_x = cos(base + lo), dlo_y = sin(base + lo);
    double dhi_x = cos(base + hi), dhi_y = sin(base + hi);
    for (int gi = 0; gi < n_idx; gi++) {
        const double* m = &metas[gi * OBST_META_STRIDE];
        double imlon = m[2], cell = m[3], minx = m[4], miny = m[5];
        int cols_i = (int)m[6], rows_i = (int)m[7];
        size_t eoff = (size_t)m[10];
        size_t foff = (size_t)m[13], cfs_off = (size_t)m[14], cfi_off = (size_t)m[15];
        size_t fes_off = (size_t)m[17], fer_off = (size_t)m[18];
        double xa = (q_min_lon - m[1]) * imlon, xb = (q_max_lon - m[1]) * imlon;
        double ya = (q_min_lat - m[0]) * M_LAT, yb = (q_max_lat - m[0]) * M_LAT;
        double inv_cell = 1.0 / cell;
        double cx0d = floor((fmin(xa, xb) - minx) * inv_cell);
        double cx1d = floor((fmax(xa, xb) - minx) * inv_cell);
        double cy0d = floor((fmin(ya, yb) - miny) * inv_cell);
        double cy1d = floor((fmax(ya, yb) - miny) * inv_cell);
        if (cx1d < 0.0 || cx0d > (double)(cols_i - 1)) continue;
        if (cy1d < 0.0 || cy0d > (double)(rows_i - 1)) continue;
        int cx0 = (int)fmax(cx0d, 0.0), cx1 = (int)fmin(cx1d, (double)(cols_i - 1));
        int cy0 = (int)fmax(cy0d, 0.0), cy1 = (int)fmin(cy1d, (double)(rows_i - 1));
        // Receiver and the two fan boundaries expressed in THIS index's local
        // frame. Only x rescales (imlon vs mlon); a positive x-scale multiplies
        // every 2D cross product by the same positive constant, so the sign
        // tests below are exact in either frame.
        double rxl = (rlon - m[1]) * imlon, ryl = (rlat - m[0]) * M_LAT;
        double kx = imlon / mlon;
        double alx = dlo_x * kx, aly = dlo_y;
        double ahx = dhi_x * kx, ahy = dhi_y;
        // Triangle vertices in this index's local frame — the scanline walk's
        // input (`cx0..cx1` stays only as the clamp).
        double tax = (alon - m[1]) * imlon, tay = (alat - m[0]) * M_LAT;
        double tbx = (blon - m[1]) * imlon, tby = (blat - m[0]) * M_LAT;
        for (int cy = cy0; cy <= cy1; cy++) {
            size_t row = (size_t)cy * (size_t)cols_i;
            int rcx0 = cx0, rcx1 = cx1;
#if ARC_TRI_WALK
            double y_lo = miny + (double)cy * cell, y_hi = y_lo + cell;
            double xs_lo, xs_hi;
            if (!tri_row_x_span(rxl, ryl, tax, tay, tbx, tby, y_lo, y_hi, &xs_lo, &xs_hi))
                continue;
            rcx0 = (int)fmax(floor((xs_lo - minx) * inv_cell), (double)cx0);
            rcx1 = (int)fmin(floor((xs_hi - minx) * inv_cell), (double)cx1);
#endif
            for (int cx = rcx0; cx <= rcx1; cx++) {
                size_t c = row + (size_t)cx;
                // Read-only (__ldg) path: these five arrays are immutable for
                // the kernel's lifetime, and coming through a u64 pointer table
                // they carry no __restrict__ const for nvcc to infer it from.
                unsigned int j0 = __ldg(&cfs[cfs_off + c]), j1 = __ldg(&cfs[cfs_off + c + 1]);
                if (ABL_ARC_CELLS) { iv_overflow += (int)(j1 - j0); continue; }
                for (unsigned int j = j0; j < j1; j++) {
                    unsigned int f = __ldg(&cfi[cfi_off + j]);
                    // O(1) dedup: this listing is the footprint's FIRST inside
                    // the walked range iff its back-link points outside it.
                    // Walk the back-link CHAIN: this listing is the first one
                    // inside the visited set iff NO earlier listing of the same
                    // footprint falls in it. Checking only the immediately
                    // previous listing is wrong whenever the set skips a cell
                    // between two it does visit — which the triangle walk does
                    // constantly, so the smaller set was processing MORE
                    // duplicates than the bbox. The chain is as long as the
                    // number of cells the footprint occupies (1-4 at 64 m).
                    bool already = false;
                    for (unsigned int q = __ldg(&cfp[cfi_off + j]); q != 0xFFFFFFFFu;
                         q = __ldg(&cfp[cfi_off + q])) {
                        unsigned int qc = __ldg(&cfc[cfi_off + q]);
                        int pcy = (int)(qc / (unsigned int)cols_i);
                        int pcx = (int)(qc - (unsigned int)pcy * (unsigned int)cols_i);
                        if (pcy < cy0 || pcy > cy1) continue;
                        int p0 = cx0, p1 = cx1;
#if ARC_TRI_WALK
                        double py_lo = miny + (double)pcy * cell, py_hi = py_lo + cell;
                        double pxs_lo, pxs_hi;
                        if (!tri_row_x_span(rxl, ryl, tax, tay, tbx, tby,
                                            py_lo, py_hi, &pxs_lo, &pxs_hi)) continue;
                        p0 = (int)fmax(floor((pxs_lo - minx) * inv_cell), (double)cx0);
                        p1 = (int)fmin(floor((pxs_hi - minx) * inv_cell), (double)cx1);
#endif
                        if (pcx >= p0 && pcx <= p1) { already = true; break; }
                    }
                    if (already) continue;
                    // Wedge prune, no transcendentals: a direction p is inside
                    // the (convex, ≤π) fan iff cross(d_lo,p) ≥ 0 and
                    // cross(p,d_hi) ≥ 0. Both are linear in p, so their MAXIMUM
                    // over the footprint's bbox sits at a supporting corner and
                    // costs one select per axis. A negative maximum means every
                    // vertex — hence every arc between them — lies outside the
                    // fan, so the hull could not have survived the clip.
                    const float* fb = &foot_box[(foff + f) * FOOT_BOX_STRIDE];
                    double bx0 = (double)__ldg(&fb[0]) - rxl, by0 = (double)__ldg(&fb[1]) - ryl;
                    double bx1 = (double)__ldg(&fb[2]) - rxl, by1 = (double)__ldg(&fb[3]) - ryl;
                    double g1 = alx * (alx >= 0.0 ? by1 : by0) - aly * (aly >= 0.0 ? bx0 : bx1);
                    if (g1 < 0.0) continue;
                    double g2 = ahy * (ahy >= 0.0 ? bx1 : bx0) - ahx * (ahx >= 0.0 ? by0 : by1);
                    if (g2 < 0.0) continue;
                    // Hull from the footprint's own edges — same short-arc rule
                    // per edge as the edge walk, just gathered in one place.
                    unsigned int k0 = __ldg(&foot_es[fes_off + f]);
                    unsigned int k1 = __ldg(&foot_es[fes_off + f + 1]);
                    // The hull is a cheap exact REJECT and nothing else: every
                    // footprint EMITS the per-edge union below, with each arc
                    // carrying its OWN range (see the emit block). It therefore
                    // computes NO range — the footprint-wide minimum that used to
                    // live here was the parity defect, and a variable named
                    // `near_m` in this scope is what let it hide.
                    double h_lo = 1e300, h_hi = -1e300;
                    // First vertex's unwrapped angle — the turn every other
                    // vertex of THIS footprint is unwrapped onto (see below).
                    double hull_anchor = 0.0;
#if ARC_HULL_CACHE
                    // Absolute-azimuth hull, cached per (receiver, footprint).
                    unsigned int ckey = ((unsigned int)gi << 28) | (f & 0x0FFFFFFFu);
                    int cslot = (int)((f * 2654435761u) & (unsigned int)(ARC_HULL_CACHE - 1));
                    if (PROF_COUNTERS && arcstat) {
                        arcstat[2] += 1.0f;                       // hull lookups
                        if (hc_key[cslot] == ckey) arcstat[3] += 1.0f;   // hits
                    }
                    if (hc_key[cslot] == ckey) {
                        h_lo = wrap_pi_d(hc_lo[cslot] - base);
                        h_hi = h_lo + hc_hi[cslot];           // stored as the WIDTH
                    } else {
#endif
                    for (unsigned int kk = k0; kk < k1; kk++) {
                        const float* g = &edges[(eoff + __ldg(&foot_er[fer_off + kk])) * 5];
                        double lat0 = m[0] + (double)g[1] / M_LAT;
                        double lon0 = m[1] + (double)g[0] / imlon;
                        double lat1 = m[0] + (double)g[3] / M_LAT;
                        double lon1 = m[1] + (double)g[2] / imlon;
                        double a0 = arc_az(rlat, rlon, mlon, lat0, lon0);
                        double a1 = arc_az(rlat, rlon, mlon, lat1, lon1);
                        // Unwrap every vertex onto the SAME TURN as the first
                        // one. Wrapping each edge independently into (−π, π]
                        // TEARS a footprint whose vertices straddle the seam
                        // relative to `base`: half the edges land near +π and
                        // half near −π, so fmin/fmax spans almost the whole
                        // circle.
                        //
                        // A torn hull is now only WIDER than the true one, and
                        // this hull is only a REJECT, so the tear can cost an
                        // admitted footprint's worth of per-edge work and cannot
                        // change an answer. It used to change answers: while a
                        // convex footprint EMITTED its hull, the confirmation ray
                        // was aimed at the middle of the bogus span — a direction
                        // the building is nowhere near — it missed, and the whole
                        // footprint was DISCARDED, so the pixel came out LOUDER
                        // than with no obstacles at all (P4's worst cell: raster
                        // GPU 51.812 = CPU 51.812, but with vectors the CPU went
                        // −9.8 dB while the GPU went +5.1 dB). The per-edge
                        // emission below removes that mechanism — each arc is
                        // confirmed on ITS OWN centre ray — and the anchor stays
                        // because a hull that spans the circle rejects nothing and
                        // makes the prune worthless. Review 2026-08-04.
                        double r0;
                        if (kk == k0) {
                            r0 = wrap_pi_d(a0 - base);
                            hull_anchor = r0;
                        } else {
                            r0 = hull_anchor + wrap_pi_d(a0 - base - hull_anchor);
                        }
                        double r1 = r0 + wrap_pi_d(a1 - a0);
                        h_lo = fmin(h_lo, fmin(r0, r1));
                        h_hi = fmax(h_hi, fmax(r0, r1));
                    }
#if ARC_HULL_CACHE
                        hc_key[cslot] = ckey;
                        hc_lo[cslot] = wrap_pi_d(base + h_lo);
                        hc_hi[cslot] = h_hi - h_lo;
                    }
#endif
                    // The CONVEX HULL is used ONLY as a cheap exact REJECT: the
                    // union of the per-edge arcs is contained in it, so an empty
                    // hull clip means an empty union. `st`/`en` are the clip's
                    // outputs and nothing downstream reads them — every emitted
                    // arc clips ITSELF against the span below.
                    double st = 0.0, en = -1.0;
                    if (!arc_clip_span(h_lo, h_hi, lo, hi, &st, &en)) continue;
                    if (PROF_COUNTERS && arcstat) arcstat[4] += 1.0f;   // survived the clip
                    // ---- Emit the silhouette as the exact UNION OF THE PER-EDGE
                    // ARCS, for EVERY footprint — convex ones included. This is
                    // the rule `ObstacleIndex::skyline_arcs_within` follows
                    // (obstacle_index.rs: one `SkylineArc` per edge, each with its
                    // own `near_m` from `origin_to_segment_dist`), and the reason
                    // it is not merely a nicety is the RANGE, not the shape:
                    //
                    // one hull arc can carry only ONE range, so the pre-fix code
                    // gave every direction of a convex footprint that footprint's
                    // MINIMUM range — and `b` is then handed to the admission test
                    // and to δ ≈ h²·d/(2ab) as if the diffracting edge stood
                    // there. Any part of a house closer than 1 m therefore
                    // rejected the WHOLE house through the `b < 1.0` gate below,
                    // and a receiver standing INSIDE a footprint (hull 6.2832 rad,
                    // min range 0.05 m) had its entire panorama declared clear.
                    // With per-edge arcs the near edge is the only one that
                    // rejects; its neighbours keep screening at their own ranges,
                    // and for a receiver inside the ring the union of the edge
                    // arcs still covers the full turn.
                    //
                    // MEASURED, e2-full on rail 2206/1391 (cells >0.5 dB / max dB /
                    // share GPU-louder), 2026-08-09:
                    //
                    //   footprint-wide range, hull for convex   4469 / 17.63 / 66.7 %
                    //   per-EDGE range, hull for convex         3472 / 16.12 / 59.7 %
                    //   per-edge arcs everywhere (THIS, = CPU)  2298 /  5.91 / 39.5 %
                    //
                    // The middle row is why the shape cannot be left convex-only:
                    // a hull arc's range is not a property of the hull's own edge
                    // set, so fixing the range without fixing the emission leaves
                    // half the fault in place. Cost of the third row over the
                    // first: +17 % kernel time (156.4 s -> 183.8 s on that tile),
                    // paid in per-edge `arc_az` pairs and one confirmation ray per
                    // emitted arc.
                    //
                    // `foot_box` is down to its bbox (FOOT_BOX_STRIDE 4): the
                    // footprint's max height and the host's convexity flag went
                    // with the hull emission, since the per-edge arc reads the
                    // EDGE's own height and range.
                    for (unsigned int ei = k0; ei < k1; ei++) {
                        const float* g = &edges[(eoff + __ldg(&foot_er[fer_off + ei])) * 5];
                        double lat0 = m[0] + (double)g[1] / M_LAT;
                        double lon0 = m[1] + (double)g[0] / imlon;
                        double lat1 = m[0] + (double)g[3] / M_LAT;
                        double lon1 = m[1] + (double)g[2] / imlon;
                        double a0 = arc_az(rlat, rlon, mlon, lat0, lon0);
                        double a1 = arc_az(rlat, rlon, mlon, lat1, lon1);
                        double r0 = wrap_pi_d(a0 - base);
                        double r1 = r0 + wrap_pi_d(a1 - a0);
                        double est = 0.0, een = -1.0;
                        if (!arc_clip_span(fmin(r0, r1), fmax(r0, r1), lo, hi, &est, &een))
                            continue;
                        // THIS EDGE's closest approach to the receiver — the arc's
                        // own `SkylineArc::near_m` (`origin_to_segment_dist`,
                        // obstacle_index.rs:444), in the index-local frame.
                        double ex0 = (double)g[0] - rxl, ey0 = (double)g[1] - ryl;
                        double ex1 = (double)g[2] - rxl, ey1 = (double)g[3] - ryl;
                        double vx = ex1 - ex0, vy = ey1 - ey0;
                        double vv = vx * vx + vy * vy;
                        double tt2 = (vv > 0.0) ? fmin(fmax(-(ex0 * vx + ey0 * vy) / vv, 0.0), 1.0)
                                                : 0.0;
                        double qx = ex0 + tt2 * vx, qy = ey0 + tt2 * vy;
                        double near_m = sqrt(qx * qx + qy * qy);
                        // …and its own HEIGHT (`SkylineArc::height_m` is the
                        // EDGE's, edges stride 5 = x0,y0,x1,y1,height). Equal to
                        // the footprint maximum for every store this lane can be
                        // given — `add_polygon_wkb` stamps one height on every ring
                        // of an id — but it is the per-edge value the fuse key is
                        // defined on, so read it where the definition is.
                        double h_edge = (double)__ldg(&g[4]);
                        if (PROF_COUNTERS && arcstat) arcstat[5] += 1.0f;   // emitted
                        // Straight into the union: an emitted arc carries its
                        // azimuth range, its own range and its own height, and
                        // NOTHING is decided about it here.
                        //
                        // Three tests used to stand in this spot and all three are
                        // gone. Two of them — the confirmation ray (does the ray to
                        // this arc's centre really cross the footprint) and the
                        // per-edge δ test against the sight line — have no CPU twin
                        // that could ever run per raw edge: `ArcSkyline` is built
                        // once per RECEIVER and clipped by every segment of it, so a
                        // segment-dependent test cannot precede the merge there. The
                        // third, the range floor, IS the CPU's, but the CPU applies
                        // it to the MERGED arc, which is the more permissive
                        // granularity. All three therefore only ever made this lane
                        // stricter than the reference, and the δ one made it wrong:
                        // pixel 109/509 of rail 2206/1391 lost a whole segment's
                        // screening on five sources (closest-point ray 12.49 dB,
                        // arc verdict 0.00) where the CPU kept 12.49. See
                        // `arc_screening`'s "Why the blocked test is geometry only"
                        // for why a prefilter that is not a sound
                        // over-approximation is worse than none at all.
                        //
                        // `arc_fuse_key(near, height)` still does its job: the arcs
                        // of one footprint arrive with DIFFERENT ranges, so they
                        // land in the range strata they belong to instead of all
                        // sharing the footprint minimum's stratum and fusing into
                        // one arc that carried the nearest edge's range across the
                        // whole silhouette.
                        niv = arc_iv_union(iv_s, iv_e, iv_near, iv_h, iv_key, niv, est, een,
                                           (float)near_m, (float)h_edge, &iv_overflow);
                    }
                }
            }
        }
    }
    if (ABL_ARC_CELLS) { if (iv_overflow == 12345678) screen[0] += 1.0f; return; }
    // ---- Noise WALLS are the other half of the skyline (fix-pack Fix 5). A wall
    // IS a segment, so its arc is the same primitive a building edge produces:
    // the short arc between its endpoint azimuths, at its nearest range. Without
    // this a receiver behind a wall gets NO blocked interval, the cp verdict
    // stands for the whole segment, and the constant-width shadow band survives
    // exactly where a wall makes it most visible. The CPU has carried this since
    // the fix-pack (`ArcSkyline::ensure`'s barrier arm); this kernel walked
    // obstacle-index footprints only, so `barr` reached the interval RAYS but
    // could never create an interval. The fixture's scene D — the same wall
    // through the production `types::Barrier` lane — read the pre-fix-pack
    // `current` verdict exactly (0.63 dB vs the CPU's 0.28) until this landed,
    // and reads 0.29 with it.
    //
    // No ray confirmation, mirroring the CPU skyline: a wall arc is kept on the
    // range test alone. `need` is the far end of the segment — nothing beyond it
    // can stand between the receiver and any source point on it.
    {
        double sax = (alon - rlon) * mlon, say = (alat - rlat) * M_LAT;
        double sbx = (blon - rlon) * mlon, sby = (blat - rlat) * M_LAT;
        double need = fmax(sqrt(sax * sax + say * say), sqrt(sbx * sbx + sby * sby));
        for (int wi = 0; wi < nbarr; wi++) {
            const double* w = &barr[wi * BARR_STRIDE];
            if (w[5] > need + BARR_HORIZON_M) break;      // slice is sorted by dist_m
            if (w[4] <= fmax(src_height, 0.0)) continue;  // los_floor_m
            double x0 = (w[1] - rlon) * mlon, y0 = (w[0] - rlat) * M_LAT;
            double x1 = (w[3] - rlon) * mlon, y1 = (w[2] - rlat) * M_LAT;
            double vx = x1 - x0, vy = y1 - y0;
            double vv = vx * vx + vy * vy;
            double tw = (vv > 0.0) ? fmin(fmax(-(x0 * vx + y0 * vy) / vv, 0.0), 1.0) : 0.0;
            double qx = x0 + tw * vx, qy = y0 + tw * vy;
            double near_m = sqrt(qx * qx + qy * qy);
            if (near_m > need || near_m < 1e-6) continue;
            double wa0 = atan2(y0, x0), wa1 = atan2(y1, x1);
            double wr0 = wrap_pi_d(wa0 - base);
            double wr1 = wr0 + wrap_pi_d(wa1 - wa0);
            double wst = 0.0, wen = -1.0;
            if (!arc_clip_span(fmin(wr0, wr1), fmax(wr0, wr1), lo, hi, &wst, &wen)) continue;
            niv = arc_iv_union(iv_s, iv_e, iv_near, iv_h, iv_key, niv, wst, wen,
                               (float)near_m, (float)w[4], &iv_overflow);
        }
    }
    // ARC_MAX_MERGED overflow: `arc_iv_union` DROPS the arc it cannot place
    // (`if (niv >= ARC_MAX_MERGED) { *overflow += 1; return niv; }`), so every
    // one of these UNDER-screens — a direction that is genuinely blocked is
    // painted clear. That is the direction of the error; an earlier revision of
    // this comment claimed the opposite (a merge-on-overflow fallback that
    // over-screens), which is the shape that cost 15.0 dB as ARC_MAX_IV and is
    // NOT what the code above does.
    //
    // Reported ALWAYS, not only under -DPROF_COUNTERS=1. The counters below are
    // a dev instrument that defaults OFF, so for the ten months this cap has
    // existed every production build read its overflow count as a literal zero
    // and there was no way to tell "never happened" from "not measured" — which
    // is how the strata could raise the drop rate 422x in silence. The
    // per-thread total is folded into the launch-wide fault slot once, at the
    // end of the kernel, so an overflowing tile costs one atomic per pixel and a
    // clean one costs none. That slot is an f32 and therefore saturates once the
    // running total passes ~2^24 times the increment: it is a fault FLAG with an
    // order of magnitude, not a census. The exact census is `arcstat[6]`, under
    // -DPROF_COUNTERS=1, which is where a re-sizing measurement should read it.
    if (iv_overflow > 0) *arc_drops += (unsigned int)iv_overflow;
    if (PROF_COUNTERS && arcstat) {
        arcstat[6] += (float)iv_overflow;
        if (iv_overflow > 0) arcstat[7] += 1.0f;
    }
    if (niv == 0) return;
    // ---- ADMISSION, `arc_screening::arc_screened_attenuation` §1, at the
    // granularity that lane can share: per MERGED arc, geometry only. The arc has
    // to stand in FRONT of the source point its own centre picks out on the
    // segment, and not under the receiver's feet. Nothing else is asked; the ray
    // march below re-derives δ from the real profile, penumbra branch included.
    //
    // The 1 m floors are the CPU's exactly. `b` is the distance to the
    // DIFFRACTING EDGE, so the floor is a statement about the edge, not the
    // building: δ ≈ h²·d/(2ab) is the parabolic thin-screen expansion, it diverges
    // as b -> 0, and it needs the receiver in the edge's FAR field, b ≫ λ, which at
    // b < 1 m already fails for every band below ~340 Hz — half of the model's
    // 63 Hz .. 8 kHz range. CNOSSOS/ISO 9613-2 place a facade receiver 2 m OFF the
    // facade for the same reason. Dropping such an edge errs toward LESS screening
    // (louder), never toward a phantom shadow.
    //
    // KNOWN RESIDUAL FORK, and it is in the `b`, not in the floors. Edges are
    // clipped to this segment's span BEFORE the union above, so `iv_near` is the
    // minimum over IN-SPAN members only; the CPU builds its skyline per RECEIVER
    // and takes `near = min` over EVERY member of the stratum
    // (`arc_screening.rs`, `insert_merged`). A min over a subset is >= the min
    // over the superset, so for one physical chain this lane's `b` can be LARGER,
    // its `a = d - b` smaller, and it can therefore fail `a > 1` on an arc the
    // reference lane admits — under-screening, i.e. louder, which is the sign of
    // the measured residual on rail 2206/1391 (67.4 % GPU louder). Closing it
    // means carrying the fused group's minimum range across the span clip; that
    // is a collection change and needs its own measurement, so it is recorded
    // rather than patched here.
    {
        int keep = 0;
        for (int i = 0; i < niv; i++) {
            double aslat, aslon;
            if (!arc_source_point(rlat, rlon, mlon, alat, alon, blat, blon,
                                  base + 0.5 * (iv_s[i] + iv_e[i]), &aslat, &aslon))
                continue;
            double adx = (aslon - rlon) * mlon, ady = (aslat - rlat) * M_LAT;
            double d_a = sqrt(adx * adx + ady * ady);
            double b_a = (double)iv_near[i];
            if (d_a - b_a <= 1.0 || b_a < 1.0) continue;
            iv_s[keep] = iv_s[i]; iv_e[keep] = iv_e[i];
            iv_near[keep] = iv_near[i]; iv_h[keep] = iv_h[i]; iv_key[keep] = iv_key[i];
            keep++;
        }
        niv = keep;
    }
    if (niv == 0) return;
    if (ABL_ARC_HULL || ABL_ARC_CONFIRM) {   // DEV ablation stops
        double sink = 0.0;
        for (int i = 0; i < niv; i++) sink += iv_s[i] + iv_e[i];
        if (sink == 1.2345678e300) screen[0] += 1.0f;
        return;
    }
#else
    // DEV ABLATION ONLY, and no longer a CPU mirror: this walk pre-dates the
    // per-arc admission test, so its intervals carry no range or height at all —
    // it cannot express the per-EDGE range the footprint walk above (and the CPU
    // skyline) is built on. Keep it for what it was kept for, pricing the area
    // query against the footprint walk; do not read its field as a reference.
    //
    // 1. One angular hull per footprint over the (receiver, start, end) triangle
    //    bbox — the AREA sibling of the cp ray's crossings query: the cp ray's
    //    own hits would miss exactly the obstacles screening the segment's other
    //    end. Bbox, not exact triangle (one grid walk; a false positive costs one
    //    angular clip that yields no interval). Each edge contributes the SHORT
    //    arc between its endpoint azimuths, so an edge passing behind the
    //    receiver stays a narrow arc near ±π instead of wrapping the circle.
    double hull_lo[ARC_MAX_IV], hull_hi[ARC_MAX_IV];
    unsigned long long hull_key[ARC_MAX_IV];
    int nh = 0;
    double q_min_lat = fmin(fmin(rlat, alat), blat), q_max_lat = fmax(fmax(rlat, alat), blat);
    double q_min_lon = fmin(fmin(rlon, alon), blon), q_max_lon = fmax(fmax(rlon, alon), blon);
    int n_idx = (int)obst[0];
    const double* metas = (const double*)obst[1];
    const unsigned int* starts = (const unsigned int*)obst[2];
    const unsigned int* refs = (const unsigned int*)obst[3];
    const float* edges = (const float*)obst[4];
    const unsigned int* eids = (const unsigned int*)obst[6];
    for (int gi = 0; gi < n_idx; gi++) {
        const double* m = &metas[gi * OBST_META_STRIDE];
        double imlon = m[2], cell = m[3], minx = m[4], miny = m[5];
        int cols_i = (int)m[6], rows_i = (int)m[7];
        size_t soff = (size_t)m[8], roff = (size_t)m[9], eoff = (size_t)m[10];
        // ObstacleIndex::to_local of the two bbox corners, then the same
        // floor/clamp cell range `append_edges_in_bbox` walks.
        double xa = (q_min_lon - m[1]) * imlon, xb = (q_max_lon - m[1]) * imlon;
        double ya = (q_min_lat - m[0]) * M_LAT, yb = (q_max_lat - m[0]) * M_LAT;
        double inv_cell = 1.0 / cell;
        double cx0d = floor((fmin(xa, xb) - minx) * inv_cell);
        double cx1d = floor((fmax(xa, xb) - minx) * inv_cell);
        double cy0d = floor((fmin(ya, yb) - miny) * inv_cell);
        double cy1d = floor((fmax(ya, yb) - miny) * inv_cell);
        if (cx1d < 0.0 || cx0d > (double)(cols_i - 1)) continue;
        if (cy1d < 0.0 || cy0d > (double)(rows_i - 1)) continue;
        int cx0 = (int)fmax(cx0d, 0.0), cx1 = (int)fmin(cx1d, (double)(cols_i - 1));
        int cy0 = (int)fmax(cy0d, 0.0), cy1 = (int)fmin(cy1d, (double)(rows_i - 1));
        for (int cy = cy0; cy <= cy1; cy++) {
            size_t row = (size_t)cy * (size_t)cols_i;
            for (int cx = cx0; cx <= cx1; cx++) {
                unsigned int elo = starts[soff + row + cx], ehi = starts[soff + row + cx + 1];
                for (unsigned int k = elo; k < ehi; k++) {
                    unsigned int eref = refs[roff + k];
                    const float* e = &edges[(eoff + eref) * 5];
                    // ObstacleIndex::to_lat_lon — the set concatenates per-cell
                    // indexes that do not share an origin, so hulls are built in
                    // GEOGRAPHIC coords.
                    double lat0 = m[0] + (double)e[1] / M_LAT, lon0 = m[1] + (double)e[0] / imlon;
                    double lat1 = m[0] + (double)e[3] / M_LAT, lon1 = m[1] + (double)e[2] / imlon;
                    double a0 = arc_az(rlat, rlon, mlon, lat0, lon0);
                    double a1 = arc_az(rlat, rlon, mlon, lat1, lon1);
                    double r0 = wrap_pi_d(a0 - base);
                    double r1 = r0 + wrap_pi_d(a1 - a0);
                    // (gi, id) — never `id` alone: ids are dense PER INDEX, so
                    // grouping by id would fuse two buildings from different
                    // cells into one over-wide hull. 64-bit so neither half is
                    // ever truncated.
                    unsigned long long key = ((unsigned long long)gi << 32) | eids[eoff + eref];
                    int slot = -1;
                    for (int h = 0; h < nh; h++) if (hull_key[h] == key) { slot = h; break; }
                    if (slot < 0) {
                        if (nh >= ARC_MAX_IV) continue;   // capacity — see ARC_MAX_IV
                        slot = nh++;
                        hull_lo[slot] = 1e300; hull_hi[slot] = -1e300; hull_key[slot] = key;
                    }
                    hull_lo[slot] = fmin(hull_lo[slot], fmin(r0, r1));
                    hull_hi[slot] = fmax(hull_hi[slot], fmax(r0, r1));
                }
            }
        }
    }
    if (nh == 0) return;
    if (ABL_ARC_HULL) {
        // DEV ablation stop: consume the hull so the whole loop above cannot be
        // dead-code-eliminated (the compiler cannot prove this sum never hits
        // the sentinel), then return without changing `screen`.
        double sink = 0.0;
        for (int h = 0; h < nh; h++) sink += hull_lo[h] + hull_hi[h];
        if (sink == 1.2345678e300) screen[0] += 1.0f;
        return;
    }

    // Clip each hull to the span. A hull can sit one turn away from it (base
    // near ±π), hence the three shifts.
    double iv_s[ARC_MAX_IV], iv_e[ARC_MAX_IV];
    unsigned int iv_k[ARC_MAX_IV];
    int niv = 0;
    for (int h = 0; h < nh; h++) {
        for (int sh = 0; sh < 3; sh++) {
            double shift = sh == 0 ? 0.0 : (sh == 1 ? TAU_D : -TAU_D);
            double st = fmax(hull_lo[h] + shift, lo);
            double en = fmin(hull_hi[h] + shift, hi);
            if (en > st) {
                // The confirmation matches on the ID alone, exactly as the CPU
                // does — the low half of the key.
                iv_s[niv] = st; iv_e[niv] = en; iv_k[niv] = (unsigned int)hull_key[h]; niv++;
                break;
            }
        }
    }
    if (niv == 0) return;

    // 2. Confirm each interval: the ray to its centre must actually CROSS that
    //    obstacle. Kills hulls of footprints that sit beyond the segment or
    //    behind the receiver — the projection alone cannot tell.
    int kept = 0;
    for (int i = 0; i < niv; i++) {
        double slat, slon;
        if (!arc_source_point(rlat, rlon, mlon, alat, alon, blat, blon,
                              base + 0.5 * (iv_s[i] + iv_e[i]), &slat, &slon)) continue;
        if (!arc_ray_hits_id(obst, slat, slon, rlat, rlon, iv_k[i])) continue;
        iv_s[kept] = iv_s[i]; iv_e[kept] = iv_e[i]; iv_k[kept] = iv_k[i]; kept++;
    }
    niv = kept;
    if (niv == 0) return;
    if (ABL_ARC_CONFIRM) {   // DEV ablation stop after the confirmation walk
        double sink = 0.0;
        for (int i = 0; i < niv; i++) sink += iv_s[i] + iv_e[i];
        if (sink == 1.2345678e300) screen[0] += 1.0f;
        return;
    }

    // 3. Merge overlaps — two footprints screening the same directions are one
    //    blocked arc, not two fractions. Sorted by start azimuth (insertion sort,
    //    stable, mirroring the CPU's small-slice sort), which also pins the f64
    //    accumulation order below.
    for (int i = 1; i < niv; i++) {
        double s = iv_s[i], e = iv_e[i]; unsigned int k = iv_k[i];
        int j = i - 1;
        while (j >= 0 && iv_s[j] > s) { iv_s[j+1] = iv_s[j]; iv_e[j+1] = iv_e[j]; iv_k[j+1] = iv_k[j]; j--; }
        iv_s[j+1] = s; iv_e[j+1] = e; iv_k[j+1] = k;
    }
    int mg = 0;
    for (int i = 1; i < niv; i++) {
        if (iv_s[i] <= iv_e[mg]) iv_e[mg] = fmax(iv_e[mg], iv_e[i]);
        else { mg++; iv_s[mg] = iv_s[i]; iv_e[mg] = iv_e[i]; }
    }
    niv = mg + 1;
#endif  // ARC_FOOTPRINT_CSR

    // 4. One screening evaluation per (sub-)interval centre, energy-averaged on
    //    the ground/barrier term. An interval wider than ~15° gets three
    //    sub-evaluations — once, no recursion.
    float cp_screen[NB];
    for (int i = 0; i < NB; i++) cp_screen[i] = screen[i];
    double ground[NB];
    for (int i = 0; i < NB; i++) ground[i] = ground_atten_d(i, (double)ground_g);
    double cp_rel = wrap_pi_d(arc_az(rlat, rlon, mlon, cplat, cplon) - base);
    double blocked = 0.0;
    double energy[NB];
    for (int i = 0; i < NB; i++) energy[i] = 0.0;
    // ---- BOUNDARY SPLIT (2026-08-04). `arc_iv_union` fuses only neighbours
    // whose heights agree within ARC_FUSE_HEIGHT_TOL_M — it must, because one
    // (range, height) pair cannot describe a near-and-low and a far-and-tall
    // obstacle at once — so two footprints at different RANGES with materially
    // different heights stay side by side, OVERLAPPING. Averaging that list
    // directly adds `step / span` for EACH of them, so every shared direction is
    // counted twice: `blocked` overruns the fan, the clear remainder is clamped
    // away, and the mean is taken over a total weight > 1.
    //
    // MEASURED (dense rail tile 2206/1391, -DPROF_COUNTERS=1): 24 557 633 of
    // 67 927 893 arc pairs (36.2 %) carried an overlapping list and 22 516 198
    // (33.1 %) drove `blocked` past 1.
    //
    // MERGING the overlap away is not the fix — it was tried: one interval
    // carries one centre ray, and whichever obstacle that ray happens to hit is
    // applied to every direction in the union, including directions where only
    // the other one stands. That is the envelope the fuse tolerance exists to
    // prevent, re-created one step later; P4 read max 15.10 dB / 37 665 cells
    // with it (from 14.94 / 72 552), i.e. half the cells and the same worst
    // pixel.
    //
    // Cut at every arc endpoint and evaluate each COVERED piece instead. The
    // union of the pieces is exactly the union of the arcs, so the blocked
    // fraction is right and nothing is double counted; and each piece gets its
    // OWN centre ray, so in a doubly covered direction the dominant screen wins
    // through the delta ranking already inside ray_path_bands — no "max, not
    // envelope" rule has to be encoded here. `n` arcs yield at most `2n-1`
    // pieces. Walked boundary-by-boundary with no extra array: a thread cannot
    // afford another 2*ARC_MAX_MERGED doubles of local storage, and `niv` is
    // small enough that the O(n) scan per boundary is cheaper than the spill.
    //
    // The CPU reached the same answer from the other side on the same day
    // (`arc_screening`, scenes G/H 3.04 -> 0.63 dB); the fixture's `v4`
    // integrator mirrors THIS code and scores 0.63 dB on scene G against the
    // 33-point reference, exactly matching the CPU.
    //
    // The walk runs the WHOLE fan, `lo` to `hi`, not just the arcs' own hull:
    // an uncovered piece is a CLEAR direction and now takes its own terrain ray
    // (see the clear arm below), so it has to be enumerated like any other.
    double piece_lo = lo;
    while (1) {
        double piece_hi = hi;
        for (int i = 0; i < niv; i++) {
            if (iv_s[i] > piece_lo && iv_s[i] < piece_hi) piece_hi = iv_s[i];
            if (iv_e[i] > piece_lo && iv_e[i] < piece_hi) piece_hi = iv_e[i];
        }
        if (piece_hi <= piece_lo) break;
        double piece_mid = 0.5 * (piece_lo + piece_hi);
        bool covered = false;
        for (int i = 0; i < niv && !covered; i++)
            covered = (iv_s[i] <= piece_mid && piece_mid <= iv_e[i]);
        // ---- CLEAR ARM. CLEAR IS NOT TERRAIN-FREE. These directions used to
        // fall through to the lumped remainder below, which carried the CP
        // RAY's A_terrain — right only over level ground. Over relief the
        // source point at one edge of a 250 m microsegment can stand tens of
        // metres above the one at the other, and the crest between it and the
        // receiver is a different crest in every direction. Measured on
        // `screening_fixture` scene I (the only scene with terrain): 1.51 dB of
        // shadow error against the 99-point reference, the fixture's one breach
        // of the owner's 1.0 dB line; 0.93 dB with this arm. Flat ground is
        // unchanged — every direction returns the cp's own terrain — so the
        // cost is paid only where relief makes it a correction.
        if (!covered) {
            int cparts = (int)ceil((piece_hi - piece_lo) / ARC_ESCALATE_SPAN);
            cparts = min(max(cparts, 1), ARC_ESCALATE_MAX_PARTS);
            double cstep = (piece_hi - piece_lo) / (double)cparts;
            for (int p = 0; p < cparts; p++) {
                double st = piece_lo + (double)p * cstep;
                float cterr[NB];
                for (int j = 0; j < NB; j++) cterr[j] = cp_terr[j];
                double slat, slon;
                if (arc_source_point(rlat, rlon, mlon, alat, alon, blat, blon,
                                     base + st + 0.5 * cstep, &slat, &slon)) {
                    double midr = ((slat + rlat) * 0.5) * (PI_D / 180.0);
                    double dmlon = M_LON_EQ * fmax(cos(midr), 0.01);
                    double ddx = (rlon - slon) * dmlon, ddy = (rlat - slat) * M_LAT;
                    double idist = sqrt(ddx * ddx + ddy * ddy);
                    if (idist >= 1.0) {
                        double isalt = tile_elev(inner, elev, rows, cols, lat_min, lon_min,
                                                 inv, bb, slat, slon) + src_height;
                        ray_terrain_bands(elev, rows, cols, lat_min, lon_min, inv,
                                          slat, slon, isalt, rlat, rlon, ralt, idist,
                                          tprof, ed, cterr);
                    }
                }
                double fraction = cstep / span;
                // `blocked` is now "covered by an evaluation ray", clear pieces
                // included — the trailing remainder below is what neither arm
                // reached, which after this walk is rounding.
                blocked += fraction;
                for (int j = 0; j < NB; j++)
                    energy[j] += fraction * fexp(-arc_gob((double)cterr[j], ground[j], 0.0)
                                                 * LN10 * 0.1);
            }
            piece_lo = piece_hi;
            continue;
        }
        // Split to a fixed angular RESOLUTION, capped — the CPU's rule
        // (ESCALATE_SPAN_RAD / ESCALATE_MAX_PARTS). This kernel took a FIXED
        // 3-way split however wide the piece was, which leaves 57 deg per sample
        // on a receiver walled in by a long wall; worth 0.15 dB on fixture scene
        // E (1.03 -> 0.88) and 0.03 on C, and it was the last fork between the
        // lanes once the boundary split and the wall arm landed.
        int parts = (int)ceil((piece_hi - piece_lo) / ARC_ESCALATE_SPAN);
        parts = min(max(parts, 1), ARC_ESCALATE_MAX_PARTS);
        double step = (piece_hi - piece_lo) / (double)parts;
        for (int p = 0; p < parts; p++) {
            double st = piece_lo + (double)p * step, en = st + step;
            float bands[NB];
            // The terrain the average is TAKEN ON: this piece's own ray, the cp
            // ray's only where no interval ray runs. It was already computed
            // and thrown away — `ray_path_bands` fills it either way — while the
            // increment it is the base of was re-applied on the CP's terrain,
            // so over relief the handback did not reproduce the ray it measured.
            float iterr[NB];
            for (int j = 0; j < NB; j++) iterr[j] = cp_terr[j];
            bool have = false;
            // ±ARC_CP_EPS, mirroring the CPU's CP_AZIMUTH_EPS. A receiver
            // sitting on a segment ENDPOINT — every other pixel of a straight
            // road, since adjacent microsegments share it — puts the cp azimuth
            // exactly on an interval boundary, where the two sides are the same
            // number computed two ways. Without the tolerance the lanes hand
            // that interval to different rays: at cp_rel = st − 5e-10 the CPU
            // contributes its 12 dB cp verdict and CUDA contributes 0.
            if (ABL_ARC_CPEVAL || (cp_rel >= st - ARC_CP_EPS && cp_rel <= en + ARC_CP_EPS)) {
                for (int j = 0; j < NB; j++) bands[j] = cp_screen[j];
                have = true;
            } else {
                double slat, slon;
                if (arc_source_point(rlat, rlon, mlon, alat, alon, blat, blon,
                                     base + 0.5 * (st + en), &slat, &slon)) {
                    // geo::flat_dist
                    double midr = ((slat + rlat) * 0.5) * (PI_D / 180.0);
                    double dmlon = M_LON_EQ * fmax(cos(midr), 0.01);
                    double ddx = (rlon - slon) * dmlon, ddy = (rlat - slat) * M_LAT;
                    double idist = sqrt(ddx * ddx + ddy * ddy);
                    if (idist >= 1.0) {
                        double isalt = tile_elev(inner, elev, rows, cols, lat_min, lon_min,
                                                 inv, bb, slat, slon) + src_height;
                        ray_path_bands(elev, cover, rows, cols, lat_min, lon_min, inv,
                                       slat, slon, isalt, rlat, rlon, ralt, idist,
                                       barr, nbarr, obst, tprof, ed, comp,
                                       bld, forr, imdp, iterr, bands, 0);
                        have = true;
                    }
                }
                if (!have) {
                    for (int j = 0; j < NB; j++) bands[j] = cp_screen[j];
                    for (int j = 0; j < NB; j++) iterr[j] = cp_terr[j];
                }
            }
            double fraction = step / span;
            blocked += fraction;
            for (int j = 0; j < NB; j++)
                energy[j] += fraction * fexp(-arc_gob((double)iterr[j], ground[j], (double)bands[j])
                                             * LN10 * 0.1);
        }
        piece_lo = piece_hi;
    }
    // …plus whatever the walk did not reach. It covers `[lo, hi]` exactly, so
    // this residual is rounding — it is kept so a capped or truncated walk still
    // leaves the energy normalised.
    double clear = fmax(1.0 - blocked, 0.0);
    for (int j = 0; j < NB; j++) {
        double mean_db = -10.0 * log10(fmax(energy[j]
            + clear * fexp(-arc_gob(cp_terr[j], ground[j], 0.0) * LN10 * 0.1), 1e-12));
        // Hand back the increment that reproduces `mean_db` once the caller
        // re-applies max(A_ground, A_terrain + A_screen): the mean is ≥ every
        // input, so the max resolves to A_terrain + screen by construction.
        screen[j] = (float)fmax(mean_db - (double)cp_terr[j], 0.0);
    }
}

// BIN_W is injected by build.rs from noise_gpu::BIN_W, like TPX: it fixes this
// kernel's block geometry, and the HOST launches N_BINS blocks of BIN_W² from
// the Rust constant — a drift makes every lane compute the wrong pixel.
#ifndef BIN_W
#define BIN_W 16
#endif
#define BIN_TILES (TPX / BIN_W)  // 32 bins per axis, 1024 bins per tile

// ── tile-painter/src/byte_stop.rs mirror ─────────────────────────────────────
// A pixel's whole answer is one HM3 byte (`u8 × 0.5 dB`), so it is finished once
// its BYTE is pinned, not once its value is. The caller keeps `kept` = P⁻ (what
// is computed exactly) and `resid` = Σ bound over everything left, and stops the
// moment both ends quantise the same. That is EXACT — it never drops energy that
// could change the output — and it is what replaced the energy-budget skip
// (`skipped ≤ η·kept`) this kernel shipped with, whose verdict depended on the
// source order and admitted up to 10·log10(1.4) = 1.46 dB of uncounted energy.
// Keep in lockstep with byte_stop.rs: the two lanes must agree on the BYTE, and
// they now can even though they walk different tails (this lane in source order,
// the CPU loudest-bound-first — the order is a cost knob, not an answer).
//
// BENCHMARK THIS LANE BEFORE SHIPPING IT — the order is a cost knob and this lane
// has the bad setting. A thread has no room to sort its pixel's pairs, so the walk
// is source order, and source order is what the CPU measured at 8.6-11.0 % of
// pairs skipped instead of 49.9-51.7 % loudest-first (the table on
// `scatter_band::PairBound`), which came out 1.5-1.8× SLOWER than the η rule it
// replaces. The fix does not need per-thread sorting — ordering the SOURCE ARRAY
// once on the host (descending Lden emission, a pixel-independent key) gives every
// thread a near-loudest-first walk for free, and keeps all threads on ONE order so
// the two lanes stay comparable pair for pair. Until then the accuracy is right
// and the speed is not.
#define HM3_QUANT_DB 0.5
#define HM3_NO_DATA_Q (-1)
#define HM3_MAX_Q 254.0
#define BS_F32_U 5.960464477539063e-8
#define BS_ACC_SAFETY 2.0
#define BS_LDEN_SCALE (1.0 / 24.0)

// wire_hm3::quantise_lden on a Lden ENERGY; HM3_NO_DATA_Q where it writes 255.
__device__ __forceinline__ int bs_quant(double e) {
    double x = e * BS_LDEN_SCALE;
    if (!(x > 0.0)) return HM3_NO_DATA_Q;
    double db = 10.0 * log10(x);
    if (!isfinite(db) || db < 0.0) return HM3_NO_DATA_Q;
    double q = round(db / HM3_QUANT_DB);
    return (q >= HM3_MAX_Q) ? (int)HM3_MAX_Q : (int)q;
}

// Relative margin the decision must clear on both ends: the output accumulator
// is f32, so a sequential sum of `npair` terms carries up to (npair+1)·u of
// rounding either way, and widening the interval by that makes the byte-identity
// a proof rather than a hope. It also covers this lane's ONE extra imprecision
// over the CPU's: a thread has no room for the CPU's suffix sums, so `resid` is
// maintained by subtraction, whose ≤ npair·u_f64 (~1e-12) drift sits nine orders
// below this margin.
__device__ __forceinline__ double bs_margin(int npair) {
    return ((double)npair + 1.0) * BS_F32_U * BS_ACC_SAFETY;
}

__device__ __forceinline__ bool bs_decided(double p_lo, double p_hi, double margin) {
    if (!isfinite(p_hi)) return false;   // a broken bound must never read decided
    return bs_quant(p_lo * (1.0 - margin)) == bs_quant(p_hi * (1.0 + margin));
}

// ── One source's contribution to one receiver pixel: geometry → cheap bound →
// cadence ray-march → terrain/screening/ground/veg → max(A_gr,A_bar) combine →
// accumulate 3-period energy (f32) + kept (f64). Shared by `line` (scans all
// sources) and `line_binned_fused` (scans a block's culled chunks). seg/sp point
// at THIS source's 4-tuple, em at its 24 emission bands; e0..e2/kept/resid/npair
// are the caller's per-pixel running state. Returns on reach-cull.
//
// `ub_only` is the byte-stop's CHEAP PASS: price the pair (`resid += ub`, count
// it) and return without marching anything. One function rather than two so the
// geometry the bound is built on cannot drift from the geometry the exact path
// uses — that identity is what makes `ub ≥ exact` true per pair.
__device__ __forceinline__ void line_source(
    const float* elev, const float* inner, const unsigned char* cover,
    int rows, int cols, double lat_min, double lon_min, double inv, const double* bb,
    double rlat, double rlon, double ralt, double refl, bool ub_only,
    const double* seg, const double* sp, const float* em,
    const double* barr, int nbarr, const unsigned long long* obst,
    double* tprof, double* ed, double* comp,
    unsigned char* bld, unsigned char* forr, unsigned char* imdp,
    float& e0, float& e1, float& e2, double& kept, double& resid, int& npair,
    float* arcstat,
    unsigned int* hc_key, double* hc_lo, double* hc_hi, unsigned int* arc_drops)
{
    double dend, dperp, cplat, cplon, frac;
    p2s(rlat, rlon, seg[0], seg[1], seg[2], seg[3], &dend, &dperp, &cplat, &cplon, &frac);
    if (dend > sp[1]) return;   // exact per-pixel reach cull (matches scatter_band)
    double salt = tile_elev(inner, elev, rows, cols, lat_min, lon_min, inv, bb, cplat, cplon) + sp[2];
    double dz = salt - ralt;
    float dslant = fmaxf((float)sqrt(dend * dend + dz * dz), 1.0f);
    // FLC paired with the DIVERGENCE distance (fix-pack C): perpendicular
    // distance to the segment's INFINITE line + the SIGNED foot fraction, while
    // divergence/atmosphere stay on the endpoint distance.
    float fc = flc_div((float)sp[0], (float)dperp, (float)frac, (float)dend);
    float base = (float)refl + fc - 10.0f * log10f(2.0f * (float)PI_D * dslant);
    float atm_km = dslant / 1000.0f;

    // The pair's cheap UPPER BOUND (f64 — the byte decision needs the precision):
    // free field, no terrain/screening/veg, most favourable ground, so provably
    // ≥ this pair's exact contribution. The per-band Lden weight
    // Σ_p LDEN_W[p]·em[p·8+i] is host-precomputed into sp[4+i] (pack_sources) —
    // same f64 FMA, hoisted off the per-source×receiver hot path. Byte-identical
    // to the in-kernel `LDEN_W[0]*em[i]+…` form.
    double ub = 0.0;
    for (int i = 0; i < NB; i++) {
        float pdb = (base - (float)ALPHA_ATM[i] * atm_km + GROUND_GAIN_UB_F + (float)A_W[i]) * (float)LN10 * 0.1f;
        ub += sp[4 + i] * fexp((double)pdb);
    }
    ub *= UB_SAFETY;
    if (ub_only) { resid += ub; npair++; return; }   // byte-stop cheap pass

    // ONE ray evaluator for the cp ray and every arc-screening interval ray
    // (ray_path_bands): cadence march + barriers + terrain + vector candidate +
    // screening. It OWNS tprof/ed/comp/bld/forr/imdp, so anything read off the
    // cp ray's samples must be taken before the arc pass overwrites them.
    float terr[NB], screen[NB], veg[NB];
    int n = ray_path_bands(elev, cover, rows, cols, lat_min, lon_min, inv,
                           cplat, cplon, salt, rlat, rlon, ralt, dend,
                           barr, nbarr, obst, tprof, ed, comp, bld, forr, imdp,
                           terr, screen, 1);
    // Ground G and vegetation ride the cp ray (they vary slowly along the
    // segment, DECISION 2026-08-03) — read them out BEFORE the arc pass.
    float gimd = path_integral_imd(tprof, imdp, n);
    float ground_g = (sp[3] != 0.0) ? 0.0f : fminf(fmaxf(1.0f - gimd / 100.0f, 0.0f), 1.0f);
    veg_bands(veg_run_length(tprof, forr, n, (float)dend), veg);

    // Arc screening (fix-pack Fix 1): the cp ray's verdict covers only the
    // directions it flies through; the rest of the segment's angular span gets
    // its own evaluation, energy-averaged over the blocked fractions. Vector
    // regions only — a raster-fallback region has no footprints to clip arcs
    // against and keeps today's cp-ray behaviour exactly.
    // Span PRE-GATE, transcendental-free: a segment of length L whose nearest
    // point is `dend` away subtends at most 2·atan(L/2·dend) ≤ L/dend at this
    // receiver. Both quantities are already in hand from p2s, so a pair that
    // provably cannot reach ARC_MIN_SPAN is rejected before the two f64 atan2
    // the span itself costs — measured at 18 s on the dense rail tile, 71 % of
    // the whole pre-fix-pack kernel, paid on every surviving pair. At the
    // default ARC_MIN_SPAN (= the degenerate threshold) this never fires, so the
    // exact kernel is unchanged; it only bites when the bound is raised.
    bool arc_span_possible = sp[0] > dend * (double)ARC_MIN_SPAN;
    if (PROF_COUNTERS && arcstat) {
        arcstat[0] += 1.0f;
        if (arc_span_possible) arcstat[1] += 1.0f;
    }
    // Enter the arc kernel when there is ANYTHING to clip against: vector
    // footprints OR noise walls. Walls arrive in `barr`, not in the obstacle
    // table, so gating on the table alone denied every wall its angular
    // treatment in raster-fallback regions — the shipped behaviour behind the
    // owner's D4/Voznice report. Safe with an empty table: the footprint walk
    // is bounded by `n_idx = obst[0]`, so at 0 it dereferences nothing, and the
    // `nbarr` arm inside runs on its own. Mirrors noise-compute/src/lib.rs
    // `arc_screened_line_segment` (review 2026-08-04).
    bool has_footprints = obst[0] != 0ULL && obst[6] != 0ULL;
    if (!ABL_ARC_OFF && arc_span_possible && (has_footprints || nbarr > 0))
        arc_screen_bands(elev, inner, cover, rows, cols, lat_min, lon_min, inv, bb,
                         rlat, rlon, ralt, seg[0], seg[1], seg[2], seg[3], sp[2],
                         cplat, cplon, terr, ground_g,
                         barr, nbarr, obst, tprof, ed, comp, bld, forr, imdp, screen,
                         hc_key, hc_lo, hc_hi, arcstat, arc_drops);

    float pf[NB];
    for (int i = 0; i < NB; i++) {
        float a_gr = ground_atten_f(i, ground_g);
        float a_bar = terr[i] + screen[i];
        float gob = (a_bar > 0.0f) ? fmaxf(a_gr, a_bar) : a_gr;   // barrier REPLACES ground
        float pdb = (base - (float)ALPHA_ATM[i] * atm_km - gob - veg[i] + (float)A_W[i]) * (float)LN10 * 0.1f;
        pf[i] = (float)fexp((double)pdb);
    }
    double kept_add = 0.0;   // summed over periods then added once (matches scatter_band)
    for (int p = 0; p < 3; p++) {
        double power = 0.0;
        for (int i = 0; i < NB; i++) power += (double)em[p*8 + i] * pf[i];
        if (isfinite(power) && power > 0.0) {
            float pw = (float)power;
            if (p == 0) e0 += pw; else if (p == 1) e1 += pw; else e2 += pw;
            kept_add += power * LDEN_W[p];
        }
    }
    // THE SOUNDNESS INVARIANT (scatter_band asserts it; a device assert would
    // cost a trap frame per pair, so it is stated and enforced by construction:
    // `ub` above is this same geometry with every attenuation term dropped and
    // the ground at its most favourable, times UB_SAFETY).
    kept += kept_add;
    resid -= ub;   // this pair is now known EXACTLY — it leaves the bound
}

// RAIL scatter: free-field + terrain diffraction + building screening + ground +
// vegetation, combined max(A_ground,A_terrain+A_screen) (ISO 9613-2 §7.3.1), with
// the per-pixel BYTE-SPACE STOP. Thread-per-pixel; mirrors scatter_band
// (per-thread kept/resid). Tiled (swizzled) pixel
// mapping (meta[10]=tile width) keeps each block's rays' terrain L2-hot. Scans
// ALL sources (reach cull is inside line_source); `line_binned` is the pre-binned
// variant. Per-period energy in f32 (matching TileAccumulator), kept in f64.
//   meta = [rows, cols, lat_min, lon_min, inv, north, south, west, east,
//           byte_stop_on (the old η slot: 0 = off/exact, non-zero = on),
//           tile_width, nbarr, nsrc, out_slots]  (out_slots = f32 slots the host
//           allocated in `out` — see the `out` LAYOUT block near the top)
//   inner = TPX×TPX tile DEM; cover = halo [building,forest,imd] u8.
//   sp = nsrc×12 {length_m, max_distance_m, source_height_m, bridge, then the 8
//   host-precomputed Lden band energies} — see `line_buffers` in
//   noise-gpu/src/lib.rs, which writes it.
//   barr = nbarr×BARR_STRIDE (6) {start_lat, start_lon, end_lat, end_lon,
//   height_m, dist_m} — this tile's sorted for_tile()
//   barrier slice (nbarr in meta[11]). obst = the 14-slot vector-obstacle
//   pointer table (slot map above `obstacle_best_candidate`; obst[0]==0 ⇒
//   raster mode); nsrc rides in meta[12] because cudarc's tuple launch caps at
//   12 args.
extern "C" __global__ void line(
    const float*  __restrict__ elev,
    const float*  __restrict__ inner,
    const unsigned char* __restrict__ cover,
    const double* __restrict__ meta,
    const double* __restrict__ seg,
    const double* __restrict__ sp,
    const float*  __restrict__ semis,
    const double* __restrict__ rxll,
    const float*  __restrict__ rxar,
    const double* __restrict__ barr,
    const unsigned long long* __restrict__ obst,
    float* __restrict__ out)
{
    int pix = blockIdx.x * blockDim.x + threadIdx.x;
    if (pix >= TPX * TPX) return;
    int rows = (int)meta[0], cols = (int)meta[1];
    double lat_min = meta[2], lon_min = meta[3], inv = meta[4];
    const double* bb = &meta[5];   // north_lat, south_lat, west_lon, east_lon
    // meta[9] used to carry the budget skip's η. It now carries the byte-stop's
    // ON/OFF, and the mapping is deliberate: every host, harness and doc already
    // writes 0 there to mean "exact reference, skip nothing", and that is exactly
    // what a disabled byte-stop is. Non-zero (the shipped 0.40) = stop on. So no
    // host change, and `SURFACE_BUDGET_ETA=0` still means what it always meant.
    bool stop_on = meta[9] != 0.0;
    int nsrc = (int)meta[12];
    // Tiled (swizzled) pixel mapping: consecutive threads fill a 16×16 pixel tile
    // before the next, so each warp/block covers a COMPACT 2D region — its rays to a
    // given source overlap, keeping the terrain halo L2-hot. Row-major decomposition
    // gave each warp a 32-wide stripe whose rays fanned out. Output is
    // identical; only the memory access pattern changes.
    int TW = (int)meta[10], TPR = TPX / TW;   // tile width (swept; must divide TPX)
    int tl = pix / (TW * TW), it = pix % (TW * TW);
    int py = (tl / TPR) * TW + it / TW;
    int pxi = (tl % TPR) * TW + it % TW;
    int opix = py * TPX + pxi;   // actual pixel index (rxar/out are pixel-indexed)
    double rlat = rxll[py], rlon = rxll[TPX + pxi];
    double ralt = rxar[opix * 2], refl = rxar[opix * 2 + 1];
    int nbarr = (int)meta[11];
    float e0 = 0.0f, e1 = 0.0f, e2 = 0.0f;
    double kept = 0.0, resid = 0.0;
    int npair = 0;
    double tprof[MAXT], ed[MAXT], comp[MAXT];
    unsigned char bld[MAXT], forr[MAXT], imdp[MAXT];
    // Optional `out` regions, entered only on the host's declared buffer size —
    // see the `out` LAYOUT block near the top of this file.
    int out_slots = (int)meta[13];
    float* arcstat = (PROF_COUNTERS && out_slots >= OUT_SLOTS_PROF)
                         ? &out[OUT_ARCSTAT_BASE + opix * 8] : (float*)0;
    float* fault = (out_slots > OUT_FAULT_SLOT) ? &out[OUT_FAULT_SLOT] : (float*)0;
    unsigned int arc_drops = 0;
    unsigned int hc_key[ARC_HULL_CACHE ? ARC_HULL_CACHE : 1];
    double hc_lo[ARC_HULL_CACHE ? ARC_HULL_CACHE : 1], hc_hi[ARC_HULL_CACHE ? ARC_HULL_CACHE : 1];
    for (int i = 0; i < (ARC_HULL_CACHE ? ARC_HULL_CACHE : 1); i++) hc_key[i] = 0xFFFFFFFFu;

    // CHEAP PASS: price every pair, so `resid` is a bound over the WHOLE tail
    // before any of it is computed. This is the bound the superseded budget skip
    // already paid on every pair; what it buys now is a certain upper bound.
    for (int s = 0; s < nsrc; s++)
        line_source(elev, inner, cover, rows, cols, lat_min, lon_min, inv, bb,
                    rlat, rlon, ralt, refl, true, &seg[s * 4], &sp[s * 12], &semis[s * 24],
                    barr, nbarr, obst,
                    tprof, ed, comp, bld, forr, imdp, e0, e1, e2, kept, resid, npair, arcstat,
                    hc_key, hc_lo, hc_hi, &arc_drops);
    double margin = bs_margin(npair);
    // THE WALK: exact per pair until [kept, kept+resid] pins one byte. No
    // barriers in this kernel, so a plain break is safe (line_binned_fused below
    // must use a per-thread flag instead).
    for (int s = 0; s < nsrc; s++) {
        if (stop_on && bs_decided(kept, kept + resid, margin)) break;
        line_source(elev, inner, cover, rows, cols, lat_min, lon_min, inv, bb,
                    rlat, rlon, ralt, refl, false, &seg[s * 4], &sp[s * 12], &semis[s * 24],
                    barr, nbarr, obst,
                    tprof, ed, comp, bld, forr, imdp, e0, e1, e2, kept, resid, npair, arcstat,
                    hc_key, hc_lo, hc_hi, &arc_drops);
    }
    out[opix * 3 + 0] = e0;
    out[opix * 3 + 1] = e1;
    out[opix * 3 + 2] = e2;
    if (fault && arc_drops) atomicAdd(fault, (float)arc_drops);
}

// Binned scatter — bins ON the GPU (deletes the CPU build_pixel_bins prep, the
// gpu-surface bottleneck). Same 16×16-block / 256-thread mapping as line_binned, but
// instead of a CPU CSR bin it scans all nsrc sources in 256-source CHUNKS: the 256
// lanes cull one chunk cooperatively (one source per lane) into shared keep[256],
// then REPLAY that chunk in source order, each thread (= its own pixel) calling
// line_source only on survivors. cos=1 block radius (block_reach_ub) is a universal
// upper bound on the p2s block-corner distance ⇒ conservative superset at every
// latitude; the per-pixel cull in line_source stays authoritative; the 0..nsrc
// ordered replay walks the pairs in the same order `line` does ⇒ byte-identical
// to `line`. (The byte-stop no longer NEEDS that order to agree — it drops only
// tails it has proven immaterial — but keeping it means the two kernels remain
// comparable pair for pair, which is how a divergence gets localised.) One cull
// per source (not once per pixel), fixed BIN_W²-byte shared, no atomics.
// See docs/dev/gpu-binning-plan.md.
extern "C" __global__ void __launch_bounds__(BIN_W * BIN_W, 2) line_binned_fused(
    const float*  __restrict__ elev,
    const float*  __restrict__ inner,
    const unsigned char* __restrict__ cover,
    const double* __restrict__ meta,
    const double* __restrict__ seg,
    const double* __restrict__ sp,
    const float*  __restrict__ semis,
    const double* __restrict__ rxll,
    const float*  __restrict__ rxar,
    const double* __restrict__ barr,
    const unsigned long long* __restrict__ obst,
    float* __restrict__ out)
{
    int bid = blockIdx.x, lane = threadIdx.x;
    if (bid >= BIN_TILES * BIN_TILES || lane >= BIN_W * BIN_W) return;
    int rows = (int)meta[0], cols = (int)meta[1];
    double lat_min = meta[2], lon_min = meta[3], inv = meta[4];
    const double* bb = &meta[5];
    bool stop_on = meta[9] != 0.0;   // see `line` for why the η slot carries this
    int nsrc = (int)meta[12];
    int by = bid / BIN_TILES, bx = bid % BIN_TILES;
    int py0 = by * BIN_W, py1 = by * BIN_W + BIN_W - 1;
    int px0 = bx * BIN_W, px1 = bx * BIN_W + BIN_W - 1;
    int py = py0 + lane / BIN_W, pxi = px0 + lane % BIN_W;
    int opix = py * TPX + pxi;
    double rlat = rxll[py], rlon = rxll[TPX + pxi];
    double ralt = rxar[opix * 2], refl = rxar[opix * 2 + 1];
    int nbarr = (int)meta[11];
    float e0 = 0.0f, e1 = 0.0f, e2 = 0.0f;
    double kept = 0.0, resid = 0.0;
    int npair = 0;
    double margin = 0.0;
    bool done = false;
    double tprof[MAXT], ed[MAXT], comp[MAXT];
    unsigned char bld[MAXT], forr[MAXT], imdp[MAXT];
    // Optional `out` regions — see the `out` LAYOUT block near the top.
    int out_slots = (int)meta[13];
    float* arcstat = (PROF_COUNTERS && out_slots >= OUT_SLOTS_PROF)
                         ? &out[OUT_ARCSTAT_BASE + opix * 8] : (float*)0;
    float* fault = (out_slots > OUT_FAULT_SLOT) ? &out[OUT_FAULT_SLOT] : (float*)0;
    unsigned int arc_drops = 0;
    unsigned int hc_key[ARC_HULL_CACHE ? ARC_HULL_CACHE : 1];
    double hc_lo[ARC_HULL_CACHE ? ARC_HULL_CACHE : 1], hc_hi[ARC_HULL_CACHE ? ARC_HULL_CACHE : 1];
    for (int i = 0; i < (ARC_HULL_CACHE ? ARC_HULL_CACHE : 1); i++) hc_key[i] = 0xFFFFFFFFu;

    // Block centre + cos=1 radius UB — once per thread (all lanes agree, no divergence).
    double clat = 0.5 * (rxll[py0] + rxll[py1]);
    double clon = 0.5 * (rxll[TPX + px0] + rxll[TPX + px1]);
    double reach = block_reach_ub(0.5 * fabs(rxll[py1] - rxll[py0]) * M_LAT,
                                  0.5 * fabs(rxll[TPX + px1] - rxll[TPX + px0]));
    __shared__ unsigned char keep[BIN_W * BIN_W];
    // TWO PASSES over the same chunked scan: `pass 0` prices every pair into
    // `resid` (the byte-stop's cheap pass), `pass 1` walks them exactly until the
    // byte is pinned. The chunk cull is cooperative and repeated in both passes —
    // it is one p2s per source per BLOCK, not per pixel.
    //
    // A per-thread `done` flag, NOT a break: the chunk loop carries
    // `__syncthreads()`, so a thread that left it early would strand the rest of
    // its block at the barrier. The flag costs a predicated branch per pair and
    // keeps every lane on the same barrier schedule.
    for (int pass = 0; pass < 2; ++pass) {
        bool ub_only = (pass == 0);
        if (!ub_only) margin = bs_margin(npair);
        for (int base = 0; base < nsrc; base += BIN_W * BIN_W) {
            int s = base + lane;
            keep[lane] = 0;
            if (s < nsrc) {
                double de, dp, cpa, cpo, fr;
                p2s(clat, clon, seg[s * 4], seg[s * 4 + 1], seg[s * 4 + 2], seg[s * 4 + 3],
                    &de, &dp, &cpa, &cpo, &fr);
                keep[lane] = (de <= sp[s * 12 + 1] + reach) ? 1 : 0;
            }
            __syncthreads();
            int chunk_n = min(BIN_W * BIN_W, nsrc - base);
            for (int j = 0; j < chunk_n; ++j) {
                if (!keep[j]) continue;
                if (!ub_only) {
                    if (stop_on && !done && bs_decided(kept, kept + resid, margin)) done = true;
                    if (done) continue;
                }
                line_source(elev, inner, cover, rows, cols, lat_min, lon_min, inv, bb,
                            rlat, rlon, ralt, refl, ub_only, &seg[(base + j) * 4],
                            &sp[(base + j) * 12], &semis[(base + j) * 24],
                            barr, nbarr, obst,
                            tprof, ed, comp, bld, forr, imdp, e0, e1, e2, kept, resid, npair,
                            arcstat, hc_key, hc_lo, hc_hi, &arc_drops);
            }
            __syncthreads();
        }
    }
    out[opix * 3 + 0] = e0;
    out[opix * 3 + 1] = e1;
    out[opix * 3 + 2] = e2;
    if (fault && arc_drops) atomicAdd(fault, (float)arc_drops);
}

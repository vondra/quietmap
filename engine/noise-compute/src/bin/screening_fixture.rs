//! `screening_fixture` — synthetic validation harness for the CNOSSOS
//! screening fix-pack (`0db-private/docs/dev/cnossos-screening-fixpack.md`,
//! §"DECISION 2026-08-03" / §"Validation protocol"; the shipped model is
//! SPEC §3.5c).
//!
//! It answers ONE question with numbers instead of eyeballed tiles: how much
//! does a ONE-RAY-PER-SEGMENT screening evaluation err behind a small
//! obstacle, against a per-segment sampled reference integration of the SAME
//! physics — and how much of that error does arc clipping remove?
//!
//! Five integrators run over the same hardcoded scenes and the same probe
//! grid, all funnelling through the production entry points
//! (`path_effects::terrain_attenuation`, `path_effects::screening_attenuation_with_meta`,
//! `iso9613::propagate_variants_full`) — only the SCREENING EVALUATION differs:
//!
//! `current` — one characteristic-point ray per 250 m microsegment, as
//! `compute::roads::compute_roads` did before the fix-pack: the nearest point
//! on the segment is the `cp`, and that single ray's attenuation is applied to
//! the whole segment's emission.
//!
//! `v3` — the shipped kernel: same one cp ray per microsegment for terrain,
//! ground and vegetation, but screening is arc-clipped
//! (`propagation::arc_screening`) — the segment's angular span at the receiver
//! is split into blocked and clear sub-intervals, each blocked one evaluated on
//! the ray to its own centre azimuth and energy-averaged.
//!
//! `reference33` — each 250 m microsegment is subdivided into 33 sub-segments of
//! ~7.6 m, each propagated as its own line element with its own `cp` /
//! finite-line correction, energy-summed. Line emission is dB per METRE and the
//! finite-line correction is additive in subtended angle (`geo.rs
//! off_end_split_conserves_energy`), so the subdivision is energy-conserving in
//! free field by construction — every difference from `current` is the
//! screening/terrain evaluation POSITION plus the atmospheric absorption's
//! (a whole segment absorbs at its closest point's distance; its sub-segments
//! each at their own, worth ~0.25 dB on scene B).
//! Convergence measured on scene A: 11→33 sub-points move the grid by ≤0.46 dB,
//! 33→99 by ≤0.23 dB, 99→297 by ≤0.12 dB (first order in sub-segment length),
//! while the shadow-region verdict stays 5.8–5.9 dB throughout. Re-measured on
//! the harder stacked scenes G/H (2026-08-04): 33→99 moves them by ≤0.57 dB,
//! mean 0.056 dB, and that 0.57 dB sits entirely on the probe row `x = 145`,
//! 6 m behind the near block row; every other row is ≤0.41 dB. That is the
//! noise floor the G/H verdicts have to clear, and they clear it by 5×.
//!
//! `v4` — the CUDA lane's arc rule, ported to CPU from
//! `engine/noise-gpu/kernels/scatter.cu` (`arc_screen_bands`, :1370) so the
//! same 33-point reference can judge THE RULE rather than the lane. The two
//! lanes each pass their own gates yet disagree by 14.94 dB on 9 % of a dense
//! Dobříš rail tile, and inference has run out; this integrator is the oracle.
//! It differs from `v3` in the GROUPING UNIT and in four smaller places — see
//! [`gpu_arc_screened_attenuation`]. Fixture-only: it never touches the
//! production kernels, and where CUDA carries fp32 it uses f64, because what is
//! on trial is the rule, not the arithmetic.
//!
//! `v5` — the rule the TILES ship since 2026-08-05
//! (`propagation::seg_sampling`, called here, not re-implemented): the same fan,
//! integrated by UNIFORM angular quadrature instead of skyline-adaptive
//! clipping. `--seg-samples` buckets across the span, one ray at each bucket's
//! centre, `max(A_ground, A_terrain + A_screen)` energy-averaged; default 5, the
//! shipped value. Arc screening inside the buckets follows the tile default —
//! off unless `QM_ARC_MIN_SPAN_DEG` names it — so one env var moves this arm and
//! the tile kernel together.
//!
//! MEASURED, 2026-08-05: the two rules did NOT order the same way on the two
//! metrics that matter, and THIS SUITE is what caught it. On four PRODUCTION
//! TILES against a converged N=9 reference, uniform quadrature was 1.1-1.7×
//! cheaper than arc screening AND 2.6-4.0× lower RMS. On THESE SCENES it was the
//! other way round in the TAIL: probes stand 45-445 m from the source line,
//! where a 250 m microsegment spans up to 2.4 rad and five buckets are 28° wide,
//! and `v5` reached 2.4-7.6 dB max where `v3` holds 0.6-0.9 — while their MEANS
//! were level (0.42-0.56 vs 0.28-0.50). Adaptive quadrature puts its nodes on
//! the geometry's edges; uniform quadrature has to resolve the finest gap
//! everywhere. A tile is mostly far-field pixels, so RMS went to `v5`; these
//! scenes are all near-field, so the tail went to `v3`. `v5` was gated at the
//! owner's limits like the other two and BREACHED them on NINE of the eleven
//! scenes — the finding, not a mis-set limit, and the reason arc screening was
//! not deleted on the strength of the tile measurement alone.
//!
//! FIXED, 2026-08-08 (`seg_sampling::SEG_ARC_MIN_SPAN_RAD`): a BUCKET wider than
//! 3° arc-screens its own sub-span, so the nodes come from the geometry exactly
//! where the fan is too wide for five of them, and nowhere else. `v5`'s worst
//! receiver over the whole suite goes 7.60 → 0.85 dB (arc screening's own worst
//! is 0.93) and its mean 0.305 → 0.262 (arc screening 0.261), for +3.9-8.6 % CPU
//! on the four tiles. It now breaches only scenes C/D at their 0.35 dB limit,
//! where `v3` and `v4` breach identically (0.44-0.48) — a suite-wide floor on the
//! 3 m wall, not this rule's tail. The intermediate attempts, and why raising
//! `--seg-samples` is NOT one of them, are recorded in `seg_sampling`'s module
//! docs; that sweep was run from this binary.
//!
//! MEASURED, 2026-08-04. On scenes A-F, v4 lands within 0.15 dB of v3, and
//! swapping ONLY its interval escalation for the CPU's makes it reproduce v3
//! exactly — so the grouping unit, the ray×polygon confirmation, the hull /
//! silhouette split, the height-tolerant fuse and the per-arc admission test are
//! all equivalent there. Scenes G/H then found the one place they were not:
//! overlapping arcs of materially different height were averaged twice, worth
//! **7.25 dB on G**. Merging them away only halves it (2.53 dB) because one
//! interval carries one ray; the BOUNDARY SPLIT that both lanes now run brings
//! v4 to **0.63 / 0.79 dB on G / H**, level with the CPU's 0.63 / 0.61. The
//! escalation fork is worth ≤1 dB and is still open on the GPU.
//!
//! The scenes carry NO raster obstacles. Buildings arrive through the
//! geodata-v2 vector lane (`obstacle_index::ObstacleSet` exact ray×edge
//! crossings, `ObstacleInput::replace_sample_buildings = true`), so screening is
//! exact and cadence-independent: neither integrator can win or lose on whether
//! the bilateral profile cadence happened to drop a probe inside a 10 m deep box.
//!
//! Scene K (2026-08-04) is the first scene that fails for a reason none of A-J
//! can express, and it was built from a MEASUREMENT on a production tile rather
//! than from a hypothesis: on tile 2206/1391 the CPU's height-only `fusible`
//! chains a receiver's whole skyline into a handful of ~190°-wide arcs carrying
//! the range of the house at the receiver's own facade. A-J cannot see the
//! consequence because they all stand 45-445 m from their source, where a
//! microsegment subtends up to 2.12 rad and the interval escalation splits a
//! fused arc into nine rays whatever the fuse did. Move the source to 3 km, and
//! the span drops to 0.07 rad: one fused arc covers it, escalation returns ONE
//! part, and the entire arc-clipping fix collapses back to the single cp ray it
//! was written to replace. `v3` then reads `current` to the decimal —
//! **1.48 dB shadow / 1.22 dB outside, against 0.20 / 0.17 with `fusible`
//! disabled**, on a reference converged to 0.045 dB. That ratio is what makes
//! scene K a bench: a fuse candidate can be judged on it in a second of CPU.
//!
//! Scenes C and D are the SAME 3 m wall through the two different code paths a
//! barrier can take: C puts it in the obstacle index (a path production never
//! uses for walls), D hands it to the kernel as the `types::Barrier` slice the
//! tiles and the popup actually build (`BarrierData::for_tile` /
//! `source-reader::query`), which `path_effects` §1 intersects with each ray.
//! Two paths, one wall, one answer — the pair is the proof.
//!
//! Deterministic: no RNG, no clock, no I/O beyond stdout. NOT independent of
//! the environment: `v3` and `v5` reach `ArcBounds::from_env` through the
//! production entry points, so `QM_ARC_MIN_SPAN_DEG` MOVES this harness
//! (measured: `--scene a --integrator v3` differs with it at `0`). A
//! reproducible run needs the `QM_ARC_*` levers unset.
//!
//! Usage: the `USAGE` const next to the parser that honours it — one spelling,
//! because the copy that used to live here went stale (it offered neither
//! `v5`, the shipped rule, nor `--seg-samples`).
//!
//! Output: TSV `scene integrator x y lden_db` on stdout, followed by `#`-prefixed
//! summary lines (shadow-region vs outside max |reference33 − X| per integrator,
//! plus each integrator's wall time).

use std::sync::Arc;

use noise_compute::constants::{
    m_per_deg_lon, M_PER_DEG_LAT, M_PER_DEG_LON_EQ, SOURCE_HEIGHT_ROAD,
};
use noise_compute::defaults::WORLD_DEFAULT;
use noise_compute::emission::road;
use noise_compute::periods::compute_lden;
use noise_compute::propagation::arc_screening::{
    arc_screened_attenuation, ArcBounds, ArcScreening, ArcScreeningScratch, ArcSkyline,
};
use noise_compute::propagation::geo;
use noise_compute::propagation::iso9613::{fast_exp_f64, propagate_variants_full, SourceGeometry};
use noise_compute::propagation::obstacle_index::{
    wrap_pi, CellPrune, CrossingCandidate, GpuGridView, ObstacleIndex, ObstacleKind, ObstacleSet,
};
use noise_compute::propagation::path_effects::{
    ground_g_from_profile, screening_attenuation, screening_attenuation_with_meta,
    terrain_attenuation, vegetation_attenuation_path, ObstacleInput,
};
use noise_compute::propagation::seg_sampling::{seg_arc_bounds, SegSampleScratch};
use noise_compute::propagation::PathProfile;
use noise_compute::types::{
    Barrier, PropagationVariants, RasterSampler, Receiver, BARRIER_PATH_HORIZON_M, NUM_BANDS,
};

// ── Scene frame ───────────────────────────────────────────────────────────
// Scenes are authored in local metres (x = east, y = north) around a fixed
// origin and converted with the engine's own flat-earth constants. The
// `ObstacleIndex` builder uses the identical `m_per_deg_lon(origin_lat)` scale,
// so obstacle corners round-trip exactly. `geo::point_to_segment` rescales by
// each segment's OWN mid-latitude instead (≲0.02 % over ±2 km of latitude) —
// the engine's convention, shared by both integrators.

const ORIGIN_LAT: f64 = 50.0;
const ORIGIN_LON: f64 = 14.0;
/// Flat terrain: DEM returns this everywhere, receivers stand on it.
const GROUND_ELEV_M: f64 = 0.0;
/// Mixed ground (mirrors `lib.rs`'s `MockRasters`).
const GROUND_G: f64 = 0.5;

/// Line source: `x = 0`, `y ∈ [LINE_Y_MIN, LINE_Y_MAX]`.
const LINE_Y_MIN: f64 = -2000.0;
const LINE_Y_MAX: f64 = 2000.0;
/// `osm-extract microsegment::split` cap — the length the stripe defect lives on.
const SEGMENT_LEN_M: f64 = 250.0;
const N_SEGMENTS: usize = ((LINE_Y_MAX - LINE_Y_MIN) / SEGMENT_LEN_M) as usize;
/// Sub-points per segment for the reference integrator (fix-pack §Validation).
const REFERENCE_SUBDIVISIONS: usize = 33;

/// Scene A box obstacle: 30 m wide (y), 10 m deep (x), 8 m high.
const BOX_CORNERS_XY: [(f64, f64); 4] = [(30.0, -15.0), (40.0, -15.0), (40.0, 15.0), (30.0, 15.0)];
const BOX_HEIGHT_M: f32 = 8.0;

/// Scene C barrier polyline: a thin 3 m wall parallel to the source at x = 60.
const BARRIER_XY: [(f64, f64); 2] = [(60.0, -150.0), (60.0, 150.0)];
const BARRIER_HEIGHT_M: f32 = 3.0;
/// Scene D splits the SAME wall into `barriers.arrow`-shaped microsegments
/// (the extract caps linear features at 250 m; real walls come in chains).
const BARRIER_SEGMENT_LEN_M: f64 = 60.0;

/// Scenes E/F: a ROW of separate blocks with real gaps between them, plus one
/// L-shaped block. This is the case every earlier scene is structurally blind
/// to: A has one box, C/D one wall, so an arc rule that MERGES arcs across
/// footprints cannot diverge from a per-footprint rule there — with a single
/// footprint there is nothing to merge with. Scene E is where merging first
/// has to describe two obstacles as one, and F narrows the gaps until the arcs
/// touch, so the cost of merging can be read off as a function of gap width.
///
/// Blocks are 30 m wide (y) × 12 m deep (x) × 10 m high at x ∈ [30, 42], the
/// same stand-off as scene A's box, so receiver distances span ~3-400 m over
/// the existing probe grid.
const BLOCK_X0: f64 = 30.0;
const BLOCK_X1: f64 = 42.0;
const BLOCK_W_Y: f64 = 30.0;
const BLOCK_HEIGHT_M: f32 = 10.0;
/// Gap between neighbouring blocks: wide enough in E that a receiver resolves
/// each block under its own arc; narrow in F so the arcs nearly touch.
const BLOCK_GAP_E_M: f64 = 25.0;
const BLOCK_GAP_F_M: f64 = 8.0;
/// Block centres are laid symmetrically about y = 0; the CENTRE one is the
/// L-shape (concave), where a convex angular hull already over-blocks and a
/// merged arc over-blocks further.
const BLOCK_COUNT: usize = 5;

/// Scenes G/H: TWO rows of blocks at DIFFERENT RANGES in the same directions,
/// with materially different heights. This is the case scenes A-F are all
/// structurally blind to, and it is what a real city looks like everywhere.
///
/// Every earlier scene puts its footprints at ONE range (`x ∈ [30, 42]`, or one
/// wall), and gives them all the SAME height. Two consequences, both of which
/// hide a rule fork:
///
/// * with one range, two footprints can only ever be side by side in azimuth —
///   their arcs touch at most, they never OVERLAP;
/// * with one height, `ARC_FUSE_HEIGHT_TOL_M` always passes, so a merge rule
///   keyed on height difference can never take its other branch.
///
/// The CUDA lane's `arc_iv_union` leaves overlapping arcs of materially
/// different height SIDE BY SIDE (fusing them would invent an obstacle that is
/// simultaneously near-and-low and far-and-tall) and then adds `step / span`
/// for each, so directions covered twice are counted twice and the blocked
/// fraction can exceed 1. The CPU drops the heights before merging its clipped
/// intervals, so its interval set is always disjoint. Only depth stacking with
/// mixed heights can tell the two apart.
///
/// The far row (near the SOURCE) is offset by half a pitch so a near block's
/// arc straddles the boundary between two far blocks from most of the probe
/// grid — partial overlap, which is the realistic case; a full overlap would
/// only exercise the degenerate `blocked ≈ 2` end.
///
/// Both rows sit in a GAP of the probe grid (`x ∈ {45, 70, …}`, step 25): a
/// receiver standing INSIDE a footprint is a degenerate configuration, not a
/// test of the arc rule, and the first cut of this scene put the probe row
/// `x = 145` inside blocks spanning `x ∈ [135, 147]` — which alone read 11.71 dB
/// on `v3` while every other row of the same scene stayed under 1.1 dB.
const STACK_NEAR_X0: f64 = 127.0;
const STACK_NEAR_X1: f64 = 139.0;
const STACK_FAR_X0: f64 = 55.0;
const STACK_FAR_X1: f64 = 67.0;
/// Heights: 14 m apart, well past the 3 m fuse tolerance either way round.
const STACK_TALL_M: f32 = 20.0;
const STACK_SHORT_M: f32 = 6.0;
/// Same block footprint and gap as scenes E/F, so the only NEW variable is the
/// second range and the height split.
const STACK_W_Y: f64 = 30.0;
const STACK_GAP_M: f64 = 25.0;
/// The near row is laid symmetrically about `y = 0`; the far row carries one
/// extra block and sits half a pitch off, so it stays symmetric too.
const STACK_NEAR_COUNT: usize = 5;
const STACK_FAR_COUNT: usize = 6;

/// Scene I: scene F's geometry on a HILLSIDE. Scenes A-H are all flat, and flat
/// ground is the one condition under which the two lanes' per-arc admission
/// tests cannot disagree: the CPU merges arcs across footprints whenever their
/// heights agree within `ARC_FUSE_HEIGHT_TOL_M`, takes `near = min` /
/// `height = max`, and resolves the composite's absolute top from ONE elevation
/// sample at its own centre azimuth; the GPU resolves each footprint's top at
/// its own range and azimuth. With every footprint standing on z = 0 those are
/// the same number by construction.
///
/// The row runs along y, so the ground has to vary along Y to give neighbouring
/// blocks different tops — a slope in x would leave the whole row at one
/// elevation and discriminate nothing. F's 8 m gaps (not E's 25 m) because that
/// is where neighbouring arcs actually touch and the CPU actually merges across
/// footprints.
///
/// The tilt is CLAMPED in y to a band around the blocks. An unbounded y-slope
/// would put the source segment at y = 2 km 200 m up the hill and turn a
/// screening scene into a terrain scene; a slope faded in over x instead builds
/// a ridge on every oblique path (measured: 4.81 dB of pure terrain error on
/// `current`, `v3` AND `v4` alike, i.e. no discrimination at all — the first cut
/// of this scene did exactly that). Elevation depending on y ALONE keeps every
/// source→receiver profile monotone in terrain, so nothing diffracts off the
/// hillside itself and the scene measures what it is for. Physically: a road and
/// a block row on a 10 % hillside that levels off past the row's ends.
const SLOPE_PER_M_Y: f64 = 0.1;
const SLOPE_Y_HALF_M: f64 = 150.0;

/// Scene J: the G/H stack with the two rows at the SAME height. That is the one
/// case `fusible` actually fires across footprints at different RANGES — the CPU
/// then describes a near block and a far block as ONE arc carrying the nearer
/// range, and gives that union a single evaluation ray, where the GPU keeps two
/// arcs and the boundary split gives their union up to three. G/H cannot test it
/// (14 m apart, never fusible) and E/F cannot (one range, no overlap).
const STACK_EQUAL_M: f32 = 10.0;

/// Scene K: a VILLAGE of same-height houses spread over three decades of RANGE,
/// with the source a rail-like line 3 km away. This is the geometry the CPU's
/// arc FUSE chain actually lives on, and no scene up to J can hold it.
///
/// WHAT IT REPRODUCES. Measured on the production tile 2206/1391 (rail,
/// receiver 49.838591 / 13.893328): the CPU skyline grows CUMULATIVELY over the
/// whole panorama and over the whole source pass (42 → 137 arcs), and because
/// `fusible` keys on HEIGHT ALONE, every overlapping arc of similar height
/// chains into ONE blocked arc — `near = 1.3 m`, width 1.20-3.96 rad (median
/// 3.38 rad = 194°). 49 % of the 475 accepted arcs at that receiver come from
/// that single chain. The four nearest sources then have a CPU arc over the
/// WHOLE segment span while the other lane covers 21-40 %.
///
/// This scene puts the SAME chain under the fixture (measured here, 2026-08-04,
/// by instrumenting `ArcSkyline`): every one of the 697 probes ends with a
/// merged arc wider than the tile's narrowest (1.25-3.06 rad, median 1.92 rad =
/// 110°), 315 of them carry `near_m < 2 m`, and the skyline collapses from
/// ~2 550 raw edge arcs to a median of SIX. 92.7 % of the (segment, receiver)
/// pairs then get exactly ONE blocked interval and 99.1 % run at
/// `blocked_fraction ≥ 0.999` — the tile's "arc over the whole segment span",
/// on the bench.
///
/// Three ingredients, all absent from A-J:
///
/// * **a house at the receiver's own facade** — [`K_NEAR_GAP_M`] = 1.2 m, just
///   past the `b < 1.0` reject in `arc_screening`'s admission test, so the
///   chain inherits `near = 1.2 m` exactly as the tile does at 1.3 m;
/// * **the same height everywhere** (8/9 m, inside `ARC_FUSE_HEIGHT_TOL_M`) at
///   ranges from 1.2 m to ~600 m, so the fuse fires ACROSS three decades of
///   range rather than between two rows 80 m apart (scene J);
/// * **a FAR source**, which is what turns the chain into an ERROR. At 3 km a
///   250 m microsegment subtends 0.053-0.082 rad, far under `ESCALATE_SPAN_RAD`
///   — so once the chain swallows the span there is exactly ONE blocked
///   interval and, after escalation, exactly ONE evaluation ray: the cp ray.
///   Arc clipping degenerates to the pre-fix-pack verdict it was written to
///   replace, and `v3` reads `current` to the last decimal. Every scene up to J
///   stands 45-445 m from its source, where the span runs to 2.12 rad and the
///   escalation splits a fused interval into up to NINE pieces whatever the
///   fuse did — which is exactly why A-J are structurally blind to this and
///   why J, the one scene that does fuse across ranges, only reaches
///   `blocked_fraction ≥ 0.999` on 40 % of its pairs.
///
/// WHY 3 km AND NOT THE TILE'S 6. Swept 1-6 km with everything else fixed; the
/// scene reproduces the defect over the whole range and the fuse is the ONLY
/// thing that moves it (`v3` with / without `fusible`): 6 km 0.67 / 0.27 dB,
/// 3 km 1.48 / 0.20, 2 km 1.82 / 0.22, 1 km 2.47 / 0.31. It shrinks at 6 km for
/// a structural reason, not a tuning one: a 250 m segment subtends 0.042 rad
/// there, and an obstacle that still bends a 6 km path (`δ ∝ h²/2b`, so within
/// ~100 m of the receiver) subtends TEN TIMES that — nothing can vary inside
/// the span, so the cp ray is very nearly right and there is little for the
/// fuse to destroy. 3 km is the shortest distance that is still "kilometres",
/// keeps `a/b ≥ 5` for every screening obstacle in the scene, and leaves the
/// span 3× under the escalation width.
///
/// Rows sit one per PROBE COLUMN (pitch [`PROBE_X_STEP`]), their east face
/// [`K_NEAR_GAP_M`] west of it, and run from 8 columns WEST of the grid so even
/// the nearest probes have a deep chain in front of them.
const K_SOURCE_X_M: f64 = -3000.0;
/// East face of a house row, west of its probe column: the receiver stands at
/// its own facade. Above 1.0 m on purpose — `arc_screened_attenuation` drops an
/// arc with `near_m < 1.0` outright, and the tile's chain carries 1.3 m.
const K_NEAR_GAP_M: f64 = 1.2;
const K_ROW_DEPTH_M: f64 = 8.0;
/// Row index range, in probe columns (`x = PROBE_X_MIN + 25·r − 1.2`): 25 rows
/// from x = −156 to x = 444, i.e. ranges 1.2 m … 600 m from a probe.
const K_ROW_FIRST: i32 = -8;
const K_ROW_LAST: i32 = 16;
/// Houses along a row: 24 m of wall, 36 m of gap. The gap is what keeps the
/// scene honest — a continuous wall would screen every ray in every lane and
/// discriminate nothing.
const K_HOUSE_W_Y: f64 = 24.0;
const K_HOUSE_PITCH_Y: f64 = 60.0;
/// 13 houses per row reach `|y| ≤ 372 m`, which covers the ±17° source fan of
/// even the farthest probe (`x = 445`, `y = ±200`) out to the last row.
const K_HOUSE_COUNT: i32 = 13;
/// Per-row phase shift along y (m), `(r · 23) mod 60`: the gaps of neighbouring
/// rows must NOT line up, or the scene grows a clear corridor straight to the
/// source and the far rows stop mattering. Deterministic, no RNG.
const K_ROW_PHASE_M: f64 = 23.0;
/// Two heights 1 m apart — different enough that the scene is not a single
/// extruded height, close enough that `ARC_FUSE_HEIGHT_TOL_M` (3 m) fuses every
/// pair. That is the point: the tolerance is the ONLY gate on the chain.
const K_HOUSE_H_A: f32 = 8.0;
const K_HOUSE_H_B: f32 = 9.0;

/// Probe grid: `x ∈ [45, 445] step 25`, `y ∈ [-200, 200] step 10`, 4 m facade height.
const PROBE_X_MIN: f64 = 45.0;
const PROBE_X_MAX: f64 = 445.0;
const PROBE_X_STEP: f64 = 25.0;
const PROBE_Y_MIN: f64 = -200.0;
const PROBE_Y_MAX: f64 = 200.0;
const PROBE_Y_STEP: f64 = 10.0;
const PROBE_HEIGHT_M: f64 = 4.0;

/// Shadow region of the scene-A box, per the fix-pack regression claim
/// ("the ~11 dB stripe there"): directly behind the 30 m obstacle, out to 300 m.
const SHADOW_ABS_Y_MAX: f64 = 25.0;
/// Scenes E/F shadow band: behind the whole ROW of blocks, not one box.
const SHADOW_ABS_Y_MAX_ROW: f64 = 110.0;
/// Scenes G/H shadow band: the far row reaches `|y| ≤ 152.5`, so the band has
/// to clear it or the "outside" control would be scoring shadowed receivers.
const SHADOW_ABS_Y_MAX_STACK: f64 = 160.0;
const SHADOW_X_MIN: f64 = 45.0;
const SHADOW_X_MAX: f64 = 300.0;

/// Posted motorway speed (km/h). Heavy vehicles are capped internally by
/// `HEAVY_SPEED_CAP` inside `road::line_source_emission`.
const MOTORWAY_SPEED_KMH: f64 = 130.0;

/// Scene metres → (lat, lon).
fn to_lat_lon(x_m: f64, y_m: f64) -> (f64, f64) {
    (
        ORIGIN_LAT + y_m / M_PER_DEG_LAT,
        ORIGIN_LON + x_m / m_per_deg_lon(ORIGIN_LAT.to_radians()),
    )
}

/// Raster mock — the minimum that satisfies `RasterSampler`. Building heights
/// are ZERO everywhere on purpose: obstacles enter through the exact
/// vector-crossing lane, never the raster channel.
///
/// Flat for every scene but I, which tilts it along y (see [`SLOPE_PER_M_Y`]).
struct SceneGround {
    slope_per_m_y: f64,
}

impl SceneGround {
    /// Ground at one point of the scene frame.
    fn elev(&self, lat: f64, _lon: f64) -> f64 {
        if self.slope_per_m_y == 0.0 {
            return GROUND_ELEV_M;
        }
        let y_m = (lat - ORIGIN_LAT) * M_PER_DEG_LAT;
        GROUND_ELEV_M + self.slope_per_m_y * y_m.clamp(-SLOPE_Y_HALF_M, SLOPE_Y_HALF_M)
    }
}

impl RasterSampler for SceneGround {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.elev(lat, lon)
    }
    fn building_height(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }
    fn ground_g(&self, _lat: f64, _lon: f64) -> f64 {
        GROUND_G
    }
    fn building_enclosure(&self, _lat: f64, _lon: f64) -> f64 {
        0.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SceneId {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
}

const ALL_SCENES: [SceneId; 11] = [
    SceneId::A,
    SceneId::B,
    SceneId::C,
    SceneId::D,
    SceneId::E,
    SceneId::F,
    SceneId::G,
    SceneId::H,
    SceneId::I,
    SceneId::J,
    SceneId::K,
];

impl SceneId {
    fn label(self) -> &'static str {
        match self {
            SceneId::A => "a",
            SceneId::B => "b",
            SceneId::C => "c",
            SceneId::D => "d",
            SceneId::E => "e",
            SceneId::F => "f",
            SceneId::G => "g",
            SceneId::H => "h",
            SceneId::I => "i",
            SceneId::J => "j",
            SceneId::K => "k",
        }
    }

    /// Scenes E/F shadow the whole row and G/H two rows, so their bands are
    /// wider than scene A's.
    ///
    /// Scene K has no shadow EDGE — its village fills the grid, so every probe
    /// stands behind houses. The band is therefore not a shadow/clear split
    /// there but a near/far one: the `x ≤ 300` clause of [`SHADOW_X_MAX`] puts
    /// the columns with the SHALLOWEST chain in "shadow" and the five deepest
    /// columns (x = 320…445, up to 25 rows of houses in front of them) in
    /// "outside", so the two numbers price the chain at two depths instead of
    /// averaging over one.
    fn shadow_abs_y_max(self) -> f64 {
        match self {
            SceneId::E | SceneId::F | SceneId::I => SHADOW_ABS_Y_MAX_ROW,
            SceneId::G | SceneId::H | SceneId::J | SceneId::K => SHADOW_ABS_Y_MAX_STACK,
            _ => SHADOW_ABS_Y_MAX,
        }
    }

    /// The line source's x (m). Every scene but K puts it at x = 0, i.e.
    /// 45-445 m from the probes; K stands it 3 km away so a microsegment's span
    /// falls well under `ESCALATE_SPAN_RAD` and one fused arc can swallow it
    /// whole, leaving a single evaluation ray (see [`K_SOURCE_X_M`]).
    fn source_x_m(self) -> f64 {
        match self {
            SceneId::K => K_SOURCE_X_M,
            _ => 0.0,
        }
    }

    /// Gap between neighbouring blocks (E/F only).
    fn block_gap_m(self) -> f64 {
        match self {
            SceneId::F | SceneId::I => BLOCK_GAP_F_M,
            _ => BLOCK_GAP_E_M,
        }
    }

    /// Sub-points per 250 m microsegment for the reference integration.
    ///
    /// [`REFERENCE_SUBDIVISIONS`] (33) everywhere it is demonstrably converged.
    /// Scenes I and J are harder — a tilted ground plane and two footprint rows
    /// at different ranges — and at 33 their 33→99 movement is 0.98 / 0.69 dB,
    /// i.e. the SAME SIZE as the error they would be gating. A gate whose
    /// reference is noisier than the effect cannot fail honestly, so they run
    /// at 99.
    ///
    /// 99 is CONVERGED, measured not extrapolated (2026-08-04): refining scene I
    /// to 297 moves the v3 verdict by 0.04 dB (1.05 → 1.01) and scene J by
    /// 0.00 dB. So scene I's residual is a REAL rule error, not reference noise —
    /// an earlier note here estimated the 99-point residual at ~0.33 dB by
    /// first-order extrapolation and was wrong by an order of magnitude. Do not
    /// pay 3× for 297.
    ///
    /// Scene K stays at 33 and is the CHEAPEST reference of the suite to trust:
    /// its source is 3 km away, so a microsegment's sub-elements are nearly
    /// congruent and the quadrature has almost nothing to resolve. Measured
    /// 2026-08-04: 33→99 moves the v3 verdict by 0.03 dB (shadow 1.48 → 1.45)
    /// and 0.01 dB (outside 1.22 → 1.23), and the single worst RECEIVER of the
    /// 697 moves 0.045 dB; 99→297 moves 0.014 dB. The error it gates is 1.5 dB,
    /// i.e. 33× its own noise floor — the widest margin in the suite.
    ///
    /// `(shadow_max_db, outside_max_db)` the shipped rule must stay within.
    /// The owner's line is 1.0 dB anywhere; a scene sits TIGHTER than that only
    /// where its geometry admits no honest slack (B has no obstacle at all;
    /// C/D are one 3 m wall; J's two rows are the same height, so nothing is
    /// approximated by the fuse). Never loosened to fit a measurement — and
    /// scene K is the proof of that rule: it FAILS at 1.48 / 1.22 dB against the
    /// owner's 1.00 / 0.75, which is the whole reason it exists.
    fn limits_db(self) -> (f64, f64) {
        match self {
            SceneId::B => (0.50, 0.50),
            SceneId::C | SceneId::D => (0.35, 0.35),
            SceneId::J => (0.75, 0.50),
            SceneId::A => (1.00, 0.50),
            _ => (1.00, 0.75),
        }
    }

    fn reference_subdivisions(self) -> usize {
        match self {
            SceneId::I | SceneId::J => 99,
            _ => REFERENCE_SUBDIVISIONS,
        }
    }

    /// Ground tilt along y — scene I only (see [`SLOPE_PER_M_Y`]).
    fn ground_slope_y(self) -> f64 {
        match self {
            SceneId::I => SLOPE_PER_M_Y,
            _ => 0.0,
        }
    }

    /// Heights of the two stacked rows (G/H/J only): `(far row, near row)`, where
    /// "far" is the row standing closer to the SOURCE.
    fn stack_heights_m(self) -> (f32, f32) {
        match self {
            SceneId::H => (STACK_TALL_M, STACK_SHORT_M),
            SceneId::J => (STACK_EQUAL_M, STACK_EQUAL_M),
            _ => (STACK_SHORT_M, STACK_TALL_M),
        }
    }

    fn description(self) -> &'static str {
        match self {
            SceneId::A => "line source + 30x10x8 m box building at x=30..40, |y|<=15",
            SceneId::B => "line source only (no obstacle) — no-shadow parity control",
            SceneId::C => "line source + 3 m barrier polyline (60,-150)-(60,150)",
            SceneId::D => {
                "same wall as PRODUCTION types::Barrier microsegments (the barriers.arrow lane)"
            }
            SceneId::E => {
                "5 blocks (4 rect + 1 L-shaped) 30x12x10 m at x=30..42 with 25 m GAPS —                  the first scene where an arc rule has to keep separate footprints separate"
            }
            SceneId::F => {
                "same row with the gaps narrowed to 8 m — where merging starts to cost dB"
            }
            SceneId::G => {
                "TWO STACKED ROWS: 6 m blocks at x=55..67 behind 20 m blocks at x=135..147, \
                 offset half a pitch — the first scene where two footprints at different \
                 RANGES and materially different HEIGHTS overlap in azimuth"
            }
            SceneId::H => {
                "same stack with the heights swapped (20 m near the source, 6 m near the \
                 receiver) — the tall row now screens first"
            }
            SceneId::I => {
                "scene F's row on a HILLSIDE rising 1 m per 10 m along y — the first scene \
                 where two footprints under one merged arc stand on different ground"
            }
            SceneId::J => {
                "the stack with BOTH rows 10 m — equal heights, so the CPU's fuse tolerance \
                 merges a near and a far footprint into one arc carrying the nearer range"
            }
            SceneId::K => {
                "VILLAGE + 3 km source: 25 rows of 8/9 m houses (24 m wall, 36 m gap, \
                 per-row phase) at ranges 1.2 m to 600 m, one row 1.2 m west of every probe \
                 column — the height-only fuse chains them into one arc that swallows a \
                 0.07 rad microsegment span whole, leaving ONE evaluation ray"
            }
        }
    }
}

/// Angular buckets per microsegment for `v5` unless `--seg-samples` says
/// otherwise — the value the tile path ships with
/// (`tile_painter::scatter_band::SEG_SAMPLES_DEFAULT`). Keep the two in step:
/// the point of the arm is that the fixture gates what production paints.
const SEG_SAMPLES_DEFAULT: usize = 5;

/// `v5`'s arc bounds — literally the function
/// `tile_painter::scatter_band::tile_arc_bounds` delegates to, so the arm gates
/// the configuration production paints rather than a sibling of it. Before
/// 2026-08-08 this was a hand-kept copy of that rule and the two could drift;
/// now there is one.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Integrator {
    Reference33,
    Current,
    V3,
    V4,
    V5,
}

const ALL_INTEGRATORS: [Integrator; 5] = [
    Integrator::Reference33,
    Integrator::Current,
    Integrator::V3,
    Integrator::V4,
    Integrator::V5,
];

impl Integrator {
    fn label(self) -> &'static str {
        match self {
            Integrator::Reference33 => "reference33",
            Integrator::Current => "current",
            Integrator::V3 => "v3",
            Integrator::V4 => "v4",
            Integrator::V5 => "v5",
        }
    }

    /// Slot in [`Row::lden`] / the wall-time table.
    fn slot(self) -> usize {
        match self {
            Integrator::Reference33 => 0,
            Integrator::Current => 1,
            Integrator::V3 => 2,
            Integrator::V4 => 3,
            Integrator::V5 => 4,
        }
    }
}

/// Exact vector obstacles for one scene, in the engine's own index.
fn build_obstacles(scene: SceneId) -> ObstacleSet {
    let mut builder = ObstacleIndex::builder(ORIGIN_LAT, ORIGIN_LON);
    match scene {
        SceneId::A => {
            let ring: Vec<(f64, f64)> = BOX_CORNERS_XY
                .iter()
                .map(|&(x, y)| to_lat_lon(x, y))
                .collect();
            builder.add_ring(&ring, BOX_HEIGHT_M, ObstacleKind::Building, 0);
        }
        // B: nothing. D: the wall arrives as a `types::Barrier` slice instead
        // (see `build_barriers`) — the index must stay EMPTY there or the wall
        // would be screened twice over, by two different code paths.
        SceneId::B | SceneId::D => {}
        SceneId::C => {
            let line: Vec<(f64, f64)> = BARRIER_XY.iter().map(|&(x, y)| to_lat_lon(x, y)).collect();
            builder.add_polyline(&line, BARRIER_HEIGHT_M, ObstacleKind::Barrier, 0);
        }
        SceneId::E | SceneId::F | SceneId::I => {
            let pitch = BLOCK_W_Y + scene.block_gap_m();
            let first = -(BLOCK_COUNT as f64 - 1.0) * 0.5 * pitch;
            for i in 0..BLOCK_COUNT {
                let cy = first + i as f64 * pitch;
                let (y0, y1) = (cy - BLOCK_W_Y * 0.5, cy + BLOCK_W_Y * 0.5);
                // Each block is its OWN footprint id — that is the whole point:
                // an arc rule that fuses them answers a different question.
                let ring: Vec<(f64, f64)> = if i == BLOCK_COUNT / 2 {
                    // L-shape: the east half of the northern third is missing, so
                    // the convex hull covers ground the polygon does not and a
                    // ray can pass through the notch without touching material.
                    let ymid = y1 - BLOCK_W_Y / 3.0;
                    let xmid = BLOCK_X0 + (BLOCK_X1 - BLOCK_X0) * 0.5;
                    vec![
                        (BLOCK_X0, y0),
                        (BLOCK_X1, y0),
                        (BLOCK_X1, ymid),
                        (xmid, ymid),
                        (xmid, y1),
                        (BLOCK_X0, y1),
                    ]
                } else {
                    vec![
                        (BLOCK_X0, y0),
                        (BLOCK_X1, y0),
                        (BLOCK_X1, y1),
                        (BLOCK_X0, y1),
                    ]
                }
                .into_iter()
                .map(|(x, y)| to_lat_lon(x, y))
                .collect();
                builder.add_ring(&ring, BLOCK_HEIGHT_M, ObstacleKind::Building, i as u32);
            }
        }
        SceneId::G | SceneId::H | SceneId::J => {
            let (far_h, near_h) = scene.stack_heights_m();
            let pitch = STACK_W_Y + STACK_GAP_M;
            let mut id = 0u32;
            // Two rows, each a plain rectangle run — the ONLY new variable
            // against scene E is the second range and the height split, so
            // nothing else can explain a divergence here.
            for (x0, x1, count, height) in [
                (STACK_FAR_X0, STACK_FAR_X1, STACK_FAR_COUNT, far_h),
                (STACK_NEAR_X0, STACK_NEAR_X1, STACK_NEAR_COUNT, near_h),
            ] {
                let first = -(count as f64 - 1.0) * 0.5 * pitch;
                for i in 0..count {
                    let cy = first + i as f64 * pitch;
                    let (y0, y1) = (cy - STACK_W_Y * 0.5, cy + STACK_W_Y * 0.5);
                    let ring: Vec<(f64, f64)> = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                        .into_iter()
                        .map(|(x, y)| to_lat_lon(x, y))
                        .collect();
                    builder.add_ring(&ring, height, ObstacleKind::Building, id);
                    id += 1;
                }
            }
        }
        SceneId::K => {
            // One row per probe column, its east face K_NEAR_GAP_M west of the
            // column, running 8 columns further west than the grid starts so
            // even x = 45 has a chain in front of it. Each row is a run of
            // houses with real gaps, phase-shifted along y so no two rows'
            // gaps line up.
            let mut id = 0u32;
            for r in K_ROW_FIRST..=K_ROW_LAST {
                let east = PROBE_X_MIN + r as f64 * PROBE_X_STEP - K_NEAR_GAP_M;
                let west = east - K_ROW_DEPTH_M;
                let phase = ((r - K_ROW_FIRST) as f64 * K_ROW_PHASE_M) % K_HOUSE_PITCH_Y;
                let first = -(K_HOUSE_COUNT as f64 - 1.0) * 0.5 * K_HOUSE_PITCH_Y + phase;
                for i in 0..K_HOUSE_COUNT {
                    let cy = first + i as f64 * K_HOUSE_PITCH_Y;
                    let (y0, y1) = (cy - K_HOUSE_W_Y * 0.5, cy + K_HOUSE_W_Y * 0.5);
                    let ring: Vec<(f64, f64)> = [(west, y0), (east, y0), (east, y1), (west, y1)]
                        .into_iter()
                        .map(|(x, y)| to_lat_lon(x, y))
                        .collect();
                    // Heights alternate on BOTH axes so a row is not one
                    // extruded slab — still inside the 3 m fuse tolerance.
                    let height = if (r - K_ROW_FIRST + i) % 2 == 0 {
                        K_HOUSE_H_A
                    } else {
                        K_HOUSE_H_B
                    };
                    builder.add_ring(&ring, height, ObstacleKind::Building, id);
                    id += 1;
                }
            }
        }
    }
    ObstacleSet {
        indexes: vec![Arc::new(builder.build())],
    }
}

/// Scene D's wall as PRODUCTION barrier rows: the same polyline chopped into
/// `barriers.arrow`-shaped microsegments, carried by `types::Barrier` exactly
/// as `BarrierData::for_tile` / the popup's per-hex merge hand them to the
/// kernel. This is the path the tiles and the popup actually run for walls —
/// scene C's `ObstacleIndex` polyline is a DIFFERENT code path (production
/// never puts a barrier in that index), so the pair is what proves the two
/// agree.
fn build_barriers(scene: SceneId) -> Vec<Barrier> {
    if scene != SceneId::D {
        return Vec::new();
    }
    let (x, y0, y1) = (BARRIER_XY[0].0, BARRIER_XY[0].1, BARRIER_XY[1].1);
    let n = ((y1 - y0) / BARRIER_SEGMENT_LEN_M).round() as usize;
    (0..n)
        .map(|i| {
            let a = y0 + i as f64 * BARRIER_SEGMENT_LEN_M;
            let b = a + BARRIER_SEGMENT_LEN_M;
            let (start_lat, start_lon) = to_lat_lon(x, a);
            let (end_lat, end_lon) = to_lat_lon(x, b);
            Barrier {
                osm_id: 1_390_017_809 + i as i64,
                height_m: BARRIER_HEIGHT_M,
                start_lat,
                start_lon,
                end_lat,
                end_lon,
                dist_m: 0.0, // filled per receiver (popup semantics)
            }
        })
        .collect()
}

/// Per-period line-source emission (dB/m per octave band) for a generic
/// motorway: `defaults::WORLD_DEFAULT[0]` (30k AADT — 21600 light / 2400 medium
/// / 5700 heavy / 300 moto) at 130 km/h on dense asphalt, split by
/// `road::TIME_DIST_MOTORWAY`. The two calls are verbatim the emission pair in
/// `compute::roads::compute_roads`'s period loop.
fn motorway_emission() -> [[f64; NUM_BANDS]; 3] {
    let (light, medium, heavy, moto) = WORLD_DEFAULT[0];
    let time_dist = road::TIME_DIST_MOTORWAY;
    let mut out = [[0.0f64; NUM_BANDS]; 3];
    for (slot, (pct, hours)) in out.iter_mut().zip([
        (time_dist.day_pct, 12.0),
        (time_dist.evening_pct, 4.0),
        (time_dist.night_pct, 8.0),
    ]) {
        let flows =
            road::build_period_flows(light, medium, heavy, moto, MOTORWAY_SPEED_KMH, pct, hours);
        *slot = road::line_source_emission(&flows, 0.0);
    }
    out
}

/// One receiver's worth of engine state: scene inputs plus the two scratch
/// buffers the hot kernels amortise across rays.
struct Probe<'a> {
    rasters: &'a SceneGround,
    obstacles: &'a ObstacleSet,
    emission: &'a [[f64; NUM_BANDS]; 3],
    profile: PathProfile,
    candidates: Vec<CrossingCandidate>,
    /// Scene D's wall rows, geometry only (`dist_m` unset).
    barriers_scene: &'a [Barrier],
    /// …and the per-receiver slice the kernel actually sees: exact
    /// receiver→midpoint `dist_m`, ascending — the `types::Barrier` contract
    /// as the popup lane builds it.
    barriers: Vec<Barrier>,
    /// Arc-screening buffers, mirroring `compute_roads`' own locals.
    skyline: ArcSkyline,
    arc: ArcScreeningScratch,
    bounds: ArcBounds,
    /// v4: the scene's obstacles regrouped per FOOTPRINT, the unit the CUDA
    /// lane's arc rule works in (`noise_gpu::footprint_csr_for_index`).
    footprints: &'a [FootprintIndex],
    /// Sub-points per microsegment for `reference33` on THIS scene.
    reference_subdivisions: usize,
    /// The line source's x in scene metres — 0 everywhere but scene K, which
    /// stands it 6 km west (see [`SceneId::source_x_m`]).
    source_x_m: f64,
    /// v4: its own ray buffers — the cp `profile` above is still read after the
    /// screening call (`vegetation_attenuation_path`), so an interval ray must
    /// not overwrite it.
    v4: GpuArcScratch,
    /// v5: the quadrature's own bucket-ray buffers, for the same reason.
    seg: SegSampleScratch,
    /// v5: angular buckets per microsegment (`--seg-samples`, default the
    /// shipped [`SEG_SAMPLES_DEFAULT`]).
    seg_samples: usize,
    /// v5's arc bounds, from the same [`seg_arc_bounds`] the tile path calls:
    /// arc screening inside a bucket WIDER than
    /// `seg_sampling::SEG_ARC_MIN_SPAN_RAD`, none inside a narrower one, and
    /// `QM_ARC_MIN_SPAN_DEG` moves the tile path and this arm together — which
    /// is the only way the arm can gate what production
    /// paints.
    seg_bounds: ArcBounds,
}

impl Probe<'_> {
    /// Received energy per period (day/evening/night) for ONE straight source
    /// element — a whole 250 m microsegment under `current`, one of its 33
    /// sub-segments under `reference33`.
    ///
    /// The body mirrors the per-segment block of `compute::roads::compute_roads`
    /// one-for-one: closest point → slant distance → finite-line correction →
    /// unified path profile → ground / terrain / screening / vegetation →
    /// `propagate_variants_full`. The audibility cull and the free-field
    /// early-exit are deliberately NOT applied: at ≤ 2 km a motorway is audible
    /// everywhere on this grid, and skipping them keeps the two integrators
    /// bit-comparable at the far tail.
    fn element_energy(
        &mut self,
        receiver: &Receiver,
        integrator: Integrator,
        reflection_db: f64,
        a_xy: (f64, f64),
        b_xy: (f64, f64),
        length_m: f64,
    ) -> [f64; 3] {
        let (a_lat, a_lon) = to_lat_lon(a_xy.0, a_xy.1);
        let (b_lat, b_lon) = to_lat_lon(b_xy.0, b_xy.1);
        let pts =
            geo::point_to_segment_full(receiver.lat, receiver.lon, a_lat, a_lon, b_lat, b_lon);
        let (dist_m, cp_lat, cp_lon) = (pts.d_endpoint_m, pts.cp_lat, pts.cp_lon);

        let src_elev = self.rasters.elevation(cp_lat, cp_lon);
        let src_alt = src_elev + SOURCE_HEIGHT_ROAD;
        let rcv_alt = receiver.altitude_m();
        let d_slant = geo::slant_dist(dist_m, src_alt, rcv_alt);
        if d_slant < 1.0 {
            return [0.0; 3];
        }
        let flc = geo::finite_line_correction_for_divergence(
            length_m,
            pts.d_perp_m,
            pts.fraction,
            dist_m,
        );

        self.rasters.build_path_profile(
            cp_lat,
            cp_lon,
            receiver.lat,
            receiver.lon,
            dist_m,
            &mut self.profile,
        );
        let ground_g = ground_g_from_profile(&self.profile);
        let terrain = terrain_attenuation(&mut self.profile, src_alt, rcv_alt);

        self.obstacles.crossings_pruned(
            cp_lat,
            cp_lon,
            receiver.lat,
            receiver.lon,
            &noise_compute::propagation::obstacle_index::CellPrune::for_profile(
                &self.profile,
                src_alt,
                rcv_alt,
            ),
            &mut self.candidates,
        );
        let obstacle_input = ObstacleInput {
            candidates: &self.candidates,
            replace_sample_buildings: true,
        };
        let (cp_screening, _trace) = screening_attenuation_with_meta(
            &mut self.profile,
            &self.barriers,
            obstacle_input,
            src_alt,
            rcv_alt,
            0.0, // roads: no self-screening exclusion radius
            &terrain,
        );
        // The one line `compute_roads` adds for the fix: the cp verdict covers
        // only the directions that ray flies through.
        let arc_query = ArcScreening {
            receiver_lat: receiver.lat,
            receiver_lon: receiver.lon,
            receiver_alt_m: rcv_alt,
            start_lat: a_lat,
            start_lon: a_lon,
            end_lat: b_lat,
            end_lon: b_lon,
            source_height_m: SOURCE_HEIGHT_ROAD,
            cp_lat,
            cp_lon,
            src_alt_m: src_alt,
            cp_screening: &cp_screening,
            cp_terrain: &terrain,
            ground_g,
            barriers: &self.barriers,
            obstacles: self.obstacles,
            length_m,
            dist_m,
            exclusion_radius_m: 0.0,
            bounds: self.bounds,
        };
        let screening = match integrator {
            // v5 — THE SHIPPED TILE RULE (`propagation::seg_sampling`, which is
            // the production code itself, not a port): the microsegment's
            // angular span at this receiver is split into `seg_samples` equal
            // buckets, each evaluated on the exact source point at its centre
            // azimuth, and `max(A_ground, A_terrain + A_screen)` energy-averaged
            // over them.
            //
            // It comes back here through the SAME non-negative increment channel
            // v3 uses (`(gob - A_terrain).max(0)`), because that is what
            // `propagate_variants_full` takes — so on hard ground a fan blocked
            // under ~50 % averages to a net boost this channel cannot carry, and
            // both arms lose it identically. The TILE path takes the composite
            // directly and has no such loss, which makes this arm a LOWER BOUND
            // on what production gets, and an apples-to-apples comparison
            // against v3.
            Integrator::V5 => {
                let seg_query = ArcScreening {
                    bounds: self.seg_bounds,
                    ..arc_query
                };
                match noise_compute::propagation::seg_sampling::sampled_gob_bands(
                    &seg_query,
                    self.rasters,
                    self.seg_samples,
                    &mut self.skyline,
                    &mut self.seg,
                ) {
                    Some((gob, _cost)) => {
                        let mut out = [0.0f64; NUM_BANDS];
                        for (b, o) in out.iter_mut().enumerate() {
                            *o = (gob[b] - terrain[b]).max(0.0);
                        }
                        out
                    }
                    None => cp_screening,
                }
            }
            Integrator::V3
                if noise_compute::propagation::arc_screening::segment_can_span(
                    length_m,
                    dist_m,
                    self.bounds,
                ) =>
            {
                arc_screened_attenuation(&arc_query, self.rasters, &mut self.skyline, &mut self.arc)
            }
            // v4 carries the GPU's own span gate INSIDE the rule (`ARC_MIN_SPAN`,
            // scatter.cu:1386) — the kernel has no `segment_can_span` pre-gate,
            // and the two thresholds are the same 0.01 rad.
            Integrator::V4 => gpu_arc_screened_attenuation(
                &arc_query,
                self.rasters,
                self.footprints,
                &mut self.v4,
            ),
            _ => cp_screening,
        };
        let vegetation = vegetation_attenuation_path(&self.profile);

        let mut energy = [0.0f64; 3];
        for (slot, emission) in energy.iter_mut().zip(self.emission.iter()) {
            let variants = propagate_variants_full(
                emission,
                d_slant,
                SourceGeometry::Line,
                ground_g,
                &terrain,
                &screening,
                &vegetation,
                reflection_db,
                flc,
            );
            *slot = variants.full_energy;
        }
        energy
    }

    /// Rebuild the per-receiver barrier slice: exact receiver→midpoint
    /// distance, sorted ascending — the `types::Barrier` contract as
    /// `source-reader::query` builds it for one popup receiver.
    fn prepare_barriers(&mut self, receiver: &Receiver) {
        self.barriers.clear();
        self.barriers.extend(self.barriers_scene.iter().map(|b| {
            let (mid_lat, mid_lon) = b.midpoint();
            Barrier {
                dist_m: geo::flat_dist(receiver.lat, receiver.lon, mid_lat, mid_lon),
                ..*b
            }
        }));
        self.barriers
            .sort_unstable_by(|a, b| a.dist_m.partial_cmp(&b.dist_m).unwrap());
    }

    /// Lden at one receiver from the whole line source under one integrator.
    fn lden(&mut self, receiver: &Receiver, integrator: Integrator) -> f64 {
        let reflection_db = self.rasters.building_enclosure(receiver.lat, receiver.lon);
        let subdivisions = self.reference_subdivisions;
        let sub_len_m = SEGMENT_LEN_M / subdivisions as f64;
        let sx = self.source_x_m;
        let mut energy = [0.0f64; 3];

        for seg in 0..N_SEGMENTS {
            let y0 = LINE_Y_MIN + seg as f64 * SEGMENT_LEN_M;
            match integrator {
                Integrator::Current | Integrator::V3 | Integrator::V4 | Integrator::V5 => {
                    let e = self.element_energy(
                        receiver,
                        integrator,
                        reflection_db,
                        (sx, y0),
                        (sx, y0 + SEGMENT_LEN_M),
                        SEGMENT_LEN_M,
                    );
                    for (acc, add) in energy.iter_mut().zip(e) {
                        *acc += add;
                    }
                }
                Integrator::Reference33 => {
                    for sub in 0..subdivisions {
                        let sy0 = y0 + sub as f64 * sub_len_m;
                        let e = self.element_energy(
                            receiver,
                            integrator,
                            reflection_db,
                            (sx, sy0),
                            (sx, sy0 + sub_len_m),
                            sub_len_m,
                        );
                        for (acc, add) in energy.iter_mut().zip(e) {
                            *acc += add;
                        }
                    }
                }
            }
        }

        compute_lden(
            PropagationVariants::to_db(energy[0]),
            PropagationVariants::to_db(energy[1]),
            PropagationVariants::to_db(energy[2]),
        )
    }
}

// ── v4: the CUDA lane's arc rule as it stood BEFORE 2026-08-09, ported ────
//
// STALE ON PURPOSE, and it must not be read as "what the kernel does" any more.
// The kernel now emits one arc per obstacle EDGE for every footprint, each arc
// carrying its OWN nearest range and its own edge height — the rule
// `ObstacleIndex::skyline_arcs_within` follows. What this port reproduces is the
// PRE-FIX rule: one range for the whole footprint (`near_m` below, the min over
// its edges) handed to every arc it emits, and one hull arc for a convex
// footprint. That range then went into the `b < 1.0` admission floor, so any
// footprint with a part within a metre of the receiver was rejected WHOLE —
// measured at 4469 cells >0.5 dB and 17.63 dB max against the CPU lane on
// 2206/1391, of which the per-edge rule leaves 2298 / 5.91 dB.
//
// Porting it forward WAS two edits — move the `near_m` computation into the emit
// loop (per edge, from that edge alone) and drop the `convex` branch so
// `n_emit` is always `k1 - k0`. As of 2026-08-09 it is FOUR: the kernel also
// dropped the per-edge δ prefilter (penumbra + grazing), the ray×polygon
// confirmation, and the per-edge elevation sample that fed them, and it moved the
// surviving 1 m geometry floors onto the MERGED arc. Both of those are in the
// REPRODUCED list below and neither is in the kernel any more.
//
// So the gap this port has to close is now wider than when it was written, and
// the measured consequence is that `v4` is currently BLIND to the change that
// widened it: on `--scene all`, this commit moved 25 of 7667 `v3` rows and 0 of
// 7667 `v4` rows. Every verdict recorded in this file's comments was scored on
// the rule as written, so a port has to be re-scored against `reference33` in the
// same commit, not assumed.
//
// Authority for the pre-fix rule: `engine/noise-gpu/kernels/scatter.cu` —
// `arc_screen_bands` in its then-shipped configuration (`ARC_FOOTPRINT_CSR = 1`,
// `ARC_TRI_WALK = 1`, `ARC_EDGE_UNION = 1`, `ARC_HULL_CACHE = 0`,
// `ARC_AZ_F32 = 0`) — plus the host grouping it consumed,
// `noise_gpu::footprint_csr_for_index`. Neither `ARC_EDGE_UNION` nor the host's
// convexity flag exists any more (both went with the hull emission on
// 2026-08-09), so the port below is the only place the pre-fix rule still lives —
// which is the point of it, and the reason its own `is_convex_ring` copy stays.
//
// WHY: both lanes pass their own gates and still disagree by 14.94 dB on 9 % of
// a dense Dobříš rail tile. `v3` is the CPU rule scored against `reference33`;
// this is the GPU rule scored against the SAME reference, so the disagreement
// can be attributed to the rule or to the CUDA implementation of it.
//
// REPRODUCED: per-footprint-per-(segment, receiver) grouping; the wedge prune;
// the per-footprint angular hull with its `near_m`; the hull clip against the
// segment's span; the convex-hull / per-edge-silhouette split; the exact
// ray×polygon confirmation against the source point on the segment;
// `arc_iv_union`'s height-tolerant fuse and its capacity behaviour; the per-arc
// admission test; the fixed 1-or-3-way interval split; and the energy average
// over `max(A_ground, A_terrain + A_screen)`.
//
// NOT REPRODUCED, deliberately:
//   * The triangle scanline walk that ENUMERATES footprints (`tri_row_x_span`,
//     :1242) and its back-link dedup. Every confirmation ray runs receiver → a
//     point ON the segment, i.e. a chord of the (receiver, start, end)
//     triangle, so a footprint the cell walk misses lies wholly outside that
//     triangle and cannot be crossed by any of them — the confirmation would
//     reject it. Visiting every footprint is an exactly equivalent superset,
//     and the walk is an acceleration structure, not part of the rule.
//   * fp32. CUDA carries edge coordinates, the footprint bbox, heights and the
//     band arrays in fp32; here they are f64 throughout. The rule is on trial,
//     not the arithmetic.
//   * `ARC_MAX_MERGED` cannot bind on these scenes (5 footprints, cap 32); it
//     is ported anyway so the behaviour is on the record.

/// f32 slots per footprint in [`FootprintIndex::foot_box`]:
/// `(min_x, min_y, max_x, max_y, height_m, convex)` — what
/// `noise_gpu::FOOT_BOX_STRIDE` was before the per-edge arcs took its last two
/// slots away, since the pre-fix rule needs both.
const FOOT_BOX_STRIDE: usize = 6;
/// `ARC_MIN_SPAN` (scatter.cu:163) = `ArcBounds::min_span_rad`.
const GPU_ARC_MIN_SPAN: f64 = 0.01;
/// `ARC_FUSE_HEIGHT_TOL_M` (scatter.cu) = `arc_screening::ARC_FUSE_HEIGHT_TOL_M`
/// — now the width of a height STRATUM rather than a pairwise tolerance. Since
/// 2026-08-08 `noise-gpu/build.rs` INJECTS the kernel's copy from the Rust const,
/// so this mirror is the only remaining hand copy — kept on purpose: the whole
/// point of these `GPU_*` constants is to state what the kernel says
/// independently, so a divergence shows up as a fixture failure.
const GPU_ARC_FUSE_HEIGHT_TOL_M: f64 = 3.0;
/// `ARC_FUSE_RANGE_RATIO` (scatter.cu) = `arc_screening::ARC_FUSE_RANGE_RATIO`.
const GPU_ARC_FUSE_RANGE_RATIO: f64 = 1.5;

/// `arc_fuse_key` (scatter.cu) = `arc_screening::fuse_key`: the (height, range)
/// stratum, the equivalence class inside which a fusion is honest.
fn gpu_fuse_key(near_m: f64, height_m: f64) -> i64 {
    let h = (height_m.max(0.0) / GPU_ARC_FUSE_HEIGHT_TOL_M).floor() as i64;
    let r = (near_m.max(1e-3).ln() / GPU_ARC_FUSE_RANGE_RATIO.ln()).floor() as i64;
    (h << 20) | ((r + 0x4_0000) & 0xf_ffff)
}
/// `ARC_PENUMBRA_FLOOR_M` (scatter.cu) is the magnitude of
/// `constants::PENUMBRA_DELTA_FLOOR_M`. Also injected into the kernel by
/// `noise-gpu/build.rs` since 2026-08-08; hand-written here for the same reason as
/// `GPU_ARC_FUSE_HEIGHT_TOL_M` above.
const GPU_ARC_PENUMBRA_FLOOR_M: f64 = 340.0 / 63.0 / 20.0;
/// The grazing prune of the PRE-2026-08-09 kernel, which this port deliberately
/// preserves — `ARC_DELTA_MIN_M` no longer exists in scatter.cu, and it was inert
/// at this same 0.0 there and on the CPU before it was deleted. Mirrors nothing
/// live; it is here because `v4` reproduces the pre-fix rule (see the v4 header).
const GPU_ARC_DELTA_MIN_M: f64 = 0.0;
/// `ARC_ESCALATE_SPAN` (scatter.cu:80) = `arc_screening::ESCALATE_SPAN_RAD`.
const GPU_ARC_ESCALATE_SPAN: f64 = 0.26;
/// `ESCALATE_MAX_PARTS` — the ceiling on that split, mirroring `arc_screening`.
const GPU_ARC_ESCALATE_MAX_PARTS: usize = 9;
/// `ARC_MAX_MERGED` (scatter.cu:127) — merged blocked arcs per (segment,
/// receiver). The CPU is uncapped (`ArcBounds::max_arcs = usize::MAX`).
const GPU_ARC_MAX_MERGED: usize = 32;

/// One obstacle index regrouped by FOOTPRINT — the structure
/// `footprint_csr_for_index` uploads and the kernel's arc walk indexes into.
struct FootprintIndex {
    /// The index's own frame (`metas[0]`, `metas[1]`, `metas[2]`).
    origin_lat: f64,
    origin_lon: f64,
    m_per_deg_lon: f64,
    /// `(x0, y0, x1, y1, height_m)` per edge in that frame, stride 5.
    edges_xyxyh: Vec<f32>,
    /// CSR: footprint → its contiguous run of edge indices, in ring order.
    foot_edge_starts: Vec<u32>,
    foot_edge_refs: Vec<u32>,
    /// Stride [`FOOT_BOX_STRIDE`].
    foot_box: Vec<f32>,
}

impl FootprintIndex {
    #[inline]
    fn footprint_count(&self) -> usize {
        self.foot_edge_starts.len() - 1
    }

    /// Edge `k` of the CSR run (`(x0, y0, x1, y1, height_m)`, widened to f64).
    #[inline]
    fn edge(&self, k: usize) -> [f64; 5] {
        let e = self.foot_edge_refs[k] as usize * 5;
        std::array::from_fn(|i| self.edges_xyxyh[e + i] as f64)
    }
}

/// Group every index of the set by footprint id, exactly as
/// `noise_gpu::footprint_csr_for_index` does host-side: first-appearance id
/// order, counting sort by owner, per-footprint bbox + max height + convexity.
fn build_footprint_indexes(set: &ObstacleSet) -> Vec<FootprintIndex> {
    set.indexes
        .iter()
        .map(|index| {
            let view = index.gpu_view();
            let (foot_edge_starts, foot_edge_refs, foot_box) = footprint_csr(&view);
            FootprintIndex {
                origin_lat: view.origin_lat,
                origin_lon: view.origin_lon,
                m_per_deg_lon: view.m_per_deg_lon,
                edges_xyxyh: view.edges_xyxyh,
                foot_edge_starts,
                foot_edge_refs,
                foot_box,
            }
        })
        .collect()
}

/// `footprint_csr_for_index` — `(starts, refs, boxes)`.
fn footprint_csr(view: &GpuGridView<'_>) -> (Vec<u32>, Vec<u32>, Vec<f32>) {
    let n_edges = view.edge_ids.len();
    // 1. id ⇒ footprint index (first appearance), and the per-edge owner.
    let mut edge_foot = vec![0u32; n_edges];
    let mut of_id: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut n_foot = 0u32;
    for (e, &id) in view.edge_ids.iter().enumerate() {
        let f = *of_id.entry(id).or_insert_with(|| {
            let f = n_foot;
            n_foot += 1;
            f
        });
        edge_foot[e] = f;
    }
    let n_foot = n_foot as usize;
    // 2. counting sort by owner ⇒ the CSR, plus each footprint's bbox and height.
    let mut starts = vec![0u32; n_foot + 1];
    for &f in &edge_foot {
        starts[f as usize + 1] += 1;
    }
    for f in 0..n_foot {
        starts[f + 1] += starts[f];
    }
    let mut cursor = starts[..n_foot].to_vec();
    let mut refs = vec![0u32; n_edges];
    let mut boxes = vec![0.0f32; n_foot * FOOT_BOX_STRIDE];
    for f in 0..n_foot {
        let b = &mut boxes[f * FOOT_BOX_STRIDE..f * FOOT_BOX_STRIDE + 4];
        b[0] = f32::INFINITY;
        b[1] = f32::INFINITY;
        b[2] = f32::NEG_INFINITY;
        b[3] = f32::NEG_INFINITY;
    }
    for (e, &f) in edge_foot.iter().enumerate() {
        let f = f as usize;
        refs[cursor[f] as usize] = e as u32;
        cursor[f] += 1;
        let g = &view.edges_xyxyh[e * 5..e * 5 + 5];
        let b = &mut boxes[f * FOOT_BOX_STRIDE..f * FOOT_BOX_STRIDE + FOOT_BOX_STRIDE];
        b[0] = b[0].min(g[0]).min(g[2]);
        b[1] = b[1].min(g[1]).min(g[3]);
        b[2] = b[2].max(g[0]).max(g[2]);
        b[3] = b[3].max(g[1]).max(g[3]);
        b[4] = b[4].max(g[4]);
    }
    // 3. convexity: one closed chain, turns all the same sign.
    for f in 0..n_foot {
        let (lo, hi) = (starts[f] as usize, starts[f + 1] as usize);
        boxes[f * FOOT_BOX_STRIDE + 5] = if is_convex_ring(&view.edges_xyxyh, &refs[lo..hi]) {
            1.0
        } else {
            0.0
        };
    }
    (starts, refs, boxes)
}

/// The host's retired `noise_gpu::is_convex_ring` — do these edges form ONE
/// closed chain with consistently signed turns? Conservative: anything
/// unverifiable is reported non-convex, which only costs the exact per-edge path.
/// The live lane has no convexity notion left, so this is now the rule's only
/// copy, which is where a pre-fix port should keep it.
fn is_convex_ring(edges_xyxyh: &[f32], refs: &[u32]) -> bool {
    let n = refs.len();
    if n < 3 {
        return false;
    }
    let pt = |e: u32, k: usize| {
        let g = &edges_xyxyh[e as usize * 5..e as usize * 5 + 4];
        (g[k * 2] as f64, g[k * 2 + 1] as f64)
    };
    for i in 0..n {
        let e1 = pt(refs[i], 1);
        let s2 = pt(refs[(i + 1) % n], 0);
        if (e1.0 - s2.0).abs() > 1e-6 || (e1.1 - s2.1).abs() > 1e-6 {
            return false;
        }
    }
    let mut sign = 0i32;
    for i in 0..n {
        let (a0, a1) = (pt(refs[i], 0), pt(refs[i], 1));
        let b1 = pt(refs[(i + 1) % n], 1);
        let (ux, uy) = (a1.0 - a0.0, a1.1 - a0.1);
        let (wx, wy) = (b1.0 - a1.0, b1.1 - a1.1);
        let cr = ux * wy - uy * wx;
        if cr.abs() < 1e-12 {
            continue;
        }
        let s = if cr > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return false;
        }
    }
    sign != 0
}

/// One blocked arc in the kernel's per-pair list (`iv_s/iv_e/iv_near/iv_h/iv_top`),
/// in BASE-RELATIVE azimuth — the frame `arc_screen_bands` works in.
#[derive(Clone, Copy)]
struct GpuArc {
    start: f64,
    end: f64,
    near_m: f64,
    height_m: f64,
    top_alt_m: f64,
}

/// v4's ray buffers, the counterpart of [`ArcScreeningScratch`].
#[derive(Default)]
struct GpuArcScratch {
    profile: PathProfile,
    candidates: Vec<CrossingCandidate>,
    intervals: Vec<GpuArc>,
    /// Boundary-split working set — every arc endpoint plus the fan's own two
    /// ends, then every piece of `[lo, hi]` with its covered flag.
    bounds: Vec<f64>,
    pieces: Vec<(f64, f64, bool)>,
}

/// `arc_iv_union` (scatter.cu:1288) — union-insert one blocked arc into the
/// list sorted by `start`.
///
/// NOT a disjoint set: only FUSIBLE neighbours (heights within
/// [`GPU_ARC_FUSE_HEIGHT_TOL_M`]) are absorbed, so overlapping arcs of
/// materially different height are left side by side — and the energy average
/// downstream then counts their shared directions twice. The CPU merges its
/// clipped intervals unconditionally (`arc_screening` step 1's sort+merge),
/// which is one of the two structural forks this integrator exists to price.
///
/// Capacity: the kernel DROPS the new arc when the list is full
/// (`if (niv >= ARC_MAX_MERGED) { *overflow += 1; return niv; }`), although its
/// own header comment at scatter.cu:121-125 claims the arc is unioned into its
/// nearest neighbour instead. The code is reproduced, not the comment.
fn arc_iv_union(list: &mut Vec<GpuArc>, mut arc: GpuArc) {
    let key = gpu_fuse_key(arc.near_m, arc.height_m);
    let k = |a: &GpuArc| gpu_fuse_key(a.near_m, a.height_m);
    let mut i = 0;
    while i < list.len() && (k(&list[i]), list[i].start) < (key, arc.start) {
        i += 1;
    }
    if i > 0 && k(&list[i - 1]) == key && list[i - 1].end >= arc.start {
        i -= 1;
        absorb(&mut arc, list[i]);
        list.remove(i);
    }
    let mut fused = true;
    while fused {
        fused = false;
        // `i` is inside this stratum's run, and the run is contiguous: absorbing
        // grows `arc.end`, so the same slot is re-tested against the wider arc.
        while i < list.len() && k(&list[i]) == key && list[i].start <= arc.end {
            absorb(&mut arc, list[i]);
            list.remove(i);
            fused = true;
        }
    }
    if list.len() >= GPU_ARC_MAX_MERGED {
        return; // overflow ⇒ the arc is dropped, exactly as the kernel does
    }
    list.insert(i, arc);
}

/// A fusion takes the widest arc, the MINIMUM range and the MAXIMUM height —
/// both directions keep the arc alive through the admission test.
#[inline]
fn absorb(into: &mut GpuArc, other: GpuArc) {
    into.start = into.start.min(other.start);
    into.end = into.end.max(other.end);
    into.near_m = into.near_m.min(other.near_m);
    into.top_alt_m = into.top_alt_m.max(other.top_alt_m);
    into.height_m = into.height_m.max(other.height_m);
}

/// `arc_clip_span` (scatter.cu:1271) — clip an arc to the segment's span. A hull
/// can sit one turn away from it (base near ±π), hence the three shifts.
#[inline]
fn arc_clip_span(a_lo: f64, a_hi: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    for shift in [0.0, std::f64::consts::TAU, -std::f64::consts::TAU] {
        let (s, e) = ((a_lo + shift).max(lo), (a_hi + shift).min(hi));
        if e > s {
            return Some((s, e));
        }
    }
    None
}

/// `seg_isect_t` (scatter.cu:594) — does the ray `(sx,sy) + t·(dx,dy)` cross the
/// segment strictly inside both?
#[inline]
#[allow(clippy::too_many_arguments)]
fn seg_isect_t(sx: f64, sy: f64, dx: f64, dy: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    let (ex, ey) = (x1 - x0, y1 - y0);
    let denom = dx * ey - dy * ex;
    if denom == 0.0 {
        return false;
    }
    let (wx, wy) = (x0 - sx, y0 - sy);
    let t = (wx * ey - wy * ex) / denom;
    let u = (wx * dy - wy * dx) / denom;
    t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u)
}

/// `arc_source_point` (scatter.cu:356) — the point ON the segment the receiver
/// sees at `azimuth`, by exact ray×line intersection. `None` when the ray runs
/// parallel to the segment.
fn gpu_source_point_at(q: &ArcScreening<'_>, m_lon: f64, azimuth: f64) -> Option<(f64, f64)> {
    let (ax, ay) = (
        (q.start_lon - q.receiver_lon) * m_lon,
        (q.start_lat - q.receiver_lat) * M_PER_DEG_LAT,
    );
    let (bx, by) = (
        (q.end_lon - q.receiver_lon) * m_lon,
        (q.end_lat - q.receiver_lat) * M_PER_DEG_LAT,
    );
    let (dx, dy) = (azimuth.cos(), azimuth.sin());
    let (ex, ey) = (bx - ax, by - ay);
    let denom = dx * ey - dy * ex;
    if denom.abs() < 1e-12 {
        return None;
    }
    let u = ((dy * ax - dx * ay) / denom).clamp(0.0, 1.0);
    Some((
        q.start_lat + u * (q.end_lat - q.start_lat),
        q.start_lon + u * (q.end_lon - q.start_lon),
    ))
}

/// `arc_gob` (scatter.cu:1347) — the kernel's ISO 9613-2 §7.3.1 term for one
/// band, identical to `arc_screening::ground_or_barrier`.
#[inline]
fn gpu_ground_or_barrier(terrain_db: f64, ground_db: f64, screening_db: f64) -> f64 {
    let barrier = terrain_db + screening_db;
    if barrier > 0.0 {
        ground_db.max(barrier)
    } else {
        ground_db
    }
}

/// `10^(db/10)` through the kernel's own fast exponential — `fexp(x·ln10/10)`
/// in CUDA, `fast_exp_f64` here (the kernel's fp32 internals are not part of
/// the rule).
#[inline]
fn gpu_db_to_energy(db: f64) -> f64 {
    fast_exp_f64(db * std::f64::consts::LN_10 * 0.1)
}

/// The GPU's `arc_screen_bands`, in CPU code: the per-band effective screening
/// of the whole segment as seen from this receiver.
///
/// Five places it forked from `arc_screening::arc_screened_attenuation` when
/// this integrator was written on 2026-08-04. Three are now CLOSED — the kernel
/// carries the fix and this code mirrors it; they are kept on the record
/// because each was found here and priced here:
///
/// 1. **Grouping** (OPEN, measured immaterial). Arcs are built PER FOOTPRINT PER
///    (segment, receiver) inside the fan, not read off a per-receiver skyline
///    merged over the whole circle. On every scene here the two agree.
/// 2. **Confirmation** (OPEN, measured immaterial). Every emitted arc must be
///    crossed by the exact ray from the receiver to the source point at its own
///    centre azimuth; the CPU keeps an arc on the range test alone. Stricter, so
///    it can only remove screening, and it removes 2 arcs in 66 912 pairs here.
/// 3. **Overlap** (CLOSED). [`arc_iv_union`] leaves overlapping arcs of
///    materially different height side by side — it must, since one
///    (range, height) pair cannot describe both. Averaging that list counted
///    shared directions twice: worth **7.25 dB on scene G**, and 33 % of the
///    (segment, receiver) pairs of a dense rail tile drove the blocked fraction
///    past 1. Both lanes now take the BOUNDARY SPLIT above instead.
/// 4. **Escalation** (CLOSED). The kernel split any piece wider than
///    [`GPU_ARC_ESCALATE_SPAN`] into exactly THREE parts however wide it was;
///    both lanes now split to that fixed angular RESOLUTION, capped at
///    [`GPU_ARC_ESCALATE_MAX_PARTS`]. Worth 0.15 dB on scene E, 0.03 on C.
/// 5. **Noise walls** (CLOSED). The kernel built intervals from obstacle-index
///    footprints only, so `barr` reached the interval RAYS but could never
///    create one — scene D, the same wall through the production
///    `types::Barrier` lane, read the pre-fix-pack `current` verdict exactly
///    (0.63 dB vs the CPU's 0.28). Both lanes now carry the wall arm.
fn gpu_arc_screened_attenuation(
    q: &ArcScreening<'_>,
    rasters: &SceneGround,
    footprints: &[FootprintIndex],
    scratch: &mut GpuArcScratch,
) -> [f64; NUM_BANDS] {
    let m_lon = M_PER_DEG_LON_EQ * q.receiver_lat.to_radians().cos().max(0.01);
    let azimuth = |lat: f64, lon: f64| {
        ((lat - q.receiver_lat) * M_PER_DEG_LAT).atan2((lon - q.receiver_lon) * m_lon)
    };
    let base = azimuth(q.start_lat, q.start_lon);
    let delta = wrap_pi(azimuth(q.end_lat, q.end_lon) - base);
    // Below this span the segment IS a point source at this receiver.
    if delta.abs() < GPU_ARC_MIN_SPAN {
        return *q.cp_screening;
    }
    let (lo, hi) = if delta < 0.0 {
        (delta, 0.0)
    } else {
        (0.0, delta)
    };
    let span = hi - lo;

    let GpuArcScratch {
        profile,
        candidates,
        intervals,
        bounds,
        pieces,
    } = scratch;
    intervals.clear();

    // The fan's two boundary directions, in the RECEIVER metric.
    let (dlo_x, dlo_y) = ((base + lo).cos(), (base + lo).sin());
    let (dhi_x, dhi_y) = ((base + hi).cos(), (base + hi).sin());

    for index in footprints {
        let imlon = index.m_per_deg_lon;
        let rxl = (q.receiver_lon - index.origin_lon) * imlon;
        let ryl = (q.receiver_lat - index.origin_lat) * M_PER_DEG_LAT;
        // Only x rescales between the receiver metric and this index's; a
        // positive x-scale multiplies every 2D cross product by the same
        // positive constant, so the sign tests below are exact in either frame.
        let kx = imlon / m_lon;
        let (alx, aly) = (dlo_x * kx, dlo_y);
        let (ahx, ahy) = (dhi_x * kx, dhi_y);

        for f in 0..index.footprint_count() {
            let fb = &index.foot_box[f * FOOT_BOX_STRIDE..f * FOOT_BOX_STRIDE + FOOT_BOX_STRIDE];
            let (height_m, convex) = (fb[4] as f64, fb[5] != 0.0);
            // Wedge prune, no transcendentals: a direction p is inside the
            // (convex, ≤π) fan iff cross(d_lo, p) ≥ 0 and cross(p, d_hi) ≥ 0.
            // Both are linear in p, so their MAXIMUM over the footprint's bbox
            // sits at a supporting corner.
            let (bx0, by0) = (fb[0] as f64 - rxl, fb[1] as f64 - ryl);
            let (bx1, by1) = (fb[2] as f64 - rxl, fb[3] as f64 - ryl);
            let g1 =
                alx * if alx >= 0.0 { by1 } else { by0 } - aly * if aly >= 0.0 { bx0 } else { bx1 };
            if g1 < 0.0 {
                continue;
            }
            let g2 =
                ahy * if ahy >= 0.0 { bx1 } else { bx0 } - ahx * if ahx >= 0.0 { by0 } else { by1 };
            if g2 < 0.0 {
                continue;
            }

            // Hull from the footprint's own edges, plus the closest range in it
            // (`SkylineArc::near_m`) — one value for the WHOLE footprint, reused
            // by every silhouette arc it emits.
            let (k0, k1) = (
                index.foot_edge_starts[f] as usize,
                index.foot_edge_starts[f + 1] as usize,
            );
            let mut hull_lo = f64::INFINITY;
            let mut hull_hi = f64::NEG_INFINITY;
            let mut near_m = f64::INFINITY;
            for kk in k0..k1 {
                let (r0, r1) = edge_arc(index, kk, imlon, base, &azimuth);
                hull_lo = hull_lo.min(r0.min(r1));
                hull_hi = hull_hi.max(r0.max(r1));
                let g = index.edge(kk);
                near_m = near_m.min(point_to_segment_dist(
                    g[0] - rxl,
                    g[1] - ryl,
                    g[2] - rxl,
                    g[3] - ryl,
                ));
            }
            // The convex hull is used first as a cheap exact REJECT: the union
            // of the per-edge arcs is contained in it.
            let Some((hull_start, hull_end)) = arc_clip_span(hull_lo, hull_hi, lo, hi) else {
                continue;
            };
            // A CONVEX footprint's hull IS its silhouette; a concave one (an
            // L-block, a courtyard) emits the exact union of its per-edge arcs,
            // because the hull spans the notch.
            let n_emit = if convex { 1 } else { k1 - k0 };
            for ei in 0..n_emit {
                let (start, end) = if convex {
                    (hull_start, hull_end)
                } else {
                    let (r0, r1) = edge_arc(index, k0 + ei, imlon, base, &azimuth);
                    match arc_clip_span(r0.min(r1), r0.max(r1), lo, hi) {
                        Some(v) => v,
                        None => continue,
                    }
                };
                let centre_az = base + 0.5 * (start + end);
                let Some((src_lat, src_lon)) = gpu_source_point_at(q, m_lon, centre_az) else {
                    continue;
                };
                // Confirmation: the ray to this arc's centre must actually cross
                // THIS footprint.
                let sxc = (src_lon - index.origin_lon) * imlon;
                let syc = (src_lat - index.origin_lat) * M_PER_DEG_LAT;
                let (dxc, dyc) = (rxl - sxc, ryl - syc);
                let hit = (k0..k1).any(|kk| {
                    let g = index.edge(kk);
                    seg_isect_t(sxc, syc, dxc, dyc, g[0], g[1], g[2], g[3])
                });
                if !hit {
                    continue;
                }
                // Per-arc admission: the arc screens THIS path only if it stands
                // in front of the source and actually bends the ray.
                // δ ≈ h²·d/(2ab) on the sight line to this source point, SIGNED
                // by whether the top clears it.
                let ddx = (src_lon - q.receiver_lon) * m_lon;
                let ddy = (src_lat - q.receiver_lat) * M_PER_DEG_LAT;
                let d_src = (ddx * ddx + ddy * ddy).sqrt();
                let (b_rng, a_rng) = (near_m, d_src - near_m);
                if a_rng <= 1.0 || b_rng < 1.0 {
                    continue;
                }
                let ground_alt = rasters.elevation(
                    q.receiver_lat + b_rng * centre_az.sin() / M_PER_DEG_LAT,
                    q.receiver_lon + b_rng * centre_az.cos() / m_lon,
                );
                let top_alt_m = ground_alt + height_m;
                let los = q.receiver_alt_m - (b_rng / d_src) * (q.receiver_alt_m - q.src_alt_m);
                let h = top_alt_m - los;
                let two_ab = 2.0 * a_rng * b_rng;
                let delta_num = h * h * d_src;
                if h < 0.0 && delta_num > GPU_ARC_PENUMBRA_FLOOR_M * two_ab {
                    continue;
                }
                if h >= 0.0 && delta_num < GPU_ARC_DELTA_MIN_M * two_ab {
                    continue;
                }
                arc_iv_union(
                    intervals,
                    GpuArc {
                        start,
                        end,
                        near_m: b_rng,
                        height_m,
                        top_alt_m,
                    },
                );
            }
        }
    }
    // ---- Noise WALLS are the other half of the skyline (fix-pack Fix 5). A
    // wall IS a segment, so its arc is the same primitive a building edge
    // produces: the short arc between its endpoint azimuths, at its nearest
    // range. Without this a receiver behind a wall gets NO blocked interval, the
    // cp verdict stands for the whole segment, and the constant-width shadow
    // band survives exactly where a wall makes it most visible. The CPU has
    // carried this since the fix-pack (`ArcSkyline::ensure`'s barrier arm); the
    // kernel walked obstacle-index footprints only, which is why scene D read
    // `current` exactly (0.63 dB) until this was ported.
    //
    // No ray confirmation here, mirroring the CPU skyline: a wall arc is kept on
    // the range test alone. `need` is the far end of the segment — nothing
    // beyond it can stand between the receiver and any source point on it.
    {
        let (ax, ay) = (
            (q.start_lon - q.receiver_lon) * m_lon,
            (q.start_lat - q.receiver_lat) * M_PER_DEG_LAT,
        );
        let (bx, by) = (
            (q.end_lon - q.receiver_lon) * m_lon,
            (q.end_lat - q.receiver_lat) * M_PER_DEG_LAT,
        );
        let need = (ax * ax + ay * ay).sqrt().max((bx * bx + by * by).sqrt());
        for w in q.barriers {
            // `dist_m` is a LOWER BOUND on the receiver→midpoint distance and the
            // slice is sorted by it, so this is a `break` on the kernel side too.
            if w.dist_m > need + BARRIER_PATH_HORIZON_M {
                break;
            }
            // The sight line never runs below the SOURCE height, so only a wall
            // shorter than that can be ruled out a priori.
            if w.height_m as f64 <= q.source_height_m.max(0.0) {
                continue;
            }
            let (x0, y0) = (
                (w.start_lon - q.receiver_lon) * m_lon,
                (w.start_lat - q.receiver_lat) * M_PER_DEG_LAT,
            );
            let (x1, y1) = (
                (w.end_lon - q.receiver_lon) * m_lon,
                (w.end_lat - q.receiver_lat) * M_PER_DEG_LAT,
            );
            let near_m = point_to_segment_dist(x0, y0, x1, y1);
            if near_m > need || near_m < 1e-6 {
                continue;
            }
            let a0 = y0.atan2(x0);
            let a1 = y1.atan2(x1);
            let r0 = wrap_pi(a0 - base);
            let r1 = r0 + wrap_pi(a1 - a0);
            let Some((start, end)) = arc_clip_span(r0.min(r1), r0.max(r1), lo, hi) else {
                continue;
            };
            let centre_az = base + 0.5 * (start + end);
            let Some((src_lat, src_lon)) = gpu_source_point_at(q, m_lon, centre_az) else {
                continue;
            };
            let ddx = (src_lon - q.receiver_lon) * m_lon;
            let ddy = (src_lat - q.receiver_lat) * M_PER_DEG_LAT;
            let d_src = (ddx * ddx + ddy * ddy).sqrt();
            let (b_rng, a_rng) = (near_m, d_src - near_m);
            if a_rng <= 1.0 || b_rng < 1.0 {
                continue;
            }
            let ground_alt = rasters.elevation(
                q.receiver_lat + b_rng * centre_az.sin() / M_PER_DEG_LAT,
                q.receiver_lon + b_rng * centre_az.cos() / m_lon,
            );
            let top_alt_m = ground_alt + w.height_m as f64;
            let los = q.receiver_alt_m - (b_rng / d_src) * (q.receiver_alt_m - q.src_alt_m);
            let h = top_alt_m - los;
            let two_ab = 2.0 * a_rng * b_rng;
            let delta_num = h * h * d_src;
            if h < 0.0 && delta_num > GPU_ARC_PENUMBRA_FLOOR_M * two_ab {
                continue;
            }
            if h >= 0.0 && delta_num < GPU_ARC_DELTA_MIN_M * two_ab {
                continue;
            }
            arc_iv_union(
                intervals,
                GpuArc {
                    start,
                    end,
                    near_m: b_rng,
                    height_m: w.height_m as f64,
                    top_alt_m,
                },
            );
        }
    }

    if intervals.is_empty() {
        return *q.cp_screening;
    }

    // ---- BOUNDARY SPLIT. `arc_iv_union` fuses only neighbours whose heights
    // agree within [`GPU_ARC_FUSE_HEIGHT_TOL_M`] — it must, because one
    // (range, height) pair cannot describe a near-and-low and a far-and-tall
    // obstacle at once — so two footprints at different RANGES with materially
    // different heights stay side by side, OVERLAPPING. The average below then
    // adds `step / span` for EACH of them, so every direction they share is
    // counted twice: `blocked` overruns the fan, the clear remainder is clamped
    // away, and the mean is taken over a total weight > 1.
    //
    // Merging the overlap away is NOT the fix: one interval carries one centre
    // ray, and whichever obstacle that ray happens to hit is then applied to
    // every direction in the union — including directions where only the other
    // one stands. That is the envelope the fuse tolerance exists to prevent,
    // re-created one step later. Measured on scene G: merging reads 2.53 dB,
    // still 4× the CPU's 0.63.
    //
    // Cut at every arc endpoint and emit the COVERED pieces instead. The union
    // of the pieces is exactly the union of the arcs, so the blocked fraction is
    // right and nothing is double counted; and each piece gets its OWN centre
    // ray, so in a doubly covered direction the dominant screen wins through the
    // δ ranking already inside `screening_attenuation` — no "max, not envelope"
    // rule has to be encoded here. `n` arcs yield at most `2n − 1` pieces.
    //
    // The walk covers the WHOLE fan, `lo` to `hi`, not just the arcs' own hull:
    // an uncovered piece is a CLEAR direction and now takes its own terrain ray
    // (scatter.cu's clear arm), so it is enumerated like any other.
    bounds.clear();
    bounds.push(lo);
    bounds.push(hi);
    for iv in intervals.iter() {
        bounds.push(iv.start.clamp(lo, hi));
        bounds.push(iv.end.clamp(lo, hi));
    }
    bounds.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    pieces.clear();
    for w in bounds.windows(2) {
        let (start, end) = (w[0], w[1]);
        if end <= start {
            continue; // duplicate boundary
        }
        let mid = 0.5 * (start + end);
        let covered = intervals.iter().any(|iv| iv.start <= mid && mid <= iv.end);
        pieces.push((start, end, covered));
    }

    // One screening evaluation per (sub-)interval centre, energy-averaged on the
    // ground/barrier term.
    let ground = noise_compute::propagation::iso9613::ground_atten_bands(q.ground_g);
    let cp_rel = wrap_pi(azimuth(q.cp_lat, q.cp_lon) - base);
    let mut blocked_fraction = 0.0f64;
    let mut energy = [0.0f64; NUM_BANDS];
    for &(piece_start, piece_end, covered) in pieces.iter() {
        // ---- CLEAR ARM (scatter.cu `ray_terrain_bands`). CLEAR IS NOT
        // TERRAIN-FREE: these directions used to fall through to the lumped
        // remainder below, carrying the CP ray's `A_terrain` — right only over
        // level ground. Scene I is where that shows: 1.51 dB against the
        // 99-point reference, 0.93 dB with this arm.
        if !covered {
            let width = piece_end - piece_start;
            let parts = ((width / GPU_ARC_ESCALATE_SPAN).ceil() as usize)
                .clamp(1, GPU_ARC_ESCALATE_MAX_PARTS);
            let step = width / parts as f64;
            for p in 0..parts {
                let start = piece_start + p as f64 * step;
                let terrain =
                    gpu_interval_terrain(q, rasters, m_lon, base + start + 0.5 * step, profile)
                        .unwrap_or(*q.cp_terrain);
                let fraction = step / span;
                blocked_fraction += fraction;
                for b in 0..NUM_BANDS {
                    energy[b] += fraction
                        * gpu_db_to_energy(-gpu_ground_or_barrier(terrain[b], ground[b], 0.0));
                }
            }
            continue;
        }
        // Split to a fixed angular RESOLUTION, capped — the CPU's rule
        // (`ESCALATE_SPAN_RAD` / `ESCALATE_MAX_PARTS`). The kernel used to take a
        // FIXED 3-way split however wide the piece was, which left 57° per sample
        // on a receiver walled in by a long wall; measured worth 0.15 dB on scene
        // E (1.03 -> 0.88) and 0.03 on C, and it was the last fork between the
        // two lanes on scenes A-G.
        let parts = ((piece_end - piece_start) / GPU_ARC_ESCALATE_SPAN).ceil() as usize;
        let parts = parts.clamp(1, GPU_ARC_ESCALATE_MAX_PARTS);
        let step = (piece_end - piece_start) / parts as f64;
        for p in 0..parts {
            let start = piece_start + p as f64 * step;
            let end = start + step;
            // No epsilon and no re-basing here: the kernel compares the raw
            // base-relative cp azimuth against the raw interval bounds.
            // The terrain the average is TAKEN ON is this piece's own ray's,
            // the cp's only where no interval ray runs — `ray_path_bands` fills
            // it either way and the kernel was throwing it away.
            let (terrain, bands) = if cp_rel >= start && cp_rel <= end {
                (*q.cp_terrain, *q.cp_screening)
            } else {
                gpu_interval_screening(
                    q,
                    rasters,
                    m_lon,
                    base + 0.5 * (start + end),
                    profile,
                    candidates,
                )
                .unwrap_or((*q.cp_terrain, *q.cp_screening))
            };
            let fraction = step / span;
            blocked_fraction += fraction;
            for b in 0..NUM_BANDS {
                energy[b] += fraction
                    * gpu_db_to_energy(-gpu_ground_or_barrier(terrain[b], ground[b], bands[b]));
            }
        }
    }

    // …plus the clear rest of the fan, which carries the terrain/ground term
    // alone. With the boundary split above, `blocked_fraction` is now the exact
    // angular measure of the union and can no longer exceed 1.
    let clear_fraction = (1.0 - blocked_fraction).max(0.0);
    std::array::from_fn(|b| {
        let clear = clear_fraction
            * gpu_db_to_energy(-gpu_ground_or_barrier(q.cp_terrain[b], ground[b], 0.0));
        let mean_db = -10.0 * (energy[b] + clear).max(1e-12).log10();
        (mean_db - q.cp_terrain[b]).max(0.0)
    })
}

/// The base-relative azimuths of edge `k`'s two endpoints — the SHORT arc
/// between them, unwrapped exactly as the kernel's hull pass does.
fn edge_arc(
    index: &FootprintIndex,
    k: usize,
    imlon: f64,
    base: f64,
    azimuth: &impl Fn(f64, f64) -> f64,
) -> (f64, f64) {
    let g = index.edge(k);
    let a0 = azimuth(
        index.origin_lat + g[1] / M_PER_DEG_LAT,
        index.origin_lon + g[0] / imlon,
    );
    let a1 = azimuth(
        index.origin_lat + g[3] / M_PER_DEG_LAT,
        index.origin_lon + g[2] / imlon,
    );
    let r0 = wrap_pi(a0 - base);
    (r0, r0 + wrap_pi(a1 - a0))
}

/// Distance from the ORIGIN to the segment `(x0,y0)-(x1,y1)` — the kernel's
/// inline `near_m` form (scatter.cu:1553-1561).
#[inline]
fn point_to_segment_dist(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let (vx, vy) = (x1 - x0, y1 - y0);
    let vv = vx * vx + vy * vy;
    let t = if vv > 0.0 {
        (-(x0 * vx + y0 * vy) / vv).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (x0 + t * vx, y0 + t * vy);
    (qx * qx + qy * qy).sqrt()
}

/// Screening bands on the ray from the receiver to the source point at
/// `azimuth` — the CPU entry points standing in for the kernel's
/// `ray_path_bands` (scatter.cu:1142), which is that same path ported. Fresh
/// profile, that ray's own terrain as the increment base, its own crossings,
/// and the caller's barrier slice.
fn gpu_interval_screening(
    q: &ArcScreening<'_>,
    rasters: &SceneGround,
    m_lon: f64,
    azimuth: f64,
    profile: &mut PathProfile,
    candidates: &mut Vec<CrossingCandidate>,
) -> Option<([f64; NUM_BANDS], [f64; NUM_BANDS])> {
    let (src_lat, src_lon) = gpu_source_point_at(q, m_lon, azimuth)?;
    let dist_m = geo::flat_dist(src_lat, src_lon, q.receiver_lat, q.receiver_lon);
    if dist_m < 1.0 {
        return None;
    }
    let src_alt = rasters.elevation(src_lat, src_lon) + q.source_height_m;
    rasters.build_path_profile(
        src_lat,
        src_lon,
        q.receiver_lat,
        q.receiver_lon,
        dist_m,
        profile,
    );
    let terrain = terrain_attenuation(profile, src_alt, q.receiver_alt_m);
    q.obstacles.crossings_pruned(
        src_lat,
        src_lon,
        q.receiver_lat,
        q.receiver_lon,
        &CellPrune::for_profile(profile, src_alt, q.receiver_alt_m),
        candidates,
    );
    let screening = screening_attenuation(
        profile,
        q.barriers,
        ObstacleInput {
            candidates,
            replace_sample_buildings: true,
        },
        src_alt,
        q.receiver_alt_m,
        q.exclusion_radius_m,
        &terrain,
    );
    Some((terrain, screening))
}

/// Bare-earth terrain bands on the ray to the source point at `azimuth` — the
/// kernel's `ray_terrain_bands` (the clear-fan arm of `ray_path_bands`).
fn gpu_interval_terrain(
    q: &ArcScreening<'_>,
    rasters: &SceneGround,
    m_lon: f64,
    azimuth: f64,
    profile: &mut PathProfile,
) -> Option<[f64; NUM_BANDS]> {
    let (src_lat, src_lon) = gpu_source_point_at(q, m_lon, azimuth)?;
    let dist_m = geo::flat_dist(src_lat, src_lon, q.receiver_lat, q.receiver_lon);
    if dist_m < 1.0 {
        return None;
    }
    let src_alt = rasters.elevation(src_lat, src_lon) + q.source_height_m;
    rasters.build_path_profile(
        src_lat,
        src_lon,
        q.receiver_lat,
        q.receiver_lon,
        dist_m,
        profile,
    );
    Some(terrain_attenuation(profile, src_alt, q.receiver_alt_m))
}

/// Probe grid coordinates (deterministic integer stepping, no float accumulation).
fn grid_axis(min: f64, max: f64, step: f64) -> Vec<f64> {
    let n = ((max - min) / step).round() as usize;
    (0..=n).map(|i| min + i as f64 * step).collect()
}

/// One receiver's result per integrator slot (`None` where not requested).
struct Row {
    x: f64,
    y: f64,
    lden: [Option<f64>; 5],
}

/// Wall time each integrator spent on the scene (crude perf signal — same
/// receivers, same machine, one thread).
type SceneTimings = [std::time::Duration; 5];

fn run_scene(
    scene: SceneId,
    integrators: &[Integrator],
    subdivisions: Option<usize>,
    seg_samples: usize,
) -> (Vec<Row>, SceneTimings) {
    let rasters = SceneGround {
        slope_per_m_y: scene.ground_slope_y(),
    };
    let obstacles = build_obstacles(scene);
    let footprints = build_footprint_indexes(&obstacles);
    let barriers_scene = build_barriers(scene);
    let emission = motorway_emission();
    let mut probe = Probe {
        rasters: &rasters,
        obstacles: &obstacles,
        emission: &emission,
        profile: PathProfile::new(),
        candidates: Vec::new(),
        barriers_scene: &barriers_scene,
        barriers: Vec::new(),
        reference_subdivisions: subdivisions.unwrap_or_else(|| scene.reference_subdivisions()),
        source_x_m: scene.source_x_m(),
        skyline: ArcSkyline::default(),
        arc: ArcScreeningScratch::new(),
        bounds: ArcBounds::shipped(),
        footprints: &footprints,
        v4: GpuArcScratch::default(),
        seg: SegSampleScratch::new(),
        seg_samples,
        seg_bounds: seg_arc_bounds(),
    };

    let xs = grid_axis(PROBE_X_MIN, PROBE_X_MAX, PROBE_X_STEP);
    let ys = grid_axis(PROBE_Y_MIN, PROBE_Y_MAX, PROBE_Y_STEP);
    let mut rows = Vec::with_capacity(xs.len() * ys.len());
    let mut timings: SceneTimings = [std::time::Duration::ZERO; 5];

    for &x in &xs {
        for &y in &ys {
            let (lat, lon) = to_lat_lon(x, y);
            // Receivers stand ON the scene's ground, which scene I tilts.
            let mut receiver = Receiver::new(lat, lon, rasters.elev(lat, lon));
            receiver.height_m = PROBE_HEIGHT_M;
            probe.prepare_barriers(&receiver);
            let mut lden = [None; 5];
            for &i in &ALL_INTEGRATORS {
                if !integrators.contains(&i) {
                    continue;
                }
                let t = std::time::Instant::now();
                lden[i.slot()] = Some(probe.lden(&receiver, i));
                timings[i.slot()] += t.elapsed();
            }
            rows.push(Row { x, y, lden });
        }
    }
    (rows, timings)
}

/// `max |reference33 − X|` inside and outside the box shadow region, with the
/// receiver each maximum sits on.
struct ErrorSummary {
    shadow_max: f64,
    shadow_at: (f64, f64),
    shadow_mean: f64,
    outside_max: f64,
    outside_at: (f64, f64),
    outside_mean: f64,
    pairs: usize,
}

fn summarize(rows: &[Row], integrator: Integrator, scene: SceneId) -> Option<ErrorSummary> {
    let mut s = ErrorSummary {
        shadow_max: 0.0,
        shadow_at: (f64::NAN, f64::NAN),
        shadow_mean: 0.0,
        outside_max: 0.0,
        outside_at: (f64::NAN, f64::NAN),
        outside_mean: 0.0,
        pairs: 0,
    };
    let (mut shadow_sum, mut shadow_n) = (0.0f64, 0usize);
    let (mut outside_sum, mut outside_n) = (0.0f64, 0usize);

    for row in rows {
        let (Some(reference), Some(value)) = (
            row.lden[Integrator::Reference33.slot()],
            row.lden[integrator.slot()],
        ) else {
            continue;
        };
        s.pairs += 1;
        let err = (reference - value).abs();
        let in_shadow = row.y.abs() < scene.shadow_abs_y_max()
            && row.x >= SHADOW_X_MIN
            && row.x <= SHADOW_X_MAX;
        if in_shadow {
            shadow_sum += err;
            shadow_n += 1;
            if err > s.shadow_max {
                s.shadow_max = err;
                s.shadow_at = (row.x, row.y);
            }
        } else {
            outside_sum += err;
            outside_n += 1;
            if err > s.outside_max {
                s.outside_max = err;
                s.outside_at = (row.x, row.y);
            }
        }
    }
    if s.pairs == 0 {
        return None;
    }
    s.shadow_mean = if shadow_n > 0 {
        shadow_sum / shadow_n as f64
    } else {
        0.0
    };
    s.outside_mean = if outside_n > 0 {
        outside_sum / outside_n as f64
    } else {
        0.0
    };
    Some(s)
}

const USAGE: &str = "\
screening_fixture — CNOSSOS line-source screening validation fixture

Usage:
  screening_fixture [--scene a|b|c|d|e|f|g|h|i|j|k|all]
                    [--integrator reference33|current|v3|v4|v5|all]
                    [--reference-subdivisions N] [--seg-samples N]

Options:
  --scene       which synthetic scene to run (default: all)
  --integrator  which screening evaluation to report (default: all; `both` =
                reference33 + current, the pre-v3 spelling)
  --reference-subdivisions
                override every scene's sub-points per microsegment — the
                convergence lever (run 33 vs 99 and watch the verdict move)
  --seg-samples angular buckets per microsegment for `v5` (default 5, the
                shipped tile value); 1 collapses v5 onto `current`
  --help        print this text

Output: TSV `scene integrator x y lden_db` on stdout, then `#` summary lines.
";

fn main() {
    let mut scenes = ALL_SCENES.to_vec();
    let mut integrators = ALL_INTEGRATORS.to_vec();
    let mut subdivisions: Option<usize> = None;
    let mut seg_samples: usize = SEG_SAMPLES_DEFAULT;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let value = |i: usize| -> String {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("{}: missing value\n\n{USAGE}", args[i]);
                std::process::exit(2);
            })
        };
        match args[i].as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return;
            }
            "--scene" => {
                scenes = match value(i).as_str() {
                    "a" => vec![SceneId::A],
                    "b" => vec![SceneId::B],
                    "c" => vec![SceneId::C],
                    "d" => vec![SceneId::D],
                    "e" => vec![SceneId::E],
                    "f" => vec![SceneId::F],
                    "g" => vec![SceneId::G],
                    "h" => vec![SceneId::H],
                    "i" => vec![SceneId::I],
                    "j" => vec![SceneId::J],
                    "k" => vec![SceneId::K],
                    "all" => ALL_SCENES.to_vec(),
                    other => {
                        eprintln!("unknown --scene {other}\n\n{USAGE}");
                        std::process::exit(2);
                    }
                };
                i += 2;
            }
            "--integrator" => {
                integrators = match value(i).as_str() {
                    "reference33" => vec![Integrator::Reference33],
                    "current" => vec![Integrator::Current],
                    "v3" => vec![Integrator::V3],
                    "v4" => vec![Integrator::V4],
                    "v5" => vec![Integrator::V5],
                    "both" => vec![Integrator::Reference33, Integrator::Current],
                    "all" => ALL_INTEGRATORS.to_vec(),
                    other => {
                        eprintln!("unknown --integrator {other}\n\n{USAGE}");
                        std::process::exit(2);
                    }
                };
                i += 2;
            }
            // The convergence lever. `reference33` is only a reference where it
            // is CONVERGED, and the only way to know is to refine it and watch
            // the verdict — which used to mean editing the source and
            // rebuilding. Overrides every scene's own
            // [`SceneId::reference_subdivisions`].
            "--reference-subdivisions" => {
                subdivisions = Some(value(i).parse::<usize>().unwrap_or_else(|_| {
                    eprintln!("--reference-subdivisions wants a positive integer\n\n{USAGE}");
                    std::process::exit(2);
                }));
                i += 2;
            }
            "--seg-samples" => {
                seg_samples = value(i).parse::<usize>().unwrap_or_else(|_| {
                    eprintln!("--seg-samples wants a positive integer\n\n{USAGE}");
                    std::process::exit(2);
                });
                i += 2;
            }
            other => {
                eprintln!("unknown argument {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    println!(
        "# source: line x=0 (x={K_SOURCE_X_M} on scene k), y={LINE_Y_MIN}..{LINE_Y_MAX} m, \
         {N_SEGMENTS} x {SEGMENT_LEN_M} m microsegments, motorway WORLD_DEFAULT[0] @ \
         {MOTORWAY_SPEED_KMH} km/h"
    );
    println!(
        "# terrain: flat z={GROUND_ELEV_M} m, G={GROUND_G}, no vegetation; receivers at \
         {PROBE_HEIGHT_M} m"
    );
    println!(
        "# grid: x={PROBE_X_MIN}..{PROBE_X_MAX} step {PROBE_X_STEP}, y={PROBE_Y_MIN}..\
         {PROBE_Y_MAX} step {PROBE_Y_STEP}"
    );
    println!(
        "# reference33 = {} sub-segments per microsegment (99 on scenes i/j, whose 33->99 \
         movement matches the effect they gate), energy-sum",
        subdivisions
            .map(|n| n.to_string())
            .unwrap_or_else(|| REFERENCE_SUBDIVISIONS.to_string())
    );
    println!("# v3 bounds: {:?}", ArcBounds::shipped());
    println!(
        "# v5 = propagation::seg_sampling (THE SHIPPED TILE RULE), {seg_samples} angular \
         buckets per microsegment, per-bucket arc min_span {:?} rad (INF = off, the \
         shipped tile default); reported through v3's non-negative increment channel, so \
         it is a lower bound on what the tile path (which takes the composite directly) gets",
        seg_arc_bounds().min_span_rad
    );
    println!(
        "# v4 = noise-gpu kernels/scatter.cu arc_screen_bands, ported (min_span \
         {GPU_ARC_MIN_SPAN} rad, fuse tol {GPU_ARC_FUSE_HEIGHT_TOL_M} m, penumbra floor \
         {GPU_ARC_PENUMBRA_FLOOR_M:.6} m, escalate {GPU_ARC_ESCALATE_SPAN} rad x<={GPU_ARC_ESCALATE_MAX_PARTS}, max merged {GPU_ARC_MAX_MERGED})"
    );
    println!("scene\tintegrator\tx\ty\tlden_db");

    let mut summaries = Vec::new();
    for &scene in &scenes {
        let (rows, timings) = run_scene(scene, &integrators, subdivisions, seg_samples);
        for integrator in &integrators {
            for row in &rows {
                if let Some(lden) = row.lden[integrator.slot()] {
                    println!(
                        "{}\t{}\t{:.0}\t{:.0}\t{:.3}",
                        scene.label(),
                        integrator.label(),
                        row.x,
                        row.y,
                        lden
                    );
                }
            }
        }
        for &integrator in &integrators {
            if integrator == Integrator::Reference33 {
                continue;
            }
            if let Some(summary) = summarize(&rows, integrator, scene) {
                summaries.push((scene, integrator, summary));
            }
        }
        for &integrator in &integrators {
            println!(
                "# wall {} {} {:.2} s",
                scene.label(),
                integrator.label(),
                timings[integrator.slot()].as_secs_f64()
            );
        }
    }

    println!("#");
    println!("# summary: |reference33 - integrator| in dB");
    println!(
        "# shadow region = |y| < {SHADOW_ABS_Y_MAX} m (scene A/B/C/D), \
         {SHADOW_ABS_Y_MAX_ROW} m (scene E/F/I) or {SHADOW_ABS_Y_MAX_STACK} m (scene G/H/J/K), \
         and {SHADOW_X_MIN} <= x <= {SHADOW_X_MAX} m — on scene K that band is a \
         shallow-chain / deep-chain split, not a shadow/clear one"
    );
    if summaries.is_empty() {
        println!("# (none — reference33 plus one other integrator are needed)");
        return;
    }
    for (scene, integrator, s) in &summaries {
        println!(
            "# scene {} / {} — {}",
            scene.label(),
            integrator.label(),
            scene.description()
        );
        println!(
            "#   shadow  max {:.2} dB at (x={:.0}, y={:.0}), mean {:.2} dB",
            s.shadow_max, s.shadow_at.0, s.shadow_at.1, s.shadow_mean
        );
        println!(
            "#   outside max {:.2} dB at (x={:.0}, y={:.0}), mean {:.2} dB",
            s.outside_max, s.outside_at.0, s.outside_at.1, s.outside_mean
        );
        println!("#   receivers compared: {}", s.pairs);
    }

    // ---- VERDICT. Until 2026-08-04 this harness printed the numbers and then
    // exited 0 whatever they said, and the limits lived in an UNTRACKED scratch
    // script — so the gate could not fail and did not ship with the code it
    // gated. Both are fixed here: the limits are versioned next to the scenes,
    // and a breach exits non-zero.
    //
    // The limits are the OWNER's line (night plan §4b: anything over 1.0 dB is a
    // hard fail, 0.5 dB is the target), NOT "measured v3 plus headroom". Two
    // corrections to the scratch script they came from: scene I carried 1.25,
    // which is exactly the measured-plus-headroom the ruling forbids — it is
    // 1.00 here and scene I therefore FAILS at 1.01 dB, which is the truth and
    // the open work item (fuse on absolute top, see arc_screening.rs). And scene
    // D was absent from the table altogether, so the PRODUCTION barrier lane —
    // the one carrying every noise wall in the world — was ungated; it now
    // shares scene C's limits, since it is the same wall through the other path.
    // BOTH shipped rules are gated: `v3` is what the CPU paints with and `v4`
    // is the CUDA rule ported here, so gating only v3 would let the GPU lane
    // fail this suite silently — which is the whole reason v4 exists (review
    // 2026-08-04 caught the first version of this gate doing exactly that).
    let gated = |i: &Integrator| matches!(i, Integrator::V3 | Integrator::V4 | Integrator::V5);
    let mut failures = Vec::new();
    for (scene, integrator, s) in &summaries {
        if !gated(integrator) {
            continue;
        }
        let (lim_shadow, lim_outside) = scene.limits_db();
        if s.shadow_max > lim_shadow {
            failures.push(format!(
                "scene {} [{}] shadow {:.2} dB > {:.2} at (x={:.0}, y={:.0})",
                scene.label(),
                integrator.label(),
                s.shadow_max,
                lim_shadow,
                s.shadow_at.0,
                s.shadow_at.1
            ));
        }
        if s.outside_max > lim_outside {
            failures.push(format!(
                "scene {} [{}] outside {:.2} dB > {:.2} at (x={:.0}, y={:.0})",
                scene.label(),
                integrator.label(),
                s.outside_max,
                lim_outside,
                s.outside_at.0,
                s.outside_at.1
            ));
        }
    }
    if !summaries.iter().any(|(_, i, _)| gated(i)) {
        println!("# VERDICT skipped — no gated integrator was run");
        return;
    }
    if failures.is_empty() {
        println!("# VERDICT PASS — every scene within the owner's limits");
        return;
    }
    for f in &failures {
        println!("# VERDICT FAIL {f}");
    }
    eprintln!("physics fixture FAILED: {} breach(es)", failures.len());
    std::process::exit(1);
}

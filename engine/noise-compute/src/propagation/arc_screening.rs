//! Arc-clipping screening for line sources — Fix 1 of the CNOSSOS screening
//! fix-pack (v3, tri-review consensus 2026-08-03; bounded per the owner ruling
//! 2026-08-03 §4b, "≤ 0.5 dB at every pixel against the EXACT arc kernel").
//!
//! A microsegment runs up to 250 m (`osm-extract microsegment::split` cap) and
//! subtends a wide angle at a nearby receiver — 136° at 50 m. The kernel
//! nevertheless evaluates screening ONCE, on the ray to the segment's
//! characteristic point, and applies that single verdict to the whole
//! segment's emission: a 30 m building covering 33° of the fan either screens
//! everything (−12 dB) or nothing, toggling as the cp ray hits or misses it.
//! That toggle is the measured constant-width shadow stripe behind buildings.
//!
//! This module replaces the verdict with an angular energy average:
//!
//! 1. the segment's angular span at the receiver (two `atan2`),
//! 2. the sub-intervals of that span that vector obstacles actually block,
//!    read off the receiver's SKYLINE — the merged azimuth arcs of everything
//!    standing around it, built ONCE per receiver and shared by all its
//!    segments ([`ArcSkyline`]),
//! 3. ONE screening evaluation per blocked interval, on the ray to the EXACT
//!    source point on the segment at the interval's centre azimuth (exact
//!    range, no banding), the cp evaluation reused for the interval that
//!    contains the cp azimuth,
//! 4. an energy average over the interval fractions `f_i` of the span, taken
//!    on the term the kernel actually applies — `max(A_ground, A_terrain +
//!    A_screen)` (ISO 9613-2 §7.3.1) — and handed back as the screening
//!    increment that reproduces it.
//!
//! Step 4 averages the GROUND/BARRIER term, not the bare screening increment,
//! because the two are not independent: a segment 14 % of whose fan is blocked
//! by 17 dB yields ~0.3 dB of average screening, which `max(A_ground, …)`
//! discards entirely — the ground term is larger. Averaging the max instead
//! keeps the blocked fraction's energy where it belongs.
//!
//! Terrain, ground G and vegetation stay on the caller's cp ray (DECISION
//! 2026-08-03): they vary slowly along the segment, and the terrain+screening
//! composite pairing must stay intact. The terrain of an interval ray is used
//! ONLY inside that interval's own screening call, as the increment base — it
//! never leaves this module.
//!
//! ## What bounds the work, and why each bound is derived
//!
//! The cost the fix-pack's first cut paid was an AREA query over the
//! (receiver, start, end) triangle PER (segment, receiver) pair — 60× the GPU
//! kernel time on a dense rail cell. Every bound below replaces per-pair work
//! with per-receiver work, or skips work that provably cannot move a dB. All
//! are derived from ONE law, the same one that makes the profile cadence dense
//! at both endpoints: a screen `h` above the sight line, `a` from the source
//! and `b` from the receiver on a path of length `d`, bends the path by
//!
//! ```text
//! δ ≈ h² · d / (2 a b)
//! ```
//!
//! * **Skyline, not triangle** ([`ArcSkyline`]): every segment of one receiver
//!   sees the same obstacles, so the azimuth arcs are built once per receiver
//!   and each segment only intersects its own span against that layered list.
//!   No approximation — a strictly better candidate set than the triangle bbox,
//!   which missed nothing but also grouped nothing.
//! * **Grazing prune** ([`ArcBounds::delta_min_m`]): a grid cell whose tallest
//!   edge cannot reach `h = sqrt(2 δ_min b)` at its own range `b` cannot
//!   produce `δ ≥ δ_min` from ANY source beyond it, and
//!   `diffraction::maekawa_bands` gates such a path to zero in every band whose
//!   `λ/4 − δ*` exceeds it. Skipped in O(1) per cell.
//! * **Small-span skip** ([`ArcBounds::min_span_rad`]): below it the segment IS
//!   a point source at this receiver — one ray represents every direction it
//!   radiates in, which is exactly the cp verdict, so arc clipping has nothing
//!   to resolve.
//! * **Grow-on-demand radius** ([`ArcSkyline::ensure`]): not a bound at all —
//!   the skyline is walked out to `d + L`, the far end of the segment asking for
//!   it, and grows by ANNULUS when a later segment needs more, so every grid
//!   cell is visited at most once per receiver. Anything that screens a path
//!   stands BETWEEN receiver and source, so a skyline reaching the source
//!   reaches every screen: the candidate set is COMPLETE, which is what lets the
//!   clear fraction of the fan be treated as exactly clear.
//!
//! ## Why the skyline is LAYERED, and why that is a correctness property
//!
//! Growing on demand means a segment is clipped against arcs a PREVIOUS,
//! further-away segment dragged in. Those arcs stand beyond this segment, so the
//! admission test (`a = d − near > 1`) is supposed to discard them, and the
//! answer is then independent of what came before. That argument holds only
//! while every arc carries its OWN range — and a merge takes `near = min` and
//! hands it to the UNION of the merged azimuth ranges.
//!
//! So arcs merge only inside one STRATUM of height AND range ([`fuse_key`]):
//! the skyline is one azimuth-arc list per (height, range) layer, the idea of a
//! deep shadow map (Lokovic & Veach, SIGGRAPH 2000) with exact arcs in place of
//! quantised sectors. Keyed on height alone — as this was, and as the CUDA lane
//! was — a receiver in a city where 84.6 % of shading edges carry one of three
//! literal heights chains its whole panorama into a handful of ~190° arcs
//! carrying the range of the house at its own facade, and the clipping this
//! module exists to do collapses back to the cp verdict it replaced. Measured on
//! a real Praha receiver (50.0402 / 14.4738, 1200 m): 28 888 raw edge arcs
//! merged into 280, the widest 149.5° wide carrying `near = 0.61 m`; in 1841 of
//! 3600 azimuths the merged range was nearer than anything actually standing
//! there, worst by a factor of **192** (0.61 m against a real 118.1 m). With the
//! strata: 691 arcs, 490 of 3600, worst factor 1.4 — bounded by the stratum
//! ratio, which is what a stratum is for.
//!
//! It cuts BOTH ways, which is why the shift has no bias to hide behind. On the
//! rail receiver where the two lanes were measured 14.9 dB apart (49.838591 /
//! 13.893328, 6 km) the chain reaches 231.9° carrying `near = 0.01 m` — and
//! `b < 1.0` in the clip below then rejects that arc OUTRIGHT, so 232° of
//! genuinely blocked panorama is declared CLEAR and the receiver is painted as
//! if it stood in the open. A merge that invents a nearer obstacle over-screens
//! where the arc survives and DELETES the screen where it drags `near` under the
//! self-rejection floor.
//!
//! Every bound is switchable (`QM_ARC_*`, [`ArcBounds::from_env`]) so a LOOSER
//! kernel can paint the reference tile from the same binary that paints the
//! shipped one, and the drift between them is measured, never assumed
//! (`tile-painter compare_hm3`). [`ARC_BOUNDS_EXACT`] removes every bound at
//! once; it is a fixture/unit-test reference — on a real region its skyline is
//! the whole obstacle store per receiver, which is exactly the cost this module
//! exists to avoid.
//!
//! Blocked intervals come from the VECTOR obstacle store; a caller without one
//! (`obstacles: None`) keeps today's cp-ray verdict unchanged.
//!
//! ## Why the blocked test is geometry only
//!
//! An arc is admitted on two questions — does it cover any of this segment's
//! span, and does it stand in FRONT of the source (`a = d − near > 1`, `near ≥ 1`)
//! — and on nothing else. It used to carry a third, `|δ| ≈ h²·d/(2ab)` against the
//! sight line, to drop arcs passing too far below the line (CNOSSOS-EU §2.5.6(c)'s
//! penumbra floor) or too close to grazing. That test is GONE from both lanes.
//!
//! The reason is that admission is not a value, it is a FORK between two ray
//! marches. An admitted direction goes to [`interval_screening`]: its own source
//! point, its own path profile, its own terrain, its own exact crossings, and
//! `path_effects` ranking the candidates on the real δ — including the penumbra
//! branch. A rejected direction goes to [`interval_terrain`], the SAME source
//! point, profile and terrain with the screening term set to zero. So the two
//! arms differ in exactly one thing: whether obstacles and barriers are consulted
//! at all. A false NEGATIVE therefore deletes a real screen outright. A false
//! POSITIVE is second order but NOT free, and the first version of this note said
//! it was: the admitted arc's endpoints become quadrature boundaries, so they move
//! the sub-interval midpoints the terrain is sampled at, and a piece that ends up
//! containing the cp azimuth reuses `cp_terrain`/`cp_screening` instead of
//! marching its own ray (see the `parts` loop below). So a false positive can
//! shift the TERRAIN term even when its own screening returns ~0 dB. The errors
//! are still strongly asymmetric — bounded requadrature against an unbounded lost
//! screen — and that is what makes a prefilter that is not a sound
//! over-approximation worse than none.
//!
//! Neither lane's was sound, and they were unsound in OPPOSITE directions,
//! which is how this surfaced (parity gate, rail 2206/1391 and 2208/1390):
//!
//! * here, `top_alt_m` was ONE elevation sample, taken at the merged arc's mid
//!   azimuth at `near_m` — the range of the NEAREST member. A fused chain spans a
//!   whole range stratum radially, so on rising ground that sample is
//!   systematically the lowest footing in the arc, and it is not even a member's
//!   own value. Pixel 86/507, source 3930: 19 of the 23 arcs clipping that span
//!   were rejected with tops 10-16 m BELOW the sight line, the segment scored
//!   0.75 dB against its own closest-point ray's 12.33, and the pixel came out
//!   5.91 dB LOUDER than CUDA. The chains stand on a hillside that rises ~10 %
//!   over the 200 m their stratum spans.
//! * the kernel sampled per EDGE, at that edge's own range, and fused
//!   `top = max`, so it read those same chains 11-22 m higher — but it also ran
//!   the test per RAW EDGE, which this lane cannot: the skyline is built once per
//!   RECEIVER and clipped by every segment of it, so a segment-dependent test
//!   cannot run before the merge. Per edge is the stricter rule, and pixel
//!   109/509 is what that costs: sources 3931/1879/1877/3932/3933 with a
//!   closest-point ray of 12.49 dB and an arc verdict of 0.00, i.e. the whole
//!   segment's screening thrown away.
//!
//! A SOUND version was built and measured before it was dropped: bound the
//! ground from above over the arc's own annulus sector instead of sampling a
//! point of it. It works, and it costs MORE than having no prefilter at all —
//! the bound needs raster reads per arc, while the extra pieces the wider
//! admission produces are cheap. On 2206/1391: 138.4 s of kernel with the sound
//! bound, 132.5 s with no test, and the no-test arm is the more accurate one.

use super::geo;
use super::iso9613::ground_or_barrier_db;
use super::obstacle_index::{
    origin_to_segment_dist, wrap_pi, CellPrune, CrossingCandidate, ObstacleSet, SkylineArc,
};
use super::path_effects::{
    screening_attenuation, screening_attenuation_with_meta, terrain_attenuation, ObstacleInput,
};
use super::screening_source_id::ScreeningSourceId;
use super::PathProfile;
use crate::constants::{m_per_deg_lon, BAND_FREQ, M_PER_DEG_LAT};
use crate::types::{
    Barrier, RasterSampler, ScreeningFanIntervalTrace, ScreeningFanObstacleTrace,
    ScreeningFanTrace, ScreeningObstacleTrace, BARRIER_PATH_HORIZON_M, NUM_BANDS,
};

use std::f64::consts::TAU;

/// Below this span the segment IS a point source at this receiver: the cp ray
/// already represents it, and the interval arithmetic would divide by ~0.
const DEGENERATE_SPAN_RAD: f64 = 1e-4;

/// A blocked interval is split into sub-evaluations of at most this width
/// (~15°). One evaluation stands for its whole sub-interval, so the quadrature
/// error grows with the arc it has to represent: a receiver 10 m behind a 300 m
/// noise wall has its ENTIRE fan blocked (~172°), and a fixed three-way split
/// leaves 57° per sample — measured at 0.63 dB against a 33-point reference,
/// against a 0.35 dB gate. Splitting to a fixed ANGULAR resolution instead
/// costs extra rays only where the blocked arc is genuinely wide, which is the
/// near field where obstacles are few.
const ESCALATE_SPAN_RAD: f64 = 0.26;
/// Ceiling on that split. A blocked fan wider than `ESCALATE_MAX_PARTS ×
/// ESCALATE_SPAN_RAD` (~140°) is a receiver walled in on all sides, where the
/// remaining sub-intervals all report the same deep screening anyway.
const ESCALATE_MAX_PARTS: usize = 9;

/// Width (m) of a skyline STRATUM in height. Two arcs may fuse only if their
/// heights land in the same `[k·tol, (k+1)·tol)` band.
///
/// Fusing takes `near = min` and `height = max` over the fused set and applies
/// them to the UNION of its azimuth ranges, so a fusion is honest exactly where
/// every member describes the same obstacle to within these strata.
///
/// A TOLERANCE (`|h_a − h_b| ≤ tol`) does not: it is not transitive, so a run of
/// arcs 3 m, 6 m, 9 m … chains into one record with no bound on the total
/// spread, and WHICH chain forms depends on the order the arcs arrive in. A
/// stratum is an equivalence relation, so the fused set is the connected
/// component of "same stratum AND overlapping in azimuth" — a function of the
/// arcs, not of their order — and the spread inside one record is bounded by
/// the stratum width by construction. Membership survives fusion (`min`/`max`
/// of two values in a band stay in that band), which is what makes the key
/// stable as an arc grows.
const ARC_FUSE_HEIGHT_TOL_M: f32 = 3.0;

/// Ratio between neighbouring RANGE strata: arcs may fuse only inside one
/// geometric band `[ρ^k, ρ^(k+1))` of `near_m`.
///
/// THE ONE THAT WAS MISSING, and the defect it closes is not a corner case.
/// `near = min` over the union of azimuth ranges means a fusion lends the
/// NEAREST member's range to every direction the fused set covers — including
/// directions where only a FAR member stands. Downstream that range is the
/// admission test `a = d − near > 1` ("does this stand in FRONT of the source"),
/// so a house at the receiver's own facade makes a village 600 m away read as
/// standing at the facade, and the whole fan is declared blocked. With
/// heights keyed alone, and 84.6 % of Praha's shading edges carrying one of
/// three literal values (3.00 / 8.00 / 6.00 m), nearly every overlapping pair is
/// fusible: the chain is the NORMAL state of a dense receiver, not an edge case.
///
/// Stratifying by range makes the merge a LAYERED angular store — one azimuth
/// arc list per (height, range) layer, the same idea as a deep shadow map
/// (Lokovic & Veach, SIGGRAPH 2000) but with exact arcs instead of quantised
/// sectors, so the blocked FRACTION of a span keeps its sub-degree resolution.
/// A far layer can no longer cast its shadow at a near layer's range; it faces
/// the admission test with its own.
///
/// SWEPT on scene `k` of `screening_fixture` — the bench built from a MEASURED
/// production receiver, where a fuse candidate is judged in a second of CPU — as
/// `(shadow, outside)` max dB against its 33-point reference, with the v3 wall
/// time beside it. The unstratified merge scores 1.50 / 1.12 dB at 0.14 s:
///
/// ```text
/// ρ    4.0        3.0        2.0        1.5        1.25       1.1
/// dB   0.27/0.45  0.76/0.77  0.23/0.24  0.23/0.25  0.24/0.25  0.24/0.25
/// s    0.33       0.31       0.35       0.43       1.16       —
/// ```
///
/// Converged at 2.0 and below. The non-monotone 3.0 / 4.0 pair is the tell that
/// a coarse stratum is PHASE-sensitive — whether the facade row and the row
/// behind it land in one band depends on where the band edges happen to fall.
///
/// A LATER, FINER SWEEP (2026-08-05) resolved that cliff and the plateau either
/// side of it. Failing gate values are confined to `ρ ∈ [3.0, 3.5]` (and 6.0);
/// 1.75 / 2.25 / 2.5 / 2.75 all pass with the same 0.23–0.42 dB as 1.5, so 2.0
/// is not "at the edge" — it is three measured steps clear, and the coarse
/// original grid (1.5 / 2 / 3 / 4) is the only reason it looked adjacent.
///
/// PHASE-SENSITIVITY BITES COST TOO, which is the part to leave alone: on the
/// real Praha receiver of `arc_anatomy` a WIDER band produced MORE merged arcs,
/// 395 at ρ = 3.0 against 403 at ρ = 4.0. So "widen the band, get fewer arcs"
/// is false in general, and a future tidy-up that widens ρ on that reasoning
/// can land on a value that is both slower AND less accurate. Sweep, do not
/// reason, and re-measure `arc_anatomy` before moving this constant.
///
/// 1.5 is KEPT over the 21 %-cheaper 2.0 (548 merged arcs against 691) because
/// the synthetic scenes cannot see what the real receiver shows: 2.0 loosens
/// the bounded range error from 1× to 2× and widens the directions where the
/// merged range is nearer than anything actually standing there from 490 to 645
/// of 3600. Fidelity on real geometry outranks 21 % of the arc list; revisit
/// only if a dense-tile cost measurement says the fuse is unaffordable. 1.25
/// buys nothing and costs 3×. Over the 1 m … 12 km a skyline can span this is
/// 24 strata, and two obstacles share one only when their ranges agree to 50 %,
/// where `δ ≈ h²·d/(2ab)` moves less than the height quantisation already does.
const ARC_FUSE_RANGE_RATIO: f32 = 1.5;

/// Range strata gathered per skyline GROWTH STEP.
///
/// The snap must land on a stratum boundary or a stratum is gathered half-in,
/// which is the order-dependence the strata exist to remove — but ρ^(k·m) is a
/// stratum boundary for EVERY integer k, so the step size is free. At k = 1 a
/// dense receiver re-enters `ensure` on almost every segment and pays an
/// annulus walk each time; at k = 3 neighbouring segments land on one radius
/// and the `from >= need` short-circuit takes them. MEASURED on the Praha
/// popup receiver (50.083/14.43, 10 822 arc-screened segments): skyline build
/// 1153 ms → 573 ms, and the answer moves at most 0.002 dB on 121 real
/// benchmark receivers (the extra arcs gathered are beyond the segment and die
/// on the admission test; what is left is f64 re-association).
const RADIUS_LADDER_STEP: f64 = 3.0;

/// Angular resolution floor of the blocked-fan quadrature (rad, ≈ 0.29°).
///
/// The boundary split gives every arc-boundary window its own evaluation ray
/// so the dominant screen wins in each direction. With the range strata one
/// physical panorama becomes several arcs per direction, so the WINDOW COUNT —
/// hence the ray count — grew 5.4× while the fan it resolves did not: Praha
/// went from 20 927 windows to 113 097, and a ray march is ~26 µs. This
/// coalesces ADJACENT windows of ONE blocked run until each is at least this
/// wide. Only windows inside one run ever merge, so the blocked and clear
/// EXTENTS are untouched to the bit — `blocked_fraction`, the clear fan and
/// the arc admission tests all see exactly what they saw before; what coarsens
/// is where the quadrature nodes sit inside a run.
///
/// It is the lower bound that [`ESCALATE_SPAN_RAD`] (0.26 rad) is the upper
/// one of: the fan is now sampled between 0.005 and 0.26 rad instead of
/// between 0 and 0.26. MEASURED at 0.005: Praha 113 097 → 29 096 windows,
/// 2908 ms → 481 ms of ray marching; `screening_fixture` scenes A-K move by
/// ≤ 0.03 dB and the verdict set is unchanged; on 121 real benchmark
/// receivers the worst per-layer shift is 0.30 dB and the worst total 0.10 dB,
/// with nothing over the owner's 0.5 dB line. 0.02 rad is 2.4× cheaper again
/// and breaches it (0.79 dB), so this is the value, not a starting point.
const ARC_QUADRATURE_MIN_RAD: f64 = 0.005;

/// Angular sectors the skyline is grown in. 64 gives 5.6° each — coarse
/// enough that the per-sector bookkeeping is a handful of compares, fine
/// enough that a narrow rail span touches one or two of them instead of the
/// whole disk. Well above the blessed `min_span_rad = 0.01 rad` (0.57°), so a
/// span that survives the small-span skip still lands in ≤ 2 sectors.
const SECTORS: usize = 64;
const SECTOR_RAD: f64 = TAU / SECTORS as f64;

/// Slack on "does this blocked interval contain the cp azimuth" (rad ≈ 2 µm at
/// 1 km). A receiver on a segment ENDPOINT — every other pixel of a straight
/// road, since adjacent microsegments share it — puts the cp exactly on a span
/// boundary, where the interval edges and the cp azimuth are the same number
/// computed two ways.
const CP_AZIMUTH_EPS: f64 = 1e-9;
/// One real octave-band value avoids inventing a source-spectrum-dependent
/// scalar for a geometric interval trace.
const SCREENING_FAN_TRACE_BAND: usize = 4;
const _: () = assert!(BAND_FREQ[SCREENING_FAN_TRACE_BAND] == 1000.0);
const SCREENING_FAN_TRACE_LARGEST_INTERVALS: usize = 32;

/// The bounds that turn the exact arc kernel into the shipped one. Read once
/// per process ([`ArcBounds::from_env`]); the defaults are the derived values
/// and the env names exist so the EXACT kernel can be painted for comparison
/// (`QM_ARC_EXACT=1`) and each constant swept without a rebuild.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcBounds {
    /// Hard cap on how far a receiver's skyline may be grown (m). NOT the
    /// working radius: [`ArcSkyline::ensure`] walks out to whatever each
    /// segment needs and no further.
    pub radius_m: f64,
    /// Angular span below which the cp verdict stands unchanged (rad).
    pub min_span_rad: f64,
    /// Path-difference floor for the grazing prune (m).
    pub delta_min_m: f64,
    /// Merged-arc capacity. `usize::MAX` on the CPU; the field exists because a
    /// CUDA thread cannot grow a list, and a swept value here reproduces what a
    /// capped lane would compute. Overflow merges the pair of neighbouring arcs
    /// separated by the SMALLEST angular gap — the least information any merge
    /// can destroy — which over-blocks by that gap; the interval's own
    /// evaluation ray then reports the truth, so the failure mode is one extra
    /// ray march, not a wrong shadow.
    pub max_arcs: usize,
    /// Blocked sub-intervals evaluated per (segment, receiver) pair —
    /// `usize::MAX` on the CPU, same reason. Extra intervals are dropped in
    /// azimuth order: they fall back to the clear treatment, i.e. LESS
    /// screening, never more.
    pub max_intervals: usize,
}

/// The EXACT kernel: no radius, no span skip, no grazing prune, no arc cap.
/// The reference every bounded variant is measured against.
pub const ARC_BOUNDS_EXACT: ArcBounds = ArcBounds {
    radius_m: f64::INFINITY,
    min_span_rad: 0.0,
    delta_min_m: 0.0,
    max_arcs: usize::MAX,
    max_intervals: usize::MAX,
};

/// Shipped bounds. Provenance of each number is in the module docs; the drift
/// each one costs is MEASURED, by painting the same cell twice through
/// `QM_ARC_*` and diffing with `tile-painter compare_hm3` — never assumed.
pub const ARC_BOUNDS_DEFAULT: ArcBounds = ArcBounds {
    // A SAFETY CAP, not the working radius: the skyline grows to whatever the
    // receiver's own segments need (`ArcSkyline::ensure`), and this only stops a
    // pathological source reach from walking a whole region. Above every layer's
    // audibility radius (SPEC §4.11), so it never binds in practice.
    radius_m: 12_000.0,
    // OFF (0.0) — the 0.01 rad value was MEASURED on the GPU lane and DOES NOT
    // HOLD on this one. Swept here against the exact (0.0) arm on receiver
    // lattices: at 0.01, dense Praha puts 25 road and 37 rail receivers over the
    // 1.0 dB hard line (maxima 7.8 / 11.4 dB), Dobříš 7 and 1 — and NO non-zero
    // value holds the line on either cell. It buys only 1.15-1.33× here, because
    // the CPU already banked that work through the wedge-limited gather, the
    // slab reject and the pyramid rejector; the GPU's 5.5× was measured before
    // any of those existed. Up to 11 dB for 2-11 % of wall time is not a trade.
    //
    // ROOT CAUSE (hypothesis, evidenced): this one number drives TWO different
    // tests. The pre-gate `L ≤ min_span·d` ([`segment_can_span`]) is a genuine
    // point-source test and is sound — the fixture census shows it firing ZERO
    // times at every swept value. The in-kernel test near line 689 skips
    // whenever the ACTUAL span is small, which ALSO fires for a segment seen
    // nearly END-ON: a radial line, not a point, where "the cp ray IS that
    // direction" is false. In a street grid a receiver is nearly collinear with
    // hundreds of close, energetic segments, which is why Praha degrades ~5×
    // harder than Dobříš. Re-enabling this needs the two thresholds SEPARATED,
    // not a different constant. `QM_ARC_MIN_SPAN_DEG` is the lever to re-measure.
    min_span_rad: 0.0,
    // OFF (0.0). The grazing prune is TERRAIN-BLIND as formulated — it compares
    // a cell's tallest edge against the sight line's height above the RECEIVER's
    // ground, so a building standing 30 m up a hillside reads 8 m tall and gets
    // skipped. Measured on the hilly Dobříš rail tile (`compare_hm3`, δ_min =
    // λ/8 at 8 kHz = 0.0053 m vs no prune): max 3.0 dB, 148 cells > 0.5 dB, 31
    // cells > 1.0 dB — a hard fail, for a 1.8× cell-paint saving. It comes back
    // when the per-cell bound carries the cell's own terrain (the store has no
    // elevation today); `QM_ARC_DELTA_MIN_M` is the lever to re-measure with.
    delta_min_m: 0.0,
    // No cap. A GPU thread needs fixed arrays, the CPU does not — and the caps
    // are not free: 48 arcs / 16 intervals measured max 1.0 dB against the
    // uncapped kernel on the same tile (5 cells > 0.5 dB). Track C removes them
    // structurally on the GPU side (per-footprint CSR); the CPU simply grows.
    max_arcs: usize::MAX,
    max_intervals: usize::MAX,
};

/// Can this segment subtend `bounds.min_span_rad` at a receiver `dist_m` away?
///
/// The gate every caller runs BEFORE building an [`ArcScreening`], because the
/// answer is usually no and the alternative is two `atan2` per (segment,
/// receiver) pair — measured at 71 % of the whole pre-fix-pack line kernel on a
/// dense rail tile, i.e. the cost of merely ASKING.
///
/// `span ≤ L / d` is exact, not a heuristic: with the foot on the segment the
/// span is `2·atan(L/2p) ≤ L/p ≤ L/d`; past an endpoint it is
/// `atan(L·p/(p² + e(e+L))) ≤ L·p/d² ≤ L/d`. `d` is the distance to the NEAREST
/// point of the segment (`seg.dist_m`), the same one the reach cull uses.
#[inline]
pub fn segment_can_span(length_m: f64, dist_m: f64, bounds: ArcBounds) -> bool {
    length_m > bounds.min_span_rad * dist_m
}

impl ArcBounds {
    /// Process-wide bounds. `QM_ARC_EXACT=1` selects [`ARC_BOUNDS_EXACT`];
    /// `QM_ARC_RADIUS_M`, `QM_ARC_MIN_SPAN_DEG`, `QM_ARC_DELTA_MIN_M`,
    /// `QM_ARC_MAX_ARCS` and `QM_ARC_MAX_INTERVALS` override one field each.
    /// `QM_ARC_MIN_SPAN_DEG=180` is the PRE-fix-pack kernel — vector obstacles
    /// on, no segment wide enough to arc-clip, i.e. the cp verdict everywhere —
    /// which is how the fix-pack's own cost is measured. Kernels never read the
    /// environment; loaders and binaries call this once and thread the value.
    pub fn from_env() -> Self {
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok());
        if std::env::var("QM_ARC_EXACT").is_ok_and(|v| v == "1") {
            return ARC_BOUNDS_EXACT;
        }
        let mut b = ARC_BOUNDS_DEFAULT;
        if let Some(v) = num("QM_ARC_MIN_SPAN_DEG") {
            b.min_span_rad = v.to_radians();
        }
        if let Some(v) = num("QM_ARC_RADIUS_M") {
            b.radius_m = v;
        }
        if let Some(v) = num("QM_ARC_DELTA_MIN_M") {
            b.delta_min_m = v;
        }
        if let Some(v) = num("QM_ARC_MAX_ARCS") {
            b.max_arcs = v as usize;
        }
        if let Some(v) = num("QM_ARC_MAX_INTERVALS") {
            b.max_intervals = v as usize;
        }
        b
    }

    /// Cached [`Self::from_env`] — the one process-wide read.
    pub fn shipped() -> Self {
        static V: std::sync::OnceLock<ArcBounds> = std::sync::OnceLock::new();
        *V.get_or_init(Self::from_env)
    }
}

/// One (segment, receiver) arc-screening query.
pub struct ArcScreening<'a> {
    pub receiver_lat: f64,
    pub receiver_lon: f64,
    pub receiver_alt_m: f64,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    /// Source height above ground, applied to the DEM at every interpolated
    /// source point (`normalize::…source_height_m`, `SOURCE_HEIGHT_RAIL`).
    pub source_height_m: f64,
    /// The characteristic point the caller already evaluated (`seg.cp_*`)…
    pub cp_lat: f64,
    pub cp_lon: f64,
    /// …its absolute source altitude (DEM + source height), which fixes the
    /// sight line the grazing prune measures obstacles against…
    pub src_alt_m: f64,
    /// …and its screening bands: the degenerate-span fallback, the evaluation
    /// reused for whichever interval contains the cp azimuth, and the verdict
    /// the CLEAR fraction carries when the cp azimuth lies in it.
    pub cp_screening: &'a [f64; NUM_BANDS],
    /// The cp ray's terrain bands and path-averaged ground factor — the other
    /// two inputs of the kernel's `max(A_ground, A_terrain + A_screen)`, needed
    /// to average that term instead of the bare screening increment.
    pub cp_terrain: &'a [f64; NUM_BANDS],
    pub ground_g: f64,
    pub barriers: &'a [Barrier],
    pub obstacles: &'a ObstacleSet,
    /// Segment length and the receiver's distance to its NEAREST point — the
    /// pre-gate's `span ≤ L/d`, and the radius the skyline must reach
    /// (`d + L` bounds the far end of the segment, hence every screen).
    pub length_m: f64,
    pub dist_m: f64,
    pub exclusion_radius_m: f64,
    pub bounds: ArcBounds,
}

/// Per-call ray buffers, amortised across every (segment, receiver) pair of a
/// compute call — the same pattern as the callers' crossing scratch. The
/// receiver SKYLINE is a SEPARATE object ([`ArcSkyline`]) because the two have
/// different lifetimes: the popup kernels loop segments inside one receiver and
/// hold one of each, while the tile scatter loops receivers inside one source
/// and holds one skyline PER PIXEL of the block it owns.
#[derive(Default)]
pub struct ArcScreeningScratch {
    profile: PathProfile,
    candidates: Vec<CrossingCandidate>,
    intervals: Vec<BlockedInterval>,
    /// Split-step buffers (the arc-boundary cuts and the pre-split intervals).
    /// Here rather than local so the split does not allocate twice per
    /// arc-screened segment — a dense popup runs it ~10 k times.
    cuts: Vec<f64>,
    presplit: Vec<BlockedInterval>,
}

impl ArcScreeningScratch {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The merged azimuth arcs of everything standing around ONE receiver: what a
/// 360° panorama would show, reduced to "in these directions something blocks,
/// at this range, with this top". Built once per receiver and clipped by every
/// segment of that receiver against its own angular span.
pub struct ArcSkyline {
    lat: f64,
    lon: f64,
    /// How far out this receiver's skyline has been walked (m), PER ANGULAR
    /// SECTOR. Growing it is an annulus walk restricted to the sectors a
    /// segment's span touches, so a receiver pays for each (cell, sector)
    /// exactly once.
    ///
    /// A disk gather was the single largest wasted term in the arc: a segment
    /// can only ever be clipped by obstacles inside its OWN angular span, so a
    /// rail segment 3 km out with a 2° span used ~180× the area its query can
    /// read. Sector granularity is a COST knob, not a correctness one — a
    /// coarse sector over-gathers a little, a fine one costs bookkeeping —
    /// because an obstacle outside the span cannot clip the span. Exact,
    /// therefore gated byte-identical.
    built_radius_m: [f32; SECTORS],
    built: bool,
    arcs: Vec<MergedArc>,
    /// Slots [`insert_merged`] absorbed on the current call. Lives here only so
    /// the walk does not allocate once per raw arc (~10⁶ per dense receiver);
    /// it carries no state between calls.
    fuse_scratch: Vec<u32>,
    /// Times the arc capacity forced a smallest-gap merge while building this
    /// skyline — 0 on every receiver of a normal cell; a non-zero total is the
    /// signal that a capped lane's [`ArcBounds::max_arcs`] is too small here.
    pub overflows: u32,
    /// Largest gather need (m, ladder-snapped, post census clip) this receiver
    /// ever asked for — census telemetry only, flushed on [`Self::reset`].
    need_max_m: f64,
}

impl Default for ArcSkyline {
    fn default() -> Self {
        ArcSkyline {
            lat: 0.0,
            lon: 0.0,
            built_radius_m: [0.0; SECTORS],
            built: false,
            arcs: Vec::new(),
            fuse_scratch: Vec::new(),
            overflows: 0,
            need_max_m: 0.0,
        }
    }
}

/// One merged blocked arc. `lo`/`hi` are absolute azimuths with `lo <= hi`;
/// `near_m` is the closest range in the arc, and since the δ prefilter went it
/// has exactly ONE consumer left — the admission test's `a = d − near` and
/// `near ≥ 1` floors. No diffraction formula reads it: [`BlockedInterval`] carries
/// only the azimuths, and `interval_screening` re-derives δ from the marched
/// profile. There is no top altitude on this record either (see the module note
/// "Why the blocked test is geometry only").
#[derive(Clone, Copy, Debug)]
struct MergedArc {
    lo: f64,
    hi: f64,
    near_m: f32,
    height_m: f32,
    /// The (height, range) STRATUM this arc belongs to — see [`fuse_key`]. The
    /// list is sorted by `(key, lo)`, so one stratum's arcs are contiguous and
    /// disjoint, and only same-key arcs ever fuse.
    key: i32,
}

/// The stratum an arc belongs to: its height band and its geometric range band,
/// packed into one comparable integer.
///
/// This is the merge's equivalence relation, and it has to be INVARIANT under
/// fusion or the merge stops being a function of its input: fusing takes
/// `height = max` and `near = min`, and both stay inside the band their members
/// share, so an arc's key never moves as it grows.
#[inline]
fn fuse_key(near_m: f32, height_m: f32) -> i32 {
    let h = (height_m.max(0.0) / ARC_FUSE_HEIGHT_TOL_M).floor() as i32;
    // `near_m` is > 0 by the walk's own reject (`near_m < 1e-6` never reaches
    // here); clamp anyway so no degenerate value can produce a NaN key, which
    // would compare unequal to itself and break the sort.
    let r = (near_m.max(1e-3).ln() / ARC_FUSE_RANGE_RATIO.ln()).floor() as i32;
    // Biased so the packed key is MONOTONE in (height stratum, range stratum);
    // only equality matters to the merge, but a monotone key keeps the sorted
    // list readable in a debugger and the strata in physical order.
    (h.clamp(0, 16_000) << 16) | ((r.clamp(-16_000, 16_000) + 0x4000) & 0xffff)
}

impl MergedArc {
    /// Union `other` into this arc, keeping the nearest range and the tallest
    /// top. Both directions make the keep-test easier, so a fusion can only
    /// cost work, never discard a real screen.
    ///
    /// INVARIANT: `hi - lo <= TAU`. `lo`/`hi` are UNWRAPPED azimuths, so a
    /// chain of absorptions can otherwise push the width PAST a full turn —
    /// an arc that no longer denotes a set of directions, and that the ±TAU
    /// clip in [`arc_screened_attenuation`] cannot express. A full turn
    /// already means "blocked in every direction", so saturating there is
    /// exact, not conservative: no direction is lost and none is invented.
    #[inline]
    fn absorb(&mut self, other: &MergedArc) {
        self.lo = self.lo.min(other.lo);
        self.hi = self.hi.max(other.hi);
        if self.hi - self.lo > TAU {
            self.hi = self.lo + TAU;
        }
        self.near_m = self.near_m.min(other.near_m);
        self.height_m = self.height_m.max(other.height_m);
        debug_assert_eq!(
            self.key, other.key,
            "arcs of different strata must never fuse (see `fuse_key`)"
        );
        debug_assert_eq!(
            self.key,
            fuse_key(self.near_m, self.height_m),
            "fusion moved the stratum key — `min`/`max` left the band"
        );
        debug_assert!(
            self.hi - self.lo <= TAU + 1e-9,
            "merged arc wider than a full turn: {} rad",
            self.hi - self.lo
        );
    }
}

/// The `ensure` call an arc query will make, precomputed from geometry alone
/// (receiver, segment endpoints, `dist + length`). `None` when the span is
/// degenerate or under `bounds.min_span_rad` — those queries return the cp
/// verdict before ever touching the skyline, so there is nothing to grow.
///
/// This is the scheduling half of the popup's parallel segment loop: the
/// kernels replay the EXACT ensure sequence the sequential loop would run
/// ([`ArcSkyline::needs_growth`] + [`ArcSkyline::ensure_planned`], in segment
/// order, on one thread) and evaluate everything between two growth steps in
/// parallel against a [`SkylineSnapshot`] of the state those segments would
/// have read. Splitting the plan from the evaluation is what makes that
/// replay affordable without paying for any evaluation on the scheduling
/// thread.
#[derive(Clone, Copy, Debug)]
pub struct PlannedEnsure {
    span_lo: f64,
    span_hi: f64,
    need_radius_m: f64,
}

/// The span + radius [`arc_screened_attenuation`] hands to `ensure`, or `None`
/// when its pre-ensure early-outs fire. This IS that function's opening block
/// (it calls this), so the scheduler's replay cannot diverge from the
/// sequential kernel by even one growth step.
#[allow(clippy::too_many_arguments)]
pub fn planned_ensure(
    receiver_lat: f64,
    receiver_lon: f64,
    start_lat: f64,
    start_lon: f64,
    end_lat: f64,
    end_lon: f64,
    dist_m: f64,
    length_m: f64,
    bounds: ArcBounds,
) -> Option<PlannedEnsure> {
    let frame = Frame::at(receiver_lat, receiver_lon);
    let base = frame.azimuth(start_lat, start_lon);
    let delta = wrap_pi(frame.azimuth(end_lat, end_lon) - base);
    let span = delta.abs();
    // Small-span skip: below the threshold the segment radiates in essentially
    // one direction and the cp ray IS that direction.
    if span < DEGENERATE_SPAN_RAD || span < bounds.min_span_rad {
        return None;
    }
    // Span in absolute azimuth; a segment subtends at most π at a receiver off
    // its line, so the short arc between the endpoints IS it.
    let (lo, hi) = if delta < 0.0 {
        (base + delta, base)
    } else {
        (base, base + delta)
    };
    Some(PlannedEnsure {
        span_lo: lo,
        span_hi: hi,
        need_radius_m: dist_m + length_m,
    })
}

/// An immutable copy of a skyline's merged-arc list, shared across threads by
/// `Arc`. The evaluation side of the arc rule reads NOTHING of the skyline but
/// this list — `ensure` is the only mutator — so a snapshot taken between two
/// growth steps reproduces, bit for bit, the state every segment before the
/// next growth would have read from the live skyline.
#[derive(Clone)]
pub struct SkylineSnapshot {
    arcs: std::sync::Arc<[MergedArc]>,
}

/// The growth radius snapped UP to a range-stratum boundary — the ladder rule
/// documented inside [`ArcSkyline::ensure`]. One function, shared by `ensure`
/// and [`ArcSkyline::needs_growth`], so the two can never disagree on a rung.
#[inline]
fn snapped_need(need_radius_m: f64, bounds: ArcBounds) -> f64 {
    let rungs =
        (need_radius_m.max(1.0).ln() / (ARC_FUSE_RANGE_RATIO as f64).ln()) / RADIUS_LADDER_STEP;
    let ladder = RADIUS_LADDER_STEP * rungs.ceil();
    // `QM_ARC_NEED_CLIP_M` (census DEV lever, ∞ in production): bound the
    // gather to measure the near-field skyline demand of the gather redesign's
    // increment D. Output-changing when finite — census runs only. It lives
    // HERE so `ensure` and `needs_growth` agree under the clip as well.
    (ARC_FUSE_RANGE_RATIO as f64)
        .powf(ladder)
        .min(bounds.radius_m)
        .min(super::census::need_clip_m())
}

/// First and last angular sector a span touches (inclusive, un-wrapped — every
/// consumer applies `rem_euclid` per index). Shared by `ensure` and
/// [`ArcSkyline::needs_growth`].
#[inline]
fn sector_span(span_lo: f64, span_hi: f64) -> (i64, i64) {
    (
        (span_lo / SECTOR_RAD).floor() as i64,
        (span_hi / SECTOR_RAD).floor() as i64,
    )
}

/// "Already walked to this rung": the short-circuit of `ensure` and
/// [`ArcSkyline::needs_growth`], in F32 SPACE on both sides.
///
/// `built_radius_m` stores radii as f32 while `snapped_need` computes the rung
/// in f64, and roughly half the rungs round DOWN in storage — a raw
/// `from >= need` then reads "not yet walked" forever for that rung, so every
/// same-rung call re-walks an annulus that is provably already gathered. In
/// the old sequential kernel that was only wasted time (`insert_merged`
/// re-inserts are byte-neutral); on the popup's growth scheduler it also
/// re-snapshots the skyline per segment — an O(segments × arcs) memory hazard
/// (found by review 2026-08-09). Rounding `need` through f32 makes the test
/// agree with the bookkeeping exactly; the elided walks are exact re-walks,
/// so outputs are unchanged to the bit.
#[inline]
fn walked_to(from: f64, need: f64) -> bool {
    from >= f64::from(need as f32)
}

impl ArcSkyline {
    /// Number of merged arcs (telemetry / tests).
    pub fn len(&self) -> usize {
        self.arcs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.arcs.is_empty()
    }

    /// Least-grown radius over the sectors `[s0, s1]` touch — the `from` of
    /// the annulus walk and the short-circuit input of [`Self::needs_growth`].
    #[inline]
    fn min_built_in(&self, s0: i64, s1: i64) -> f64 {
        (s0..=s1)
            .map(|k| self.built_radius_m[k.rem_euclid(SECTORS as i64) as usize] as f64)
            .fold(f64::INFINITY, f64::min)
    }

    /// Would [`Self::ensure_planned`] with these inputs walk (grow) this
    /// skyline, or is it a no-op? EXACTLY the short-circuit test `ensure`
    /// runs — same ladder snap, same sector minimum, through the same shared
    /// helpers — so the popup's epoch scheduler and `ensure` cannot drift.
    /// A no-growth ensure on the same receiver mutates nothing (no reset,
    /// early return before the gather), which is what lets the scheduler
    /// elide it entirely.
    pub fn needs_growth(&self, lat: f64, lon: f64, p: &PlannedEnsure, bounds: ArcBounds) -> bool {
        if !self.built || self.lat != lat || self.lon != lon {
            return true;
        }
        let (s0, s1) = sector_span(p.span_lo, p.span_hi);
        !walked_to(
            self.min_built_in(s0, s1),
            snapped_need(p.need_radius_m, bounds),
        )
    }

    /// [`Self::ensure`] with a precomputed [`PlannedEnsure`] — the form the
    /// popup's segment scheduler drives the growth chain with.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_planned(
        &mut self,
        lat: f64,
        lon: f64,
        p: &PlannedEnsure,
        set: &ObstacleSet,
        barriers: &[Barrier],
        source_height_m: f64,
        bounds: ArcBounds,
    ) {
        self.ensure(
            lat,
            lon,
            p.span_lo,
            p.span_hi,
            p.need_radius_m,
            set,
            barriers,
            source_height_m,
            bounds,
        );
    }

    /// Freeze the current merged-arc list for read-only parallel evaluation.
    pub fn snapshot(&self) -> SkylineSnapshot {
        SkylineSnapshot {
            arcs: self.arcs.as_slice().into(),
        }
    }

    /// Rebuild for `(lat, lon)` unless this is already that receiver's skyline.
    /// Mark every cached skyline stale (the tile scatter reuses one array of
    /// them per receiver block).
    pub fn reset(&mut self) {
        // Census flush (QM_TILE_CENSUS=1): the tile lane resets per receiver,
        // so a built skyline dying here IS one receiver's final demand. The
        // popup's mid-`ensure` receiver switch does not flush — census runs are
        // tile builds.
        if self.built {
            super::census::skyline_receiver(self.arcs.len(), self.need_max_m);
        }
        self.built = false;
        self.built_radius_m = [0.0; SECTORS];
        self.arcs.clear();
        self.overflows = 0;
        self.need_max_m = 0.0;
    }

    /// Make this skyline cover `need_radius_m` around `(lat, lon)`, walking only
    /// what it has not walked yet.
    ///
    /// The radius a caller needs is the far end of ITS segment — everything that
    /// can screen a path stands between receiver and source, so a skyline that
    /// reaches the source reaches every screen. Growing on demand is what lets
    /// that be exact without walking a rail layer's whole 10 km reach for a
    /// receiver whose segments are all close.
    #[allow(clippy::too_many_arguments)]
    fn ensure(
        &mut self,
        lat: f64,
        lon: f64,
        span_lo: f64,
        span_hi: f64,
        need_radius_m: f64,
        set: &ObstacleSet,
        barriers: &[Barrier],
        source_height_m: f64,
        bounds: ArcBounds,
    ) {
        // Snap the growth radius UP to a range-stratum boundary, which is what
        // makes the strata finish the order-invariance argument instead of only
        // half of it.
        //
        // The skyline is grown to whatever the CURRENT segment needs, so a
        // segment evaluated after a distant one is clipped against arcs that
        // stand beyond its own reach. Those are meant to fall to the admission
        // test (`a = d − near > 1`), and with the strata they do — but only if
        // every arc's own stratum was gathered WHOLE. Left unsnapped, the
        // radius cuts through a stratum: a near member of it is in, a far member
        // is not, they fuse, and the fused record's range depends again on who
        // came first. Snapping puts every boundary on the ladder the strata
        // already use, so a stratum is either entirely gathered or entirely
        // absent and the surviving set is the same in every order.
        //
        // Measured free (sparse Praha-region road tile, `ETA=0`, full 262 144
        // px): the order-induced RMS falls 0.0049 → 0.0016 dB and the ≥45 dB
        // pixels — the ones the map shows — go to 0.0000 dB RMS / 0.0004 dB max,
        // at 51.3 s against 52.3 s unsnapped. Snapping does not only cost area,
        // it also lands neighbouring segments on the SAME radius, where the
        // `from >= need` short-circuit skips the walk outright.
        //
        // THE ONE ORDER-DEPENDENT INPUT LEFT is `built_radius_m` being PER
        // SECTOR: two arcs that fuse across a sector boundary can find their two
        // sectors grown to different radii. Isolated on the same tile by growing
        // every sector together — the order shift then reads RMS 0.0000 /
        // max −0.0000 dB over all 262 144 pixels, i.e. EXACT — for 2× the wall
        // time (104 s against 52 s). The wedge stays and its residual is stated:
        // max +0.39 dB at one pixel, 0 pixels over 0.5 dB, 0.0004 dB over the
        // ≥45 dB ones. Passing `wedge: None` to the gather below is not the same
        // experiment: it widens the GATHER, not the bookkeeping.
        let need = snapped_need(need_radius_m, bounds);
        self.need_max_m = self.need_max_m.max(need);
        if !self.built || self.lat != lat || self.lon != lon {
            self.lat = lat;
            self.lon = lon;
            self.built = true;
            self.built_radius_m = [0.0; SECTORS];
            self.arcs.clear();
            self.overflows = 0;
        }
        // Sectors this span touches. The span is under a full turn, so the
        // touched set is one contiguous run on the circle (possibly wrapping).
        let (s0, s1) = sector_span(span_lo, span_hi);
        // The annulus to walk is bounded by the LEAST-grown sector in the
        // wedge; sectors already further out are re-visited, and `insert_merged`
        // is idempotent on a repeat, so that costs time and never correctness.
        //
        // This gate and [`Self::needs_growth`] are the same computation through
        // the same helpers (`walked_to` carries the f32 bookkeeping round-trip)
        // — the popup scheduler relies on the equivalence.
        let from = self.min_built_in(s0, s1);
        if walked_to(from, need) {
            return;
        }

        // The LOWEST the sight line ever runs above local ground on a path into
        // this receiver: the SOURCE height. The line falls from receiver height
        // at this end to source height at the far end, so only an obstacle
        // shorter than the SOURCE can be ruled out a priori — gating on the
        // receiver's own 4 m would drop every 3 m noise wall and low building,
        // which is precisely the geometry whose whole purpose is screening.
        let los_floor_m = source_height_m.max(0.0);
        let cap = bounds.max_arcs;
        let arcs = &mut self.arcs;
        let fuse = &mut self.fuse_scratch;
        let mut overflows = self.overflows;
        // Gather the WHOLE of every sector this call is about to mark walked,
        // not just the caller's span. `built_radius_m` carries one radius per
        // SECTOR, so a narrower gather claims coverage it never collected: the
        // next span landing in the same sector short-circuits on `from >= need`
        // and reads an EMPTY sky — no blocked interval, the closest-point
        // verdict stands for that whole segment, and the constant-width shadow
        // stripe this module exists to remove comes back for it alone, silently
        // (pinned by `a_second_span_in_the_same_sector_still_sees_its_obstacles`;
        // found by review 2026-08-04, invisible to the gather-level wedge tests
        // because those never go through this bookkeeping).
        //
        // Snapping outward costs little: the win is area, θ/2π, and the spans
        // that dominate already cover several sectors. A span may reach just
        // under a half turn, and snapping can push it past one, which the
        // gather's two-half-plane wedge cannot express — so a wedge that wide
        // falls back to the disk, which over-collects and is always sound.
        let (w_lo, w_hi) = (s0 as f64 * SECTOR_RAD, (s1 + 1) as f64 * SECTOR_RAD);
        let wedge = (w_hi - w_lo < 0.5 * TAU).then_some((w_lo, w_hi));
        set.skyline_arcs_within(
            lat,
            lon,
            from,
            need,
            los_floor_m,
            bounds.delta_min_m,
            wedge,
            &mut |a: SkylineArc| overflows += insert_merged(arcs, cap, a, fuse),
        );

        // Noise WALLS are the other half of the skyline. They are deliberately
        // NOT in `ObstacleIndex` (three tracks are rewriting that index), so
        // they arrive as the caller's own slice — but a wall IS a segment, so
        // its arc is the same primitive a building edge produces: the short arc
        // between its endpoint azimuths, at its nearest range. Without this a
        // receiver behind a wall gets NO blocked interval, the cp verdict
        // stands for the whole segment, and the constant-width shadow band
        // survives exactly where a wall makes it most visible.
        //
        // Re-scanned on every growth step rather than annulus-clipped: the
        // slice is a tile's worth of walls, and `insert_merged` is idempotent
        // on a repeat (an arc unioned with itself is itself).
        let m_lon = m_per_deg_lon(lat.to_radians());
        for b in barriers {
            // `dist_m` is a LOWER BOUND on the receiver→midpoint distance, and a
            // wall reaches `BARRIER_SEGMENT_MAX_HALF_LEN_M` past its midpoint —
            // hence the horizon slack. Everything after this is exact geometry.
            if b.dist_m > need + BARRIER_PATH_HORIZON_M || b.height_m as f64 <= los_floor_m {
                continue;
            }
            let (x0, y0) = (
                (b.start_lon - lon) * m_lon,
                (b.start_lat - lat) * M_PER_DEG_LAT,
            );
            let (x1, y1) = ((b.end_lon - lon) * m_lon, (b.end_lat - lat) * M_PER_DEG_LAT);
            let near_m = origin_to_segment_dist(x0, y0, x1, y1);
            if near_m > need || near_m < 1e-6 {
                continue; // out of the walked radius, or the receiver is ON it
            }
            let a0 = y0.atan2(x0);
            let r1 = a0 + wrap_pi(y1.atan2(x1) - a0);
            overflows += insert_merged(
                arcs,
                cap,
                SkylineArc {
                    source_id: ScreeningSourceId::wall(b.osm_id, b.segment_idx)
                        .expect("barrier provenience outside the packed ABI"),
                    lo: a0.min(r1),
                    hi: a0.max(r1),
                    near_m: near_m as f32,
                    height_m: b.height_m,
                },
                fuse,
            );
        }
        self.overflows = overflows;
        for k in s0..=s1 {
            let i = k.rem_euclid(SECTORS as i64) as usize;
            self.built_radius_m[i] = self.built_radius_m[i].max(need as f32);
        }
    }
}

/// Insert one raw arc into the sorted, disjoint merged list, unioning it with
/// every arc it overlaps. Returns 1 if the capacity forced an extra merge.
///
/// Merged arcs take the MINIMUM range and the MAXIMUM height of their members
/// and hand both to the UNION of their azimuth ranges, so the fused set has to
/// be one STRATUM — see [`fuse_key`]. Inside a stratum the two directions only
/// keep the arc alive through the admission test, so a fusion costs at most an
/// evaluation ray and never a lost screen; ACROSS strata the same `min` invents
/// an obstacle at a range where nothing stands, which is what `fuse_key`
/// forbids.
///
/// `absorbed` is caller-owned scratch, cleared on entry; see below for why the
/// absorbed slots are MARKED rather than removed as the scan finds them.
fn insert_merged(
    v: &mut Vec<MergedArc>,
    cap: usize,
    a: SkylineArc,
    absorbed: &mut Vec<u32>,
) -> u32 {
    let mut m = MergedArc {
        lo: a.lo,
        hi: a.hi,
        near_m: a.near_m,
        height_m: a.height_m,
        key: fuse_key(a.near_m, a.height_m),
    };
    // Sorted by `(key, lo)`: one STRATUM's arcs are contiguous, and inside a
    // stratum they are sorted and DISJOINT, because every overlapping pair of
    // that stratum has already been fused. Sorting by `lo` alone (what this did
    // while every arc was in one class) cannot support that: with several strata
    // interleaved, the one arc of `m`'s own stratum that overlaps `m.lo` may sit
    // several slots left of the insertion point, and the single left-neighbour
    // test below would miss it — leaving two overlapping arcs of ONE stratum in
    // the list, which is a legal answer physically (the boundary split covers
    // the same directions) but an ORDER-DEPENDENT one: whether they are two arcs
    // or one would depend on what arrived between them.
    //
    // The scan below is the `Vec::remove`-as-it-goes one it replaces, decision
    // for decision — same start, same order, same fusion tests, same `m`. It
    // only MARKS the slots it absorbs and closes the gaps once, at the end.
    // `remove` shifts the whole TAIL per absorption and the closing `insert`
    // shifts it once more; a dense receiver's skyline is ~10³ arcs and its walk
    // offers ~10⁶ raw ones (an edge is re-emitted for every grid cell it
    // spans), and nearly every one of those absorbs EXACTLY ONE existing arc —
    // leaving the list the same length, so all that tail traffic put the bytes
    // back where they already were. Measured at 32 % of a São Paulo popup.
    // That case now writes a single slot and touches nothing beyond it.
    absorbed.clear();
    let mut i = v.partition_point(|x| (x.key, x.lo) < (m.key, m.lo));
    // At most ONE arc of this stratum can overlap `m.lo`, and it is the one
    // immediately left: the stratum's arcs are disjoint and sorted by `lo`.
    if i > 0 && v[i - 1].key == m.key && v[i - 1].hi >= m.lo {
        i -= 1;
        m.absorb(&v[i]);
        absorbed.push(i as u32);
    }
    let mut fused = true;
    while fused {
        fused = false;
        let mut j = i;
        while j < v.len() {
            // An absorbed slot is one the original had already removed: step
            // over it. It can never END the scan either — `m` swallowed its
            // `hi`, so `v[j].lo <= m.hi` holds for it by construction.
            if absorbed.contains(&(j as u32)) {
                j += 1;
                continue;
            }
            // Past this stratum's run, or past `m`'s reach inside it.
            if v[j].key != m.key || v[j].lo > m.hi {
                break;
            }
            m.absorb(&v[j]);
            absorbed.push(j as u32);
            fused = true; // a wider `m` can reach arcs the scan already passed
            j += 1;
        }
    }
    if absorbed.is_empty() {
        v.insert(i, m);
    } else {
        absorbed.sort_unstable();
        // `m` takes the FIRST absorbed slot, so the only run that grows is the
        // one between the insertion point and it — at most the arcs the
        // left-neighbour test walked, never the tail.
        let a0 = absorbed[0] as usize;
        v.copy_within(i..a0, i + 1);
        v[i] = m;
        // Every LATER absorbed slot is a plain deletion: the run following it
        // slides left by one more than the run before it.
        for k in 2..absorbed.len() {
            let from = absorbed[k - 1] as usize + 1;
            let to = absorbed[k] as usize;
            v.copy_within(from..to, from - (k - 1));
        }
        let shift = absorbed.len() - 1;
        if shift > 0 {
            let from = absorbed[absorbed.len() - 1] as usize + 1;
            let len = v.len();
            v.copy_within(from..len, from - shift);
            v.truncate(len - shift);
        }
    }

    // Over capacity: absorb the smallest gap, repeatedly if the cap shrank.
    // Deterministic (first minimum wins), so the CPU and a capped GPU lane
    // degrade identically. This IS the enveloping merge — it only runs under
    // capacity pressure, which the CPU never has (`max_arcs = usize::MAX`).
    //
    // Neighbours of the SAME stratum first: those are the merges that cost only
    // an evaluation ray. Crossing a stratum is the very thing `fuse_key` exists
    // to prevent, so it is the LAST resort, taken only when no same-stratum pair
    // is left — and then the merged record's key is recomputed, because the
    // record no longer belongs to either member's band.
    let mut capped = 0;
    while v.len() > cap {
        if v.len() < 2 {
            break; // nothing left to merge WITH (only reachable at cap = 0)
        }
        let mut k = usize::MAX;
        let mut best = f64::INFINITY;
        for w in 0..v.len() - 1 {
            if v[w + 1].key != v[w].key {
                continue;
            }
            let gap = v[w + 1].lo - v[w].hi;
            if gap < best {
                best = gap;
                k = w;
            }
        }
        if k == usize::MAX {
            for w in 0..v.len() - 1 {
                let gap = (v[w + 1].lo - v[w].hi).abs();
                if gap < best {
                    best = gap;
                    k = w;
                }
            }
        }
        let next = v[k + 1];
        let (lo, hi) = (v[k].lo.min(next.lo), v[k].hi.max(next.hi));
        let near_m = v[k].near_m.min(next.near_m);
        let height_m = v[k].height_m.max(next.height_m);
        v[k] = MergedArc {
            lo,
            hi: if hi - lo > TAU { lo + TAU } else { hi },
            near_m,
            height_m,
            key: fuse_key(near_m, height_m),
        };
        v.remove(k + 1);
        capped = 1;
    }
    if capped == 1 {
        // A forced cross-stratum merge can move a record's key, so the list is
        // no longer sorted by it. Restore the invariant the next insert relies on.
        v.sort_unstable_by(|a, b| (a.key, a.lo).partial_cmp(&(b.key, b.lo)).unwrap());
    }
    capped
}

/// An angular sub-interval of the segment span, in ABSOLUTE azimuth.
#[derive(Clone, Copy)]
struct BlockedInterval {
    start: f64,
    end: f64,
}

/// Slices arrive in walk (ascending azimuth) order; the first one holding the
/// cp azimuth under the walk's own `CP_AZIMUTH_EPS` rule carries the cp badge,
/// so the badge sits on the slice whose value the walk took from the cp ray.
struct FanTraceCollector {
    cp_azimuth: f64,
    cp_marked: bool,
    intervals: Vec<ScreeningFanIntervalTrace>,
}

impl FanTraceCollector {
    fn new(cp_azimuth: f64) -> Self {
        Self {
            cp_azimuth,
            cp_marked: false,
            intervals: Vec::with_capacity(SCREENING_FAN_TRACE_LARGEST_INTERVALS),
        }
    }

    fn push(
        &mut self,
        start: f64,
        end: f64,
        blocked: bool,
        obstacle: Option<ScreeningFanObstacleTrace>,
        terrain_db: f64,
        screen_db: f64,
    ) {
        let contains_cp = !self.cp_marked
            && self.cp_azimuth >= start - CP_AZIMUTH_EPS
            && self.cp_azimuth <= end + CP_AZIMUTH_EPS;
        self.cp_marked |= contains_cp;
        self.intervals.push(ScreeningFanIntervalTrace {
            from_deg: (start - self.cp_azimuth).to_degrees(),
            to_deg: (end - self.cp_azimuth).to_degrees(),
            blocked,
            obstacle,
            terrain_db,
            screen_db,
            contains_cp,
        });
    }

    fn finish(mut self, span: f64, blocked_fraction: f64) -> ScreeningFanTrace {
        let width = |interval: &ScreeningFanIntervalTrace| interval.to_deg - interval.from_deg;
        let interval_count = self.intervals.len();
        let full_width: f64 = self.intervals.iter().map(width).sum();
        if interval_count > SCREENING_FAN_TRACE_LARGEST_INTERVALS {
            let cp_interval = self
                .intervals
                .iter()
                .find(|interval| interval.contains_cp)
                .cloned();
            self.intervals.sort_by(|a, b| {
                (b.to_deg - b.from_deg)
                    .total_cmp(&(a.to_deg - a.from_deg))
                    .then_with(|| a.from_deg.total_cmp(&b.from_deg))
            });
            self.intervals
                .truncate(SCREENING_FAN_TRACE_LARGEST_INTERVALS);
            if let Some(cp_interval) = cp_interval {
                if !self.intervals.iter().any(|interval| interval.contains_cp) {
                    self.intervals.push(cp_interval);
                }
            }
            self.intervals
                .sort_by(|a, b| a.from_deg.total_cmp(&b.from_deg));
        }
        let kept_width: f64 = self.intervals.iter().map(width).sum();

        ScreeningFanTrace {
            span_deg: span.to_degrees(),
            blocked_fraction: blocked_fraction.clamp(0.0, 1.0),
            intervals_omitted: interval_count.saturating_sub(self.intervals.len()),
            omitted_fraction: ((full_width - kept_width) / span.to_degrees()).max(0.0),
            intervals: self.intervals,
            quadrature: "arc",
        }
    }
}

fn fan_obstacle(trace: &ScreeningObstacleTrace) -> Option<ScreeningFanObstacleTrace> {
    trace.edge.as_ref().map(|edge| ScreeningFanObstacleTrace {
        kind: edge.kind,
        height_m: edge.height_m,
    })
}

/// Receiver-centred flat-earth frame (x east, y north, metres) — the same
/// equirectangular model as `geo` and `obstacle_index`.
struct Frame {
    lat: f64,
    lon: f64,
    m_per_deg_lon: f64,
}

impl Frame {
    fn at(lat: f64, lon: f64) -> Self {
        Frame {
            lat,
            lon,
            m_per_deg_lon: m_per_deg_lon(lat.to_radians()),
        }
    }

    #[inline]
    fn xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            (lon - self.lon) * self.m_per_deg_lon,
            (lat - self.lat) * M_PER_DEG_LAT,
        )
    }

    #[inline]
    fn azimuth(&self, lat: f64, lon: f64) -> f64 {
        let (x, y) = self.xy(lat, lon);
        y.atan2(x)
    }
}

/// Per-band effective screening of the whole segment as seen from the
/// receiver: the caller's cp bands where the span is degenerate or nothing
/// blocks it, the angular energy average otherwise.
pub fn arc_screened_attenuation(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    skyline: &mut ArcSkyline,
    scratch: &mut ArcScreeningScratch,
) -> [f64; NUM_BANDS] {
    let ground_bands = crate::propagation::iso9613::legacy_ground_atten_bands(q.ground_g);
    arc_screened_attenuation_with_ground(q, rasters, skyline, &ground_bands, scratch)
}

/// [`arc_screened_attenuation`] with the caller's already-formed
/// characteristic-point ground vector.
///
/// This is the mutable counterpart of
/// [`arc_screened_attenuation_prepared_with_ground`].  It preserves the
/// current increment-return architecture's one-vector compatibility seam while
/// surface callers move from the CF surrogate to literal CNOSSOS ground.
pub fn arc_screened_attenuation_with_ground(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    skyline: &mut ArcSkyline,
    ground_bands: &[f64; NUM_BANDS],
    scratch: &mut ArcScreeningScratch,
) -> [f64; NUM_BANDS] {
    let Some(p) = planned_ensure(
        q.receiver_lat,
        q.receiver_lon,
        q.start_lat,
        q.start_lon,
        q.end_lat,
        q.end_lon,
        q.dist_m,
        q.length_m,
        q.bounds,
    ) else {
        return *q.cp_screening;
    };
    skyline.ensure_planned(
        q.receiver_lat,
        q.receiver_lon,
        &p,
        q.obstacles,
        q.barriers,
        q.source_height_m,
        q.bounds,
    );
    arc_screened_eval(q, rasters, &skyline.arcs, ground_bands, scratch, None).0
}

/// [`arc_screened_attenuation`] against a [`SkylineSnapshot`] instead of the
/// live skyline — the read-only form the popup's parallel segment loop calls
/// from worker threads. The caller (the epoch scheduler) has already replayed
/// every `ensure` this query's sequential twin would have run, so the snapshot
/// IS the state the mutable kernel would read here, and the two paths are
/// bit-identical by construction.
pub fn arc_screened_attenuation_prepared(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    snapshot: &SkylineSnapshot,
    scratch: &mut ArcScreeningScratch,
) -> [f64; NUM_BANDS] {
    let ground_bands = crate::propagation::iso9613::legacy_ground_atten_bands(q.ground_g);
    arc_screened_eval(q, rasters, &snapshot.arcs, &ground_bands, scratch, None).0
}

/// Prepared arc evaluation with the caller's already-formed CP ground vector.
///
/// This keeps the current increment-return architecture's one-ground-vector
/// convention while allowing the surface callers to replace the CF surrogate
/// without changing the arc's screening payload.  Node evaluation removes this
/// compatibility seam by carrying every ray's full composite directly. When
/// `cp_obstacle` is present, the same walk also returns its popup fan trace.
pub fn arc_screened_attenuation_prepared_with_ground(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    snapshot: &SkylineSnapshot,
    ground_bands: &[f64; NUM_BANDS],
    scratch: &mut ArcScreeningScratch,
    cp_obstacle: Option<&ScreeningObstacleTrace>,
) -> ([f64; NUM_BANDS], Option<ScreeningFanTrace>) {
    arc_screened_eval(
        q,
        rasters,
        &snapshot.arcs,
        ground_bands,
        scratch,
        cp_obstacle,
    )
}

/// The evaluation half: clip `arcs` to the span, quadrate the blocked fan,
/// energy-average. Reads the arc list and nothing else of the skyline.
fn arc_screened_eval(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    arcs: &[MergedArc],
    ground: &[f64; NUM_BANDS],
    scratch: &mut ArcScreeningScratch,
    cp_obstacle: Option<&ScreeningObstacleTrace>,
) -> ([f64; NUM_BANDS], Option<ScreeningFanTrace>) {
    let frame = Frame::at(q.receiver_lat, q.receiver_lon);
    let base = frame.azimuth(q.start_lat, q.start_lon);
    let delta = wrap_pi(frame.azimuth(q.end_lat, q.end_lon) - base);
    let span = delta.abs();
    // Small-span skip: below the threshold the segment radiates in essentially
    // one direction and the cp ray IS that direction. (Re-derived from the
    // same expressions `planned_ensure` ran — identical bits, and the prepared
    // path needs the span anyway.)
    if span < DEGENERATE_SPAN_RAD || span < q.bounds.min_span_rad {
        return (*q.cp_screening, None);
    }
    // Span in absolute azimuth; a segment subtends at most π at a receiver off
    // its line, so the short arc between the endpoints IS it.
    let (lo, hi) = if delta < 0.0 {
        (base + delta, base)
    } else {
        (base, base + delta)
    };

    let ArcScreeningScratch {
        profile,
        candidates,
        intervals,
        cuts,
        presplit,
    } = scratch;
    if arcs.is_empty() {
        return (*q.cp_screening, None);
    }

    // 1. Clip the receiver's skyline to this segment's span, then keep the arcs
    //    that stand in FRONT of the source and not under the receiver's feet.
    //    That is the whole test — geometry, no δ (see the module note "Why the
    //    blocked test is geometry only").
    intervals.clear();
    'arcs: for arc in arcs.iter() {
        // EVERY piece of the span this arc covers, not merely the first one.
        //
        // `lo`/`hi` are UNWRAPPED azimuths that [`MergedArc::absorb`] grows, so
        // an arc can be wider than half a turn and then reach the span from
        // BOTH sides of the ±π seam. `break`-on-first-shift kept whichever
        // piece the shift ORDER happened to hit first: a near-full-turn arc
        // covering the whole span was credited with the 0.003 rad sliver its
        // shift-0 image overlapped, the other 99 % of the fan counted as CLEAR,
        // and the segment lost ~16 dB of screening. Measured at 50.0402 /
        // 14.4738 (Praha, 5. května): a 3 m wall raised to 5 m fused into an
        // 8 m arc, blocked fraction 1.000 → 0.434, receiver 5.8 dB LOUDER —
        // a taller barrier making the map louder, which is impossible.
        //
        // Pieces are pushed SEPARATELY, never unioned into one interval: two
        // images a turn apart can leave a real gap between them. The split step
        // below re-cuts overlaps, so coverage is exactly the union of the
        // pieces and no direction is ever counted twice.
        let mut pieces: [(f64, f64); 3] = [(0.0, 0.0); 3];
        let mut n_pieces = 0usize;
        if arc.hi - arc.lo >= TAU {
            // A full turn blocks every azimuth; there is no shift to search for.
            pieces[0] = (lo, hi);
            n_pieces = 1;
        } else {
            for shift in [0.0, TAU, -TAU] {
                let s = (arc.lo + shift).max(lo);
                let e = (arc.hi + shift).min(hi);
                if e > s {
                    pieces[n_pieces] = (s, e);
                    n_pieces += 1;
                }
            }
        }
        for &(start, end) in &pieces[..n_pieces] {
            let Some((src_lat, src_lon)) = source_point_at(&frame, q, 0.5 * (start + end)) else {
                continue;
            };
            let d = geo::flat_dist(src_lat, src_lon, q.receiver_lat, q.receiver_lon);
            let b = arc.near_m as f64;
            let a = d - b;
            if a <= 1.0 || b < 1.0 {
                continue; // the arc stands beyond the segment, or on the receiver
            }
            // NO δ PREFILTER HERE, and that is deliberate — see the module note
            // "Why the blocked test is geometry only". The arc has an obstacle in
            // it and it stands in front of the source; whether that obstacle bends
            // THIS ray is what the ray march below answers, from the real profile.
            //
            // `b` here is a RECEIVER-WIDE minimum: the skyline is built once per
            // receiver and `insert_merged` takes `near = min` over every member of
            // the stratum, including members outside this segment's span. The CUDA
            // lane clips to the span BEFORE its union, so its `b` is a min over the
            // in-span subset and is therefore >= this one. Same floors, different
            // `b`, so the two lanes can disagree on an arc whose nearest member is
            // out-of-span — see the KNOWN RESIDUAL FORK note in `scatter.cu`'s
            // admission block. This lane is the more permissive of the two, which
            // is the safe direction: it costs a march that returns ~0 dB.
            intervals.push(BlockedInterval { start, end });
            if intervals.len() >= q.bounds.max_intervals {
                break 'arcs;
            }
        }
    }
    if intervals.is_empty() {
        return (*q.cp_screening, None);
    }
    // The skyline is disjoint on the circle, but an obstacle straddling the ±π
    // seam lands in it as two arcs that the ±2π clip shift can bring back
    // together inside one span. Merge the clipped intervals so no direction is
    // ever counted twice in the energy average.
    intervals.sort_unstable_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    // SPLIT at every arc boundary; do NOT union overlaps into one interval.
    //
    // `insert_merged` already refuses to envelope arcs of materially different
    // height (`ARC_FUSE_HEIGHT_TOL_M`), so any overlap surviving to here is two
    // obstacles at DIFFERENT ranges and heights covering some of the same
    // directions. Unioning them produced one interval evaluated on ONE centre
    // ray, and that ray's obstacle — whichever it happened to hit — was then
    // applied to directions where only the other one stands. Measured on
    // fixture scene G (6 m blocks at 60 m behind 20 m blocks at 140 m, offset
    // half a pitch): 3.04 dB, a hard fail.
    //
    // Splitting keeps the COVERAGE identical — the union of the pieces is the
    // union of the arcs, so the blocked fraction is unchanged and nothing is
    // double counted — while giving each piece its own centre ray. The physics
    // then falls out of the ray march rather than out of the merge: in a
    // direction covered by two obstacles, `interval_screening` walks the real
    // ray and `path_effects` ranks the candidates on δ, so the DOMINANT screen
    // wins, which is what diffraction over two edges actually does.
    //
    // Cost is bounded: n arcs produce at most 2n−1 pieces, and pieces only
    // appear where arcs genuinely overlap in azimuth at different heights.
    {
        cuts.clear();
        cuts.reserve(intervals.len() * 2);
        for iv in intervals.iter() {
            cuts.push(iv.start);
            cuts.push(iv.end);
        }
        cuts.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        cuts.dedup();
        presplit.clear();
        presplit.extend_from_slice(intervals);
        intervals.clear();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a {
                continue;
            }
            let mid = 0.5 * (a + b);
            if presplit.iter().any(|iv| iv.start <= mid && mid <= iv.end) {
                intervals.push(BlockedInterval { start: a, end: b });
            }
        }
        // …then coalesce the windows that are finer than the quadrature can
        // use ([`ARC_QUADRATURE_MIN_RAD`]). Only TOUCHING windows merge, so
        // this never joins two blocked runs across a clear gap: coverage, and
        // with it `blocked_fraction` and the clear fan, is bit-unchanged.
        if intervals.len() > 1 {
            let mut w = 0usize;
            for r in 1..intervals.len() {
                let cur = intervals[r];
                let acc = intervals[w];
                let touching = (cur.start - acc.end).abs() < f64::EPSILON * acc.end.abs().max(1.0);
                if touching && acc.end - acc.start < ARC_QUADRATURE_MIN_RAD {
                    intervals[w].end = cur.end;
                } else {
                    w += 1;
                    intervals[w] = cur;
                }
            }
            intervals.truncate(w + 1);
        }
    }

    // 2. One screening evaluation per (sub-)interval centre, energy-averaged on
    //    the ground/barrier term.
    // The cp sits ON the segment, so its azimuth lies in the span by
    // construction — but when it sits on an ENDPOINT (every other pixel of a
    // straight road: adjacent microsegments share it) the two differ by an ulp.
    // Clamp it in so the "which interval owns the cp evaluation" question has
    // the same answer on both lanes.
    let cp_rel = clamp_into_span(frame.azimuth(q.cp_lat, q.cp_lon), lo, hi).clamp(lo, hi);
    let mut fan_trace = cp_obstacle.map(|_| FanTraceCollector::new(cp_rel));
    let mut blocked_fraction = 0.0_f64;
    let mut clear_fraction = 0.0_f64;
    let mut energy = [0.0_f64; NUM_BANDS];
    // ONE PASS over the whole span, alternating the CLEAR gaps between the
    // blocked intervals with the intervals themselves — `intervals` is sorted
    // and disjoint by construction (the cut/window split above), so a cursor
    // walking `lo → hi` enumerates every direction exactly once.
    //
    // Both arms take the term the kernel actually applies —
    // `max(A_ground, A_terrain + A_screen)` — on THAT DIRECTION's terrain, not
    // the cp ray's (§ `interval_screening`). See the fan-terrain note below.
    let mut cursor = lo;
    for iv in intervals.iter() {
        if iv.start > cursor {
            clear_fraction += accumulate_clear(
                q,
                rasters,
                &frame,
                ground,
                cursor,
                iv.start,
                span,
                profile,
                &mut energy,
                fan_trace.as_mut(),
            );
        }
        let width = iv.end - iv.start;
        let parts = ((width / ESCALATE_SPAN_RAD).ceil() as usize).clamp(1, ESCALATE_MAX_PARTS);
        let step = width / parts as f64;
        for p in 0..parts {
            let start = iv.start + p as f64 * step;
            let end = start + step;
            let mut obstacle = None;
            let contains_cp = cp_rel >= start - CP_AZIMUTH_EPS && cp_rel <= end + CP_AZIMUTH_EPS;
            let evaluated = if contains_cp {
                None
            } else {
                let collect_obstacle = fan_trace.is_some().then_some(&mut obstacle);
                interval_screening(
                    q,
                    rasters,
                    &frame,
                    0.5 * (start + end),
                    profile,
                    candidates,
                    collect_obstacle,
                )
            };
            let (terrain, bands) = match evaluated {
                Some(result) => result,
                None => {
                    // A slice whose own ray failed borrows the cp VALUE, not
                    // the cp ray's obstacle identity.
                    if contains_cp {
                        obstacle = cp_obstacle.and_then(fan_obstacle);
                    }
                    (*q.cp_terrain, *q.cp_screening)
                }
            };
            let fraction = step / span;
            blocked_fraction += fraction;
            for b in 0..NUM_BANDS {
                energy[b] +=
                    fraction * db_to_energy(-ground_or_barrier_db(ground[b], terrain[b], bands[b]));
            }
            if let Some(trace) = fan_trace.as_mut() {
                trace.push(
                    start,
                    end,
                    true,
                    obstacle,
                    terrain[SCREENING_FAN_TRACE_BAND],
                    bands[SCREENING_FAN_TRACE_BAND],
                );
            }
        }
        cursor = cursor.max(iv.end);
    }

    // …plus the clear rest of the fan, which carries the terrain/ground term
    // alone. Nothing else CAN screen it: an obstacle blocking a path from this
    // receiver to a segment stands between the two, so it is nearer than the
    // segment — and the skyline radius is derived to cover every segment the
    // span test lets through (module docs). A skyline that is complete for the
    // arc-treated segments is what lets the clear fan be exactly clear.
    //
    // CLEAR ≠ TERRAIN-FREE, and that was the last structural error in this
    // rule. The fan's clear directions were carrying the CP RAY's `A_terrain`,
    // which is right only where the ground is level: over relief the source
    // point at one edge of a 250 m microsegment can stand tens of metres above
    // the one at the other, and the crest between it and the receiver is a
    // different crest in every direction. Measured on `screening_fixture`
    // scene I (scene F's block row on a 10 % hillside, the only scene with
    // terrain): the cp terrain covering the clear fan was worth 1.51 dB of
    // shadow error against the 99-point reference — the whole of the fixture's
    // one breach of the owner's 1.0 dB line — and 79 % of it came from the ONE
    // microsegment whose clear directions run up the slope past the block row.
    // Resolving terrain per clear piece takes it to 0.93 / 0.49 dB. Flat ground
    // is unchanged (every direction returns the same terrain the cp does), so
    // the cost is only paid where relief makes it a correction.
    //
    // AND THIS IS WHY RAY SAMPLING BEATS THE ARC RULE on the tile lane (the
    // 2026-08-05 switch to five rays across the microsegment span): a sampled
    // ray carries its OWN terrain the whole way, so it never has to hand an
    // increment back across a terrain it was not measured on. What this block
    // does by hand — resolve terrain per direction instead of pinning it to the
    // cp ray — sampling gets for free, which is a large part of why five rays
    // land closer to the reference than the arc rule does at a quarter of its
    // cost. The arc rule keeps this correction because the POPUP keeps the arc
    // rule: it is the etalon and has to stay accurate.
    if cursor < hi {
        clear_fraction += accumulate_clear(
            q,
            rasters,
            &frame,
            ground,
            cursor,
            hi,
            span,
            profile,
            &mut energy,
            fan_trace.as_mut(),
        );
    }
    // The walk covers `[lo, hi]`, so the two fractions sum to 1 up to rounding;
    // the residual keeps the energy normalised if a cap ever truncates the walk.
    let residual = (1.0 - blocked_fraction - clear_fraction).max(0.0);
    let mut out = [0.0_f64; NUM_BANDS];
    for b in 0..NUM_BANDS {
        let clear = residual * db_to_energy(-ground_or_barrier_db(ground[b], q.cp_terrain[b], 0.0));
        let mean_db = -10.0 * (energy[b] + clear).max(1e-12).log10();
        // Hand back the increment that reproduces `mean_db` once the caller
        // re-applies `max(A_ground, A_terrain + A_screen)`: the mean is ≥ every
        // input, so the max resolves to `A_terrain + out[b]` by construction.
        //
        // ONE REGIME THE CHANNEL CANNOT CARRY, and it is worth knowing about.
        // A screening increment is non-negative by contract everywhere in the
        // engine, and the caller reads `A_terrain + A_screen == 0` as "no
        // barrier". So the handback can only express `mean_db ≥ q.cp_terrain`,
        // the ONE terrain the caller will re-add.
        //
        // TWO WAYS TO LAND UNDER IT, not one:
        //
        // 1. `A_terrain = 0` — flat ground, the common urban case: then it
        //    holds iff `mean_db ≥ 0`, and since the CNOSSOS hard-ground floor
        //    made `A_ground = −3 dB` a fan that is less than about half blocked
        //    averages to a net BOOST and lands below it.
        // 2. `cp_terrain > 0` but the FAN's terrain is smaller. Each direction's
        //    own term is ≥ ITS OWN `A_terrain` — which is no longer the cp ray's
        //    since the fan's terrain became per-direction (`accumulate_clear`,
        //    `interval_screening`). A cp ray that runs over the one crest in the
        //    fan therefore sets a bar the mean of the flatter directions can sit
        //    under, screening or no screening. The pre-per-direction claim "each
        //    D_i ≥ A_terrain, hence so is their mean" no longer holds, and this
        //    branch is NOT quantified — no fixture scene isolates it (scene I is
        //    the only one with relief and it is dominated by case 1). OPEN.
        //
        // Either way the clamp returns 0, the caller falls back to
        // `max(A_ground, cp_terrain)`, and the pixel keeps that instead of the
        // fan mean: LOUDER than exact by `mean_db − max(A_ground, cp_terrain)`.
        // For case 1 that is bounded by 3.0 dB (hard ground, fans blocked
        // ≲ 50 %); for case 2 it is bounded by the cp-vs-fan terrain spread and
        // nobody has measured it. The direction is deliberate — over-predicting
        // noise is the safe failure for this product — but it is a real
        // residual, and closing it needs a signed screening term plus an
        // explicit "barrier present" flag on both the CPU and CUDA lanes
        // (SPEC §4.7). Do not restate the invariant without that change.
        out[b] = (mean_db - q.cp_terrain[b]).max(0.0);
    }
    let fan = fan_trace.map(|trace| trace.finish(span, blocked_fraction));
    (out, fan)
}

/// The cp azimuth folded into `[lo, hi]`'s turn (the span can straddle ±π, in
/// which case `hi` is past π and a raw `atan2` lands a turn below it).
#[inline]
fn clamp_into_span(az: f64, lo: f64, hi: f64) -> f64 {
    for shift in [0.0, TAU, -TAU] {
        let a = az + shift;
        if a >= lo - 1e-12 && a <= hi + 1e-12 {
            return a;
        }
    }
    az
}

/// Energy-average the ground/terrain term over ONE CLEAR gap of the fan,
/// `[start, end]`, at the same angular resolution the blocked intervals use.
/// Returns the span fraction it covered.
///
/// A clear direction has no obstacle to rank, so this is the terrain-only arm
/// of [`interval_screening`]: same ray, same profile, `A_screen = 0`. It costs
/// one path profile per piece — the price of the fan's terrain being resolved
/// per direction instead of pinned to the cp ray.
#[allow(clippy::too_many_arguments)]
fn accumulate_clear(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    frame: &Frame,
    ground: &[f64; NUM_BANDS],
    start: f64,
    end: f64,
    span: f64,
    profile: &mut PathProfile,
    energy: &mut [f64; NUM_BANDS],
    mut trace: Option<&mut FanTraceCollector>,
) -> f64 {
    let width = end - start;
    let parts = ((width / ESCALATE_SPAN_RAD).ceil() as usize).clamp(1, ESCALATE_MAX_PARTS);
    let step = width / parts as f64;
    let mut covered = 0.0;
    for p in 0..parts {
        let s = start + p as f64 * step;
        let terrain =
            interval_terrain(q, rasters, frame, s + 0.5 * step, profile).unwrap_or(*q.cp_terrain);
        let fraction = step / span;
        covered += fraction;
        for b in 0..NUM_BANDS {
            energy[b] += fraction * db_to_energy(-ground_or_barrier_db(ground[b], terrain[b], 0.0));
        }
        if let Some(trace) = trace.as_deref_mut() {
            trace.push(
                s,
                s + step,
                false,
                None,
                terrain[SCREENING_FAN_TRACE_BAND],
                0.0,
            );
        }
    }
    covered
}

/// `10^(db/10)` through the kernel's own fast exponential (same rounding as
/// every other energy conversion in the engine).
#[inline]
fn db_to_energy(db: f64) -> f64 {
    super::iso9613::fast_exp_f64(db * std::f64::consts::LN_10 * 0.1)
}

/// The point ON the segment that the receiver sees at `azimuth` — the exact
/// source position for an interval's evaluation ray, by ray×line intersection
/// (linear interpolation of the endpoints at the solved fraction). `None` when
/// the ray runs parallel to the segment.
fn source_point_at(frame: &Frame, q: &ArcScreening<'_>, azimuth: f64) -> Option<(f64, f64)> {
    let (ax, ay) = frame.xy(q.start_lat, q.start_lon);
    let (bx, by) = frame.xy(q.end_lat, q.end_lon);
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

/// Bare-earth terrain bands on the ray to the source point at `azimuth` — the
/// clear-fan arm of [`interval_screening`], without the crossing scan.
fn interval_terrain(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    frame: &Frame,
    azimuth: f64,
    profile: &mut PathProfile,
) -> Option<[f64; NUM_BANDS]> {
    let (src_lat, src_lon) = source_point_at(frame, q, azimuth)?;
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
    // Terrain bands only: this helper feeds the quadrature's terrain average,
    // which never runs the obstacle race.
    Some(terrain_attenuation(profile, src_alt, q.receiver_alt_m).0)
}

/// Terrain AND screening bands on the ray from the receiver to the source point
/// at `azimuth`: fresh path profile, that ray's own terrain — both as the
/// screening increment's base and as the term the average is taken on — and
/// that ray's own exact crossings.
///
/// The terrain used to be computed here and DISCARDED, the increment then
/// re-applied on top of the cp ray's terrain by the caller. Over relief those
/// are two different numbers, so the handed-back increment did not reproduce
/// the ray it was measured on. Returning it costs nothing: the ray is built
/// either way.
fn interval_screening(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    frame: &Frame,
    azimuth: f64,
    profile: &mut PathProfile,
    candidates: &mut Vec<CrossingCandidate>,
    trace_obstacle: Option<&mut Option<ScreeningFanObstacleTrace>>,
) -> Option<([f64; NUM_BANDS], [f64; NUM_BANDS])> {
    let (src_lat, src_lon) = source_point_at(frame, q, azimuth)?;
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
    let (terrain, terrain_delta_m) = terrain_attenuation(profile, src_alt, q.receiver_alt_m);
    q.obstacles.crossings_pruned(
        src_lat,
        src_lon,
        q.receiver_lat,
        q.receiver_lon,
        &CellPrune::for_profile(profile, src_alt, q.receiver_alt_m),
        candidates,
    );
    let screening = if let Some(out) = trace_obstacle {
        let (bands, trace) = screening_attenuation_with_meta(
            profile,
            q.barriers,
            ObstacleInput { candidates },
            src_alt,
            q.receiver_alt_m,
            q.exclusion_radius_m,
            &terrain,
            terrain_delta_m,
        );
        *out = fan_obstacle(&trace);
        bands
    } else {
        screening_attenuation(
            profile,
            q.barriers,
            ObstacleInput { candidates },
            src_alt,
            q.receiver_alt_m,
            q.exclusion_radius_m,
            &terrain,
            terrain_delta_m,
        )
    };
    Some((terrain, screening))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{M_PER_DEG_LAT, SOURCE_HEIGHT_ROAD};
    use crate::propagation::iso9613;
    use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
    use std::f64::consts::PI;
    use std::sync::Arc;

    const OLAT: f64 = 50.0;
    const OLON: f64 = 14.0;

    /// Metric offsets (m east, m north of the origin) → (lat, lon).
    fn ll(x_m: f64, y_m: f64) -> (f64, f64) {
        (
            OLAT + y_m / M_PER_DEG_LAT,
            OLON + x_m / m_per_deg_lon(OLAT.to_radians()),
        )
    }

    /// Flat ground at sea level, no raster buildings (obstacles are vector).
    struct FlatGround;
    impl RasterSampler for FlatGround {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.5
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    /// Scene A of `screening_fixture`, one segment: a 30 × 10 × 8 m box at
    /// x ∈ [30, 40], |y| ≤ 15, source line at x = 0.
    fn box_scene() -> ObstacleSet {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[
                ll(30.0, -15.0),
                ll(40.0, -15.0),
                ll(40.0, 15.0),
                ll(30.0, 15.0),
            ],
            8.0,
            ObstacleKind::Building,
            0,
        );
        ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        }
    }

    /// Flat ground contributes no terrain diffraction, so the handback the
    /// tests read is the fan-average ground/barrier term `D̄` itself — but only
    /// while `D̄ ≥ 0`. The scenes below run at `G = 0`, where `A_ground` is the
    /// −3 dB hard-ground floor, so a mostly-clear fan averages to a net boost
    /// that the non-negative handback cannot carry (see `arc_screened_bands`).
    const NO_TERRAIN: [f64; NUM_BANDS] = [0.0; NUM_BANDS];

    /// The two buffers every caller owns, bundled so the tests read as one call.
    #[derive(Default)]
    struct Buffers {
        skyline: ArcSkyline,
        scratch: ArcScreeningScratch,
    }

    impl Buffers {
        fn run(&mut self, q: &ArcScreening<'_>) -> [f64; NUM_BANDS] {
            arc_screened_attenuation(q, &FlatGround, &mut self.skyline, &mut self.scratch)
        }
    }

    /// A 250 m segment starting at the receiver's perpendicular foot, seen
    /// from `(x, 0)`, plus everything the query needs.
    fn query<'a>(
        obstacles: &'a ObstacleSet,
        cp_screening: &'a [f64; NUM_BANDS],
        rcv_x: f64,
    ) -> ArcScreening<'a> {
        let (rlat, rlon) = ll(rcv_x, 0.0);
        let (alat, alon) = ll(0.0, 0.0);
        let (blat, blon) = ll(0.0, 250.0);
        ArcScreening {
            receiver_lat: rlat,
            receiver_lon: rlon,
            receiver_alt_m: 4.0,
            start_lat: alat,
            start_lon: alon,
            end_lat: blat,
            end_lon: blon,
            source_height_m: SOURCE_HEIGHT_ROAD,
            cp_lat: alat,
            cp_lon: alon,
            src_alt_m: SOURCE_HEIGHT_ROAD,
            cp_screening,
            cp_terrain: &NO_TERRAIN,
            ground_g: 0.0,
            barriers: &[],
            obstacles,
            length_m: 250.0,
            dist_m: rcv_x,
            exclusion_radius_m: 0.0,
            bounds: ARC_BOUNDS_EXACT,
        }
    }

    /// THE fix: the cp ray is fully screened (cp at y = 0, straight through
    /// the box), but the box covers only a slice of the 250 m segment's fan,
    /// so the segment's effective screening is a fraction of it.
    ///
    /// Box half-width 15 m at 105 m from the receiver → ~0.142 rad of the
    /// segment's 1.045 rad fan (≈ 14 %). Both ground regimes are exercised
    /// because the handback channel behaves differently in each.
    #[test]
    fn box_screens_only_its_angular_share() {
        let obstacles = box_scene();
        let cp = [12.0_f64; NUM_BANDS];
        let mut buf = Buffers::default();

        // SOFT ground (G = 1): `A_ground` is ≥ 0 in every band, the fan mean
        // clears the floor, and the handback carries it. Every band must land
        // strictly between its own clear-fan value and the 12 dB cp verdict —
        // the whole point of arc averaging, that a 14 %-blocked fan is not a
        // fully screened segment.
        let mut soft = query(&obstacles, &cp, 145.0);
        soft.ground_g = 1.0;
        let out_soft = buf.run(&soft);
        for (b, &a) in out_soft.iter().enumerate() {
            let clear = iso9613::legacy_ground_atten_db(b, 1.0);
            assert!(
                a > clear && a < 12.0,
                "band {b}: arc-averaged {a:.2} dB must sit between the clear fan                  {clear:.2} dB and the cp verdict 12 dB"
            );
        }

        // HARD ground (G = 0): every band's clear fan is the −3 dB floor, so a
        // 14 %-blocked fan averages to ≈ −2.6 dB — a net boost, below the 0 dB
        // the non-negative handback can express. It saturates, and the caller
        // keeps the full floor: conservative (louder than the exact mean by
        // ≤ 3 dB), documented at `arc_screened_bands` and in SPEC §4.7. Was
        // ≈0.6 dB per band while `A_ground` wrongly read 0 dB at G = 0.
        let hard = query(&obstacles, &cp, 145.0);
        let out_hard = buf.run(&hard);
        assert_eq!(
            out_hard, [0.0; NUM_BANDS],
            "hard-ground fan mean is a boost; the handback must saturate to 0, not go negative"
        );
    }

    /// The shipped bounds must reproduce the exact kernel on the fixture
    /// geometry — this is the stripe case, and no bound may touch it.
    #[test]
    fn shipped_bounds_match_the_exact_kernel_on_the_stripe_case() {
        let obstacles = box_scene();
        let cp = [12.0_f64; NUM_BANDS];
        for rcv_x in [45.0, 100.0, 145.0, 300.0] {
            let mut q = query(&obstacles, &cp, rcv_x);
            let exact = Buffers::default().run(&q);
            q.bounds = ARC_BOUNDS_DEFAULT;
            let bounded = Buffers::default().run(&q);
            for b in 0..NUM_BANDS {
                assert!(
                    (exact[b] - bounded[b]).abs() < 0.5,
                    "x={rcv_x} band {b}: exact {:.2} vs bounded {:.2}",
                    exact[b],
                    bounded[b]
                );
            }
        }
    }

    /// No obstacle anywhere ⇒ the caller's cp verdict stands, bit for bit
    /// (the rural path must be free of surprises).
    #[test]
    fn empty_skyline_returns_cp_bands() {
        let obstacles = ObstacleSet { indexes: vec![] };
        let cp = [3.5_f64; NUM_BANDS];
        let q = query(&obstacles, &cp, 145.0);
        assert_eq!(Buffers::default().run(&q), cp);
    }

    #[test]
    fn fan_trace_keeps_the_32_largest_slices_and_the_cp_slice() {
        let mut collector = FanTraceCollector::new(0.0005);
        let mut start = 0.0;
        for i in 0..40 {
            let width = (i + 1) as f64 * 0.001;
            collector.push(start, start + width, true, None, 0.0, i as f64);
            start += width;
        }

        let fan = collector.finish(0.82, 0.5);
        assert_eq!(fan.intervals.len(), 33);
        assert_eq!(fan.intervals_omitted, 7);
        // Dropped: the seven narrowest slices after the cp slice (i = 1..=7).
        let omitted_rad: f64 = (2..=8).map(|w| w as f64 * 0.001).sum();
        assert!((fan.omitted_fraction - omitted_rad / 0.82).abs() < 1e-9);
        assert!(fan.intervals[0].contains_cp, "the cp slice is kept first");
        assert_eq!(
            fan.intervals
                .iter()
                .filter(|interval| interval.contains_cp)
                .count(),
            1
        );
        assert!(fan
            .intervals
            .iter()
            .filter(|interval| !interval.contains_cp)
            .all(|interval| interval.screen_db >= 8.0));
        assert!(fan
            .intervals
            .windows(2)
            .all(|pair| pair[0].from_deg < pair[1].from_deg));
    }

    /// A degenerate span (both endpoints at one azimuth) falls back to the cp
    /// value, as does a span under the small-span threshold.
    #[test]
    fn degenerate_and_narrow_spans_return_cp_bands() {
        let obstacles = box_scene();
        let cp = [7.0_f64; NUM_BANDS];
        let mut q = query(&obstacles, &cp, 145.0);
        q.end_lat = q.start_lat;
        q.end_lon = q.start_lon;
        assert_eq!(Buffers::default().run(&q), cp);

        // A 250 m segment 30 km away spans 0.008 rad — under the shipped
        // 0.01 rad floor, so the cp verdict stands.
        let mut far = query(&obstacles, &cp, 30_000.0);
        far.bounds = ARC_BOUNDS_DEFAULT;
        assert_eq!(Buffers::default().run(&far), cp);
    }

    /// An obstacle BEYOND the segment (behind the source line) projects into
    /// the span but blocks nothing — the range test must drop it.
    #[test]
    fn obstacle_beyond_the_segment_does_not_screen() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[
                ll(-40.0, -15.0),
                ll(-30.0, -15.0),
                ll(-30.0, 15.0),
                ll(-40.0, 15.0),
            ],
            8.0,
            ObstacleKind::Building,
            0,
        );
        let obstacles = ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        };
        let cp = [12.0_f64; NUM_BANDS];
        let q = query(&obstacles, &cp, 145.0);
        let out = Buffers::default().run(&q);
        assert_eq!(out, cp, "nothing between receiver and segment");
    }

    /// The skyline is built ONCE per receiver: a second segment of the same
    /// receiver reuses it, a different receiver rebuilds it.
    #[test]
    fn skyline_is_rebuilt_only_when_the_receiver_moves() {
        let obstacles = box_scene();
        let cp = [12.0_f64; NUM_BANDS];
        let mut buf = Buffers::default();
        let q = query(&obstacles, &cp, 145.0);
        buf.run(&q);
        // Two arcs, not one: this box sits due WEST, so its outline straddles
        // the ±π seam and the sorted list keeps the two sides apart. The
        // per-segment clip merges them back before any energy is counted.
        assert_eq!(buf.skyline.len(), 2, "one box across the ±π seam");
        let key = (buf.skyline.lat, buf.skyline.lon);
        // Same receiver, mirrored segment: no rebuild.
        let mut q2 = query(&obstacles, &cp, 145.0);
        q2.end_lon = q2.start_lon - (q2.end_lon - q2.start_lon);
        buf.run(&q2);
        assert_eq!((buf.skyline.lat, buf.skyline.lon), key);
        // Different receiver: rebuilt at the new position.
        let q3 = query(&obstacles, &cp, 200.0);
        buf.run(&q3);
        assert_ne!((buf.skyline.lat, buf.skyline.lon), key);
    }

    /// A NOISE WALL must produce blocked intervals like a building does. It
    /// arrives by a different route (the `barriers` slice, deliberately not in
    /// `ObstacleIndex`), and when it did not reach the skyline at all a receiver
    /// behind a wall silently kept the cp verdict for the whole segment — the
    /// constant-width shadow band, surviving exactly where a wall makes it most
    /// visible (physics-suite P3, 0.63 dB against a 0.35 dB gate).
    #[test]
    fn a_wall_screens_its_angular_share() {
        let obstacles = ObstacleSet { indexes: vec![] };
        let cp = [12.0_f64; NUM_BANDS];
        // A 6 m wall 30 m west of the receiver, spanning 40 m of the fan.
        let (s_lat, s_lon) = ll(115.0, -20.0);
        let (e_lat, e_lon) = ll(115.0, 20.0);
        let wall = [Barrier {
            osm_id: 1,
            segment_idx: 0,
            height_m: 6.0,
            start_lat: s_lat,
            start_lon: s_lon,
            end_lat: e_lat,
            end_lon: e_lon,
            dist_m: 30.0,
        }];
        let mut q = query(&obstacles, &cp, 145.0);
        q.barriers = &wall;
        let out = Buffers::default().run(&q);
        assert!(
            out.iter().all(|&a| (0.0..12.0).contains(&a)),
            "the wall screens the arc it covers, not the whole segment: {out:?}"
        );
        // The wall's own attenuation rises with frequency, so the upper bands'
        // fan mean clears the 0 dB the handback can express while the lowest
        // bands saturate against the −3 dB hard-ground floor (see
        // `arc_screened_bands`). At least one band must come through, or the
        // wall reached the skyline without screening anything.
        assert!(
            out.iter().any(|&a| a > 0.0),
            "no band carried the wall's angular share: {out:?}"
        );
        // Same wall over SOFT ground, where nothing saturates: every band is a
        // strict fraction of the cp verdict.
        let mut soft = query(&obstacles, &cp, 145.0);
        soft.barriers = &wall;
        soft.ground_g = 1.0;
        let out_soft = Buffers::default().run(&soft);
        for (b, &a) in out_soft.iter().enumerate() {
            let clear = iso9613::legacy_ground_atten_db(b, 1.0);
            assert!(
                a > clear && a < 12.0,
                "band {b}: soft-ground wall share {a:.2} dB outside ({clear:.2}, 12)"
            );
        }
        // …and with no wall in the slice there is nothing to clip, so the
        // caller's cp verdict stands untouched.
        let bare = query(&obstacles, &cp, 145.0);
        assert_eq!(Buffers::default().run(&bare), cp);
    }

    /// A wall shorter than the RECEIVER still screens: the sight line falls to
    /// the source height at the far end, so gating candidates on the receiver's
    /// own height would delete every 3 m noise wall.
    #[test]
    fn a_wall_below_receiver_height_still_reaches_the_skyline() {
        let obstacles = ObstacleSet { indexes: vec![] };
        let cp = [9.0_f64; NUM_BANDS];
        let (s_lat, s_lon) = ll(115.0, -60.0);
        let (e_lat, e_lon) = ll(115.0, 60.0);
        let wall = [Barrier {
            osm_id: 2,
            segment_idx: 0,
            // Below the 4 m receiver in `query`, above the 0.05 m source.
            height_m: 3.0,
            start_lat: s_lat,
            start_lon: s_lon,
            end_lat: e_lat,
            end_lon: e_lon,
            dist_m: 30.0,
        }];
        let mut q = query(&obstacles, &cp, 145.0);
        q.barriers = &wall;
        let mut buf = Buffers::default();
        buf.run(&q);
        assert!(
            !buf.skyline.is_empty(),
            "a 3 m wall must reach the skyline of a 4 m receiver"
        );
    }

    /// The interval-centre source point is the exact ray×segment intersection.
    #[test]
    fn source_point_solves_the_azimuth() {
        let obstacles = ObstacleSet { indexes: vec![] };
        let cp = [0.0_f64; NUM_BANDS];
        let q = query(&obstacles, &cp, 100.0);
        let frame = Frame::at(q.receiver_lat, q.receiver_lon);
        // 45° up-left from the receiver at (100, 0) hits the line x = 0 at
        // y = 100.
        let (lat, lon) = source_point_at(&frame, &q, PI - std::f64::consts::FRAC_PI_4).unwrap();
        let (x, y) = frame.xy(lat, lon);
        assert!((x + 100.0).abs() < 0.5, "x offset {x}");
        assert!((y - 100.0).abs() < 0.5, "y offset {y}");
    }

    /// Arc capacity: overflow merges the SMALLEST gap, so the list stays a
    /// superset of the true blocked directions and never loses one.
    #[test]
    fn arc_capacity_overflow_merges_the_smallest_gap() {
        let mut v: Vec<MergedArc> = Vec::new();
        let mk = |lo: f64, hi: f64| SkylineArc {
            source_id: ScreeningSourceId::obstacle(0).unwrap(),
            lo,
            hi,
            near_m: 10.0,
            height_m: 8.0,
        };
        let fuse = &mut Vec::new();
        // Three arcs, gaps 1.0 and 0.1 — the second pair must fuse.
        assert_eq!(insert_merged(&mut v, 3, mk(0.0, 0.1), fuse), 0);
        assert_eq!(insert_merged(&mut v, 3, mk(1.1, 1.2), fuse), 0);
        assert_eq!(insert_merged(&mut v, 3, mk(1.3, 1.4), fuse), 0);
        assert_eq!(insert_merged(&mut v, 2, mk(2.0, 2.1), fuse), 1);
        assert_eq!(v.len(), 2);
        assert!((v[1].lo - 1.1).abs() < 1e-12 && (v[1].hi - 2.1).abs() < 1e-12);
        assert!((v[0].lo).abs() < 1e-12 && (v[0].hi - 0.1).abs() < 1e-12);
    }

    /// Overlapping inserts of ONE stratum union in place, whatever the order,
    /// and the union keeps the nearest range of its members.
    #[test]
    fn overlapping_arcs_of_one_stratum_union() {
        let mk = |lo: f64, hi: f64, near: f32| SkylineArc {
            source_id: ScreeningSourceId::obstacle(0).unwrap(),
            lo,
            hi,
            near_m: near,
            height_m: 8.0,
        };
        // All four inside ONE range stratum `[ρ^9, ρ^10)`, whatever ρ is, so the
        // chain is honest: every direction the union covers really does hold an
        // obstacle at that range, to within the stratum.
        let base = ARC_FUSE_RANGE_RATIO.powi(9);
        let at = |f: f32| base * ARC_FUSE_RANGE_RATIO.powf(f);
        let raw = [
            mk(0.0, 0.5, at(0.9)),
            mk(0.4, 0.9, at(0.0)),
            mk(2.0, 2.1, at(0.5)),
            mk(0.8, 2.05, at(0.2)),
        ];
        // Every insertion order must give the SAME list — that is what makes the
        // merge a function of the arcs rather than of the walk.
        for perm in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1], [1, 3, 0, 2]] {
            let mut v: Vec<MergedArc> = Vec::new();
            let fuse = &mut Vec::new();
            for k in perm {
                insert_merged(&mut v, 16, raw[k], fuse);
            }
            assert_eq!(v.len(), 1, "order {perm:?}");
            assert!((v[0].lo).abs() < 1e-12 && (v[0].hi - 2.1).abs() < 1e-12);
            assert_eq!(v[0].near_m, base, "merged arcs keep the nearest range");
        }
    }

    /// Overlapping arcs of DIFFERENT range strata are left side by side, so the
    /// far one keeps its own range for the admission test. Unioning them is the
    /// order-dependence defect: it lends the near range to directions where only
    /// the far obstacle stands.
    #[test]
    fn overlapping_arcs_of_different_strata_stay_apart() {
        let mk = |lo: f64, hi: f64, near: f32| SkylineArc {
            source_id: ScreeningSourceId::obstacle(0).unwrap(),
            lo,
            hi,
            near_m: near,
            height_m: 8.0,
        };
        for perm in [[0, 1], [1, 0]] {
            let mut v: Vec<MergedArc> = Vec::new();
            let fuse = &mut Vec::new();
            let raw = [mk(0.0, 0.5, 1.2), mk(0.4, 0.9, 600.0)];
            for k in perm {
                insert_merged(&mut v, 16, raw[k], fuse);
            }
            assert_eq!(
                v.len(),
                2,
                "order {perm:?}: a facade and a village 600 m out"
            );
            let mut nears: Vec<f32> = v.iter().map(|a| a.near_m).collect();
            nears.sort_by(f32::total_cmp);
            assert_eq!(nears, vec![1.2, 600.0]);
        }
    }

    /// The skyline must reach past the segment that asked for it — the whole
    /// basis for treating the clear fraction of the fan as exactly clear.
    #[test]
    fn skyline_grows_to_the_far_end_of_the_segment() {
        let obstacles = box_scene();
        let cp = [12.0_f64; NUM_BANDS];
        let mut buf = Buffers::default();
        let q = query(&obstacles, &cp, 145.0);
        buf.run(&q);
        assert!(
            buf.skyline
                .built_radius_m
                .iter()
                .any(|&r| r as f64 >= q.dist_m + q.length_m),
            "walked {} m for a segment whose far end is {} m out",
            buf.skyline
                .built_radius_m
                .iter()
                .fold(0.0f32, |a, &b| a.max(b)),
            q.dist_m + q.length_m
        );
    }

    #[test]
    fn wrap_pi_folds_into_the_principal_range() {
        for a in [-7.0, -3.2, -0.1, 0.0, 3.2, 7.0, 12.0] {
            let w = wrap_pi(a);
            assert!(w > -PI - 1e-12 && w <= PI + 1e-12, "{a} → {w}");
            assert!(((a - w) / TAU).fract().abs() < 1e-9, "{a} → {w}");
        }
    }

    // =====================================================================
    // PHYSICAL INVARIANT: raising a screen can never make a receiver LOUDER.
    //
    // This is a gate, not a tolerance. Every discretisation in this module —
    // which arcs fuse, where an interval is cut, which azimuth an evaluation
    // ray is fired down — may move the answer a little, but none of them may
    // move it the WRONG WAY when an obstacle grows: a taller screen intercepts
    // a superset of the rays the shorter one intercepted.
    //
    // Both defects these tests were written for were invisible to fixed-scene
    // regression numbers. They only show up as a STEP in a height sweep:
    //   * a merged arc grown past a full turn, clipped to a 0.003 rad sliver
    //     instead of the whole span — blocked fraction 1.000 → 0.434, +5.8 dB
    //     at 50.0402/14.4738 (Praha) when a 3 m wall was raised to 5 m;
    //   * `diffraction::compute_single_edge_at`'s favourable δ_F stepping at the
    //     sight-line crossing, where its two arms did not meet (2.9 dB
    //     A-weighted, measured).
    // =====================================================================

    /// A wall as a `types::Barrier`, parallel to the source line at `x_m`.
    fn sweep_wall(x_m: f64, half_len_m: f64, h: f32) -> Barrier {
        let (s_lat, s_lon) = ll(x_m, -half_len_m);
        let (e_lat, e_lon) = ll(x_m, half_len_m);
        Barrier {
            osm_id: 1,
            segment_idx: 0,
            height_m: h,
            start_lat: s_lat,
            start_lon: s_lon,
            end_lat: e_lat,
            end_lon: e_lon,
            dist_m: 0.0,
        }
    }

    /// A merged arc that reaches a FULL TURN blocks every azimuth, so it clips
    /// to the WHOLE span — whichever side of the ±π seam its unwrapped `lo`
    /// happens to sit on.
    ///
    /// The pre-fix clip took the FIRST of `[0.0, TAU, -TAU]` that produced any
    /// overlap, so an arc whose shift-0 image merely grazed the far end of the
    /// span was credited with that graze and the other 99 % of the fan was
    /// declared CLEAR. Here the graze is 0.001 rad of a 0.125 rad span, over a
    /// wall long enough to screen every direction the segment radiates in: the
    /// pre-fix kernel hands back under 1 dB, the fixed one the full verdict.
    #[test]
    fn full_turn_arc_blocks_the_whole_span() {
        // 25 m of road seen from 200 m: a 0.125 rad span, under
        // ESCALATE_SPAN_RAD, so each blocked piece is evaluated once.
        let (rcv_x, seg_half) = (200.0_f64, 12.5_f64);
        let (rlat, rlon) = ll(rcv_x, 0.0);
        let (alat, alon) = ll(0.0, -seg_half);
        let (blat, blon) = ll(0.0, seg_half);
        let (cplat, cplon) = ll(0.0, 0.0);
        let empty = ObstacleSet { indexes: vec![] };
        // An 8 m wall halfway, long enough to cross every ray in the fan.
        let bars = [sweep_wall(0.5 * rcv_x, 400.0, 8.0)];
        let mut q = ArcScreening {
            receiver_lat: rlat,
            receiver_lon: rlon,
            receiver_alt_m: 4.0,
            start_lat: alat,
            start_lon: alon,
            end_lat: blat,
            end_lon: blon,
            source_height_m: SOURCE_HEIGHT_ROAD,
            cp_lat: cplat,
            cp_lon: cplon,
            src_alt_m: SOURCE_HEIGHT_ROAD,
            cp_screening: &NO_TERRAIN,
            cp_terrain: &NO_TERRAIN,
            ground_g: 0.0,
            barriers: &bars,
            obstacles: &empty,
            length_m: 2.0 * seg_half,
            dist_m: rcv_x,
            exclusion_radius_m: 0.0,
            bounds: ARC_BOUNDS_DEFAULT,
        };
        let frame = Frame::at(rlat, rlon);
        // The span exactly as `arc_screened_attenuation` builds it — this
        // fixture straddles ±π (the source line is due west of the receiver),
        // which is the seam the ±TAU shifts exist for.
        let base = frame.azimuth(alat, alon);
        let delta = wrap_pi(frame.azimuth(blat, blon) - base);
        let (lo, hi) = if delta < 0.0 {
            (base + delta, base)
        } else {
            (base, base + delta)
        };
        assert!(
            hi - lo > DEGENERATE_SPAN_RAD && hi - lo < ESCALATE_SPAN_RAD,
            "fixture span {:.4} rad must be one evaluated part",
            hi - lo
        );
        let mut prof = PathProfile::new();
        let mut cands: Vec<CrossingCandidate> = Vec::new();
        let cp = interval_screening(
            &q,
            &FlatGround,
            &frame,
            frame.azimuth(cplat, cplon),
            &mut prof,
            &mut cands,
            None,
        )
        .expect("cp ray must evaluate")
        .1;
        q.cp_screening = &cp;
        assert!(cp[4] > 5.0, "fixture wall must screen the cp ray: {cp:?}");

        // A pre-built skyline holding ONE full-turn arc whose unwrapped `lo`
        // sits 0.001 rad below the span's far end — 100 m out, 8 m up, i.e.
        // the wall's own arc as `insert_merged` would have saturated it.
        let mut skyline = ArcSkyline {
            lat: rlat,
            lon: rlon,
            built: true,
            built_radius_m: [f32::MAX; SECTORS],
            arcs: vec![MergedArc {
                lo: hi - 0.001,
                hi: hi - 0.001 + TAU,
                near_m: 100.0,
                height_m: 8.0,
                key: fuse_key(100.0, 8.0),
            }],
            fuse_scratch: Vec::new(),
            overflows: 0,
            need_max_m: 0.0,
        };
        let mut scratch = ArcScreeningScratch::new();
        let out = arc_screened_attenuation(&q, &FlatGround, &mut skyline, &mut scratch);
        assert!(
            out[4] > 0.9 * cp[4],
            "a full-turn arc must block the WHOLE span: got {:.2} dB against the \
             cp verdict's {:.2} dB — the clip kept only its shift-0 graze and \
             called the rest of the fan clear",
            out[4],
            cp[4]
        );
    }

    /// Ground that is LEVEL along the cp ray and rises into a ridge either side
    /// of it — the minimal shape of the relief `screening_fixture` scene I puts
    /// under the whole rule. Elevation depends on northing alone, so the cp ray
    /// (due west, `y = 0`) sees flat ground while a ray to either end of the
    /// segment crosses a 30 m crest.
    struct RidgeGround;
    impl RasterSampler for RidgeGround {
        fn elevation(&self, lat: f64, _: f64) -> f64 {
            let y_m = (lat - OLAT) * M_PER_DEG_LAT;
            ((y_m.abs() - 100.0) * 0.3).clamp(0.0, 30.0)
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    /// THE CLEAR FAN CARRIES TERRAIN. A segment whose cp ray is level but whose
    /// off-centre directions run over a crest must hand back an increment that
    /// accounts for the crest — the clear part of the fan is clear of
    /// OBSTACLES, not of ground.
    ///
    /// Before the fix every clear direction was scored on the cp ray's
    /// `A_terrain` (zero here), so the whole ridge vanished from the average and
    /// the segment came back as loud as it would be on a plain. That is the
    /// mechanism behind fixture scene I's 1.51 dB, the last breach of the
    /// owner's 1.0 dB line, and it is worth many dB on this fixture.
    #[test]
    fn clear_fan_directions_carry_their_own_terrain() {
        let (rcv_x, seg_half) = (200.0_f64, 400.0_f64);
        let (rlat, rlon) = ll(rcv_x, 0.0);
        let (alat, alon) = ll(0.0, -seg_half);
        let (blat, blon) = ll(0.0, seg_half);
        let (cplat, cplon) = ll(0.0, 0.0);
        // Scene A's box (|y| ≤ 15 m): enough to put one blocked interval in the
        // list — the rule returns the cp bands with none — nowhere near enough
        // to cover the fan, so the ridge directions stay CLEAR.
        let obstacles = box_scene();
        let cp_flat = [0.0f64; NUM_BANDS];
        let q = ArcScreening {
            receiver_lat: rlat,
            receiver_lon: rlon,
            receiver_alt_m: 4.0,
            start_lat: alat,
            start_lon: alon,
            end_lat: blat,
            end_lon: blon,
            source_height_m: SOURCE_HEIGHT_ROAD,
            cp_lat: cplat,
            cp_lon: cplon,
            src_alt_m: SOURCE_HEIGHT_ROAD,
            cp_screening: &cp_flat,
            // The cp ray is level: no terrain on it, which is exactly why the
            // old rule lost the ridge.
            cp_terrain: &cp_flat,
            // SOFT ground: `A_ground ≥ 0`, so the handback channel's
            // hard-ground clamp (`mean_db < 0 ⇒ 0`, documented in
            // `arc_screened_attenuation`) cannot mask the effect under test.
            ground_g: 1.0,
            barriers: &[],
            obstacles: &obstacles,
            length_m: 2.0 * seg_half,
            dist_m: rcv_x,
            exclusion_radius_m: 0.0,
            bounds: ARC_BOUNDS_DEFAULT,
        };
        let mut skyline = ArcSkyline::default();
        let mut scratch = ArcScreeningScratch::new();
        let ridge = arc_screened_attenuation(&q, &RidgeGround, &mut skyline, &mut scratch);
        let mut skyline_flat = ArcSkyline::default();
        let mut scratch_flat = ArcScreeningScratch::new();
        let flat = arc_screened_attenuation(&q, &FlatGround, &mut skyline_flat, &mut scratch_flat);
        assert!(
            ridge[4] > flat[4] + 1.0,
            "the ridge must reach the average: ridge {:.2} dB vs flat {:.2} dB",
            ridge[4],
            flat[4]
        );
    }

    /// A free-standing 10 m deep block across the path at `x_m`.
    fn sweep_block(x_m: f64, h: f32) -> ObstacleSet {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[
                ll(x_m - 5.0, -60.0),
                ll(x_m + 5.0, -60.0),
                ll(x_m + 5.0, 60.0),
                ll(x_m - 5.0, 60.0),
            ],
            h,
            ObstacleKind::Building,
            1,
        );
        ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        }
    }

    /// The receiver's OWN building — a ring AROUND it, which is what puts a
    /// FULL-TURN arc (`near_m` a few metres, `hi - lo == TAU`) in the skyline.
    fn sweep_ring(rcv_x: f64, h: f32) -> ObstacleSet {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        let half = 6.0;
        b.add_ring(
            &[
                ll(rcv_x - half, -half),
                ll(rcv_x + half, -half),
                ll(rcv_x + half, half),
                ll(rcv_x - half, half),
            ],
            h,
            ObstacleKind::Building,
            0,
        );
        ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        }
    }

    /// Residual level of a flat 0 dB source after `atten` — LOWER = MORE
    /// screening, so the invariant reads "this must never go UP".
    fn residual_db(atten: &[f64; NUM_BANDS]) -> f64 {
        let e: f64 = (0..NUM_BANDS)
            .map(|i| 10f64.powf((crate::constants::A_WEIGHTING[i] - atten[i]) / 10.0))
            .sum();
        10.0 * e.log10()
    }

    /// Arc-averaged screening of one 250 m segment, with the cp verdict
    /// computed on the real cp ray exactly as `compute::roads` does.
    fn sweep_screening(
        rcv_x: f64,
        rcv_alt: f64,
        obstacles: &ObstacleSet,
        bars: &[Barrier],
        buf: &mut Buffers,
    ) -> [f64; NUM_BANDS] {
        let (rlat, rlon) = ll(rcv_x, 0.0);
        let (alat, alon) = ll(0.0, -125.0);
        let (blat, blon) = ll(0.0, 125.0);
        let (cplat, cplon) = ll(0.0, 0.0);
        let mut q = ArcScreening {
            receiver_lat: rlat,
            receiver_lon: rlon,
            receiver_alt_m: rcv_alt,
            start_lat: alat,
            start_lon: alon,
            end_lat: blat,
            end_lon: blon,
            source_height_m: SOURCE_HEIGHT_ROAD,
            cp_lat: cplat,
            cp_lon: cplon,
            src_alt_m: SOURCE_HEIGHT_ROAD,
            cp_screening: &NO_TERRAIN,
            cp_terrain: &NO_TERRAIN,
            ground_g: 0.0,
            barriers: bars,
            obstacles,
            length_m: 250.0,
            dist_m: rcv_x,
            exclusion_radius_m: 0.0,
            bounds: ARC_BOUNDS_DEFAULT,
        };
        let frame = Frame::at(rlat, rlon);
        let az = frame.azimuth(cplat, cplon);
        let mut prof = PathProfile::new();
        let mut cands: Vec<CrossingCandidate> = Vec::new();
        let cp = interval_screening(&q, &FlatGround, &frame, az, &mut prof, &mut cands, None)
            .map_or(NO_TERRAIN, |(_terrain, screening)| screening);
        q.cp_screening = &cp;
        buf.run(&q)
    }

    /// The whole grid, one screen at a time: 4 source distances × 3 receiver
    /// heights × 3 screen positions × 3 screen widths = 108 geometries per
    /// family, each swept over 33 screen heights. `with_ring` adds the
    /// receiver's own building, which is a RIVAL diffraction edge — see
    /// [`max_delta_edge_can_pick_the_weaker_screen`].
    ///
    /// Returns `(violating geometries, total, worst step dB, description)`.
    fn sweep_grid(family: &str, with_ring: bool) -> (usize, usize, f64, String) {
        let mut buf = Buffers::default();
        let (mut cases, mut bad_cases) = (0usize, 0usize);
        let (mut worst, mut worst_desc) = (0.0_f64, String::new());
        for &rcv_x in &[40.0_f64, 60.0, 100.0, 180.0] {
            for &rcv_alt in &[1.5_f64, 4.0, 10.0] {
                for &frac in &[0.25_f64, 0.5, 0.75] {
                    for &half_len in &[30.0_f64, 60.0, 120.0] {
                        let screen_x = rcv_x * (1.0 - frac);
                        let mut prev = f64::NAN;
                        let (mut bad, mut w, mut at) = (0usize, 0.0_f64, 0.0_f64);
                        for k in 0..=32 {
                            let (obstacles, bars, h) = if family == "wall" {
                                let h = 2.0 + k as f64 * 0.125; // 2 → 6 m
                                let o = if with_ring {
                                    sweep_ring(rcv_x, 10.0)
                                } else {
                                    ObstacleSet { indexes: vec![] }
                                };
                                (o, vec![sweep_wall(screen_x, half_len, h as f32)], h)
                            } else {
                                let h = 3.0 + k as f64 * 0.25; // 3 → 11 m
                                let mut o = sweep_block(screen_x, h as f32);
                                if with_ring {
                                    o.indexes
                                        .extend(sweep_ring(rcv_x, 10.0).indexes.iter().cloned());
                                }
                                (o, vec![], h)
                            };
                            let lvl = residual_db(&sweep_screening(
                                rcv_x, rcv_alt, &obstacles, &bars, &mut buf,
                            ));
                            // 0.05 dB of slack for f32 height/raster rounding; a
                            // real violation here is dB, not hundredths.
                            if prev.is_finite() && lvl - prev > 0.05 {
                                bad += 1;
                                if lvl - prev > w {
                                    w = lvl - prev;
                                    at = h;
                                }
                            }
                            prev = lvl;
                        }
                        cases += 1;
                        if bad > 0 {
                            bad_cases += 1;
                            if w > worst {
                                worst = w;
                                worst_desc = format!(
                                    "{family} d={rcv_x} m, rcv {rcv_alt} m, screen at {screen_x:.0} m (half-length {half_len} m): +{w:.2} dB at h={at:.3} m"
                                );
                            }
                        }
                    }
                }
            }
        }
        (bad_cases, cases, worst, worst_desc)
    }

    /// THE GATE, and it RUNS. 216 geometries (108 walls, 108 buildings) × 33
    /// heights each, 0 violations in both families.
    ///
    /// It was `#[ignore]`d while `diffraction::compute_single_edge_at` paired a
    /// CURVED favourable δ above the sight line with a STRAIGHT one below it:
    /// the two arms did not meet, δ_F stepped ~0.1 m at the crossing, and
    /// raising a screen through the sight line cut its 8 kHz attenuation from
    /// 4.77 to 1.76 dB — 47/108 walls and 18/108 buildings violated, worst
    /// +2.87 dB. CNOSSOS-EU (2.5.27) is the standard's own expression for the
    /// unblocked branch (`2ŜA + 2ÂR − ŜO − ÔR − ŜR`, arcs throughout) and it
    /// meets (2.5.26) exactly on the sight line, which closes the step at its
    /// source. The 1.76 dB is the CORRECT near-miss value and stayed;
    /// `diffraction::favourable_delta_is_continuous_across_the_sight_line` pins
    /// the continuity that makes this gate hold.
    #[test]
    fn taller_screen_never_makes_the_receiver_louder() {
        let mut failures: Vec<String> = Vec::new();
        for family in ["wall", "building"] {
            let (bad, cases, worst, desc) = sweep_grid(family, false);
            println!("{family}: {bad}/{cases} geometries violated, worst {worst:.2} dB [{desc}]");
            if bad > 0 {
                failures.push(format!(
                    "a TALLER screen made {bad}/{cases} {family} geometries LOUDER, worst +{worst:.2} dB [{desc}]"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }

    /// OPEN DEFECT, pinned so it cannot silently get worse: the single-edge
    /// rule picks the crossing with the largest δ, but the attenuation that
    /// crossing then earns is gated by its OWN Rayleigh δ\*, which depends on
    /// where along the path it stands. Two candidates with nearly equal δ can
    /// therefore attenuate very differently, and as a screen grows past its
    /// rival's δ the evaluated edge SWAPS — a step of up to 3.2 dB, upward.
    ///
    /// Reproduced here by a receiver standing within a couple of metres of its
    /// OWN roofline: the ring 6 m away is a rival edge whose δ is within a hair
    /// of the screen's. Every violation this reports is at `rcv_alt = 10 m`
    /// with a 10 m ring; no geometry with the receiver clear of its roof
    /// violates. Fixing it means ranking candidates by attenuation rather than
    /// δ, i.e. a δ\* fit per candidate instead of per winner — a hot-loop cost
    /// change that needs its own measurement, so it is NOT part of the clip /
    /// δ_F fix.
    ///
    /// Measured 2026-08-05: 15/108 walls and 33/108 buildings, worst +3.52 dB —
    /// IDENTICAL before and after the CNOSSOS (2.5.27) δ_F fix, to the digit.
    /// That is the useful part: the two defects are disjoint. Making δ_F
    /// continuous took its own sweep (`taller_screen_never_makes_the_receiver_louder`,
    /// same grid without the ring) from 47/108 + 18/108 to 0/108 + 0/108 and
    /// left this one untouched, so everything remaining here is the edge SWAP
    /// and nothing else. (The older "pre-fix 12/108, post-fix 5/108" note was
    /// stale — it predated the rest of the fix-pack.)
    #[test]
    #[ignore = "open defect: max-δ edge selection is not max-attenuation selection"]
    fn max_delta_edge_can_pick_the_weaker_screen() {
        let mut failures: Vec<String> = Vec::new();
        for family in ["wall", "building"] {
            let (bad, cases, worst, desc) = sweep_grid(family, true);
            println!(
                "{family} (with the receiver's own building): {bad}/{cases} violated, worst {worst:.2} dB [{desc}]"
            );
            if bad > 0 {
                failures.push(format!(
                    "{family}: {bad}/{cases}, worst +{worst:.2} dB [{desc}]"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }

    // =====================================================================
    // PHYSICAL INVARIANT: the map may not depend on the ORDER the sources
    // arrive in.
    //
    // A receiver's skyline is a mutable accumulator: it is grown on demand to
    // whatever radius the segment currently being evaluated needs, and every
    // later segment of that receiver reads the grown result. So the SET of arcs
    // a segment is clipped against depends on which segments came before it —
    // and if the merge conflates arcs at different RANGES, the extra arcs a
    // far-away source dragged in are no longer distinguishable from the near
    // ones, the admission test can no longer discard them, and a segment's
    // answer moves because of a source it has nothing to do with.
    //
    // Reordering the loader's rows is not supposed to change anything at all.
    // Measured on a production Praha road tile before the strata (2026-08-05,
    // `SURFACE_BUDGET_ETA=0`, so no energy-budget skip is in play and both
    // orders do bit-identically the same 10 183 548 path calls): RMS 0.069 dB,
    // max +5.43 dB, 21 pixels over 1 dB.
    // =====================================================================

    /// Deterministic permutation `k` of `0..n` — a tiny LCG shuffle, so the
    /// orders are reproducible and no RNG crate enters the kernel's tests.
    fn permutation(n: usize, k: u64) -> Vec<usize> {
        let mut v: Vec<usize> = (0..n).collect();
        let mut state = k.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        for i in (1..n).rev() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            v.swap(i, (state >> 33) as usize % (i + 1));
        }
        v
    }

    /// A village of same-height houses receding from the receiver, plus a mixed
    /// row — the geometry a dense receiver actually stands in, where a chain of
    /// similar heights spans three decades of range.
    ///
    /// `heights` cycles through its values row by row, so `&[8.0]` is the
    /// single-height city that makes every pair fusible on height alone, and a
    /// mixed slice is the varied-data case.
    fn village(rows: &[f64], heights: &[f32]) -> ObstacleSet {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        let mut id = 0u32;
        for (r, &x) in rows.iter().enumerate() {
            let h = heights[r % heights.len()];
            for k in -3i32..=3 {
                let y = k as f64 * 60.0 + (r % 3) as f64 * 17.0;
                b.add_ring(
                    &[
                        ll(x, y - 12.0),
                        ll(x + 8.0, y - 12.0),
                        ll(x + 8.0, y + 12.0),
                        ll(x, y + 12.0),
                    ],
                    h,
                    ObstacleKind::Building,
                    id,
                );
                id += 1;
            }
        }
        ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        }
    }

    /// Per-path state for [`segment_bands_both`]: the mutable kernel's skyline
    /// and, in lockstep, the popup scheduler's replay (its own skyline, the
    /// current epoch snapshot, its own scratch).
    #[derive(Default)]
    struct BothPaths {
        sky_mut: ArcSkyline,
        scratch_mut: ArcScreeningScratch,
        sky_sched: ArcSkyline,
        epoch_snap: Option<SkylineSnapshot>,
        scratch_sched: ArcScreeningScratch,
    }

    /// One arc-screened 250 m segment whose midpoint sits `dist_m` west of the
    /// receiver, offset `off_m` north — the caller's own cp ray included, so the
    /// number compared is the one the kernel hands the tile. Evaluated TWICE:
    /// through the mutable kernel, and through the scheduler replay the popup's
    /// parallel segment loop runs (`planned_ensure` → `needs_growth` →
    /// `ensure_planned` → `snapshot` → `arc_screened_attenuation_prepared`).
    /// The two must agree to the bit on every source, whatever order the
    /// sources arrive in — that is the equivalence the parallel loop stands on.
    fn segment_bands_both(
        rcv_x: f64,
        dist_m: f64,
        off_m: f64,
        obstacles: &ObstacleSet,
        bars: &[Barrier],
        state: &mut BothPaths,
    ) -> ([f64; NUM_BANDS], [f64; NUM_BANDS]) {
        let (rlat, rlon) = ll(rcv_x, 0.0);
        let sx = rcv_x - dist_m;
        let (alat, alon) = ll(sx, off_m - 125.0);
        let (blat, blon) = ll(sx, off_m + 125.0);
        let (cplat, cplon) = ll(sx, off_m.clamp(-125.0, 125.0));
        let mut q = ArcScreening {
            receiver_lat: rlat,
            receiver_lon: rlon,
            receiver_alt_m: 4.0,
            start_lat: alat,
            start_lon: alon,
            end_lat: blat,
            end_lon: blon,
            source_height_m: SOURCE_HEIGHT_ROAD,
            cp_lat: cplat,
            cp_lon: cplon,
            src_alt_m: SOURCE_HEIGHT_ROAD,
            cp_screening: &NO_TERRAIN,
            cp_terrain: &NO_TERRAIN,
            ground_g: 0.5,
            barriers: bars,
            obstacles,
            length_m: 250.0,
            dist_m,
            exclusion_radius_m: 0.0,
            bounds: ARC_BOUNDS_DEFAULT,
        };
        let frame = Frame::at(rlat, rlon);
        let az = frame.azimuth(cplat, cplon);
        let mut prof = PathProfile::new();
        let mut cands: Vec<CrossingCandidate> = Vec::new();
        let cp = interval_screening(&q, &FlatGround, &frame, az, &mut prof, &mut cands, None)
            .map_or(NO_TERRAIN, |(_terrain, screening)| screening);
        q.cp_screening = &cp;
        let via_mut =
            arc_screened_attenuation(&q, &FlatGround, &mut state.sky_mut, &mut state.scratch_mut);
        let via_sched = match planned_ensure(
            q.receiver_lat,
            q.receiver_lon,
            q.start_lat,
            q.start_lon,
            q.end_lat,
            q.end_lon,
            q.dist_m,
            q.length_m,
            q.bounds,
        ) {
            None => *q.cp_screening,
            Some(p) => {
                if state
                    .sky_sched
                    .needs_growth(q.receiver_lat, q.receiver_lon, &p, q.bounds)
                {
                    state.epoch_snap = None;
                    state.sky_sched.ensure_planned(
                        q.receiver_lat,
                        q.receiver_lon,
                        &p,
                        q.obstacles,
                        q.barriers,
                        q.source_height_m,
                        q.bounds,
                    );
                }
                let snap = state
                    .epoch_snap
                    .get_or_insert_with(|| state.sky_sched.snapshot())
                    .clone();
                arc_screened_attenuation_prepared(&q, &FlatGround, &snap, &mut state.scratch_sched)
            }
        };
        (via_mut, via_sched)
    }

    /// THE GATE. Shuffle the order the sources reach one receiver and every
    /// source's own answer must come back bit-identical.
    ///
    /// Runs on four geometries — the single-height village that makes the fuse
    /// chain on production data (84.6 % of Praha's shading edges carry one of
    /// three literal heights), a HEIGHT LADDER whose 2.5 m steps each sit inside
    /// a 3 m tolerance so the whole street chains transitively, a village with a
    /// noise wall in it (walls enter the skyline by a different route), and a
    /// shallow one with no far rows at all — each over 12 permutations of 9
    /// sources spread from 60 m to 3 km. The far sources are what grow the
    /// skyline past the near ones' needs, so a permutation that puts them first
    /// is the one that used to poison the near answers.
    ///
    /// Measured against the height-only tolerance this replaced: worst
    /// **11.64 dB** (height-ladder village, permutation 2, the 400 m source's
    /// 1 kHz band, 8.17 vs 19.82 dB). With the strata it is 0.000000 dB, and the
    /// gate is exact equality — a reordering that changes anything at all is a
    /// defect, not a tolerance to be widened.
    ///
    /// EXTENDED for the popup's parallel segment loop: every source is also
    /// evaluated through the scheduler replay (`planned_ensure` →
    /// `needs_growth` → `ensure_planned` → `snapshot` →
    /// `arc_screened_attenuation_prepared`) in lockstep, and the two paths
    /// must agree to the bit on every source in every permutation — pinning
    /// both the snapshot-eval equivalence and the no-op-ensure elision.
    #[test]
    fn source_order_never_changes_the_answer() {
        // Ranges deliberately spread over three decades: 1.2 m (the receiver's
        // own facade) out to 600 m, the spread measured on tile 2206/1391.
        let far_rows: Vec<f64> = (0..14).map(|k| -1.2 - k as f64 * 45.0).collect();
        let shallow_rows: Vec<f64> = (0..3).map(|k| -1.2 - k as f64 * 45.0).collect();
        let wall = [sweep_wall(-40.0, 200.0, 4.0)];
        let scenes: [(&str, ObstacleSet, &[Barrier]); 4] = [
            ("one-height village", village(&far_rows, &[8.0]), &[]),
            (
                "height-ladder village",
                village(&far_rows, &[3.0, 5.5, 8.0, 10.5]),
                &[],
            ),
            ("village + noise wall", village(&far_rows, &[8.0]), &wall),
            ("shallow village", village(&shallow_rows, &[8.0]), &[]),
        ];
        // (distance to the segment, along-track offset): near sources first in
        // the unshuffled list, so array order is the benign one and the shuffles
        // have somewhere to go.
        let sources: [(f64, f64); 9] = [
            (60.0, 0.0),
            (85.0, 40.0),
            (140.0, -90.0),
            (220.0, 30.0),
            (400.0, -150.0),
            (700.0, 200.0),
            (1200.0, -300.0),
            (2000.0, 500.0),
            (3000.0, -800.0),
        ];
        let rcv_x = 0.0;
        let mut worst = 0.0_f64;
        let mut worst_desc = String::new();
        for (name, obstacles, bars) in scenes.iter() {
            let mut reference: Vec<[f64; NUM_BANDS]> = Vec::new();
            for p in 0..12u64 {
                let order = if p == 0 {
                    (0..sources.len()).collect::<Vec<_>>()
                } else {
                    permutation(sources.len(), p)
                };
                let mut state = BothPaths::default();
                let mut got = vec![[0.0_f64; NUM_BANDS]; sources.len()];
                for &s in order.iter() {
                    let (d, off) = sources[s];
                    let (via_mut, via_sched) =
                        segment_bands_both(rcv_x, d, off, obstacles, bars, &mut state);
                    // The popup's scheduler replay (snapshot + prepared eval,
                    // no-op ensures elided) must reproduce the mutable kernel
                    // to the BIT, in every arrival order — the equivalence the
                    // parallel segment loop stands on.
                    assert_eq!(
                        via_mut.map(f64::to_bits),
                        via_sched.map(f64::to_bits),
                        "{name}, permutation {p}, source at {d} m (offset {off} m): \
                         prepared/snapshot path diverged from the mutable kernel"
                    );
                    got[s] = via_mut;
                }
                if p == 0 {
                    reference = got;
                    continue;
                }
                for (s, bands) in got.iter().enumerate() {
                    for b in 0..NUM_BANDS {
                        let d = (bands[b] - reference[s][b]).abs();
                        if d > worst {
                            worst = d;
                            let (dist, off) = sources[s];
                            worst_desc = format!(
                                "{name}, permutation {p}, source at {dist} m (offset {off} m), band {b}: {:.4} vs {:.4} dB",
                                bands[b], reference[s][b]
                            );
                        }
                    }
                }
            }
        }
        println!("worst order-induced shift {worst:.6} dB [{worst_desc}]");
        assert_eq!(
            worst, 0.0,
            "the SAME sources in a different order gave a different answer — \
             worst {worst:.4} dB [{worst_desc}]"
        );
    }
}

#[cfg(test)]
mod wedge_tests {
    use super::*;
    use crate::constants::{m_per_deg_lon, M_PER_DEG_LAT};
    use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleKind};
    use std::sync::Arc;

    const OLAT: f64 = 50.0;
    const OLON: f64 = 14.0;

    fn ll(x_m: f64, y_m: f64) -> (f64, f64) {
        (
            OLAT + y_m / M_PER_DEG_LAT,
            OLON + x_m / m_per_deg_lon(OLAT.to_radians()),
        )
    }

    /// Buildings scattered all round the receiver, so a disk gather collects
    /// far more than any one span can read.
    fn ring_city() -> ObstacleSet {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        let mut id = 0u32;
        for k in 0..48 {
            let a = k as f64 * std::f64::consts::TAU / 48.0;
            for r in [180.0f64, 420.0, 900.0] {
                let (cx, cy) = (r * a.cos(), r * a.sin());
                b.add_ring(
                    &[
                        ll(cx - 9.0, cy - 9.0),
                        ll(cx + 9.0, cy - 9.0),
                        ll(cx + 9.0, cy + 9.0),
                        ll(cx - 9.0, cy + 9.0),
                    ],
                    10.0,
                    ObstacleKind::Building,
                    id,
                );
                id += 1;
            }
        }
        ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        }
    }

    /// THE gate: for every span, the arcs the wedge gather produces INSIDE that
    /// span must be identical to the disk gather's. An obstacle outside the
    /// span cannot clip the span, so restricting the gather is exact — this
    /// pins that claim rather than asserting it.
    #[test]
    fn wedge_gather_matches_the_disk_inside_the_span() {
        let set = ring_city();
        let (rlat, rlon) = ll(0.0, 0.0);
        let mut mismatches = Vec::new();
        for k in 0..40 {
            let lo = -std::f64::consts::PI + k as f64 * 0.157;
            for &width in &[0.02f64, 0.2, 1.0] {
                let hi = lo + width;
                let collect = |wedge: Option<(f64, f64)>| {
                    let mut v: Vec<(i64, i64)> = Vec::new();
                    set.skyline_arcs_within(rlat, rlon, 0.0, 1200.0, 0.05, 0.0, wedge, &mut |a| {
                        // Only arcs that MEET the span can matter to it.
                        if a.hi >= lo && a.lo <= hi {
                            v.push(((a.lo * 1e9) as i64, (a.hi * 1e9) as i64));
                        }
                    });
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let disk = collect(None);
                let wedge = collect(Some((lo, hi)));
                if disk != wedge && mismatches.len() < 3 {
                    mismatches.push(format!(
                        "span [{lo:.3},{hi:.3}]: disk {} arcs, wedge {} arcs",
                        disk.len(),
                        wedge.len()
                    ));
                }
            }
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }

    /// Two 6 m-wide buildings 3.2° apart, both inside sector 0 (azimuth
    /// [0, 5.625°)) — the geometry the sector bookkeeping cannot tell apart but
    /// the wedge gather can. Both stand at 800 m, where 3.2° is 45 m of
    /// separation: the gather filters by GRID CELL (64 m), so at short range it
    /// over-collects and hides the difference. Distance is what makes the wedge
    /// bite, and two roads 45 m apart 800 m away is an ordinary city block.
    fn two_in_one_sector() -> ObstacleSet {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        for (id, deg) in [1.2f64, 4.4].into_iter().enumerate() {
            let a = deg.to_radians();
            let (cx, cy) = (3000.0 * a.cos(), 3000.0 * a.sin());
            b.add_ring(
                &[
                    ll(cx - 3.0, cy - 3.0),
                    ll(cx + 3.0, cy - 3.0),
                    ll(cx + 3.0, cy + 3.0),
                    ll(cx - 3.0, cy + 3.0),
                ],
                10.0,
                ObstacleKind::Building,
                id as u32,
            );
        }
        ObstacleSet {
            indexes: vec![Arc::new(b.build())],
        }
    }

    /// THE bookkeeping gate. `built_radius_m` is one radius PER SECTOR, but the
    /// gather is restricted to the caller's WEDGE, which is routinely far
    /// narrower than a 5.6° sector (a 250 m microsegment seen end-on, or any
    /// distant one, spans a fraction of a degree). Marking the whole sector
    /// walked to `need` after collecting only that wedge claims coverage that
    /// was never collected, so the NEXT span inside the same sector short-
    /// circuits on `from >= need` and reads an empty sky: no blocked interval,
    /// the closest-point verdict stands for the whole segment, and the constant-
    /// width shadow stripe this module exists to remove comes back — silently,
    /// for that segment alone. The cure is to snap the wedge out to the sector
    /// boundaries it is about to mark, which makes the invariant true by
    /// construction and still gathers a small fraction of the disk.
    ///
    /// Order matters: the FAR span goes first, so the near one is the victim.
    #[test]
    fn a_second_span_in_the_same_sector_still_sees_its_obstacles() {
        let set = two_in_one_sector();
        let mut sky = ArcSkyline::default();
        let far = (1.0f64.to_radians(), 1.4f64.to_radians());
        let near = (4.2f64.to_radians(), 4.6f64.to_radians());
        for (span, need) in [(far, 3500.0), (near, 3000.0)] {
            sky.ensure(
                OLAT,
                OLON,
                span.0,
                span.1,
                need,
                &set,
                &[],
                0.05,
                ARC_BOUNDS_DEFAULT,
            );
        }
        let az = 4.4f64.to_radians();
        assert!(
            sky.arcs.iter().any(|a| a.lo <= az && a.hi >= az),
            "second span read an empty sky: {} arc(s), none covering {az:.4} rad",
            sky.arcs.len()
        );
    }

    /// And it must actually cut the gather, or it is bookkeeping for nothing.
    #[test]
    fn wedge_gather_collects_far_less_than_the_disk() {
        let set = ring_city();
        let (rlat, rlon) = ll(0.0, 0.0);
        let count = |wedge: Option<(f64, f64)>| {
            let mut n = 0usize;
            set.skyline_arcs_within(rlat, rlon, 0.0, 1200.0, 0.05, 0.0, wedge, &mut |_| n += 1);
            n
        };
        let disk = count(None);
        let narrow = count(Some((0.4, 0.44))); // ~2.3°, a rail-like span
        assert!(
            narrow * 8 < disk,
            "wedge gathered {narrow} of the disk's {disk} — not worth the bookkeeping"
        );
        println!("disk {disk} arcs vs wedge {narrow} arcs");
    }
}

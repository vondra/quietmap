//! Uniform angular quadrature of a line microsegment's screening — the tile
//! path's replacement for the single characteristic-point verdict, and the
//! sibling of [`arc_screening`](super::arc_screening).
//!
//! A 250 m microsegment subtends an ANGLE at a receiver, and every direction in
//! that fan flies past its own buildings. The kernel shipped with one ray for
//! the whole fan — the closest point, the "characteristic point" — which is
//! right only when the fan is narrow or the obstacles are uniform across it. In
//! a street grid it is neither.
//!
//! Two ways to fix that, and they compute the SAME integral:
//!
//! * `arc_screening` clips the fan against the receiver's obstacle SKYLINE and
//!   evaluates one ray per blocked sub-interval — adaptive quadrature, whose
//!   nodes come from the geometry.
//! * this module splits the fan into `n` EQUAL angular buckets and evaluates one
//!   ray at each bucket's centre — uniform quadrature, whose nodes come from
//!   nothing but `n`.
//!
//! Same measure (uniform in angle, which for a line source is uniform in length
//! weighted by the `dl/r²` its elements actually contribute), same terrain
//! convention (each ray's `max(A_ground, A_terrain + A_screen)` is taken on THAT
//! DIRECTION's own terrain — see below), same ground and vegetation (resolved
//! once on the cp ray). So they differ ONLY in quadrature rule, and a high-`n`
//! arm is a converged reference for BOTH.
//!
//! `n = 1` is NOT the shipped cp verdict: one uniform bucket puts its ray at the
//! fan's CENTRE azimuth, while the cp is the segment's CLOSEST point, and the
//! two coincide only when the receiver faces the segment's middle. The tile
//! kernel keeps `n = 1` on the cp path for exactly that reason (it does not call
//! this function at all).
//!
//! `QM_SEG_SAMPLES=1` is therefore NOT by itself the pre-2026-08-05 output. It
//! only routes the painter down the cp fallback
//! (`tile_painter::scatter_band`). With a complete vector store that fallback
//! still arc-clips every segment wider than [`SEG_ARC_MIN_SPAN_RAD`] (3°, the
//! 2026-08-08 gate); without that complete skyline authority, raster fallback
//! keeps the cp verdict. Narrow vector-backed spans also keep a plain cp verdict
//! they did not keep before. Restoring the old vector-backed bytes takes
//! `QM_SEG_SAMPLES=1 QM_ARC_MIN_SPAN_DEG=0` together, which until 2026-08-19 was
//! also the pairing the CPU-vs-GPU tile comparison pinned (SPEC §4.7) — the
//! kernel then had no quadrature and arc-screened every pair; it now paints this
//! rule (5 buckets + the 3° gate, compiled in).
//!
//! ## CLEAR IS NOT TERRAIN-FREE, and neither is a bucket
//!
//! Every bucket ray marches its own path profile and therefore already knows its
//! own `A_terrain`; compositing it against the CP ray's instead is right only
//! over level ground. Over relief the source point at one edge of a 250 m
//! microsegment can stand tens of metres above the one at the other, and the
//! crest between it and the receiver is a different crest in every direction.
//! `arc_screening` carried exactly this defect until 2026-08-05, where fixing it
//! costs extra rays for the clear gaps (worth 1.51 → 0.93 dB on the fixture's one
//! hillside scene); here the ray is already marched and the terrain already
//! computed, so using it instead of throwing it away is FREE.
//!
//! MEASURED 2026-08-05 on four production tiles against an `n = 9` reference
//! (checked against `n = 17`): the uniform rule at `n = 5` is cheaper AND closer
//! than arc screening on every one of them, and the two corrections OVERLAP
//! (r = 0.67-0.80) rather than add. The tile path therefore runs this
//! (`tile_painter::scatter_band::seg_samples`, default 5). The popup keeps arc
//! screening on its own single cp ray: one receiver can afford the adaptive rule,
//! and it is the accuracy etalon.
//!
//! The two also NEST — pass a live [`ArcBounds`](super::arc_screening::ArcBounds)
//! in the query and every bucket runs its own arc query over its own sub-span,
//! the bucket's centre ray being that sub-segment's characteristic point. Since
//! 2026-08-08 that is not an option but the shipped configuration, gated on
//! bucket width by [`SEG_ARC_MIN_SPAN_RAD`] — read that before touching `n`.
//!
//! ## A BIGGER `n` IS NOT A FIX — measured 2026-08-08, and this is the whole file
//!
//! The uniform rule left a tail: on the 11 `screening_fixture` scenes `n = 5`
//! reaches 2.4-7.6 dB at its worst receiver where arc screening holds 0.6-0.9,
//! and the owner's line is 1.0 dB at EVERY receiver. The obvious lever is `n`,
//! and it does not work, at any price:
//!
//! | `n` | 5 | 9 | 17 | 33 | 49 | 65 | 129 |
//! |---|---|---|---|---|---|---|---|
//! | scene E worst | 7.60 | 4.49 | 1.98 | 0.89 | 0.72 | 0.66 | 0.96 |
//! | scene F worst | 2.17 | 1.86 | 1.75 | 1.09 | 1.19 | 0.88 | 0.89 |
//! | scene J worst | 3.08 | 2.89 | 0.99 | 1.67 | 0.63 | 0.63 | 0.78 |
//! | scene G mean  | 0.40 | 0.34 | 0.32 | 0.30 | 0.30 | 0.30 | 0.30 |
//!
//! The MEAN falls as `n^-1`, exactly as the tile measurement said. The WORST
//! RECEIVER does not fall monotonically at all: scene F reads 2.17 dB at `n = 5`
//! and **8.21 dB at `n = 6`**; scene J reads 0.99 at 17 and 1.67 at 33. Uniform
//! nodes ALIAS against a building row — a bucket pitch that happens to land the
//! nodes in the gaps reports the row as open, and whether it does is a resonance
//! between the pitch and the geometry, not a function of how many nodes there
//! are. Nothing under `n ≈ 65` is reliably inside 1.0 dB, and `n = 65` is 13× the
//! ray budget, i.e. slower than the arc screening this rule replaced.
//!
//! Two shapes of "spend nodes where the fan needs them" were built and MEASURED
//! on the same 11 scenes, and both are rejected — the numbers are here so nobody
//! pays for them twice:
//!
//! * **Bucket-WIDTH cap** (`n = clamp(ceil(span / W), 5, ..)`, the lever the
//!   2026-08-05 note proposed): non-monotone for the same aliasing reason. At the
//!   proposed `W = 0.26` rad, six of the eleven scenes get WORSE than uncapped
//!   (F 2.17 → 3.26, G 2.75 → 4.20, J 3.08 → 4.97); it takes `W = 0.03` rad
//!   before every scene is under 1.0 dB, and that is 3.5× the rays. It only ever
//!   multiplies nodes, and a node COUNT is not what the tail is short of.
//! * **Refine where the rays DISAGREED** (trisect any bucket whose neighbour
//!   came back more than τ dB away, nodes nesting so a split costs two rays):
//!   halves the mean (0.40 → 0.26 dB, past arc screening's 0.30) and fixes scene
//!   A, but leaves E/F/G/H/I/J at 1.6-3.2 dB. A disagreement test can only see
//!   what it already sampled, and a gap wholly inside one bucket is invisible to
//!   the two rays either side of it. On top of the arc gate it is a REGRESSION
//!   (worst 0.85 → 1.30 dB): changing the bucket widths changes which buckets
//!   clear the gate.
//!
//! What is left is the thing arc screening always had: nodes that come from the
//! GEOMETRY. Hence [`SEG_ARC_MIN_SPAN_RAD`] — the nesting was already
//! implemented, it just had to be turned on where it pays.

use crate::propagation::arc_screening::{
    arc_screened_attenuation_with_ground, segment_can_span, ArcBounds, ArcScreening,
    ArcScreeningScratch, ArcSkyline,
};
use crate::propagation::iso9613::{fast_exp_f64, ground_or_barrier_db, legacy_ground_atten_bands};
use crate::propagation::obstacle_index::{CellPrune, CrossingCandidate};
use crate::propagation::path_effects::{screening_attenuation, terrain_attenuation, ObstacleInput};
use crate::propagation::PathProfile;
use crate::types::{RasterSampler, NUM_BANDS};

/// `ln(10)/10` — the dB↔energy exponent, through the kernel's own fast
/// exponential so the rounding matches every other energy conversion.
const DB_TO_ENERGY_EXP: f64 = std::f64::consts::LN_10 / 10.0;

/// A BUCKET wider than this arc-screens its own sub-span; a narrower one keeps
/// its single centre ray. In radians of azimuth — the gate is
/// [`segment_can_span`], so it is a width test, not a distance test.
///
/// This is the whole fix of 2026-08-08, and it is a SELECTOR, not a new rule:
/// the nesting already existed (module docs), off everywhere. What the module
/// docs establish is that the tail cannot be bought with more uniform nodes at
/// any price, and that only geometry-placed nodes reach it. What this constant
/// adds is the observation that geometry-placed nodes are only NEEDED where the
/// fan is wide — which on a tile is a small minority of pairs, because tile cost
/// is dominated by far sources whose whole 250 m microsegment subtends less than
/// one bucket's worth of this.
///
/// MEASURED, `screening_fixture`, all 11 scenes, worst receiver over the suite:
///
/// | gate | off | 15° | 10° | 7° | 5° | **4°** | **3°** | 2° | 1° |
/// |---|---|---|---|---|---|---|---|---|---|
/// | worst | 7.60 | 2.75 | 2.76 | 2.00 | 1.33 | **0.96** | **0.85** | 0.85 | 0.85 |
///
/// A clean knee: 5° fails the owner's line (scene H 1.33 dB), 4° passes, and
/// below 3° the suite sits on a 0.85 dB PLATEAU that is not quadrature error at
/// all — arc screening itself reads 0.88 there, and refining the fixture's own
/// reference moves it. So 3° is one notch inside the knee AND on the plateau,
/// which is the definition of not fitting the constant to the measurement.
///
/// COST, four production tiles, CPU-seconds against the same paint with the gate
/// off, sandwiched arms: **+4.1 % (praha) / +5.2 % (suburb) / +7.6 % (d1open) /
/// +1.7 % (rail2206)**, i.e. it keeps 92-98 % of the uniform rule's own speedup
/// over arc screening. Nearly free for the reason above — a 250 m microsegment
/// 3 km out has a 0.08 rad fan, one bucket of which is 0.017 rad, so it never
/// asks, and pairs like that are what a tile's cost is made of. Re-confirmed on a
/// later tree state that re-prices every short ray: +6.2 % / +1.3 % on the two
/// tiles re-run. SPEC §4.7 carries the production policy.
/// Spelled `to_radians` and not `3·π/180` on purpose: the two differ by 1 ULP
/// (`…bebd6` vs `…bebd5`), and `QM_ARC_MIN_SPAN_DEG=3` — the spelling the cost and
/// accuracy above were MEASURED through — goes via `to_radians`. Same bits, same
/// `segment_can_span` verdict on a pair that lands on the threshold.
pub const SEG_ARC_MIN_SPAN_RAD: f64 = 3.0_f64.to_radians();

/// Buckets per microsegment fan — the `n` the tile painter ships with and the
/// CUDA surface kernel compiles in (`noise-gpu/build.rs` injects it). The
/// module docs carry the four-tile measurement this comes from and why a bigger
/// `n` is not a fix.
pub const SEG_SAMPLES_DEFAULT: usize = 5;

/// The arc bounds a bucket's nested query runs under: [`ArcBounds::shipped`] with
/// the span gate raised to [`SEG_ARC_MIN_SPAN_RAD`], so only a wide bucket pays.
///
/// ONE function for the tile painter AND `screening_fixture`, which is the point:
/// the fixture's `v5` arm is what gates this rule, and a fixture that gated a
/// configuration production does not paint would gate nothing. `QM_ARC_MIN_SPAN_DEG`
/// still overrides — `=0` is arc screening at its own shipped threshold (every
/// bucket queries), `=180` is the pre-2026-08-08 rule (no bucket does).
pub fn seg_arc_bounds() -> ArcBounds {
    let mut b = ArcBounds::shipped();
    if std::env::var_os("QM_ARC_MIN_SPAN_DEG").is_none() {
        b.min_span_rad = SEG_ARC_MIN_SPAN_RAD;
    }
    b
}

/// Per-call ray buffers, amortised across every (segment, receiver) pair — the
/// same pattern as [`ArcScreeningScratch`], and separate from it because a
/// bucket's arc query needs its own buffers while this one is live.
#[derive(Default)]
pub struct SegSampleScratch {
    profile: PathProfile,
    candidates: Vec<CrossingCandidate>,
    arc: ArcScreeningScratch,
}

impl SegSampleScratch {
    pub fn new() -> Self {
        Self::default()
    }
}

/// What one [`sampled_gob_bands`] call cost, for the caller's own ray counters
/// (the tile scatter reports `path_calls` / `raster_samples` per layer).
#[derive(Clone, Copy, Default)]
pub struct SegSampleCost {
    /// Bucket rays actually marched (`≤ n`; a degenerate bucket is skipped).
    pub rays: u64,
    /// Raster cadence samples those rays took.
    pub raster_samples: u64,
    /// Buckets that cleared [`SEG_ARC_MIN_SPAN_RAD`] and ran the nested arc
    /// query (SPEC §4.7) — `> 0` marks this pair ELIGIBLE in the gather-redesign
    /// census ([`census`](super::census)).
    pub escalated: u64,
}

/// The receiver's flat-earth view of one microsegment's fan — the frame that
/// turns "angular fraction of the span" into an exact point ON the segment.
/// Same equirectangular model as `geo`, `obstacle_index` and `arc_screening`.
struct SegFan {
    /// Segment start and direction in the receiver-centred metric frame.
    ax: f64,
    ay: f64,
    ex: f64,
    ey: f64,
    /// Azimuth of the start point and the signed span to the end point.
    az0: f64,
    span: f64,
    m_per_deg_lon: f64,
}

impl SegFan {
    /// `None` when the segment is degenerate (sub-millimetre) at this receiver —
    /// there is no fan to quadrature and the caller keeps the cp verdict.
    fn new(rx_lat: f64, rx_lon: f64, start: (f64, f64), end: (f64, f64)) -> Option<Self> {
        let mlon = crate::constants::m_per_deg_lon(rx_lat.to_radians());
        let ax = (start.1 - rx_lon) * mlon;
        let ay = (start.0 - rx_lat) * crate::constants::M_PER_DEG_LAT;
        let bx = (end.1 - rx_lon) * mlon;
        let by = (end.0 - rx_lat) * crate::constants::M_PER_DEG_LAT;
        let (ex, ey) = (bx - ax, by - ay);
        if ex * ex + ey * ey < 1e-6 {
            return None;
        }
        let az0 = ay.atan2(ax);
        let mut span = by.atan2(bx) - az0;
        while span > std::f64::consts::PI {
            span -= std::f64::consts::TAU;
        }
        while span < -std::f64::consts::PI {
            span += std::f64::consts::TAU;
        }
        Some(Self {
            ax,
            ay,
            ex,
            ey,
            az0,
            span,
            m_per_deg_lon: mlon,
        })
    }

    /// The point on the segment the receiver sees at fraction `f ∈ [0,1]` of the
    /// span: `(lat, lon, horizontal distance)`. Ray × line intersection, the same
    /// solve `arc_screening` runs for an interval's centre azimuth. `None` when
    /// the receiver is degenerate against that direction (on the segment's line,
    /// or closer than a metre — where no screening geometry is left to resolve).
    fn at(&self, rx_lat: f64, rx_lon: f64, f: f64) -> Option<(f64, f64, f64)> {
        let az = self.az0 + f * self.span;
        let (ux, uy) = (az.cos(), az.sin());
        // a + t·e parallel to u  ⇒  cross(u, a) + t·cross(u, e) = 0.
        let cr_e = ux * self.ey - uy * self.ex;
        let cr_a = ux * self.ay - uy * self.ax;
        let tt = if cr_e.abs() > 1e-12 {
            (-cr_a / cr_e).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let (sx, sy) = (self.ax + tt * self.ex, self.ay + tt * self.ey);
        let d = (sx * sx + sy * sy).sqrt();
        if !d.is_finite() || d < 1.0 {
            return None;
        }
        Some((
            rx_lat + sy / crate::constants::M_PER_DEG_LAT,
            rx_lon + sx / self.m_per_deg_lon,
            d,
        ))
    }
}

/// The GROUND-OR-BARRIER term `max(A_ground, A_terrain + A_screen)` of one
/// (microsegment, receiver) pair, by uniform angular quadrature over `n` buckets
/// of the segment's fan, energy-averaged. In dB, per band.
///
/// This returns the COMPOSITE, not a screening increment, and that is
/// deliberate: `arc_screened_attenuation` has to hand its answer back as an
/// increment because its callers re-apply the `max`, and the channel cannot
/// carry a mean that lands below `A_ground` (its own docs spell out the ≤3 dB
/// hard-ground regime this loses). A caller that takes the composite directly —
/// the tile scatter does — has no such loss. A caller that must go back through
/// the increment channel can do `(gob[b] - cp_terrain[b]).max(0.0)` and inherit
/// exactly the same limitation `arc_screening` has, which is what makes the two
/// rules comparable arm-for-arm in the physics fixture.
///
/// `q.cp_screening` is UNUSED: every bucket carries its own ray, which is why
/// the caller may skip the cp screening evaluation entirely. Everything else in
/// the query means what it means for `arc_screened_attenuation`, including
/// `q.bounds` — leave it at a live value and each bucket arc-screens its own
/// sub-span; set `min_span_rad` beyond reach and the quadrature runs alone.
///
/// `None` when the segment is degenerate at this receiver (no fan): the caller
/// keeps its single-ray verdict.
pub fn sampled_gob_bands(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    n: usize,
    skyline: &mut ArcSkyline,
    scratch: &mut SegSampleScratch,
) -> Option<([f64; NUM_BANDS], SegSampleCost)> {
    let ground_bands = legacy_ground_atten_bands(q.ground_g);
    sampled_gob_bands_with_ground(q, rasters, n, &ground_bands, skyline, scratch)
}

/// [`sampled_gob_bands`] with a caller-formed characteristic-point ground
/// vector.  Individual bucket rays retain their own terrain/screening, while
/// node evaluation will later remove this current arc transport seam and carry
/// a full direct-ground composite per ray.
pub fn sampled_gob_bands_with_ground(
    q: &ArcScreening<'_>,
    rasters: &dyn RasterSampler,
    n: usize,
    ground_bands: &[f64; NUM_BANDS],
    skyline: &mut ArcSkyline,
    scratch: &mut SegSampleScratch,
) -> Option<([f64; NUM_BANDS], SegSampleCost)> {
    let n = n.max(1);
    let fan = SegFan::new(
        q.receiver_lat,
        q.receiver_lon,
        (q.start_lat, q.start_lon),
        (q.end_lat, q.end_lon),
    )?;
    let SegSampleScratch {
        profile,
        candidates,
        arc,
    } = scratch;
    let mut cost = SegSampleCost::default();
    let inv_n = 1.0 / n as f64;
    // Bucket width in azimuth — the chord law below needs it, and it is the
    // same for every bucket by construction.
    let d_az = fan.span.abs() * inv_n;
    let mut acc = [0.0f64; NUM_BANDS];
    let mut used = 0usize;
    let mut lo_pt = fan.at(q.receiver_lat, q.receiver_lon, 0.0);
    for k in 0..n {
        let hi_pt = fan.at(q.receiver_lat, q.receiver_lon, (k + 1) as f64 * inv_n);
        // A bucket whose centre ray lands within a metre of the receiver has no
        // screening geometry left to resolve; drop it from the mean rather than
        // march a degenerate ray.
        let Some((slat, slon, sdist)) =
            fan.at(q.receiver_lat, q.receiver_lon, (k as f64 + 0.5) * inv_n)
        else {
            lo_pt = hi_pt;
            continue;
        };
        let salt = rasters.elevation(slat, slon) + q.source_height_m;
        rasters.build_path_profile(slat, slon, q.receiver_lat, q.receiver_lon, sdist, profile);
        cost.rays += 1;
        cost.raster_samples += profile.len() as u64;
        let (terr, terr_delta_m) = terrain_attenuation(profile, salt, q.receiver_alt_m);
        q.obstacles.crossings_pruned(
            slat,
            slon,
            q.receiver_lat,
            q.receiver_lon,
            &CellPrune::for_profile(profile, salt, q.receiver_alt_m),
            candidates,
        );
        let mut scr = screening_attenuation(
            profile,
            q.barriers,
            ObstacleInput { candidates },
            salt,
            q.receiver_alt_m,
            q.exclusion_radius_m,
            &terr,
            terr_delta_m,
        );
        // Arc screening INSIDE the bucket, over the bucket's own sub-span. Off
        // whenever `q.bounds.min_span_rad` is beyond what a sub-span can reach.
        if let (Some((lat0, lon0, d0)), Some((lat1, lon1, d1))) = (lo_pt, hi_pt) {
            // Chord of the sub-span in the receiver's flat frame — exact, and
            // cheaper than another lat/lon distance.
            let sub_len = (d0 * d0 + d1 * d1 - 2.0 * d0 * d1 * d_az.cos())
                .max(0.0)
                .sqrt();
            let sub_dist = d0.min(d1).min(sdist);
            if segment_can_span(sub_len, sub_dist, q.bounds) {
                cost.escalated += 1;
                scr = arc_screened_attenuation_with_ground(
                    &ArcScreening {
                        start_lat: lat0,
                        start_lon: lon0,
                        end_lat: lat1,
                        end_lon: lon1,
                        cp_lat: slat,
                        cp_lon: slon,
                        src_alt_m: salt,
                        cp_screening: &scr,
                        // The bucket IS the sub-segment, so its own terrain —
                        // not the parent segment's cp — is what the nested
                        // quadrature composites against.
                        cp_terrain: &terr,
                        length_m: sub_len,
                        dist_m: sub_dist,
                        ..*q
                    },
                    rasters,
                    skyline,
                    ground_bands,
                    arc,
                );
            }
        }
        // The composite is taken on THIS bucket's terrain, not the cp ray's:
        // `terr` is already in hand from the ray we just marched.
        for (i, a) in acc.iter_mut().enumerate() {
            let gob = ground_or_barrier_db(ground_bands[i], terr[i], scr[i]);
            *a += fast_exp_f64(-gob * DB_TO_ENERGY_EXP);
        }
        used += 1;
        lo_pt = hi_pt;
    }
    super::census::quad_pair(cost.rays, cost.escalated);
    let mut out = [0.0f64; NUM_BANDS];
    if used == 0 {
        // Every bucket degenerate: the receiver sits ON this microsegment, where
        // the shipped cp ray also resolves to no screening. Ground alone, which
        // is what `max(A_gr, A_terrain + 0)` gives on a zero-length path.
        for (i, o) in out.iter_mut().enumerate() {
            let a_gr = ground_bands[i];
            *o = if q.cp_terrain[i] > 0.0 {
                a_gr.max(q.cp_terrain[i])
            } else {
                a_gr
            };
        }
        return Some((out, cost));
    }
    let inv = 1.0 / used as f64;
    for (i, o) in out.iter_mut().enumerate() {
        *o = -10.0 * (acc[i] * inv).max(1e-12).log10();
    }
    Some((out, cost))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::propagation::arc_screening::{ArcBounds, ARC_BOUNDS_EXACT};
    use crate::propagation::obstacle_index::{ObstacleIndex, ObstacleSet};
    use std::sync::Arc;

    const LAT: f64 = 50.0;
    const LON: f64 = 14.0;

    /// A flat world with nothing in it — every bucket ray sees the same ground,
    /// so the quadrature's own arithmetic is the only thing under test.
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

    fn empty_obstacles() -> ObstacleSet {
        ObstacleSet {
            indexes: vec![Arc::new(ObstacleIndex::builder(LAT, LON).build())],
        }
    }

    /// Arc screening beyond reach: no sub-span can ever clear the gate, so this
    /// test isolates the uniform quadrature instead of the shipped 3° nesting.
    fn arc_off() -> ArcBounds {
        ArcBounds {
            min_span_rad: f64::INFINITY,
            ..ARC_BOUNDS_EXACT
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn query<'a>(
        obstacles: &'a ObstacleSet,
        cp_screening: &'a [f64; NUM_BANDS],
        cp_terrain: &'a [f64; NUM_BANDS],
        barriers: &'a [crate::types::Barrier],
        half_len_deg: f64,
    ) -> ArcScreening<'a> {
        ArcScreening {
            receiver_lat: LAT,
            receiver_lon: LON,
            receiver_alt_m: 4.0,
            start_lat: LAT + 0.001,
            start_lon: LON + half_len_deg,
            end_lat: LAT + 0.001,
            end_lon: LON - half_len_deg,
            source_height_m: 0.05,
            cp_lat: LAT + 0.001,
            cp_lon: LON,
            src_alt_m: 0.05,
            cp_screening,
            cp_terrain,
            ground_g: 0.5,
            barriers,
            obstacles,
            length_m: 200.0,
            dist_m: 111.0,
            exclusion_radius_m: 0.0,
            bounds: arc_off(),
        }
    }

    /// Nothing screens, so every bucket returns the bare ground term and the
    /// energy mean of a constant is that constant — for ANY `n`. The invariant
    /// that catches a mis-weighted bucket or a bad energy round-trip. On this
    /// empty flat scene `n = 1` also matches the cp result; its fan-centre ray is
    /// not the cp ray in general (module docs).
    #[test]
    fn empty_world_is_ground_for_every_n() {
        let obstacles = empty_obstacles();
        let (zero, barriers) = ([0.0f64; NUM_BANDS], Vec::new());
        let q = query(&obstacles, &zero, &zero, &barriers, 0.0014);
        let mut skyline = ArcSkyline::default();
        let mut scratch = SegSampleScratch::new();
        for n in [1usize, 2, 5, 9, 17] {
            let (gob, cost) =
                sampled_gob_bands(&q, &FlatGround, n, &mut skyline, &mut scratch).expect("fan");
            assert_eq!(cost.rays, n as u64, "n={n} should march n rays");
            for (i, g) in gob.iter().enumerate() {
                let want = legacy_ground_atten_bands(0.5)[i];
                // 1e-4 dB, not exact: the energy round-trip goes through the
                // kernel's `fast_exp_f64`, worth ~1e-5 dB here — the same
                // approximation every other energy conversion in the engine
                // carries, deliberately shared so the rounding matches. What is
                // under test is the quadrature weighting, not the exponential.
                assert!(
                    (g - want).abs() < 1e-4,
                    "n={n} band {i}: {g} != ground {want}"
                );
            }
        }
    }

    /// A degenerate segment has no fan to quadrature: the caller must be told so
    /// it can keep its own single-ray verdict rather than silently lose screening.
    #[test]
    fn degenerate_segment_has_no_fan() {
        let obstacles = empty_obstacles();
        let (zero, barriers) = ([0.0f64; NUM_BANDS], Vec::new());
        let q = query(&obstacles, &zero, &zero, &barriers, 0.0);
        let mut skyline = ArcSkyline::default();
        let mut scratch = SegSampleScratch::new();
        assert!(sampled_gob_bands(&q, &FlatGround, 5, &mut skyline, &mut scratch).is_none());
    }

    /// Each BUCKET's own terrain — not the cp ray's — is what the composite is
    /// built on, and `max(A_ground, A_terrain + A_screen)` is applied INSIDE the
    /// average, per bucket, never after it.
    ///
    /// Over flat ground every bucket ray returns `A_terrain = 0`, so a cp
    /// terrain of 2.5 dB must be IGNORED entirely: if it leaked into the
    /// composite the answer would be `max(A_ground, 2.5) = 2.5`. That leak is
    /// the defect `arc_screening` carried until 2026-08-05 (module docs), and it
    /// is what makes a hillside read as level ground.
    #[test]
    fn composite_uses_each_buckets_own_terrain() {
        let obstacles = empty_obstacles();
        let (zero, barriers) = ([0.0f64; NUM_BANDS], Vec::new());
        let cp_terrain = [2.5f64; NUM_BANDS];
        let q = query(&obstacles, &zero, &cp_terrain, &barriers, 0.0014);
        let mut skyline = ArcSkyline::default();
        let mut scratch = SegSampleScratch::new();
        let (gob, _) =
            sampled_gob_bands(&q, &FlatGround, 5, &mut skyline, &mut scratch).expect("fan");
        for (i, g) in gob.iter().enumerate() {
            let want = legacy_ground_atten_bands(0.5)[i];
            assert!(
                (g - want).abs() < 1e-4,
                "band {i}: {g} != {want} — the cp terrain leaked into the composite"
            );
        }
    }

    /// [`SEG_ARC_MIN_SPAN_RAD`] gates on the BUCKET's own width, and that is what
    /// makes it nearly free: the sub-span a bucket covers is `1/n` of the
    /// segment's, so what asks for an arc query is a NEAR source, and a tile's
    /// cost is far sources.
    ///
    /// Pinned with the numbers the cost claim rests on — a 250 m microsegment
    /// under `n = 5` buckets, which is what production paints:
    ///  * 200 m out: 0.25 rad sub-span, well over the gate — queries.
    ///  * 3 km out: 0.017 rad, well under — never does, and that is why the
    ///    +7.5 % holds.
    ///
    /// The 3° threshold itself sits between them by an order of magnitude either
    /// way, so this is a test of the SELECTOR, not of a coincidence.
    #[test]
    fn arc_gate_is_a_bucket_width_test() {
        let bounds = ArcBounds {
            min_span_rad: SEG_ARC_MIN_SPAN_RAD,
            ..ARC_BOUNDS_EXACT
        };
        // A bucket is 1/n of the segment, in length as in angle.
        let sub_len = 250.0 / 5.0;
        assert!(
            segment_can_span(sub_len, 200.0, bounds),
            "a bucket of a microsegment 200 m out must arc-screen: that is the fan \
             the fixture's 7.60 dB lived in"
        );
        assert!(
            !segment_can_span(sub_len, 3000.0, bounds),
            "a bucket of a microsegment 3 km out must NOT: those pairs are the tile's \
             cost, and paying the skyline for them is what made arc screening 2.5x"
        );
        // And the whole point of gating per BUCKET rather than per segment: the
        // same 3 km segment DOES clear the gate over its full span, so a
        // segment-level gate would have queried it.
        assert!(segment_can_span(250.0, 3000.0, bounds));
    }
}

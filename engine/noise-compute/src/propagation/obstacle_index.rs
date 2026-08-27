//! Uniform-grid edge index for vector obstacles (buildings ∪ noise barriers)
//! — exact ray×edge crossings for screening, geodata-v2 Phase 1.
//!
//! Obstacle footprints are decomposed into EDGES in a local equirectangular
//! metric frame (origin fixed at construction; same flat-earth model as the
//! kernels' `M_PER_DEG_LAT` / `M_PER_DEG_LON_EQ·cos(lat)` math), binned into a
//! uniform grid, and each source→receiver ray walks its grid cells (DDA)
//! collecting exact intersection chainages. Crossings are dominant-edge
//! CANDIDATES for `path_effects` — they never extend the cadence sample
//! arrays (GPU `MAXT` envelope, IMD/vegetation integral algebra and the
//! bare-earth δ* fit stay untouched by construction; plan v5 Phase 1).

use crate::constants::{m_per_deg_lon, BUILDING_HEIGHT_MAX_M, M_PER_DEG_LAT};
use crate::envelope::EnvelopeClass;

use super::obstacle_index_file::IndexArray;
use super::streaming_reduction::SourceId64;

/// One obstacle edge in the index's local metric frame.
///
/// `#[repr(C)]` with only 4-byte POD fields: this struct IS the on-disk edge
/// record ([`super::obstacle_index_file`]), so a cached index maps straight
/// into the query walks with no decode step. `kind` is a plain code rather than
/// the enum because a byte pattern read back from a file must never be able to
/// forge an invalid discriminant.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(super) struct ObstacleEdge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    height_m: f32,
    id: u32,
    kind: u32,
}

/// No padding anywhere: the file layout and the in-memory layout are the same
/// bytes, and `size_of` is what the section arithmetic assumes.
const _: () = assert!(std::mem::size_of::<ObstacleEdge>() == 28);

impl ObstacleEdge {
    #[inline]
    fn kind(&self) -> ObstacleKind {
        ObstacleKind::from_code(self.kind)
    }
}

/// What produced an edge — popup trace classification ("building" vs
/// "barrier") becomes exact instead of raster-inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObstacleKind {
    Building,
    Barrier,
}

impl ObstacleKind {
    /// Stored form. Fixed for all time — these codes live in cached index
    /// files, so renumbering them silently reclassifies every cached edge.
    #[inline]
    const fn code(self) -> u32 {
        match self {
            ObstacleKind::Building => 0,
            ObstacleKind::Barrier => 1,
        }
    }

    /// Inverse of [`Self::code`]. Anything unknown reads as `Building`, the
    /// conservative arm: an edge screens either way, only the popup's label
    /// would differ.
    #[inline]
    const fn from_code(code: u32) -> Self {
        match code {
            1 => ObstacleKind::Barrier,
            _ => ObstacleKind::Building,
        }
    }
}

/// One exact ray×edge crossing: chainage `t ∈ (0, 1)` along the ray plus the
/// obstacle's height above local ground. `path_effects` turns it into a
/// dominant-edge candidate `z = terrain(t) + height_m`.
#[derive(Clone, Copy, Debug)]
pub struct CrossingCandidate {
    pub t: f64,
    pub height_m: f32,
    pub kind: ObstacleKind,
    pub id: u32,
}

/// The directions in which ONE obstacle edge stands, as seen from a query
/// origin — the unit [`super::arc_screening`] builds a receiver's skyline from.
///
/// `lo`/`hi` are absolute azimuths (`atan2(north, east)`, radians) of the SHORT
/// arc between the edge's endpoints, unwrapped so `lo <= hi` and `hi - lo < π`:
/// an edge subtends less than a half-turn from any point off it, so the pair is
/// an ordinary interval on the line, never a wrap-around case. `near_m` is the
/// nearest range from the origin to the edge — the "does this stand in FRONT of
/// the source" test, which replaces a second ray query per candidate.
#[derive(Clone, Copy, Debug)]
pub struct SkylineArc {
    /// Stable flattened edge identity; repeated cell emissions keep this ID.
    pub source_id: SourceId64,
    pub lo: f64,
    pub hi: f64,
    pub near_m: f32,
    /// Edge height above its own local ground (m).
    pub height_m: f32,
}

/// Branch-and-bound context for [`ObstacleIndex::crossings_pruned`]: everything
/// needed to bound, per grid cell, the best path difference any edge in it could
/// produce — so a cell that cannot beat the floor is skipped without touching an
/// edge. In Dobříš 94 % of 50 m rays and 83 % of 400 m rays find nothing at all
/// (A3 survey); this is what harvests that.
///
/// The bound is `terr_win + cell_max_h`, where `terr_win` is the max terrain
/// over THAT CELL'S OWN CHAINAGE WINDOW — not the profile-wide max, which is the
/// whole point: in Brdy relief one hilltop anywhere on the ray poisons a global
/// bound and nothing is ever pruned. Exactness: a candidate's terrain is LERPed
/// between the two samples bracketing its crossing (`path_effects` §5b), so it
/// can never exceed the max of the samples bracketing the window.
pub struct CellPrune<'a> {
    /// Profile chainages, ascending, `0..=1` — `PathProfile::t`.
    pub t: &'a [f64],
    /// Bare-earth elevation at each chainage — `PathProfile::elevation_m`.
    pub elevation_m: &'a [f32],
    /// Absolute source / receiver altitudes (ground + height).
    pub src_e: f64,
    pub rcv_e: f64,
    pub dist_m: f64,
    /// Floor of the LOOP THIS PRUNE ACCELERATES — `path_effects` §5b's
    /// candidate race, not the physics and not some other lane's loop. See
    /// [`ObstacleIndex::crossings_pruned`].
    pub floor_m: f64,
}

/// Reused per-worker state for the ray-edge dedup table.
///
/// The DDA visits an edge's supercover cells, so one edge may appear in more
/// than one cell. The historical implementation cleared a 64-entry table for
/// every ray. A generation tag preserves the exact same direct-mapped
/// collision/eviction semantics without 64 stores per `(source, receiver)`
/// query. The table is deliberately bounded: collisions only cause an extra
/// exact intersection test and the post-sort dedup remains authoritative.
#[derive(Clone)]
pub struct CrossingScratch {
    recent: [u64; 64],
    epoch: u32,
}

impl Default for CrossingScratch {
    fn default() -> Self {
        Self {
            recent: [0; 64],
            epoch: 0,
        }
    }
}

impl CrossingScratch {
    #[inline]
    fn begin_ray(&mut self) -> u32 {
        if self.epoch == u32::MAX {
            // Four billion rays per worker is outside any tile, but keep the
            // reusable scratch correct if a long-lived stream ever reaches it.
            self.recent = [0; 64];
            self.epoch = 1;
        } else {
            self.epoch += 1;
        }
        self.epoch
    }
}

impl<'a> CellPrune<'a> {
    /// Prune context for a ray whose profile is already built, floored at the
    /// consumer's own floor. Callers do NOT choose the floor: it belongs to
    /// `path_effects` §5b, the loop that ranks these candidates, and picking it
    /// at the call site is how a prune ends up above its loop.
    pub fn for_profile(profile: &'a super::PathProfile, src_e: f64, rcv_e: f64) -> Self {
        CellPrune {
            t: &profile.t,
            elevation_m: &profile.elevation_m,
            src_e,
            rcv_e,
            dist_m: profile.dist_m,
            floor_m: cell_prune_floor_m(),
        }
    }
}

/// The prune floor, or `-inf` under `QM_ARC_DISABLE_CELL_PRUNE=1` — the A/B
/// lever that turns the branch-and-bound off without a rebuild. The prune is
/// meant to be OUTPUT-NEUTRAL (it only skips cells that provably cannot reach
/// the consumer's floor), and this is what makes that claim measurable on a real
/// cell instead of asserted. KEPT deliberately (2026-08-08 review): while the
/// 17.6 dB CPU↔GPU gap is undiagnosed this is the only way to price the prune's
/// share of it, and the bug fixed in [`CellPrune::max_delta`] the same day is
/// the reason "output-neutral" cannot be taken on trust.
fn cell_prune_floor_m() -> f64 {
    static V: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        if std::env::var("QM_ARC_DISABLE_CELL_PRUNE").is_ok_and(|v| v == "1") {
            f64::NEG_INFINITY
        } else {
            crate::constants::PENUMBRA_DELTA_FLOOR_M
        }
    })
}

impl CellPrune<'_> {
    /// Upper bound on the SIGNED δ any edge in `[t_lo, t_hi]` with top at most
    /// `top` can produce, in `path_effects` §5b's own form
    /// (`sign·(d_sb + d_br − d_SR)`, negative below the sight line).
    ///
    /// δ is monotone increasing in `top`, so the cell's tallest possible top
    /// bounds every candidate in it. In `t`, `detour` is CONVEX for a fixed
    /// `top`, so:
    ///
    /// * the POSITIVE branch (`top` above the sight line, δ = +detour) takes its
    ///   max over the window at an ENDPOINT;
    /// * the NEGATIVE branch (δ = −detour) is concave, so it takes its max at
    ///   `detour`'s stationary point — the REFLECTION point
    ///   `t* = |h_s| / (|h_s| + |h_r|)`, clamped to the window.
    ///
    /// Evaluating `{t_lo, t_hi, t*}` therefore attains the true max of both
    /// branches, and the bound is exact rather than merely sound.
    ///
    /// This mirrors `scatter.cu`'s `tstar` line for line, and that is the point:
    /// until 2026-08-08 this used the point where the SIGHT LINE crosses `top`
    /// instead. Those two agree exactly whenever the crossing is inside the
    /// window — `h_s` and `h_r` then have opposite signs and
    /// `h_s/(h_s − h_r) ≡ |h_s|/(|h_s| + |h_r|)` — but when `top` runs BELOW
    /// both ends of the window (the penumbra case the floor exists for) the
    /// crossing is outside, the clamp collapses it onto an endpoint, and the
    /// bound comes out far too low: a 50 m ray with source and receiver at 4 m
    /// over a 3 m top bounded at −1.010 m against a −0.2698 m floor, pruning a
    /// cell whose real candidate at `t* = 0.5` scores δ = −0.040 m. Sound-but-
    /// loose would have been survivable; too LOW is a silent loss of screening.
    /// `low_top_prune_keeps_penumbra_candidate` pins that geometry.
    #[inline]
    fn max_delta(&self, top: f64, t_lo: f64, t_hi: f64) -> f64 {
        let dz = self.rcv_e - self.src_e;
        let dsr = (self.dist_m * self.dist_m + dz * dz).sqrt();
        let at = |tt: f64| {
            let los = self.src_e + dz * tt;
            let (d_sg, d_rg) = (tt * self.dist_m, (1.0 - tt) * self.dist_m);
            let detour = (d_sg * d_sg + (top - self.src_e).powi(2)).sqrt()
                + (d_rg * d_rg + (top - self.rcv_e).powi(2)).sqrt()
                - dsr;
            if top >= los {
                detour
            } else {
                -detour
            }
        };
        // The reflection point — where `detour` is stationary, hence the
        // negative branch's peak. Same expression as `scatter.cu`'s `tstar`
        // inside `obstacle_best_candidate`'s below-sight-line branch.
        let (ahs, ahr) = ((top - self.src_e).abs(), (top - self.rcv_e).abs());
        let t_star = if ahs + ahr > 0.0 {
            (ahs / (ahs + ahr)).clamp(t_lo, t_hi)
        } else {
            // Sight line runs exactly through `top` at both ends (flat, grazing):
            // δ ≡ 0 everywhere in the window, so any point attains the max.
            t_lo
        };
        at(t_lo).max(at(t_hi)).max(at(t_star))
    }
}

/// Uniform-grid spatial index over obstacle edges. Build once per tile+halo
/// (or per popup query), then run many rays against it. CSR layout: cell →
/// slice of edge refs.
///
/// The four arrays are [`IndexArray`]s, not `Vec`s: an index built from the
/// Arrow shards owns its heap, one loaded from the cache
/// ([`super::obstacle_index_file`]) reads straight out of a mapped file. Both
/// deref to the same slices, so every query below is written once.
pub struct ObstacleIndex {
    pub(super) origin_lat: f64,
    pub(super) origin_lon: f64,
    pub(super) m_per_deg_lon: f64,
    /// Grid cell size (m). ~2× the raster cell keeps cells-per-ray low while
    /// average edges-per-cell stays small in cities.
    pub(super) cell_m: f64,
    pub(super) min_x: f64,
    pub(super) min_y: f64,
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) cell_starts: IndexArray<u32>,
    pub(super) edge_refs: IndexArray<u32>,
    pub(super) edges: IndexArray<ObstacleEdge>,
    /// Per grid cell: the tallest edge binned into it (0 for empty cells). The
    /// O(1) input to every branch-and-bound prune over this grid — the CUDA
    /// ray walk's exact δ bound (`obstacle_best_candidate`) and the skyline
    /// walk's grazing prune ([`ObstacleIndex::skyline_arcs_within`]).
    pub(super) cell_max_h: IndexArray<f32>,
    /// Per-footprint (id-indexed) min local x over all its rings — the
    /// containment walk skips footprints whose bbox lies strictly east of the
    /// probe. Requires DENSE ids (the loaders' sequential ordinals).
    pub(super) footprint_xmin: IndexArray<f32>,
    /// Overture envelope class, indexed by the same dense footprint ordinal.
    pub(super) footprint_class: IndexArray<u8>,
    /// Max per-footprint bbox width (m) — bounds the containment walk: a
    /// footprint straddling the probe cannot extend further east than this.
    pub(super) max_footprint_w: f64,
}

/// Default grid pitch (m) — coarse enough that a 10 km ray walks ~160 cells.
pub const OBSTACLE_GRID_CELL_M: f64 = 64.0;

/// Flat per-index CSR view for GPU upload — see [`ObstacleIndex::gpu_view`].
pub struct GpuGridView<'a> {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub m_per_deg_lon: f64,
    pub cell_m: f64,
    pub min_x: f64,
    pub min_y: f64,
    pub cols: usize,
    pub rows: usize,
    pub cell_starts: &'a [u32],
    pub edge_refs: &'a [u32],
    /// `(x0, y0, x1, y1, height_m)` per edge, stride 5.
    pub edges_xyxyh: Vec<f32>,
    /// Per grid cell: the tallest edge binned into it (0 = empty). The CUDA
    /// lane's branch-and-bound prune reads it directly — one source of truth
    /// with the CPU walks, never recomputed host-side.
    pub cell_max_h: &'a [f32],
    /// Owning obstacle id per edge, parallel to `edges_xyxyh` (stride 1).
    ///
    /// Identity used to be "a host-only concern" because the kernel kept a
    /// running max-δ and never needed to know WHICH obstacle it had hit. Arc
    /// screening broke that: its angular hulls are per FOOTPRINT, so the CUDA
    /// lane has to group edges by owner. Exposing the ids here replaces a
    /// reconstruction that re-queried the index and re-joined edges on their
    /// endpoint BIT PATTERNS — correct but fragile, and silently degrading to a
    /// private id whenever the join missed.
    pub edge_ids: Vec<u32>,
}

impl ObstacleIndex {
    /// Build from closed rings given as `(lat, lon)` sequences (first point
    /// need not be repeated at the end; the closing edge is added). Open
    /// polylines (noise barriers) go through [`Builder::add_polyline`].
    pub fn builder(origin_lat: f64, origin_lon: f64) -> Builder {
        Builder {
            origin_lat,
            origin_lon,
            m_per_deg_lon: m_per_deg_lon(origin_lat.to_radians()),
            edges: Vec::new(),
            footprint_class: Vec::new(),
        }
    }

    #[inline]
    fn to_local(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            (lon - self.origin_lon) * self.m_per_deg_lon,
            (lat - self.origin_lat) * M_PER_DEG_LAT,
        )
    }

    /// Every edge within `radius_m` of `(lat, lon)` that can still break a line
    /// of sight there, as a [`SkylineArc`] handed to `visit`.
    ///
    /// The AREA sibling of [`Self::crossings`]: a ray query answers "what does
    /// THIS ray hit", this one answers "what stands around this point, and in
    /// which directions" — the receiver skyline every segment of that receiver
    /// then clips its own angular span against, instead of re-running an area
    /// query per (segment, receiver) pair.
    ///
    /// TWO prunes, both O(1) per grid cell and both exact-by-construction:
    ///
    /// * empty cells (`cell_max_h == 0`) cost one CSR compare;
    /// * the ISO 9613-2 §7.3 GRAZING prune — an obstacle rising `h` above the
    ///   sight line at distance `b` from this end of a path much longer than
    ///   `b` bends the path by `δ ≈ h²/(2b)`, and `diffraction::maekawa_bands`
    ///   zeroes every band whose `δ ≤ λ/4 − δ*` (flat ground: `δ ≤ λ/8`). So a
    ///   cell whose TALLEST edge cannot reach `h ≥ sqrt(2·δ_min·b)` cannot
    ///   produce a single dB in any band, whatever its geometry — skip it whole.
    ///   `los_floor_m` is the LOWEST the sight line ever runs above local
    ///   ground on such a path — the SOURCE height, not the receiver's: the
    ///   line drops from receiver height to source height as it goes out, so an
    ///   obstacle only shorter than the receiver can still break it further
    ///   along. Gating on the receiver's height would silently drop every 3 m
    ///   noise wall and low building. `delta_min_m` is the caller's δ floor.
    ///
    /// An edge listed in several cells is visited several times; the caller's
    /// merge is idempotent on repeats (identical arcs union to themselves), so
    /// no dedup pass is needed — the reason this walk emits arcs directly
    /// instead of materialising an edge list.
    #[allow(clippy::too_many_arguments)]
    pub fn skyline_arcs_within(
        &self,
        edge_ordinal_base: u64,
        lat: f64,
        lon: f64,
        min_radius_m: f64,
        radius_m: f64,
        los_floor_m: f64,
        delta_min_m: f64,
        wedge: Option<(f64, f64)>,
        visit: &mut impl FnMut(SkylineArc),
    ) {
        if self.edges.is_empty() {
            return;
        }
        let (ox, oy) = self.to_local(lat, lon);
        let inv_cell = 1.0 / self.cell_m;
        let cell_range = |lo: f64, hi: f64, base: f64, n: usize| -> Option<(usize, usize)> {
            let c0 = ((lo - base) * inv_cell).floor();
            let c1 = ((hi - base) * inv_cell).floor();
            if c1 < 0.0 || c0 > (n - 1) as f64 {
                return None; // query box entirely outside the grid slab
            }
            Some((c0.max(0.0) as usize, c1.min((n - 1) as f64) as usize))
        };
        let Some((cx0, cx1)) = cell_range(ox - radius_m, ox + radius_m, self.min_x, self.cols)
        else {
            return;
        };
        let Some((cy0, cy1)) = cell_range(oy - radius_m, oy + radius_m, self.min_y, self.rows)
        else {
            return;
        };
        let r2 = radius_m * radius_m;
        // Wedge reject. A segment can only be clipped by obstacles inside its
        // OWN angular span, so gathering the whole disk collects area no query
        // can read — ~180× for a rail segment 3 km out with a 2° span. The
        // span is under a half turn, so the wedge is the intersection of two
        // half-planes and a cell is rejected when all four of its corners sit
        // strictly outside one of them. Cross products only: no `atan2` in a
        // loop that runs per cell.
        let wedge_dirs = wedge.map(|(lo, hi)| ((lo.cos(), lo.sin()), (hi.cos(), hi.sin())));

        for cy in cy0..=cy1 {
            let row = cy * self.cols;
            // Nearest point of this cell ROW to the origin, then of the cell —
            // the largest `b` lower bound the grid can give without touching an
            // edge, which is what makes the grazing prune tight.
            let y_lo = self.min_y + cy as f64 * self.cell_m;
            let dy = (y_lo - oy).max(oy - (y_lo + self.cell_m)).max(0.0);
            for cx in cx0..=cx1 {
                let cell = row + cx;
                let lo = self.cell_starts[cell] as usize;
                let hi = self.cell_starts[cell + 1] as usize;
                if lo == hi {
                    continue;
                }
                let x_lo = self.min_x + cx as f64 * self.cell_m;
                let dx = (x_lo - ox).max(ox - (x_lo + self.cell_m)).max(0.0);
                let b2 = dx * dx + dy * dy;
                if b2 > r2 {
                    continue;
                }
                if let Some(((lx, ly), (hx, hy))) = wedge_dirs {
                    let (cx0, cy0c) = (x_lo - ox, y_lo - oy);
                    let (cx1, cy1c) = (cx0 + self.cell_m, cy0c + self.cell_m);
                    let corners = [(cx0, cy0c), (cx1, cy0c), (cx0, cy1c), (cx1, cy1c)];
                    // Outside the LOW edge: the corner is clockwise of it.
                    let all_below = corners.iter().all(|&(px, py)| lx * py - ly * px < 0.0);
                    // Outside the HIGH edge: the corner is anticlockwise of it.
                    let all_above = corners.iter().all(|&(px, py)| px * hy - py * hx < 0.0);
                    if all_below || all_above {
                        continue;
                    }
                }
                // Already covered by an earlier, smaller-radius pass: the cell's
                // FARTHEST corner is inside it, so every edge it holds was
                // visited then. Growing a skyline is an annulus walk, never a
                // re-walk (`ArcSkyline::ensure`).
                if min_radius_m > 0.0 {
                    let fx = (x_lo - ox).abs().max((x_lo + self.cell_m - ox).abs());
                    let fy = (y_lo - oy).abs().max((y_lo + self.cell_m - oy).abs());
                    if fx * fx + fy * fy <= min_radius_m * min_radius_m {
                        continue;
                    }
                }
                let h = self.cell_max_h[cell] as f64 - los_floor_m;
                if h <= 0.0 || h * h < 2.0 * delta_min_m * b2.sqrt() {
                    continue; // grazing: zero dB in every band, whatever the edge
                }
                for &eref in &self.edge_refs[lo..hi] {
                    let e = self.edges[eref as usize];
                    let (ex0, ey0) = (e.x0 as f64 - ox, e.y0 as f64 - oy);
                    let (ex1, ey1) = (e.x1 as f64 - ox, e.y1 as f64 - oy);
                    let near_m = origin_to_segment_dist(ex0, ey0, ex1, ey1);
                    if near_m > radius_m || near_m < 1e-6 {
                        continue; // out of range, or the origin sits ON the edge
                    }
                    let a0 = ey0.atan2(ex0);
                    let a1 = ey1.atan2(ex1);
                    // The SHORT arc between the endpoints: the set of directions
                    // that hit this edge. Taking it per EDGE (not a per-footprint
                    // hull) is exact for concave outlines too — a ray leaving the
                    // origin hits a closed ring iff it hits one of its edges.
                    let r1 = a0 + wrap_pi(a1 - a0);
                    visit(SkylineArc {
                        source_id: SourceId64::obstacle(
                            edge_ordinal_base
                                .checked_add(u64::from(eref))
                                .expect("flattened obstacle edge ordinal overflow"),
                        )
                        .expect("flattened obstacle edge ordinal entered wall namespace"),
                        lo: a0.min(r1),
                        hi: a0.max(r1),
                        near_m: near_m as f32,
                        height_m: e.height_m,
                    });
                }
            }
        }
    }

    /// Can the segment `src→rcv` touch this index's grid at all?
    ///
    /// A query set holds the 7 per-cell indexes of a `grid_disk(1)` ring, and
    /// the CUDA kernel's own comment notes a ray touches only 1-3 of them —
    /// but the CPU lane walked all 7, clamping each DDA into a grid the ray
    /// never enters and testing the edges it happens to land on. This is an
    /// exact REJECT, not an approximation: a segment that misses a grid's
    /// bounding box cannot cross an edge binned inside it.
    ///
    /// Slab test in the index's own local frame (each has its own origin, so
    /// the two `to_local` calls are the price of admission — four multiplies
    /// against a whole DDA walk).
    #[inline]
    pub fn segment_may_hit(&self, src_lat: f64, src_lon: f64, rcv_lat: f64, rcv_lon: f64) -> bool {
        if self.edges.is_empty() {
            return false;
        }
        let (sx, sy) = self.to_local(src_lat, src_lon);
        let (rx, ry) = self.to_local(rcv_lat, rcv_lon);
        let max_x = self.min_x + self.cols as f64 * self.cell_m;
        let max_y = self.min_y + self.rows as f64 * self.cell_m;
        // Cheap AABB-vs-AABB reject first — it catches the common case (a ray
        // wholly on the far side of a neighbouring cell) without any division.
        if sx.max(rx) < self.min_x
            || sx.min(rx) > max_x
            || sy.max(ry) < self.min_y
            || sy.min(ry) > max_y
        {
            return false;
        }
        // Slab test for the diagonal cases the bbox overlap cannot decide.
        let (dx, dy) = (rx - sx, ry - sy);
        let mut lo = 0.0f64;
        let mut hi = 1.0f64;
        for (s0, d, b0, b1) in [(sx, dx, self.min_x, max_x), (sy, dy, self.min_y, max_y)] {
            if d.abs() < 1e-12 {
                if s0 < b0 || s0 > b1 {
                    return false;
                }
                continue;
            }
            let (mut a, mut b) = ((b0 - s0) / d, (b1 - s0) / d);
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            lo = lo.max(a);
            hi = hi.min(b);
            if lo > hi {
                return false;
            }
        }
        true
    }

    /// Upper bound, in absolute metres ASL, on the top of anything in `cell` —
    /// the single seam every branch-and-bound over this grid reads.
    ///
    /// Today: the ray's own windowed terrain max plus the cell's tallest edge.
    /// A3's `bake_top_asl` replaces this with a per-cell bound baked from the
    /// DEM at load time (per-edge ground × per-edge height, dilated by the
    /// coarsest cadence gap), which is strictly tighter — the tallest building
    /// in a cell need not stand on its highest ground — and needs no windowed
    /// max at all. When that lands, this body becomes
    /// `self.cell_max_top_asl[cell]` with the windowed value as the fallback
    /// for an index whose bake was inert (no DEM), and every caller is unchanged.
    #[inline]
    fn cell_top_bound(&self, cell: usize, terr_win_m: f64) -> f64 {
        terr_win_m + self.cell_max_h[cell] as f64
    }

    /// Number of indexed edges (telemetry / memory accounting).
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Flat CSR view for the CUDA lane (geodata-v2 1.6): grid frame constants
    /// plus borrowed CSR arrays, a materialised `(x0,y0,x1,y1,height)` edge
    /// array and the per-edge owner id. `kind` stays host-only; `id` does NOT
    /// — arc screening groups edges by footprint, so the kernel reads
    /// [`GpuGridView::edge_ids`] (`obst` slot 6) and the "never an identity"
    /// rule this doc used to state has not held since TRACK C (2026-08-03).
    /// The kernel walk must mirror [`Self::crossings`] cell-for-cell; e2-full
    /// is the parity gate.
    pub fn gpu_view(&self) -> GpuGridView<'_> {
        let mut edges_xyxyh = Vec::with_capacity(self.edges.len() * 5);
        let mut edge_ids = Vec::with_capacity(self.edges.len());
        for e in self.edges.iter() {
            edges_xyxyh.extend_from_slice(&[e.x0, e.y0, e.x1, e.y1, e.height_m]);
            edge_ids.push(e.id);
        }
        GpuGridView {
            origin_lat: self.origin_lat,
            origin_lon: self.origin_lon,
            m_per_deg_lon: self.m_per_deg_lon,
            cell_m: self.cell_m,
            min_x: self.min_x,
            min_y: self.min_y,
            cols: self.cols,
            rows: self.rows,
            cell_starts: &self.cell_starts,
            edge_refs: &self.edge_refs,
            edges_xyxyh,
            cell_max_h: &self.cell_max_h,
            edge_ids,
        }
    }

    /// Exact crossings of the ray `src→rcv`, endpoint-exclusive
    /// (`t ∈ (0, 1)`), appended to `out` (cleared first), sorted by `t` and
    /// deduped to one candidate per (obstacle, chainage). Endpoint
    /// exclusivity drops hits AT the endpoints only; a footprint CONTAINING
    /// an endpoint still reports its entry/exit edge — filtering the
    /// source's own building is the caller's job (`exclusion_radius_m`
    /// semantics live in `path_effects`, not here).
    pub fn crossings(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        out: &mut Vec<CrossingCandidate>,
    ) {
        out.clear();
        if self.edges.is_empty() {
            return;
        }
        self.append_crossings(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            None,
            &mut CrossingScratch::default(),
            out,
        );
    }

    /// [`Self::crossings`] with the per-cell branch-and-bound prune.
    ///
    /// `prune.floor_m` MUST be the floor of the loop this accelerates — the
    /// consumer that ranks these candidates. For `path_effects` §5b that floor
    /// is [`crate::constants::PENUMBRA_DELTA_FLOOR_M`], NOT zero: that loop
    /// deliberately keeps below-sight-line near misses with a negative δ
    /// (fix-pack Fix 2), and a prune floored at 0 would delete exactly the
    /// geometry a noise wall exists to create. A prune whose floor sits above
    /// its loop's floor is unsound however tight its bound is.
    ///
    /// The floor is δ\*-FREE on purpose. The rejection threshold is DECREASING
    /// in δ\*, so assuming a δ\* larger than the true one rejects paths that
    /// still carry energy; only a proven LOWER bound on δ\* is admissible, and
    /// at prune time δ\* is not yet computed. The infimum over all δ\* is
    /// −λ/20 at the longest wavelength in the model, which is exactly this
    /// constant.
    pub fn crossings_pruned(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        prune: &CellPrune<'_>,
        out: &mut Vec<CrossingCandidate>,
    ) {
        out.clear();
        if self.edges.is_empty() {
            return;
        }
        self.append_crossings(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            Some(prune),
            &mut CrossingScratch::default(),
            out,
        );
    }

    /// [`Self::crossings`] without the clear: appends this index's hits and
    /// sort+dedups ONLY the appended tail, so [`ObstacleSet`] can chain
    /// per-cell indexes into one buffer with zero per-ray allocation (the
    /// hot scatter loop runs this per receiver ray).
    /// Test-only view of the unpruned per-index walk (the slab bench's OFF lane).
    pub fn append_crossings_pub(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        out: &mut Vec<CrossingCandidate>,
    ) {
        if self.edges.is_empty() {
            return;
        }
        self.append_crossings(
            src_lat,
            src_lon,
            rcv_lat,
            rcv_lon,
            None,
            &mut CrossingScratch::default(),
            out,
        );
    }

    fn append_crossings(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        prune: Option<&CellPrune<'_>>,
        scratch: &mut CrossingScratch,
        out: &mut Vec<CrossingCandidate>,
    ) {
        let start = out.len();
        if self.edges.is_empty() {
            return;
        }
        let generation = scratch.begin_ray();
        let (sx, sy) = self.to_local(src_lat, src_lon);
        let (rx, ry) = self.to_local(rcv_lat, rcv_lon);
        let (dx, dy) = (rx - sx, ry - sy);

        // DDA over grid cells (Amanatides & Woo), clamped to the grid slab.
        let inv_cell = 1.0 / self.cell_m;
        let mut cx = (((sx - self.min_x) * inv_cell).floor() as i64).clamp(0, self.cols as i64 - 1);
        let mut cy = (((sy - self.min_y) * inv_cell).floor() as i64).clamp(0, self.rows as i64 - 1);
        let end_cx = (((rx - self.min_x) * inv_cell).floor() as i64).clamp(0, self.cols as i64 - 1);
        let end_cy = (((ry - self.min_y) * inv_cell).floor() as i64).clamp(0, self.rows as i64 - 1);

        let step_x: i64 = if dx >= 0.0 { 1 } else { -1 };
        let step_y: i64 = if dy >= 0.0 { 1 } else { -1 };
        // Δt of one cell step per axis (Amanatides & Woo tDelta).
        let t_delta_x = if dx != 0.0 {
            (self.cell_m / dx).abs()
        } else {
            f64::INFINITY
        };
        let t_delta_y = if dy != 0.0 {
            (self.cell_m / dy).abs()
        } else {
            f64::INFINITY
        };
        let next_x_boundary = self.min_x + (cx + i64::from(dx >= 0.0)) as f64 * self.cell_m;
        let next_y_boundary = self.min_y + (cy + i64::from(dy >= 0.0)) as f64 * self.cell_m;
        let mut t_max_x = if dx != 0.0 {
            ((next_x_boundary - sx) / dx).abs()
        } else {
            f64::INFINITY
        };
        let mut t_max_y = if dy != 0.0 {
            ((next_y_boundary - sy) / dy).abs()
        } else {
            f64::INFINITY
        };

        // An edge spans every supercover cell it passes through, so the ray
        // can re-test it in each of them. The generation-tagged direct-mapped
        // 64-slot table records EVERY edge that reaches the exact predicate,
        // not only hits: ray and edge are immutable within this walk, so
        // repeating the predicate cannot change its answer. An AABB rejection
        // is deliberately not remembered: the same edge can span a later DDA
        // cell that contains the true crossing. A hash collision merely evicts
        // the older entry and performs an extra test; it can never suppress a
        // distinct edge. CORRECTNESS still belongs to the post-sort dedup below
        // (a shared ring vertex can hit two edges of one footprint at one
        // chainage).

        let mut guard = (self.cols + self.rows) as i64 + 4;
        // Chainage the ray entered the current cell at, and a monotone pointer
        // into the profile samples — both only ever advance, so the windowed
        // terrain max costs O(samples) over the whole walk.
        let mut t_enter = 0.0_f64;
        let mut win_lo = 0usize;
        loop {
            let cell = cy as usize * self.cols + cx as usize;
            let mut lo = self.cell_starts[cell] as usize;
            let hi = self.cell_starts[cell + 1] as usize;
            if hi > lo {
                // The DDA visit covers this closed ray interval. Both ends
                // are retained because the edge supercover and this DDA walk
                // meet at cell boundaries. Their accumulated and cross-product
                // chainages can differ by a few ulps, so the AABB below is
                // padded before it filters the authoritative exact predicate.
                let t_exit = t_max_x.min(t_max_y).min(1.0);
                let (cell_t_lo, cell_t_hi) = (t_enter.clamp(0.0, 1.0), t_exit.clamp(0.0, 1.0));
                if let Some(p) = prune {
                    while win_lo + 1 < p.t.len() && p.t[win_lo + 1] <= cell_t_lo {
                        win_lo += 1;
                    }
                    let mut terr_win = p.elevation_m[win_lo] as f64;
                    let mut k = win_lo;
                    while k + 1 < p.t.len() && p.t[k] < cell_t_hi {
                        k += 1;
                        terr_win = terr_win.max(p.elevation_m[k] as f64);
                    }
                    let top_bound = self.cell_top_bound(cell, terr_win);
                    if p.max_delta(top_bound, cell_t_lo, cell_t_hi) < p.floor_m {
                        lo = hi; // no edge here can reach the consumer's floor
                    }
                }
                if lo < hi {
                    let (ray_x, ray_y) = ray_cell_aabb(sx, sy, dx, dy, cell_t_lo, cell_t_hi);
                    for &eref in &self.edge_refs[lo..hi] {
                        let slot = eref as usize & (scratch.recent.len() - 1);
                        let tag = (u64::from(generation) << 32) | u64::from(eref);
                        if scratch.recent[slot] == tag {
                            continue;
                        }
                        let e = &self.edges[eref as usize];
                        if !ray_cell_aabb_may_overlap(ray_x, ray_y, e) {
                            continue;
                        }
                        // Remember only after the AABB gate, exactly as the
                        // historical local table did: an edge rejected by one
                        // supercover cell must remain eligible in a later cell.
                        scratch.recent[slot] = tag;
                        if let Some(t) = segment_intersection_t(
                            sx,
                            sy,
                            dx,
                            dy,
                            e.x0 as f64,
                            e.y0 as f64,
                            e.x1 as f64,
                            e.y1 as f64,
                        ) {
                            out.push(CrossingCandidate {
                                t,
                                height_m: e.height_m,
                                kind: e.kind(),
                                id: e.id,
                            });
                        }
                    }
                }
            }
            if (cx == end_cx && cy == end_cy) || guard <= 0 {
                break;
            }
            guard -= 1;
            t_enter = t_max_x.min(t_max_y);
            if t_max_x < t_max_y {
                t_max_x += t_delta_x;
                cx += step_x;
            } else {
                t_max_y += t_delta_y;
                cy += step_y;
            }
            if cx < 0 || cy < 0 || cx >= self.cols as i64 || cy >= self.rows as i64 {
                break;
            }
        }
        out[start..].sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
        // One candidate per (obstacle, chainage): kills ring-eviction repeats
        // (same edge ⇒ bit-identical t) and vertex double-counts (two edges of
        // one ring meeting at the hit point; tolerance covers their last-ulp
        // difference). A tangent ray thus yields ONE conservative candidate.
        // In-place tail dedup (slices have no `dedup_by`), keep-first.
        let mut w = start;
        for r in start..out.len() {
            if w > start && out[r].id == out[w - 1].id && (out[r].t - out[w - 1].t).abs() < 1e-9 {
                continue;
            }
            out[w] = out[r];
            w += 1;
        }
        out.truncate(w);
    }
}

/// A query-scoped set of per-cell [`ObstacleIndex`]es (Arc-shared from a
/// process cache). The ingest's half-open centroid ownership guarantees a
/// footprint lives in exactly ONE cell index, so per-index results concat
/// without cross-index dedupe; one final sort restores chainage order.
pub struct ObstacleSet {
    pub indexes: Vec<std::sync::Arc<ObstacleIndex>>,
}

impl ObstacleSet {
    /// Total indexed edges across the set (telemetry / emptiness check).
    pub fn edge_count(&self) -> usize {
        self.indexes.iter().map(|i| i.edge_count()).sum()
    }

    /// Tallest BUILDING footprint crossed by the straight path
    /// `src → rcv`, as `(height_m, t_of_max)` — the vector twin of the raster
    /// `RasterSampler::max_building_along_path` group-histogram probe. Used
    /// ONLY for the popup's "N of M segments had obstacles" transparency and
    /// its trace; no dB anywhere reads it. Exact edge crossings replace the
    /// raster's 30–184 m cadence walk, so a footprint between cadence samples
    /// — invisible to the raster probe — is counted here. Walls
    /// (`ObstacleKind::Barrier`) are excluded to keep parity with what the
    /// raster building channel answered.
    pub fn max_height_crossed(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        scratch: &mut Vec<CrossingCandidate>,
    ) -> (f64, f64) {
        self.crossings(src_lat, src_lon, rcv_lat, rcv_lon, scratch);
        let mut best = (0.0_f64, 0.0_f64);
        for c in scratch.iter() {
            if c.kind == ObstacleKind::Building && c.height_m as f64 > best.0 {
                best = (c.height_m as f64, c.t);
            }
        }
        best
    }

    /// Exact crossings of the ray across every cell index, t-sorted.
    pub fn crossings(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        out: &mut Vec<CrossingCandidate>,
    ) {
        out.clear();
        let mut scratch = None;
        for idx in &self.indexes {
            if idx.edge_count() == 0 || !idx.segment_may_hit(src_lat, src_lon, rcv_lat, rcv_lon) {
                continue;
            }
            idx.append_crossings(
                src_lat,
                src_lon,
                rcv_lat,
                rcv_lon,
                None,
                scratch.get_or_insert_with(CrossingScratch::default),
                out,
            );
        }
        out.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    }

    /// [`Self::crossings`] with the per-cell branch-and-bound prune — see
    /// [`ObstacleIndex::crossings_pruned`] for what `floor_m` must be.
    pub fn crossings_pruned(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        prune: &CellPrune<'_>,
        out: &mut Vec<CrossingCandidate>,
    ) {
        out.clear();
        let mut scratch = None;
        for idx in &self.indexes {
            if idx.edge_count() == 0 || !idx.segment_may_hit(src_lat, src_lon, rcv_lat, rcv_lon) {
                continue;
            }
            idx.append_crossings(
                src_lat,
                src_lon,
                rcv_lat,
                rcv_lon,
                Some(prune),
                scratch.get_or_insert_with(CrossingScratch::default),
                out,
            );
        }
        out.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    }

    /// [`Self::crossings_pruned`] with a reused per-worker edge-dedup table.
    /// This is byte/ordering-identical to the ordinary method; it only avoids
    /// clearing a 64-entry direct-mapped table for every ray.
    pub fn crossings_pruned_with_scratch(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        prune: &CellPrune<'_>,
        scratch: &mut CrossingScratch,
        out: &mut Vec<CrossingCandidate>,
    ) {
        out.clear();
        for idx in &self.indexes {
            if !idx.segment_may_hit(src_lat, src_lon, rcv_lat, rcv_lon) {
                continue;
            }
            idx.append_crossings(
                src_lat,
                src_lon,
                rcv_lat,
                rcv_lon,
                Some(prune),
                scratch,
                out,
            );
        }
        out.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    }

    /// [`ObstacleIndex::skyline_arcs_within`] over every member index. Arcs are
    /// origin-relative angles, so per-index results simply concatenate — the
    /// consumer's merge is what turns them into one skyline.
    #[allow(clippy::too_many_arguments)]
    pub fn skyline_arcs_within(
        &self,
        lat: f64,
        lon: f64,
        min_radius_m: f64,
        radius_m: f64,
        los_floor_m: f64,
        delta_min_m: f64,
        wedge: Option<(f64, f64)>,
        visit: &mut impl FnMut(SkylineArc),
    ) {
        let mut edge_ordinal_base = 0_u64;
        for idx in &self.indexes {
            idx.skyline_arcs_within(
                edge_ordinal_base,
                lat,
                lon,
                min_radius_m,
                radius_m,
                los_floor_m,
                delta_min_m,
                wedge,
                visit,
            );
            edge_ordinal_base = edge_ordinal_base
                .checked_add(idx.edges.len() as u64)
                .expect("flattened obstacle edge count overflow");
        }
    }
}

impl ObstacleIndex {
    /// Point-in-footprint test via PER-FOOTPRINT crossing parity along the
    /// probe's eastward half-line: a point is inside footprint `id` iff that
    /// footprint's boundary crosses the half-line an odd number of times
    /// (holes share the outer ring's id, so courtyards read outside; a
    /// global parity bit would break on overlapping footprints). Only
    /// footprints with `height_m > min_height_m` count.
    ///
    /// Exactness (gg review 2026-07-28, both reviewers):
    /// - Vertices use the classic half-open straddle rule
    ///   `(y0 > y) != (y1 > y)` — the same convention as `wkb.rs`
    ///   point-in-polygon: transit vertices count once, tangent vertices
    ///   twice or zero, horizontal edges never. No epsilon, no dedup.
    /// - An edge is listed in every row cell it passes; only the cell
    ///   CONTAINING the crossing point counts it (owner-cell rule), so
    ///   multi-cell edges cannot double-count.
    /// - The walk is bounded by `max_footprint_w`, the max footprint bbox
    ///   width IN THIS INDEX: any footprint whose bbox straddles the probe
    ///   ends within that distance east, and footprints starting east of the
    ///   probe (`footprint_xmin > x`) cannot contain it and are skipped —
    ///   both false-positive (ray "ending inside" a far footprint) and
    ///   false-negative (footprint wider than a fixed cast) failure modes of
    ///   a constant-length ray are structurally impossible.
    pub fn contains_built(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> bool {
        self.collect_containing_footprints(lat, lon, min_height_m, seen);
        seen.iter().any(|(_, crossings, _)| crossings % 2 == 1)
    }

    /// Winning enclosed footprint at a point: tallest wins, then lower ordinal.
    /// Hole parity is evaluated per footprint, exactly as `contains_built`.
    pub fn containing_enclosed(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> Option<(EnvelopeClass, f32, u32)> {
        self.collect_containing_footprints(lat, lon, min_height_m, seen);
        seen.iter()
            .filter(|(_, crossings, _)| crossings % 2 == 1)
            .filter_map(|(id, _, height)| {
                let class = EnvelopeClass::from_u8(self.footprint_class[*id as usize]);
                class.delta_db().map(|_| (class, *height, *id))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.cmp(&a.2)))
    }

    /// Winning footprint at a point, including Outdoor-class structures.
    /// Indoor estimates use [`Self::containing_enclosed`] instead because
    /// Outdoor has no attenuation value, while the building hover still needs
    /// to name visible carports and roof structures.
    pub fn containing_footprint(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) -> Option<(EnvelopeClass, f32, u32)> {
        self.collect_containing_footprints(lat, lon, min_height_m, seen);
        seen.iter()
            .map(|(id, _, height)| {
                let class = EnvelopeClass::from_u8(self.footprint_class[*id as usize]);
                (class, *height, *id)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.2.cmp(&a.2)))
    }

    /// Walk the eastward containment ray once and retain each footprint's
    /// crossing parity and height at first sighting. Both boolean containment
    /// and envelope winner selection use this same CSR walk; the height is
    /// carried in `seen`, so winner lookup is O(1) instead of rescanning all
    /// edges for every indoor pixel.
    fn collect_containing_footprints(
        &self,
        lat: f64,
        lon: f64,
        min_height_m: f32,
        seen: &mut Vec<(u32, u32, f32)>,
    ) {
        seen.clear();
        if self.edges.is_empty() {
            return;
        }
        let (x, y) = self.to_local(lat, lon);
        // Bbox reject: a point outside this index's edge extent cannot be
        // inside any footprint it owns — kills the ~7x wasted walks when a
        // probe queries every ring cell's index.
        let max_x = self.min_x + self.cols as f64 * self.cell_m;
        let max_y = self.min_y + self.rows as f64 * self.cell_m;
        if x < self.min_x || x > max_x || y < self.min_y || y > max_y {
            return;
        }
        let inv_cell = 1.0 / self.cell_m;
        let cy = (((y - self.min_y) * inv_cell).floor() as i64).clamp(0, self.rows as i64 - 1);
        let mut cx = (((x - self.min_x) * inv_cell).floor() as i64).clamp(0, self.cols as i64 - 1);
        let end_cx = (((x + self.max_footprint_w - self.min_x) * inv_cell).floor() as i64)
            .clamp(0, self.cols as i64 - 1);
        // Horizontal half-line ⇒ the walk stays on one row.
        let row = cy as usize * self.cols;
        while cx <= end_cx {
            let cell_lo = self.min_x + cx as f64 * self.cell_m;
            let cell_hi = cell_lo + self.cell_m;
            let cell = row + cx as usize;
            let lo = self.cell_starts[cell] as usize;
            let hi = self.cell_starts[cell + 1] as usize;
            for &eref in &self.edge_refs[lo..hi] {
                let e = self.edges[eref as usize];
                if e.height_m <= min_height_m {
                    continue;
                }
                let (y0, y1) = (e.y0 as f64, e.y1 as f64);
                if (y0 > y) == (y1 > y) {
                    continue; // no straddle (also skips horizontal edges)
                }
                if (self.footprint_xmin[e.id as usize] as f64) > x {
                    continue; // footprint entirely east of the probe
                }
                let (x0, x1) = (e.x0 as f64, e.x1 as f64);
                let xc = x0 + (y - y0) * (x1 - x0) / (y1 - y0);
                if xc > x && xc >= cell_lo && xc < cell_hi {
                    match seen.iter_mut().find(|(id, _, _)| *id == e.id) {
                        Some((_, crossings, _)) => *crossings += 1,
                        None => seen.push((e.id, 1, e.height_m)),
                    }
                }
            }
            cx += 1;
        }
    }
}

/// Receiver-local enclosure over a set of per-cell indexes — the vector twin
/// of the raster 3×3 probe (`RealRasters::building_enclosure`): fraction of 9
/// probe points at ±`ENCLOSURE`-metre offsets that sit inside a footprint
/// taller than 5 m → 0 / 1.5 / 3 dB. Same thresholds, same footprint metric
/// (a parcel split into several footprints cannot inflate it).
pub fn enclosure_db(set: &ObstacleSet, lat: f64, lon: f64, radius_m: f64) -> f64 {
    let step_lat = radius_m / M_PER_DEG_LAT;
    let step_lon = radius_m / m_per_deg_lon(lat.to_radians());
    let mut built = 0u32;
    let mut scratch: Vec<(u32, u32, f32)> = Vec::new();
    for dr in [-1.0, 0.0, 1.0] {
        for dc in [-1.0_f64, 0.0, 1.0] {
            let plat = lat + dr * step_lat;
            let plon = ((lon + dc * step_lon + 180.0).rem_euclid(360.0)) - 180.0;
            if set
                .indexes
                .iter()
                .any(|i| i.contains_built(plat, plon, 5.0, &mut scratch))
            {
                built += 1;
            }
        }
    }
    let density = built as f64 / 9.0;
    if density > 0.5 {
        3.0
    } else if density > 0.2 {
        1.5
    } else {
        0.0
    }
}

/// Read `QM_VECTOR_BUILDINGS` once per process. Loaders (tile-painter,
/// source-reader) call this at init and thread the bool — kernels never read
/// the environment. ON by default since the Wave-1 cutover (2026-07-31:
/// world obstacle store complete — 13 694 tiles / 67 272 cells; A/B record
/// in geodata-v2-plan.md §1.9, m25_j17 +12.9 dB overshoot → −0.26 dB vs
/// Defra); `QM_VECTOR_BUILDINGS=0` restores the raster path (A/B,
/// bisection). Regions without staged obstacle cells keep the raster path
/// via the loaders' all-or-raster policy, unchanged.
pub fn vector_buildings_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| !std::env::var("QM_VECTOR_BUILDINGS").is_ok_and(|v| v == "0"))
}

/// [`RasterSampler`] wrapper that swaps ONLY the receiver reflection probe
/// for the vector enclosure (plan 1.4b — the popup twin of the pipeline's
/// `rx_refl` pre-bake): `building_enclosure` answers from exact footprints
/// via [`enclosure_db`], every other lookup delegates to the raster sampler
/// unchanged. Wrapping at the sampler keeps ALL popup kernels (roads, rail,
/// points, airport ground) on one reflection source with zero signature
/// churn — SPEC §3.8 semantics on both paths.
pub struct VectorReflectionSampler<'a> {
    pub inner: &'a dyn crate::types::RasterSampler,
    pub set: &'a ObstacleSet,
}

impl crate::types::RasterSampler for VectorReflectionSampler<'_> {
    fn elevation(&self, lat: f64, lon: f64) -> f64 {
        self.inner.elevation(lat, lon)
    }
    fn building_height(&self, lat: f64, lon: f64) -> f64 {
        self.inner.building_height(lat, lon)
    }
    fn ground_g(&self, lat: f64, lon: f64) -> f64 {
        self.inner.ground_g(lat, lon)
    }
    fn building_enclosure(&self, lat: f64, lon: f64) -> f64 {
        enclosure_db(self.set, lat, lon, crate::constants::ENCLOSURE_RADIUS_M)
    }
    fn build_path_profile(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        out: &mut crate::propagation::PathProfile,
    ) {
        self.inner
            .build_path_profile(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, out)
    }
    fn max_building_along_path(
        &self,
        src_lat: f64,
        src_lon: f64,
        rcv_lat: f64,
        rcv_lon: f64,
        dist_m: f64,
        excl_start_m: f64,
    ) -> (f64, f64) {
        self.inner
            .max_building_along_path(src_lat, src_lon, rcv_lat, rcv_lon, dist_m, excl_start_m)
    }
}

/// Distance from the ORIGIN to the segment `(x0,y0)-(x1,y1)` (both already
/// origin-relative). The `near_m` of a [`SkylineArc`]: how far away the thing
/// standing in those directions actually is. Shared with
/// [`super::arc_screening`], which runs it on noise-wall endpoints — a wall IS
/// a segment, so its skyline arc is the same primitive as a building edge's.
#[inline]
pub(crate) fn origin_to_segment_dist(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let (ex, ey) = (x1 - x0, y1 - y0);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 0.0 {
        (-(x0 * ex + y0 * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = (x0 + t * ex, y0 + t * ey);
    (px * px + py * py).sqrt()
}

/// Closed, padded AABB of one DDA cell visit.
#[inline]
fn ray_cell_aabb(
    sx: f64,
    sy: f64,
    dx: f64,
    dy: f64,
    t_lo: f64,
    t_hi: f64,
) -> ((f64, f64), (f64, f64)) {
    let ray_x0 = sx + dx * t_lo;
    let ray_x1 = sx + dx * t_hi;
    let ray_y0 = sy + dy * t_lo;
    let ray_y1 = sy + dy * t_hi;
    // DDA boundaries accumulate t_delta, whereas the exact intersection uses
    // independent cross products. This pad only covers their f64 last-ulp
    // disagreement; it cannot change the exact predicate's answer.
    let pad = 1e-9 * (1.0 + dx.abs() + dy.abs());
    (
        (ray_x0.min(ray_x1) - pad, ray_x0.max(ray_x1) + pad),
        (ray_y0.min(ray_y1) - pad, ray_y0.max(ray_y1) + pad),
    )
}

/// Conservative broad phase for one edge in one DDA cell visit.
///
/// The edge is binned by supercover and the ray DDA visits every crossed cell;
/// a closed, padded box therefore only rejects an edge whose crossing cannot
/// be in this cell. Boundary touches always reach the exact predicate below.
#[inline]
fn ray_cell_aabb_may_overlap(ray_x: (f64, f64), ray_y: (f64, f64), edge: &ObstacleEdge) -> bool {
    let (edge_x_lo, edge_x_hi) = (edge.x0.min(edge.x1) as f64, edge.x0.max(edge.x1) as f64);
    let (edge_y_lo, edge_y_hi) = (edge.y0.min(edge.y1) as f64, edge.y0.max(edge.y1) as f64);

    !(ray_x.1 < edge_x_lo || edge_x_hi < ray_x.0 || ray_y.1 < edge_y_lo || edge_y_hi < ray_y.0)
}

/// Angle folded into `(−π, π]` — shared by the skyline walk and
/// [`super::arc_screening`], which must agree on the unwrapping convention.
#[inline]
pub fn wrap_pi(a: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let a = a % TAU;
    if a > PI {
        a - TAU
    } else if a <= -PI {
        a + TAU
    } else {
        a
    }
}

/// Chainage of the intersection of ray `(sx,sy)+t·(dx,dy)` with segment
/// `(x0,y0)–(x1,y1)`, if any, with `t` strictly inside `(0, 1)` and the hit
/// strictly inside the segment (`u ∈ [0, 1]`). Standard 2D cross-product
/// parametric form; collinear overlap returns `None` (a ray sliding along a
/// wall face grazes it, it does not cross it).
///
/// Shared with `path_effects`' noise-barrier crossings, which never enter this
/// index (they arrive per-tile as `types::Barrier` segments) but must solve the
/// identical geometry — one primitive, one rounding, one set of edge cases.
#[inline]
pub(crate) fn segment_intersection_t(
    sx: f64,
    sy: f64,
    dx: f64,
    dy: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> Option<f64> {
    let ex = x1 - x0;
    let ey = y1 - y0;
    let denom = dx * ey - dy * ex;
    if denom == 0.0 {
        return None;
    }
    let wx = x0 - sx;
    let wy = y0 - sy;
    let t = (wx * ey - wy * ex) / denom;
    let u = (wx * dy - wy * dx) / denom;
    if t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&u) {
        Some(t)
    } else {
        None
    }
}

/// Accumulates edges, then freezes them into the CSR grid.
pub struct Builder {
    origin_lat: f64,
    origin_lon: f64,
    m_per_deg_lon: f64,
    edges: Vec<ObstacleEdge>,
    pub(super) footprint_class: Vec<u8>,
}

impl Builder {
    #[inline]
    fn to_local(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            (lon - self.origin_lon) * self.m_per_deg_lon,
            (lat - self.origin_lat) * M_PER_DEG_LAT,
        )
    }

    /// Add a closed ring (footprint outer ring or hole — holes screen too:
    /// a courtyard wall is a wall). The closing edge back to the first
    /// point is added automatically.
    pub fn add_ring(&mut self, ring: &[(f64, f64)], height_m: f32, kind: ObstacleKind, id: u32) {
        // `is_finite` + `<= 0` together reject NaN heights; non-finite
        // coordinates would otherwise bin into cell (0,0) and panic the
        // t-sort downstream.
        if ring.len() < 3
            || !height_m.is_finite()
            || height_m <= 0.0
            || ring.iter().any(|(a, o)| !a.is_finite() || !o.is_finite())
        {
            return;
        }
        // This is the single formation site for building obstacle edges: both
        // WKB loaders route every outer ring and hole through it. Noise barriers
        // are a separate physical domain and retain their mapped height.
        let height_m = if kind == ObstacleKind::Building {
            height_m.min(BUILDING_HEIGHT_MAX_M as f32)
        } else {
            height_m
        };
        for i in 0..ring.len() {
            let (lat0, lon0) = ring[i];
            let (lat1, lon1) = ring[(i + 1) % ring.len()];
            if lat0 == lat1 && lon0 == lon1 {
                continue; // explicit closing repeat in the source data
            }
            let (x0, y0) = self.to_local(lat0, lon0);
            let (x1, y1) = self.to_local(lat1, lon1);
            self.edges.push(ObstacleEdge {
                x0: x0 as f32,
                y0: y0 as f32,
                x1: x1 as f32,
                y1: y1 as f32,
                height_m,
                id,
                kind: kind.code(),
            });
        }
    }

    /// Add every ring of a raw-WKB Polygon/MultiPolygon footprint (outer
    /// rings AND holes — a courtyard wall is a wall). Invalid or non-areal
    /// WKB adds nothing. This is the obstacle-store ingestion entry: the
    /// per-cell arrows carry Overture WKB bytes unencoded.
    pub fn add_polygon_wkb(
        &mut self,
        wkb: &[u8],
        height_m: f32,
        kind: ObstacleKind,
        id: u32,
        class: EnvelopeClass,
    ) {
        let slot = id as usize;
        if self.footprint_class.len() <= slot {
            self.footprint_class
                .resize(slot + 1, EnvelopeClass::Default as u8);
        }
        self.footprint_class[slot] = class as u8;
        for (outer, holes) in crate::wkb::parse_wkb_polygons_bytes(wkb) {
            self.add_ring(&outer, height_m, kind, id);
            for hole in &holes {
                self.add_ring(hole, height_m, kind, id);
            }
        }
    }

    /// Add an open polyline (noise barrier segment chain).
    pub fn add_polyline(&mut self, pts: &[(f64, f64)], height_m: f32, kind: ObstacleKind, id: u32) {
        if pts.len() < 2
            || !height_m.is_finite()
            || height_m <= 0.0
            || pts.iter().any(|(a, o)| !a.is_finite() || !o.is_finite())
        {
            return;
        }
        for w in pts.windows(2) {
            let (x0, y0) = self.to_local(w[0].0, w[0].1);
            let (x1, y1) = self.to_local(w[1].0, w[1].1);
            self.edges.push(ObstacleEdge {
                x0: x0 as f32,
                y0: y0 as f32,
                x1: x1 as f32,
                y1: y1 as f32,
                height_m,
                id,
                kind: kind.code(),
            });
        }
    }

    /// Freeze into the CSR grid index. Empty builder yields an index whose
    /// `crossings` is a no-op (the rural fast path).
    pub fn build(mut self) -> ObstacleIndex {
        let cell_m = OBSTACLE_GRID_CELL_M;
        let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
        let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
        for e in &self.edges {
            min_x = min_x.min(e.x0 as f64).min(e.x1 as f64);
            min_y = min_y.min(e.y0 as f64).min(e.y1 as f64);
            max_x = max_x.max(e.x0 as f64).max(e.x1 as f64);
            max_y = max_y.max(e.y0 as f64).max(e.y1 as f64);
        }
        if self.edges.is_empty() {
            return ObstacleIndex {
                origin_lat: self.origin_lat,
                origin_lon: self.origin_lon,
                m_per_deg_lon: self.m_per_deg_lon,
                cell_m,
                min_x: 0.0,
                min_y: 0.0,
                cols: 1,
                rows: 1,
                cell_starts: vec![0, 0].into(),
                edge_refs: Vec::new().into(),
                edges: Vec::new().into(),
                cell_max_h: vec![0.0].into(),
                footprint_xmin: Vec::new().into(),
                footprint_class: Vec::new().into(),
                max_footprint_w: 0.0,
            };
        }
        // Per-footprint bboxes for the containment walk (edges carry every
        // ring vertex, so the per-id min/max over edge endpoints IS the
        // union bbox of that id's rings). Dense-id contract: the loaders
        // assign sequential ordinals; each footprint has ≥ 3 edges, so a
        // sparse id space signals a broken caller, not big data.
        let max_id = self.edges.iter().map(|e| e.id).max().unwrap() as usize;
        if self.footprint_class.len() <= max_id {
            self.footprint_class
                .resize(max_id + 1, EnvelopeClass::Default as u8);
        }
        self.footprint_class.truncate(max_id + 1);
        assert!(
            max_id < self.edges.len().saturating_mul(4) + 1024,
            "obstacle ids must be dense loader ordinals (max id {max_id}, {} edges)",
            self.edges.len()
        );
        let mut footprint_xmin = vec![f32::INFINITY; max_id + 1];
        let mut footprint_xmax = vec![f32::NEG_INFINITY; max_id + 1];
        for e in &self.edges {
            let i = e.id as usize;
            footprint_xmin[i] = footprint_xmin[i].min(e.x0).min(e.x1);
            footprint_xmax[i] = footprint_xmax[i].max(e.x0).max(e.x1);
        }
        let max_footprint_w = footprint_xmin
            .iter()
            .zip(&footprint_xmax)
            .map(|(lo, hi)| (hi - lo) as f64)
            .fold(0.0, f64::max)
            + cell_m; // one-cell slack so the owner cell of the last crossing is walked
        let cols = (((max_x - min_x) / cell_m).floor() as usize + 1).max(1);
        let rows = (((max_y - min_y) / cell_m).floor() as usize + 1).max(1);

        // Two-pass CSR fill: count per-cell refs, prefix-sum, then place.
        // Edges are binned by SUPERCOVER (the cells the segment actually
        // passes through, Amanatides & Woo — same traversal the query ray
        // uses), not by bbox: a 10 km diagonal barrier touches ~313 cells,
        // its bbox ~25k (gg review 2026-07-28).
        let mut counts = vec![0u32; cols * rows + 1];
        for e in &self.edges {
            for_each_segment_cell(e, min_x, min_y, cell_m, cols, rows, |c| {
                counts[c + 1] += 1;
            });
        }
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let total = *counts.last().unwrap() as usize;
        assert!(
            u32::try_from(total).is_ok() && total < u32::MAX as usize,
            "obstacle CSR overflow: {total} refs"
        );
        let cell_starts = counts.clone();
        let mut cursor: Vec<u32> = cell_starts[..cols * rows].to_vec();
        let mut edge_refs = vec![0u32; total];
        let mut cell_max_h = vec![0.0f32; cols * rows];
        for (i, e) in self.edges.iter().enumerate() {
            for_each_segment_cell(e, min_x, min_y, cell_m, cols, rows, |c| {
                edge_refs[cursor[c] as usize] = i as u32;
                cursor[c] += 1;
                cell_max_h[c] = cell_max_h[c].max(e.height_m);
            });
        }

        ObstacleIndex {
            origin_lat: self.origin_lat,
            origin_lon: self.origin_lon,
            m_per_deg_lon: self.m_per_deg_lon,
            cell_m,
            min_x,
            min_y,
            cols,
            rows,
            cell_starts: cell_starts.into(),
            edge_refs: edge_refs.into(),
            edges: self.edges.into(),
            cell_max_h: cell_max_h.into(),
            footprint_xmin: footprint_xmin.into(),
            footprint_class: self.footprint_class.into(),
            max_footprint_w,
        }
    }
}

/// Visit every grid cell the segment passes through (4-connected supercover,
/// Amanatides & Woo), clamped to the grid. Shared shape with the query-ray
/// walk in [`ObstacleIndex::crossings`] so binning and querying agree on
/// which cells a segment can be found in.
fn for_each_segment_cell(
    e: &ObstacleEdge,
    min_x: f64,
    min_y: f64,
    cell_m: f64,
    cols: usize,
    rows: usize,
    mut visit: impl FnMut(usize),
) {
    let (x0, y0, x1, y1) = (e.x0 as f64, e.y0 as f64, e.x1 as f64, e.y1 as f64);
    let inv_cell = 1.0 / cell_m;
    let mut cx = (((x0 - min_x) * inv_cell).floor() as i64).clamp(0, cols as i64 - 1);
    let mut cy = (((y0 - min_y) * inv_cell).floor() as i64).clamp(0, rows as i64 - 1);
    let end_cx = (((x1 - min_x) * inv_cell).floor() as i64).clamp(0, cols as i64 - 1);
    let end_cy = (((y1 - min_y) * inv_cell).floor() as i64).clamp(0, rows as i64 - 1);
    let (dx, dy) = (x1 - x0, y1 - y0);
    let step_x: i64 = if dx >= 0.0 { 1 } else { -1 };
    let step_y: i64 = if dy >= 0.0 { 1 } else { -1 };
    let t_delta_x = if dx != 0.0 {
        (cell_m / dx).abs()
    } else {
        f64::INFINITY
    };
    let t_delta_y = if dy != 0.0 {
        (cell_m / dy).abs()
    } else {
        f64::INFINITY
    };
    let next_x_boundary = min_x + (cx + i64::from(dx >= 0.0)) as f64 * cell_m;
    let next_y_boundary = min_y + (cy + i64::from(dy >= 0.0)) as f64 * cell_m;
    let mut t_max_x = if dx != 0.0 {
        ((next_x_boundary - x0) / dx).abs()
    } else {
        f64::INFINITY
    };
    let mut t_max_y = if dy != 0.0 {
        ((next_y_boundary - y0) / dy).abs()
    } else {
        f64::INFINITY
    };
    let mut guard = (cols + rows) as i64 + 4;
    loop {
        visit(cy as usize * cols + cx as usize);
        if (cx == end_cx && cy == end_cy) || guard <= 0 {
            return;
        }
        guard -= 1;
        if t_max_x < t_max_y {
            t_max_x += t_delta_x;
            cx += step_x;
        } else {
            t_max_y += t_delta_y;
            cy += step_y;
        }
        if cx < 0 || cy < 0 || cx >= cols as i64 || cy >= rows as i64 {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLAT: f64 = 50.0;
    const OLON: f64 = 14.0;

    /// Metric offsets (m east, m north of the origin) → (lat, lon).
    fn ll(x_m: f64, y_m: f64) -> (f64, f64) {
        (
            OLAT + y_m / M_PER_DEG_LAT,
            OLON + x_m / m_per_deg_lon(OLAT.to_radians()),
        )
    }

    fn square(cx: f64, cy: f64, half: f64) -> Vec<(f64, f64)> {
        vec![
            ll(cx - half, cy - half),
            ll(cx + half, cy - half),
            ll(cx + half, cy + half),
            ll(cx - half, cy + half),
        ]
    }

    fn run(idx: &ObstacleIndex, from: (f64, f64), to: (f64, f64)) -> Vec<CrossingCandidate> {
        let mut out = Vec::new();
        idx.crossings(from.0, from.1, to.0, to.1, &mut out);
        out
    }

    #[test]
    fn empty_index_yields_no_crossings() {
        let idx = ObstacleIndex::builder(OLAT, OLON).build();
        assert_eq!(idx.edge_count(), 0);
        assert!(run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0)).is_empty());
    }

    /// Skyline walk: the box inside the radius reports its four edges (repeats
    /// from multi-cell binning are allowed — the consumer's merge is idempotent),
    /// each arc pointing at the box and carrying its true range; the far box is
    /// out of range entirely.
    #[test]
    fn skyline_reports_in_range_edges_with_their_bearing() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(200.0, 0.0, 15.0), 8.0, ObstacleKind::Building, 0);
        b.add_ring(&square(2000.0, 0.0, 15.0), 8.0, ObstacleKind::Building, 1);
        let idx = b.build();
        let mut arcs = Vec::new();
        let o = ll(0.0, 0.0);
        idx.skyline_arcs_within(0, o.0, o.1, 0.0, 500.0, 0.0, 0.0, None, &mut |a| {
            arcs.push(a)
        });
        assert_eq!(arcs.len(), 4, "one ring in range, four edges: {arcs:?}");
        for a in &arcs {
            assert!(a.hi - a.lo < std::f64::consts::PI, "short arc: {a:?}");
            assert!(a.lo.abs() < 0.2 && a.hi.abs() < 0.2, "due east: {a:?}");
            assert!(
                (185.0..=216.0).contains(&(a.near_m as f64)),
                "range to the near face: {a:?}"
            );
            assert_eq!(a.height_m, 8.0);
        }
        // A radius that reaches neither box.
        arcs.clear();
        idx.skyline_arcs_within(0, o.0, o.1, 0.0, 100.0, 0.0, 0.0, None, &mut |a| {
            arcs.push(a)
        });
        assert!(arcs.is_empty());
    }

    /// The grazing prune is a HEIGHT gate, not a distance gate: an 8 m box
    /// 200 m away bends a long path by δ ≈ (8−4)²/(2·200) = 0.04 m, so a δ floor
    /// above that skips it whole and one below keeps it.
    #[test]
    fn skyline_grazing_prune_follows_the_delta_law() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(200.0, 0.0, 15.0), 8.0, ObstacleKind::Building, 0);
        let idx = b.build();
        let o = ll(0.0, 0.0);
        let count = |delta_min: f64| {
            let mut n = 0;
            idx.skyline_arcs_within(0, o.0, o.1, 0.0, 500.0, 4.0, delta_min, None, &mut |_| {
                n += 1
            });
            n
        };
        assert_eq!(count(0.02), 4, "δ_min below the box's 0.04 m: kept");
        assert_eq!(count(0.08), 0, "δ_min above it: pruned whole");
        // A box shorter than the sight line cannot break it at any δ floor.
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(200.0, 0.0, 15.0), 3.0, ObstacleKind::Building, 0);
        let low = b.build();
        let mut n = 0;
        low.skyline_arcs_within(0, o.0, o.1, 0.0, 500.0, 4.0, 0.0, None, &mut |_| n += 1);
        assert_eq!(n, 0, "top below the 4 m sight line");
    }

    /// A wall longer than the grid pitch spans many cells; every visit must
    /// report the SAME arc, so the consumer's merge collapses them to one.
    #[test]
    fn skyline_multicell_wall_repeats_are_identical() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_polyline(
            &[ll(60.0, -400.0), ll(60.0, 400.0)],
            8.0,
            ObstacleKind::Barrier,
            0,
        );
        let idx = b.build();
        let o = ll(0.0, 0.0);
        let mut arcs = Vec::new();
        idx.skyline_arcs_within(0, o.0, o.1, 0.0, 1000.0, 0.0, 0.0, None, &mut |a| {
            arcs.push(a)
        });
        assert!(!arcs.is_empty());
        assert!(
            arcs.iter().all(|a| a.lo == arcs[0].lo
                && a.hi == arcs[0].hi
                && a.source_id == arcs[0].source_id),
            "one wall segment, one arc geometry: {arcs:?}"
        );
        // It spans from due south-ish to due north-ish through due east.
        assert!(arcs[0].lo < -1.0 && arcs[0].hi > 1.0, "{:?}", arcs[0]);
    }

    /// Set-level walk concatenates its member indexes' arcs.
    #[test]
    fn set_skyline_concatenates_indexes() {
        let mut b0 = ObstacleIndex::builder(OLAT, OLON);
        b0.add_ring(&square(200.0, -60.0, 10.0), 8.0, ObstacleKind::Building, 0);
        let mut b1 = ObstacleIndex::builder(OLAT, OLON);
        b1.add_ring(&square(200.0, 60.0, 10.0), 8.0, ObstacleKind::Building, 0);
        let set = ObstacleSet {
            indexes: vec![
                std::sync::Arc::new(b0.build()),
                std::sync::Arc::new(b1.build()),
            ],
        };
        let o = ll(0.0, 0.0);
        let mut arcs = Vec::new();
        set.skyline_arcs_within(o.0, o.1, 0.0, 500.0, 0.0, 0.0, None, &mut |a| arcs.push(a));
        assert_eq!(arcs.len(), 8);
        let first_ids: std::collections::BTreeSet<_> = arcs
            .iter()
            .take(4)
            .map(|arc| arc.source_id.bits())
            .collect();
        let second_ids: std::collections::BTreeSet<_> = arcs
            .iter()
            .skip(4)
            .map(|arc| arc.source_id.bits())
            .collect();
        assert_eq!(first_ids, [0, 1, 2, 3].into_iter().collect());
        assert_eq!(second_ids, [4, 5, 6, 7].into_iter().collect());
        assert!(first_ids.is_disjoint(&second_ids));
        assert!(arcs.iter().any(|a| a.hi < 0.0), "the southern box");
        assert!(arcs.iter().any(|a| a.lo > 0.0), "the northern box");
    }

    /// A ray straight through a square building enters and exits: exactly two
    /// crossings, ordered by t, at the expected chainages.
    #[test]
    fn ray_through_square_crosses_twice() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(500.0, 0.0, 10.0), 12.0, ObstacleKind::Building, 7);
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
        assert_eq!(c.len(), 2, "enter + exit");
        assert!(c[0].t < c[1].t);
        assert!(
            (c[0].t - 0.49).abs() < 0.005,
            "entry at ~490 m, got {}",
            c[0].t
        );
        assert!(
            (c[1].t - 0.51).abs() < 0.005,
            "exit at ~510 m, got {}",
            c[1].t
        );
        assert!(c
            .iter()
            .all(|x| x.height_m == 12.0 && x.kind == ObstacleKind::Building && x.id == 7));
    }

    #[test]
    fn ray_beside_square_misses() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(500.0, 100.0, 10.0), 12.0, ObstacleKind::Building, 1);
        let idx = b.build();
        assert!(run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0)).is_empty());
    }

    #[test]
    fn ray_cell_aabb_keeps_boundary_touches_and_rejects_separation() {
        let edge = |x0, y0, x1, y1| ObstacleEdge {
            x0,
            y0,
            x1,
            y1,
            height_m: 1.0,
            id: 0,
            kind: ObstacleKind::Building.code(),
        };
        let ray_x = (0.0, 1.0);
        let ray_y = (0.0, 1.0);
        for (name, touch, outside) in [
            (
                "left",
                edge(0.0, 0.2, 0.0, 0.8),
                edge(-1e-6, 0.2, -1e-6, 0.8),
            ),
            (
                "right",
                edge(1.0, 0.2, 1.0, 0.8),
                edge(1.0 + 1e-6, 0.2, 1.0 + 1e-6, 0.8),
            ),
            (
                "bottom",
                edge(0.2, 0.0, 0.8, 0.0),
                edge(0.2, -1e-6, 0.8, -1e-6),
            ),
            (
                "top",
                edge(0.2, 1.0, 0.8, 1.0),
                edge(0.2, 1.0 + 1e-6, 0.8, 1.0 + 1e-6),
            ),
        ] {
            assert!(
                ray_cell_aabb_may_overlap(ray_x, ray_y, &touch),
                "a closed AABB must keep its {name} boundary touch"
            );
            assert!(
                !ray_cell_aabb_may_overlap(ray_x, ray_y, &outside),
                "strictly separated {name} AABBs can skip the exact predicate"
            );
        }
    }

    #[test]
    fn ray_cell_aabb_never_rejects_an_exact_intersection_point() {
        let mut state = 0x0d15_ea5e_5eed_u64;
        let mut hits = 0usize;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            ((state >> 32) as u32 as f64 / u32::MAX as f64) * 200.0 - 100.0
        };
        for _ in 0..4_000 {
            let (sx, sy) = (next(), next());
            let (dx, dy) = (next(), next());
            let edge = ObstacleEdge {
                x0: next() as f32,
                y0: next() as f32,
                x1: next() as f32,
                y1: next() as f32,
                height_m: 1.0,
                id: 0,
                kind: ObstacleKind::Building.code(),
            };
            if let Some(t) = segment_intersection_t(
                sx,
                sy,
                dx,
                dy,
                edge.x0 as f64,
                edge.y0 as f64,
                edge.x1 as f64,
                edge.y1 as f64,
            ) {
                hits += 1;
                let (ray_x, ray_y) = ray_cell_aabb(sx, sy, dx, dy, t, t);
                assert!(
                    ray_cell_aabb_may_overlap(ray_x, ray_y, &edge),
                    "exact hit t={t} cannot be outside its edge AABB"
                );
            }
        }
        assert!(
            hits > 100,
            "test distribution must exercise exact hits: {hits}"
        );
    }

    fn unscreened_crossings(
        idx: &ObstacleIndex,
        from: (f64, f64),
        to: (f64, f64),
    ) -> Vec<CrossingCandidate> {
        let (sx, sy) = idx.to_local(from.0, from.1);
        let (rx, ry) = idx.to_local(to.0, to.1);
        let (dx, dy) = (rx - sx, ry - sy);
        let mut out: Vec<_> = idx
            .edges
            .iter()
            .filter_map(|edge| {
                segment_intersection_t(
                    sx,
                    sy,
                    dx,
                    dy,
                    edge.x0 as f64,
                    edge.y0 as f64,
                    edge.x1 as f64,
                    edge.y1 as f64,
                )
                .map(|t| CrossingCandidate {
                    t,
                    height_m: edge.height_m,
                    kind: edge.kind(),
                    id: edge.id,
                })
            })
            .collect();
        out.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
        out.dedup_by(|a, b| a.id == b.id && (a.t - b.t).abs() < 1e-9);
        out
    }

    /// REGRESSION (S6 C8). The index origin is the minimum edge coordinate, so
    /// an axis-aligned exterior wall sits on gridline zero and has no neighbour
    /// cell to absorb accumulated-DDA versus cross-product last-ulp drift.
    #[test]
    fn min_y_wall_matches_unscreened_reference() {
        let mut builder = ObstacleIndex::builder(OLAT, OLON);
        builder.add_ring(&square(0.0, 0.0, 10.0), 10.0, ObstacleKind::Building, 0);
        builder.add_ring(&square(0.0, 200.0, 10.0), 10.0, ObstacleKind::Building, 1);
        let idx = builder.build();
        let from = ll(0.0, 2_000.0);
        let to = ll(0.0, -2_000.0);
        let screened = run(&idx, from, to);
        let reference = unscreened_crossings(&idx, from, to);
        assert_eq!(
            screened.len(),
            4,
            "both footprints, entry + exit: {screened:?}"
        );
        assert_eq!(
            screened.len(),
            reference.len(),
            "screened={screened:?}, reference={reference:?}"
        );
        for (got, expected) in screened.iter().zip(&reference) {
            assert_eq!(got.t, expected.t);
            assert_eq!(got.id, expected.id);
            assert_eq!(got.height_m, expected.height_m);
            assert_eq!(got.kind, expected.kind);
        }
    }

    /// REGRESSION (S6 C8). The screen must be a strict SUPERSET filter. Two
    /// invariants are load-bearing and each is one line from silent removal:
    /// the pad in `ray_cell_aabb` (the index origin IS the minimum edge
    /// coordinate, so an axis-aligned exterior wall sits on gridline zero with a
    /// degenerate AABB, binned into one row only, with no neighbour cell to
    /// absorb accumulated-DDA versus cross-product last-ulp drift), and NOT
    /// writing `recent` on an AABB reject (the crossing may live in a later
    /// cell). A single fixed geometry cannot guard a last-ulp property — which
    /// ulp side it lands on is luck — so this sweeps. Removing the pad loses
    /// ~38 % of crossings on gridline-snapped footprints; remembering a reject
    /// loses ~17 %.
    #[test]
    fn screen_never_loses_a_crossing_over_a_swept_population() {
        let mut state = 0xC85E_ED01_u64;
        let mut next = |lo: f64, hi: f64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            lo + (hi - lo) * ((state >> 11) as f64 / (1u64 << 53) as f64)
        };
        let mut checked = 0usize;
        for trial in 0..200 {
            let mut builder = ObstacleIndex::builder(OLAT, OLON);
            for id in 0..12 {
                // Half the trials snap corners onto the 64 m grid pitch, so
                // crossings sit exactly ON cell boundaries by construction.
                let (cx, cy, half) = if trial % 2 == 0 {
                    (
                        (next(-6.0, 6.0) as i64) as f64 * OBSTACLE_GRID_CELL_M,
                        (next(-6.0, 6.0) as i64) as f64 * OBSTACLE_GRID_CELL_M,
                        OBSTACLE_GRID_CELL_M / 2.0,
                    )
                } else {
                    (next(-450.0, 450.0), next(-450.0, 450.0), next(4.0, 30.0))
                };
                builder.add_ring(&square(cx, cy, half), 10.0, ObstacleKind::Building, id);
            }
            let idx = builder.build();
            for k in 0..120 {
                let off = -500.0 + k as f64 * 8.4;
                for &(from, to) in &[
                    (ll(off, -1500.0), ll(off, 1500.0)), // axis-aligned
                    (ll(-1500.0, off), ll(1500.0, off)), // axis-aligned
                    (ll(off - 1500.0, -1500.0), ll(off + 1500.0, 1500.0)), // diagonal
                ] {
                    let screened = run(&idx, from, to);
                    for want in unscreened_crossings(&idx, from, to) {
                        assert!(
                            screened
                                .iter()
                                .any(|got| got.id == want.id && (got.t - want.t).abs() < 1e-9),
                            "screen dropped id={} t={} (trial {trial})",
                            want.id,
                            want.t
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 100_000, "sweep must reach the walk: {checked}");
    }

    /// Receiver inside the footprint: entry edge only — and endpoint
    /// exclusivity keeps t strictly below 1.
    #[test]
    fn receiver_inside_footprint_sees_entry_only() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(1000.0, 0.0, 15.0), 20.0, ObstacleKind::Building, 3);
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
        assert_eq!(c.len(), 1, "only the entry edge crosses");
        assert!(c[0].t > 0.0 && c[0].t < 1.0);
    }

    /// A barrier polyline whose single long edge spans many grid cells is
    /// reported ONCE (dedupe across DDA cells).
    #[test]
    fn long_wall_crossing_reported_once() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_polyline(
            &[ll(500.0, -400.0), ll(500.0, 400.0)],
            4.0,
            ObstacleKind::Barrier,
            9,
        );
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
        assert_eq!(c.len(), 1, "one wall, one crossing");
        assert_eq!(c[0].kind, ObstacleKind::Barrier);
        assert!((c[0].t - 0.5).abs() < 0.005);
    }

    /// Two buildings along the ray: four crossings, strictly t-sorted.
    #[test]
    fn two_buildings_sorted_by_chainage() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(300.0, 0.0, 10.0), 6.0, ObstacleKind::Building, 1);
        b.add_ring(&square(700.0, 0.0, 10.0), 9.0, ObstacleKind::Building, 2);
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
        assert_eq!(c.len(), 4);
        assert!(c.windows(2).all(|w| w[0].t < w[1].t));
        assert_eq!(
            c.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![1, 1, 2, 2],
            "near building's two edges first"
        );
    }

    /// The obstacle store must not turn the same bad building tag rejected by
    /// settlement normalization into a 31 km screening wall. The building-only
    /// ceiling deliberately leaves the independent noise-barrier domain alone.
    #[test]
    fn building_obstacle_height_is_clamped_at_edge_formation() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &square(300.0, 0.0, 10.0),
            31_231.0,
            ObstacleKind::Building,
            1,
        );
        b.add_polyline(
            &[ll(700.0, -20.0), ll(700.0, 20.0)],
            31_231.0,
            ObstacleKind::Barrier,
            2,
        );
        let idx = b.build();
        let crossings = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));

        let building_heights: Vec<_> = crossings
            .iter()
            .filter(|candidate| candidate.kind == ObstacleKind::Building)
            .map(|candidate| candidate.height_m)
            .collect();
        assert_eq!(building_heights, vec![828.0, 828.0]);
        assert_eq!(
            crossings
                .iter()
                .find(|candidate| candidate.kind == ObstacleKind::Barrier)
                .map(|candidate| candidate.height_m),
            Some(31_231.0)
        );
    }

    /// Degenerate inputs are ignored: sub-3-point rings, non-positive heights.
    #[test]
    fn degenerate_inputs_are_ignored() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[ll(0.0, 0.0), ll(10.0, 0.0)],
            5.0,
            ObstacleKind::Building,
            1,
        );
        b.add_ring(&square(500.0, 0.0, 10.0), 0.0, ObstacleKind::Building, 2);
        b.add_polyline(&[ll(0.0, 0.0)], 3.0, ObstacleKind::Barrier, 3);
        let idx = b.build();
        assert_eq!(idx.edge_count(), 0);
    }

    /// A diagonal ray against a diagonal-ish wall — the DDA must not step
    /// past a crossing that sits exactly on a cell boundary region.
    #[test]
    fn diagonal_ray_hits_offset_wall() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_polyline(
            &[ll(400.0, 260.0), ll(640.0, 100.0)],
            5.0,
            ObstacleKind::Barrier,
            4,
        );
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 500.0));
        assert_eq!(c.len(), 1, "diagonal wall must be hit exactly once");
    }

    /// gg stress case: a long shallow-diagonal wall is binned into many cells;
    /// with >8 building crossings after the wall hit, the ring buffer evicts
    /// the wall's edge and the DDA re-tests it — the post-sort dedup must
    /// still report it exactly once.
    #[test]
    fn shallow_wall_with_many_intervening_hits_reported_once() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        // Wall from (100, -40) to (2000, 80): shallow diagonal, crossed early.
        b.add_polyline(
            &[ll(100.0, -40.0), ll(2000.0, 80.0)],
            4.0,
            ObstacleKind::Barrier,
            99,
        );
        // Ten small buildings straight along the ray after the wall crossing.
        for i in 0..10 {
            let cx = 500.0 + 120.0 * i as f64;
            b.add_ring(&square(cx, 0.0, 8.0), 6.0, ObstacleKind::Building, i);
        }
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(2000.0, 0.0));
        let walls = c.iter().filter(|x| x.id == 99).count();
        assert_eq!(walls, 1, "wall must appear exactly once, got {walls}");
        assert_eq!(c.len(), 21, "1 wall + 10 buildings x 2 edges");
    }

    /// A ray through a ring VERTEX touches two edges of the same obstacle at
    /// one chainage — dedup must collapse it to a single candidate.
    #[test]
    fn ring_vertex_hit_is_single_candidate() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &square(500.0, 100.0, 100.0),
            10.0,
            ObstacleKind::Building,
            5,
        );
        let idx = b.build();
        // Diagonal ray through the square's bottom-left corner (400, 0):
        // from (300, -100) toward (500, 100) direction — corner at t=0.5 of
        // a (300,-100)->(700,300) ray. It then EXITS through the top edge.
        let c = run(&idx, ll(300.0, -100.0), ll(700.0, 300.0));
        let at_corner: Vec<_> = c.iter().filter(|x| (x.t - 0.25).abs() < 0.01).collect();
        assert!(
            at_corner.len() <= 1,
            "corner hit must dedup to one candidate, got {}",
            at_corner.len()
        );
        assert!(!c.is_empty());
    }

    /// A due-north ray (dx == 0) exercises the INFINITY t_delta branch.
    #[test]
    fn vertical_ray_crosses_horizontal_wall() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_polyline(
            &[ll(-50.0, 300.0), ll(50.0, 300.0)],
            3.0,
            ObstacleKind::Barrier,
            1,
        );
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(0.0, 600.0));
        assert_eq!(c.len(), 1);
        assert!((c[0].t - 0.5).abs() < 0.005);
    }

    /// Endpoints outside the indexed slab still collect the crossings the
    /// clipped path covers.
    #[test]
    fn ray_from_outside_slab_still_hits() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 20.0), 9.0, ObstacleKind::Building, 2);
        let idx = b.build();
        let c = run(&idx, ll(-5000.0, 0.0), ll(5000.0, 0.0));
        assert_eq!(c.len(), 2, "enter + exit despite far-outside endpoints");
    }

    /// Reversed ray sees the same crossings mirrored in t.
    #[test]
    fn reversed_ray_is_symmetric() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(300.0, 0.0, 10.0), 6.0, ObstacleKind::Building, 1);
        let idx = b.build();
        let fwd = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
        let rev = run(&idx, ll(1000.0, 0.0), ll(0.0, 0.0));
        assert_eq!(fwd.len(), 2);
        assert_eq!(rev.len(), 2);
        assert!((fwd[0].t - (1.0 - rev[1].t)).abs() < 1e-9);
        assert!((fwd[1].t - (1.0 - rev[0].t)).abs() < 1e-9);
    }

    /// Two polyline segments sharing a vertex: a ray through the shared point
    /// dedups to one candidate (same obstacle id).
    #[test]
    fn shared_polyline_vertex_dedups() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_polyline(
            &[ll(500.0, -100.0), ll(500.0, 0.0), ll(500.0, 100.0)],
            4.0,
            ObstacleKind::Barrier,
            8,
        );
        let idx = b.build();
        let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
        assert_eq!(c.len(), 1, "shared vertex must not double-count");
    }

    /// Non-finite inputs are rejected wholesale.
    #[test]
    fn non_finite_inputs_are_rejected() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[ll(0.0, 0.0), (f64::NAN, 14.0), ll(10.0, 10.0)],
            5.0,
            ObstacleKind::Building,
            1,
        );
        b.add_ring(
            &square(500.0, 0.0, 10.0),
            f32::NAN,
            ObstacleKind::Building,
            2,
        );
        b.add_polyline(
            &[ll(0.0, 0.0), (50.0, f64::INFINITY)],
            3.0,
            ObstacleKind::Barrier,
            3,
        );
        let idx = b.build();
        assert_eq!(idx.edge_count(), 0);
    }

    /// Crossing-parity containment + the 9-probe enclosure thresholds.
    #[test]
    fn contains_and_enclosure_thresholds() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 60.0), 12.0, ObstacleKind::Building, 1);
        let idx = b.build();
        let mut sc = Vec::new();
        assert!(
            idx.contains_built(OLAT, OLON, 5.0, &mut sc),
            "centre is inside"
        );
        let (out_lat, out_lon) = ll(300.0, 0.0);
        assert!(
            !idx.contains_built(out_lat, out_lon, 5.0, &mut sc),
            "outside"
        );
        assert!(
            !idx.contains_built(OLAT, OLON, 20.0, &mut sc),
            "min-height gate must exclude the 12 m footprint"
        );

        let set = ObstacleSet {
            indexes: vec![std::sync::Arc::new(idx)],
        };
        // 60 m half-size square vs 75 m probes: only the centre probe is
        // inside → density 1/9 → 0 dB.
        assert_eq!(enclosure_db(&set, OLAT, OLON, 75.0), 0.0);

        // A 200 m half-size block swallows all 9 probes → 3 dB.
        let mut b2 = ObstacleIndex::builder(OLAT, OLON);
        b2.add_ring(&square(0.0, 0.0, 200.0), 12.0, ObstacleKind::Building, 1);
        let set2 = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b2.build())],
        };
        assert_eq!(enclosure_db(&set2, OLAT, OLON, 75.0), 3.0);
    }

    /// gg case: a point inside TWO overlapping tall footprints must read
    /// inside (per-footprint parity — the old global bit XORed to false).
    #[test]
    fn overlapping_footprints_contain_correctly() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 50.0), 12.0, ObstacleKind::Building, 1);
        b.add_ring(&square(20.0, 0.0, 50.0), 15.0, ObstacleKind::Building, 2);
        let idx = b.build();
        let mut sc = Vec::new();
        assert!(idx.contains_built(OLAT, OLON, 5.0, &mut sc), "inside both");
        let (lat_e, lon_e) = ll(60.0, 0.0);
        assert!(
            idx.contains_built(lat_e, lon_e, 5.0, &mut sc),
            "inside #2 only"
        );
        let (lat_o, lon_o) = ll(200.0, 0.0);
        assert!(
            !idx.contains_built(lat_o, lon_o, 5.0, &mut sc),
            "outside both"
        );
    }

    /// A probe exactly at a footprint's south-west corner latitude (the
    /// horizontal parity ray grazes vertices) stays consistent: the
    /// half-open vertex rule counts a transit vertex once.
    #[test]
    fn parity_ray_through_vertices_is_consistent() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(100.0, 0.0, 40.0), 10.0, ObstacleKind::Building, 1);
        let idx = b.build();
        let mut sc = Vec::new();
        // Probe WEST of the square at exactly the corner row (y = -40 m is a
        // vertex latitude): the horizontal parity ray grazes both corners.
        // OUTSIDE must stay outside despite the graze; a mid-edge-row probe
        // west of the square is outside too; INSIDE stays inside.
        let (corner_lat, west_lon) = (ll(0.0, -40.0).0, ll(-200.0, 0.0).1);
        assert!(!idx.contains_built(corner_lat, west_lon, 5.0, &mut sc));
        let (mid_lat, _unused) = ll(0.0, 0.0);
        assert!(!idx.contains_built(mid_lat, west_lon, 5.0, &mut sc));
        let (in_lat, in_lon) = (ll(100.0, -39.9).0, ll(100.0, 0.0).1);
        assert!(idx.contains_built(in_lat, in_lon, 5.0, &mut sc));
    }

    /// gg case (Codex): a fixed-length cast could END inside a far footprint
    /// and report a phantom "inside". The `footprint_xmin` skip makes a
    /// footprint entirely east of the probe uncountable by construction.
    #[test]
    fn far_footprint_does_not_phantom_capture() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 20.0), 10.0, ObstacleKind::Building, 0);
        // 800 m wide block whose interior would swallow a 2 km ray end.
        b.add_ring(&square(1800.0, 0.0, 400.0), 10.0, ObstacleKind::Building, 1);
        let idx = b.build();
        let mut sc = Vec::new();
        let (plat, plon) = ll(100.0, 0.0); // between the two, inside neither
        assert!(!idx.contains_built(plat, plon, 5.0, &mut sc));
        let (ilat, ilon) = ll(1800.0, 0.0);
        assert!(idx.contains_built(ilat, ilon, 5.0, &mut sc));
    }

    /// gg case (Codex): a footprint WIDER than any fixed cast length must
    /// still read inside near its west wall — the walk bound is derived
    /// from the data (max footprint bbox width), not a constant.
    #[test]
    fn oversized_footprint_still_contained() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 1500.0), 10.0, ObstacleKind::Building, 0);
        let idx = b.build();
        let mut sc = Vec::new();
        let (plat, plon) = ll(-1400.0, 0.0); // 2.9 km from the east wall
        assert!(idx.contains_built(plat, plon, 5.0, &mut sc));
        let (olat, olon) = ll(-1600.0, 0.0);
        assert!(!idx.contains_built(olat, olon, 5.0, &mut sc));
    }

    /// Holes share the outer ring's id: a courtyard probe crosses hole+outer
    /// east walls (even ⇒ outside), a probe between them only the outer wall
    /// (odd ⇒ inside).
    #[test]
    fn courtyard_reads_outside_annulus_inside() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 50.0), 12.0, ObstacleKind::Building, 0);
        b.add_ring(&square(0.0, 0.0, 20.0), 12.0, ObstacleKind::Building, 0);
        let idx = b.build();
        let mut sc = Vec::new();
        assert!(
            !idx.contains_built(OLAT, OLON, 5.0, &mut sc),
            "courtyard centre"
        );
        let (alat, alon) = ll(35.0, 0.0);
        assert!(
            idx.contains_built(alat, alon, 5.0, &mut sc),
            "annulus between hole and outer wall"
        );
    }

    /// gg case (Codex): a TANGENT vertex (both adjacent edges on the same
    /// side of the probe row) must contribute even parity. The half-open
    /// u-rule counted it once; the straddle rule counts both edges or
    /// neither. Apex-down triangle, probe row through the apex.
    #[test]
    fn tangent_vertex_keeps_parity() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        let tri = vec![ll(0.0, 0.0), ll(30.0, 40.0), ll(-30.0, 40.0)];
        b.add_ring(&tri, 10.0, ObstacleKind::Building, 0);
        let idx = b.build();
        let mut sc = Vec::new();
        // Probe west of the apex, ON the apex row: both slanted edges cross
        // the row AT the apex — two counts (even), outside. One count would
        // report phantom containment all the way west.
        let (alat, wlon) = (ll(0.0, 0.0).0, ll(-200.0, 0.0).1);
        assert!(!idx.contains_built(alat, wlon, 5.0, &mut sc));
        let (ilat, ilon) = ll(0.0, 20.0);
        assert!(idx.contains_built(ilat, ilon, 5.0, &mut sc), "interior");
    }

    /// Histogram twin: `max_height_crossed` reports the tallest BUILDING
    /// footprint the path actually crosses (exact), ignoring barriers and
    /// footprints off the path — the vector replacement for the popup's
    /// raster `max_building_along_path` group histogram.
    #[test]
    fn max_height_crossed_reads_exact_crossings() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        // 12 m block 300 m east, 8 m block 600 m west, wall to the north.
        b.add_ring(&square(300.0, 0.0, 60.0), 12.0, ObstacleKind::Building, 0);
        b.add_ring(&square(-600.0, 0.0, 60.0), 8.0, ObstacleKind::Building, 1);
        b.add_ring(&square(0.0, 400.0, 60.0), 20.0, ObstacleKind::Barrier, 2);
        let set = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b.build())],
        };
        let mut scratch = Vec::new();
        // Ray east from the origin: crosses the 12 m block at ~t=0.27.
        let (h, t) = set.max_height_crossed(OLAT, OLON, OLAT, OLON + 0.02, &mut scratch);
        assert_eq!(h, 12.0);
        assert!((0.05..0.5).contains(&t), "crossing t {t} not mid-path");
        // Ray west: the 8 m block; the 20 m wall stands aside, not on it.
        let (h, _) = set.max_height_crossed(OLAT, OLON, OLAT, OLON - 0.02, &mut scratch);
        assert_eq!(h, 8.0);
        // Ray north: only the wall (a Barrier) is there — buildings say 0.
        let (h, _) = set.max_height_crossed(OLAT, OLON, OLAT + 0.02, OLON, &mut scratch);
        assert_eq!(h, 0.0);
        // Clear path: nothing.
        let (h, _) = set.max_height_crossed(OLAT, OLON, OLAT - 0.02, OLON, &mut scratch);
        assert_eq!(h, 0.0);
    }

    /// 1.4b wrapper: `building_enclosure` answers from the store, every
    /// other lookup delegates to the wrapped sampler unchanged.
    #[test]
    fn vector_reflection_sampler_overrides_only_enclosure() {
        use crate::types::RasterSampler;
        struct Flat;
        impl RasterSampler for Flat {
            fn elevation(&self, _: f64, _: f64) -> f64 {
                123.0
            }
            fn building_height(&self, _: f64, _: f64) -> f64 {
                7.0
            }
            fn ground_g(&self, _: f64, _: f64) -> f64 {
                0.25
            }
            fn building_enclosure(&self, _: f64, _: f64) -> f64 {
                99.0 // sentinel: must never surface through the wrapper
            }
            fn build_path_profile(
                &self,
                _: f64,
                _: f64,
                _: f64,
                _: f64,
                dist_m: f64,
                out: &mut crate::propagation::PathProfile,
            ) {
                // sentinel override: dist_m must round-trip through the
                // wrapper's forwarder (a dropped forwarder would fall back
                // to the trait default and lose the inner override).
                out.dist_m = dist_m * 2.0;
            }
            fn max_building_along_path(
                &self,
                _: f64,
                _: f64,
                _: f64,
                _: f64,
                _: f64,
                _: f64,
            ) -> (f64, f64) {
                (42.0, 0.5)
            }
        }
        // Dense block around the origin ⇒ all nine probes inside ⇒ 3 dB.
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(&square(0.0, 0.0, 200.0), 12.0, ObstacleKind::Building, 0);
        let set = ObstacleSet {
            indexes: vec![std::sync::Arc::new(b.build())],
        };
        let w = VectorReflectionSampler {
            inner: &Flat,
            set: &set,
        };
        assert_eq!(w.elevation(OLAT, OLON), 123.0);
        assert_eq!(w.building_height(OLAT, OLON), 7.0);
        assert_eq!(w.ground_g(OLAT, OLON), 0.25);
        assert_eq!(w.building_enclosure(OLAT, OLON), 3.0);
        let (far_lat, far_lon) = ll(5_000.0, 5_000.0);
        assert_eq!(w.building_enclosure(far_lat, far_lon), 0.0);
        // The two defaultable methods must forward to the INNER override,
        // not fall back to the trait default (gg review 1.4b #1).
        let mut prof = crate::propagation::PathProfile::new();
        w.build_path_profile(OLAT, OLON, OLAT, OLON, 100.0, &mut prof);
        assert_eq!(prof.dist_m, 200.0);
        assert_eq!(
            w.max_building_along_path(OLAT, OLON, OLAT, OLON, 100.0, 0.0),
            (42.0, 0.5)
        );
    }
}

#[cfg(test)]
mod slab_reject_tests {
    use super::*;

    const OLAT: f64 = 50.0;
    const OLON: f64 = 14.0;

    fn ll(x_m: f64, y_m: f64) -> (f64, f64) {
        (
            OLAT + y_m / M_PER_DEG_LAT,
            OLON + x_m / m_per_deg_lon(OLAT.to_radians()),
        )
    }

    fn boxes_at(offsets: &[(f64, f64)]) -> ObstacleSet {
        let mut indexes = Vec::new();
        for (i, &(cx, cy)) in offsets.iter().enumerate() {
            let mut b = ObstacleIndex::builder(OLAT, OLON);
            b.add_ring(
                &[
                    ll(cx - 20.0, cy - 20.0),
                    ll(cx + 20.0, cy - 20.0),
                    ll(cx + 20.0, cy + 20.0),
                    ll(cx - 20.0, cy + 20.0),
                ],
                9.0,
                ObstacleKind::Building,
                i as u32,
            );
            indexes.push(std::sync::Arc::new(b.build()));
        }
        ObstacleSet { indexes }
    }

    /// The reject is EXACT: over a dense sweep of rays against a 7-index ring,
    /// the pruned set's crossings must be identical to the unpruned walk's —
    /// same count, same chainages, same heights.
    #[test]
    fn slab_reject_never_changes_the_crossing_set() {
        // A grid_disk(1)-shaped ring of seven separated footprints.
        let set = boxes_at(&[
            (0.0, 0.0),
            (300.0, 0.0),
            (150.0, 260.0),
            (-150.0, 260.0),
            (-300.0, 0.0),
            (-150.0, -260.0),
            (150.0, -260.0),
        ]);
        let mut with = Vec::new();
        let mut without = Vec::new();
        let mut checked = 0usize;
        let mut skipped = 0usize;
        for i in 0..60 {
            for j in 0..60 {
                let src = ll(-500.0 + i as f64 * 17.3, -450.0 + j as f64 * 15.1);
                let rcv = ll(480.0 - j as f64 * 16.7, 430.0 - i as f64 * 14.9);
                set.crossings(src.0, src.1, rcv.0, rcv.1, &mut with);
                // Reference: every index walked, no reject.
                without.clear();
                for idx in &set.indexes {
                    idx.append_crossings(
                        src.0,
                        src.1,
                        rcv.0,
                        rcv.1,
                        None,
                        &mut CrossingScratch::default(),
                        &mut without,
                    );
                }
                without.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
                checked += 1;
                skipped += set
                    .indexes
                    .iter()
                    .filter(|idx| !idx.segment_may_hit(src.0, src.1, rcv.0, rcv.1))
                    .count();
                assert_eq!(with.len(), without.len(), "count differs");
                for (a, b) in with.iter().zip(&without) {
                    assert_eq!(a.t, b.t, "chainage differs");
                    assert_eq!(a.height_m, b.height_m);
                    assert_eq!(a.id, b.id);
                }
            }
        }
        // And it must actually reject: the whole point is the walks not taken.
        let per_ray = skipped as f64 / checked as f64;
        assert!(
            per_ray > 2.0,
            "only {per_ray:.2} of 7 indexes rejected per ray — no win"
        );
        println!("slab reject: {per_ray:.2}/7 indexes skipped per ray over {checked} rays");
    }
}

#[cfg(test)]
mod cell_prune_tests {
    use super::*;
    use crate::constants::PENUMBRA_DELTA_FLOOR_M;

    const OLAT: f64 = 50.0;
    const OLON: f64 = 14.0;

    fn ll(x_m: f64, y_m: f64) -> (f64, f64) {
        (
            OLAT + y_m / M_PER_DEG_LAT,
            OLON + x_m / m_per_deg_lon(OLAT.to_radians()),
        )
    }

    /// A 50 m ray, source and receiver both 4 m up, over ground at 0 with a 3 m
    /// top: the penumbra geometry the −0.2698 m floor exists for (a 3 m wall in
    /// front of a 4 m facade still screens ~0.6 dB).
    fn flat_penumbra_prune<'a>(t: &'a [f64], elev: &'a [f32]) -> CellPrune<'a> {
        CellPrune {
            t,
            elevation_m: elev,
            src_e: 4.0,
            rcv_e: 4.0,
            dist_m: 50.0,
            floor_m: PENUMBRA_DELTA_FLOOR_M,
        }
    }

    /// REGRESSION (2026-08-08). `max_delta`'s negative branch is `−detour`, which
    /// is CONCAVE in `t`: its max sits at the reflection point, not at a window
    /// endpoint. The old code evaluated the point where the SIGHT LINE crosses
    /// `top` instead — outside the window whenever `top` runs below both ends, so
    /// the clamp collapsed it onto an endpoint and the bound came out BELOW the
    /// true max. A bound that is too low prunes cells that hold a real candidate:
    /// silent loss of screening, and a direct breach of the prune's
    /// output-neutrality invariant.
    #[test]
    fn low_top_prune_keeps_penumbra_candidate() {
        let (t, elev) = ([0.0, 1.0], [0.0f32, 0.0]);
        let p = flat_penumbra_prune(&t, &elev);

        // The true max over t ∈ [0,1]: reflection point t* = 1/(1+1) = 0.5.
        let exact = -(2.0 * (25.0f64 * 25.0 + 1.0).sqrt() - 50.0);
        let bound = p.max_delta(3.0, 0.0, 1.0);
        assert!(
            (bound - exact).abs() < 1e-12,
            "bound {bound} is not the exact max {exact}"
        );
        assert!((bound - -0.039_984_012_8).abs() < 1e-9, "bound {bound}");
        // …and it clears the floor, so the cell survives the prune.
        assert!(
            bound > PENUMBRA_DELTA_FLOOR_M,
            "{bound} <= {PENUMBRA_DELTA_FLOOR_M}"
        );

        // What the endpoints alone said — 25× too deep, under the floor, cell
        // dropped. This is the number the fix moved.
        let endpoints = -(1.0 + (50.0f64 * 50.0 + 1.0).sqrt() - 50.0);
        assert!((endpoints - -1.009_999_0).abs() < 1e-6, "{endpoints}");
        assert!(endpoints < PENUMBRA_DELTA_FLOOR_M, "{endpoints}");

        // Sub-windows: the clamp must still land on the exact max of the window
        // it is given, on both sides of the reflection point.
        for &(a, b) in &[(0.0, 0.4), (0.6, 1.0), (0.45, 0.55), (0.0, 1.0)] {
            let got = p.max_delta(3.0, a, b);
            let brute = (0..=2000)
                .map(|k| a + (b - a) * k as f64 / 2000.0)
                .map(|tt| {
                    -(((tt * 50.0f64).powi(2) + 1.0).sqrt()
                        + (((1.0 - tt) * 50.0f64).powi(2) + 1.0).sqrt()
                        - 50.0)
                })
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(
                got >= brute - 1e-12,
                "window [{a},{b}]: bound {got} below sampled max {brute}"
            );
            assert!(
                got <= brute + 1e-6,
                "window [{a},{b}]: bound {got} far above sampled max {brute}"
            );
        }
    }

    /// End to end: the same 3 m wall at mid-chainage must come out of
    /// `crossings_pruned` exactly as it comes out of the unpruned walk. This is
    /// the invariant `crossings_pruned` claims, and what the bug broke.
    #[test]
    fn penumbra_wall_survives_the_pruned_walk() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[
                ll(24.0, -30.0),
                ll(26.0, -30.0),
                ll(26.0, 30.0),
                ll(24.0, 30.0),
            ],
            3.0,
            ObstacleKind::Barrier,
            7,
        );
        let idx = b.build();
        let src = ll(0.0, 0.0);
        let rcv = ll(50.0, 0.0);

        let (t, elev) = ([0.0, 0.5, 1.0], [0.0f32, 0.0, 0.0]);
        let p = flat_penumbra_prune(&t, &elev);
        let mut pruned = Vec::new();
        idx.crossings_pruned(src.0, src.1, rcv.0, rcv.1, &p, &mut pruned);
        let mut plain = Vec::new();
        idx.crossings(src.0, src.1, rcv.0, rcv.1, &mut plain);

        assert!(!plain.is_empty(), "the unpruned walk must see the wall");
        assert_eq!(
            pruned.len(),
            plain.len(),
            "prune dropped a candidate the loop's floor keeps"
        );
        for (a, c) in pruned.iter().zip(&plain) {
            assert_eq!((a.t, a.height_m, a.id), (c.t, c.height_m, c.id));
        }
    }

    /// A generation-tagged scratch table is only a storage optimization: it
    /// must reproduce the fresh 64-slot table even when one worker reuses it
    /// across different rays and the direct-mapped slots collide.
    #[test]
    fn generation_scratch_matches_fresh_pruned_walk() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        for id in 0..24 {
            let x = 4.0 + id as f64 * 2.0;
            b.add_ring(
                &[ll(x, -8.0), ll(x + 0.8, -8.0), ll(x + 0.8, 8.0), ll(x, 8.0)],
                6.0,
                ObstacleKind::Building,
                id,
            );
        }
        let idx = b.build();
        let set = ObstacleSet {
            indexes: vec![std::sync::Arc::new(idx)],
        };
        let src = ll(0.0, 0.0);
        let (t, elev) = ([0.0, 0.5, 1.0], [0.0f32, 0.0, 0.0]);
        let p = flat_penumbra_prune(&t, &elev);
        let mut fresh = Vec::new();
        let mut reused = Vec::new();
        let mut scratch = CrossingScratch::default();
        for end_x in [40.0, 55.0, 70.0, 85.0, 100.0, 115.0] {
            let rcv = ll(end_x, 0.0);
            set.crossings_pruned(src.0, src.1, rcv.0, rcv.1, &p, &mut fresh);
            set.crossings_pruned_with_scratch(
                src.0,
                src.1,
                rcv.0,
                rcv.1,
                &p,
                &mut scratch,
                &mut reused,
            );
            assert_eq!(
                reused.len(),
                fresh.len(),
                "scratch changed ray ending at {end_x} m"
            );
            for (actual, expected) in reused.iter().zip(&fresh) {
                assert_eq!(
                    (actual.t, actual.height_m, actual.kind, actual.id),
                    (expected.t, expected.height_m, expected.kind, expected.id),
                    "scratch changed ray ending at {end_x} m"
                );
            }
        }
    }
}

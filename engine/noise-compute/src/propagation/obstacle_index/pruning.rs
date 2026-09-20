//! Bound signed diffraction path differences for whole grid cells.

/// Bound candidate path differences with the terrain samples bracketing each cell.
/// Interpolated crossing terrain cannot exceed the window sample maximum.
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
    /// [`super::ObstacleIndex::crossings_pruned`].
    pub floor_m: f64,
}

impl<'a> CellPrune<'a> {
    /// Prune context for a ray whose profile is already built, floored at the
    /// consumer's own floor. Callers do NOT choose the floor: it belongs to
    /// `path_effects` §5b, the loop that ranks these candidates, and picking it
    /// at the call site is how a prune ends up above its loop.
    pub fn for_profile(
        profile: &'a crate::propagation::PathProfile,
        src_e: f64,
        rcv_e: f64,
    ) -> Self {
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

/// Disable only the acceleration for output-parity measurements.
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
    #[inline]
    pub(super) fn max_delta(&self, top: f64, t_lo: f64, t_hi: f64) -> f64 {
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
        // negative branch's peak. Same expression as the CUDA surface kernel's `tstar`
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

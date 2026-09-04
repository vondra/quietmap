//! Single-edge δ diffraction kernel — the shared core for surface-source
//! terrain/screening, replacing the multi-edge upper-convex-hull selection with
//! the CNOSSOS "edge of largest path-length difference δ".
//!
//! Same `max(A_ground, A_terrain + A_screen)` contract as the old multi-edge
//! path, but each of the two attenuation terms is a SINGLE deterministic δ-edge:
//!   A_terrain  = diffraction over the max-δ edge of the BARE-EARTH profile
//!   A_combined = diffraction over the max-δ edge of the COMPOSITE profile
//!   A_screen   = (A_combined − A_terrain).max(0)        ← INCREMENT, never a sum
//! so `terrain + screen` = max(A_terrain, A_combined) per band — ≡ A_combined
//! whenever the composite edge dominates (the usual case, since composite ≥ bare),
//! and the clamp keeps the stronger bare-earth band when the single composite
//! edge happens to gate a band the bare hill still screens. This is exactly
//! the `(combined − terrain).max(0)` contract in `path_effects::screening_attenuation_with_meta`. Summing
//! two independent Maekawa terms would instead double-count (Maekawa non-linear).
//!
//! δ ∝ 1/(L−x) toward each endpoint, so the single max-δ edge is the
//! near-endpoint barrier that the hull's LOS-excess ranking systematically
//! under-weighted (excess favours near-source obstacles, where the LOS sits low).

use super::diffraction::{compute_single_edge, diffraction_attenuation_mixed, DiffractionResult};
use crate::types::NUM_BANDS;

/// Index of the obstacle with the largest CNOSSOS path-length difference
/// δ = d_S→O + d_O→R − d_S→R over `1..n-1`, among samples above the
/// source→receiver line of sight. `None` if the path is clear. The geometry
/// matches [`compute_single_edge`] so the selected δ equals the diffracted δ.
fn max_delta_idx(
    t: &[f64],
    profile: &[f64],
    total_dist: f64,
    src_elev: f64,
    rcv_elev: f64,
    dsr: f64,
) -> Option<usize> {
    let n = profile.len();
    let mut best: Option<usize> = None;
    let mut best_delta = 0.0_f64;
    for i in 1..n - 1 {
        let top = profile[i];
        let los = src_elev + (rcv_elev - src_elev) * t[i];
        if top <= los {
            continue;
        }
        let d_sg = t[i] * total_dist;
        let d_rg = (1.0 - t[i]) * total_dist;
        let d_sb = (d_sg * d_sg + (top - src_elev).powi(2)).sqrt();
        let d_br = (d_rg * d_rg + (top - rcv_elev).powi(2)).sqrt();
        let delta = d_sb + d_br - dsr;
        if delta > best_delta {
            best_delta = delta;
            best = Some(i);
        }
    }
    best
}

/// Per-band attenuation over the max-δ edge of `top` (the composite OR the
/// bare-earth profile), with the CNOSSOS §2.5.6(c) Rayleigh δ\* fit ALWAYS on
/// `bare` (feeding rooftops to the OLS mean-ground would break ground physics).
/// Returns the single-edge [`DiffractionResult`] (δ, Rayleigh δ\*, edge index)
/// for trace/geometry, or `None` + zero bands when `top` clears the line of sight.
///
/// THE shared primitive: surface terrain calls it with `top == bare`, screening
/// with `top == composite`; popup and pipeline both funnel through
/// [`super::path_effects`], so they agree by construction.
pub(crate) fn single_edge_atten(
    t: &[f64],
    top: &[f64],
    bare: &[f64],
    total_dist: f64,
    src_height: f64,
    rcv_height: f64,
) -> ([f64; NUM_BANDS], Option<DiffractionResult>) {
    debug_assert!(
        t.len() == top.len() && top.len() == bare.len(),
        "profile arrays must be equal length"
    );
    let n = bare.len();
    let src_elev = bare[0] + src_height;
    let rcv_elev = bare[n - 1] + rcv_height;
    let dsr = (total_dist * total_dist + (rcv_elev - src_elev).powi(2)).sqrt();
    match max_delta_idx(t, top, total_dist, src_elev, rcv_elev, dsr) {
        Some(idx) => {
            let r = compute_single_edge(
                t, top, bare, total_dist, idx, src_elev, rcv_elev, dsr, src_height, rcv_height,
            );
            (diffraction_attenuation_mixed(&r), Some(r))
        }
        None => ([0.0; NUM_BANDS], None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_t(n: usize) -> Vec<f64> {
        (0..n).map(|i| i as f64 / (n - 1) as f64).collect()
    }

    /// Terrain + combined bands for one profile pair — the exact call shape
    /// `path_effects` uses (terrain over bare, combined over composite with the
    /// Rayleigh fit on bare); screening there is `(combined − terrain).max(0)`.
    fn edge_pair(
        t: &[f64],
        bare: &[f64],
        composite: &[f64],
        dist: f64,
        src_h: f64,
        rcv_h: f64,
    ) -> (
        [f64; NUM_BANDS],
        [f64; NUM_BANDS],
        Option<DiffractionResult>,
    ) {
        let (terrain, _) = single_edge_atten(t, bare, bare, dist, src_h, rcv_h);
        let (combined, res) = single_edge_atten(t, composite, bare, dist, src_h, rcv_h);
        (terrain, combined, res)
    }

    /// (a) Clear path → zero bands, no edge.
    #[test]
    fn flat_path_is_silent() {
        let t = uniform_t(11);
        let bare = vec![100.0; 11];
        let (atten, res) = single_edge_atten(&t, &bare, &bare, 500.0, 0.05, 4.0);
        assert_eq!(atten, [0.0; NUM_BANDS]);
        assert!(res.is_none());
    }

    /// (b) A bare hill with NO building: terrain attenuates, and the combined
    /// pass over an identical composite yields the identical bands — so the
    /// screening increment is exactly zero.
    #[test]
    fn bare_hill_has_no_screening_increment() {
        let t = uniform_t(11);
        let mut bare = vec![100.0; 11];
        bare[5] = 112.0;
        let (terrain, combined, _) = edge_pair(&t, &bare, &bare, 500.0, 0.05, 4.0);
        assert!(terrain.iter().any(|&a| a > 0.0), "hill must attenuate");
        assert_eq!(terrain, combined, "no building → zero increment");
    }

    /// (c) A building on flat ground: zero terrain, combined attenuates, and the
    /// dominant edge sits mid-path at the building's sample.
    #[test]
    fn building_on_flat_is_pure_screening() {
        let t = uniform_t(11);
        let bare = vec![100.0; 11];
        let mut composite = bare.clone();
        composite[5] = 106.0; // 6 m building
        let (terrain, combined, res) = edge_pair(&t, &bare, &composite, 500.0, 0.05, 4.0);
        assert_eq!(terrain, [0.0; NUM_BANDS], "flat ground → zero terrain");
        assert!(combined.iter().any(|&a| a > 0.0), "building must screen");
        let r = res.expect("composite must yield an edge");
        assert_eq!(r.edge_idx, 5, "edge at the building sample");
        assert!((composite[r.edge_idx] - bare[r.edge_idx] - 6.0).abs() < 1e-9);
        assert!(
            ((1.0 - t[r.edge_idx]) * 500.0 - 250.0).abs() < 1.0,
            "mid-path"
        );
    }

    /// (d) THE fix the single-edge selection exists for: δ favours the
    /// near-RECEIVER obstacle over a mid-path one of equal height
    /// (δ ∝ 1/(L−x) is minimised mid-path). LOS-excess ranking would not.
    #[test]
    fn delta_picks_near_endpoint_over_midpath() {
        let t = uniform_t(11);
        let bare = vec![100.0; 11];
        let mut composite = bare.clone();
        composite[5] = 108.0; // mid-path, 8 m
        composite[9] = 108.0; // near receiver (t=0.9), same 8 m
                              // Symmetric endpoint heights so the only discriminator is δ.
        let (_, _, res) = edge_pair(&t, &bare, &composite, 500.0, 2.0, 2.0);
        let r = res.expect("obstructed path must yield an edge");
        assert_eq!(
            r.edge_idx, 9,
            "near-receiver edge must win on δ, got idx {}",
            r.edge_idx
        );
    }

    /// A shallow bare hill BLOCKS the sight line, so every band diffracts and
    /// the attenuation rises with frequency — the plain Maekawa shape.
    ///
    /// THE EXPECTED VALUE MOVED, AND THE OLD ONE WAS PINNING A DEFECT. This
    /// test used to assert `atten[0] == 0.0`, "63 Hz must be gated by δ*",
    /// because `maekawa_bands` applied the 2021/1226 Rayleigh criterion
    /// `δ ≤ λ/4 − δ*` to blocked rays as well as unblocked ones. The amendment
    /// scopes that criterion to "*If the direct ray is not blocked*"; this hill
    /// blocks it (δ > 0), so no λ/4 test applies and 63 Hz reads 2.07 dB — the
    /// value `10·lg(3 + 20δ/λ)` always gave. The old assertion was holding a
    /// 7.4–9.0 dB cliff in place: on this hill 63 Hz went 0 → 8.7 dB across
    /// a millimetre of crest height (`diffraction::attenuation_is_continuous_in_obstacle_height`).
    #[test]
    fn shallow_hill_diffracts_in_every_band() {
        let n = 61;
        let mut bare = vec![400.0_f64; n];
        bare[n / 2] = 419.0;
        let t: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        let (atten, _) = single_edge_atten(&t, &bare, &bare, 1850.0, 0.05, 4.0);
        assert!(
            (atten[0] - 2.074).abs() < 0.01,
            "63 Hz over a blocking crest is not gated, got {:.3}",
            atten[0]
        );
        // Mixed values (FAVOURABLE_MIXING on since 2026-07-28); monotone in
        // frequency, which is the invariant worth pinning.
        assert!(atten[4] > 2.0, "1 kHz, got {:.3}", atten[4]);
        for i in 1..atten.len() {
            assert!(
                atten[i] >= atten[i - 1] - 1e-9,
                "Maekawa is monotone in frequency: {atten:?}"
            );
        }
    }
}

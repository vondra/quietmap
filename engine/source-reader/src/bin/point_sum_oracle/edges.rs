//! Single-edge diffraction geometry of one sampled ray, as production builds it: the max-δ
//! terrain sample (`horizon::single_edge_atten`) and each vector crossing
//! (`diffraction::compute_single_edge_at`), with δ, the favourable-ray δ_F and the Rayleigh δ*;
//! `Edge::bands` is `diffraction::maekawa_bands` per state with the #27 slope switch.

use crate::ray::{Bands, Method, StateBands};
use noise_compute::constants::{BAND_FREQ, FAV_RAY_CURVATURE_MIN_M, FAV_RAY_CURVATURE_PER_DSR, SINGLE_DIFF_CAP, SPEED_OF_SOUND};
use noise_compute::propagation::obstacle_index::CrossingCandidate;
use noise_compute::propagation::path_effects::{RECEIVER_HEIGHT_FLOOR_M, SCREENING_MIN_PATH_M, SOURCE_HEIGHT_FLOOR_M};
use noise_compute::propagation::path_profile::clamp_source_platform;
use noise_compute::propagation::PathProfile;

/// One diffracting edge: straight δ, favourable-ray δ_F and the Rayleigh δ*.
pub struct Edge {
    delta: f64,
    delta_favourable: f64,
    delta_star: f64,
}

impl Edge {
    /// `diffraction::maekawa_bands` per state; one Rayleigh verdict on the straight δ.
    pub fn bands(&self, method: Method) -> StateBands {
        let arm = |delta: f64| -> Bands {
            std::array::from_fn(|band| {
                let lambda = SPEED_OF_SOUND / BAND_FREQ[band];
                if !(self.delta >= 0.0 || self.delta > lambda / 4.0 - self.delta_star) {
                    return 0.0;
                }
                let n = if delta < 0.0 {
                    3.0 + 40.0 * delta / lambda
                } else {
                    3.0 + method.diffraction_positive_slope * delta * BAND_FREQ[band] / SPEED_OF_SOUND
                };
                if n <= 1.0 {
                    0.0
                } else {
                    (10.0 * n.log10()).min(SINGLE_DIFF_CAP)
                }
            })
        };
        StateBands {
            homogeneous: arm(self.delta),
            favourable: arm(self.delta_favourable),
        }
    }
}

/// Carved bare-earth profile and endpoint geometry shared by terrain and crossings.
pub struct RayGeometry {
    t: Vec<f64>,
    ground: Vec<f64>,
    dist: f64,
    src_h: f64,
    rcv_h: f64,
    src_e: f64,
    rcv_e: f64,
    dsr: f64,
}

impl RayGeometry {
    pub fn new(profile: &PathProfile, source_altitude_m: f64, receiver_altitude_m: f64) -> Option<Self> {
        let n = profile.t.len();
        if n < 3 || profile.dist_m < SCREENING_MIN_PATH_M {
            return None;
        }
        let mut ground: Vec<f64> = profile.elevation_m.iter().map(|&e| e as f64).collect();
        clamp_source_platform(&profile.t, &mut ground, profile.dist_m);
        let src_h = (source_altitude_m - ground[0]).max(SOURCE_HEIGHT_FLOOR_M);
        let rcv_h = (receiver_altitude_m - ground[n - 1]).max(RECEIVER_HEIGHT_FLOOR_M);
        let (src_e, rcv_e) = (ground[0] + src_h, ground[n - 1] + rcv_h);
        Some(Self {
            t: profile.t.clone(),
            dist: profile.dist_m,
            dsr: (profile.dist_m.powi(2) + (rcv_e - src_e).powi(2)).sqrt(),
            ground,
            src_h,
            rcv_h,
            src_e,
            rcv_e,
        })
    }

    fn gamma(&self) -> f64 {
        FAV_RAY_CURVATURE_MIN_M.max(FAV_RAY_CURVATURE_PER_DSR * self.dsr)
    }

    /// `path_effects::compute_terrain_diffraction`: max-δ sample above the sight line.
    pub fn terrain_edge(&self, profile: &PathProfile, src_alt: f64, rcv_alt: f64) -> Option<Edge> {
        let hill = profile
            .t
            .iter()
            .zip(&profile.elevation_m)
            .any(|(&t, &e)| e as f64 > src_alt + (rcv_alt - src_alt) * t);
        if !hill {
            return None;
        }
        let n = self.t.len();
        let mut best: Option<(usize, f64)> = None;
        for i in 1..n - 1 {
            let (top, los) = (self.ground[i], self.src_e + (self.rcv_e - self.src_e) * self.t[i]);
            if top <= los {
                continue;
            }
            let (d_sb, d_br) = self.legs(self.t[i], top);
            let delta = d_sb + d_br - self.dsr;
            if delta > best.map_or(0.0, |(_, d)| d) {
                best = Some((i, delta));
            }
        }
        let (idx, delta) = best?;
        let (d_sb, d_br) = self.legs(self.t[idx], self.ground[idx]);
        let (d_sg, d_rg) = (self.t[idx] * self.dist, (1.0 - self.t[idx]) * self.dist);
        let (_, b_src) = fit_plane(&self.t[..=idx], &self.ground[..=idx], None, 0.0, self.dist);
        let (a_rcv, b_rcv) = fit_plane(&self.t[idx..], &self.ground[idx..], None, self.t[idx], self.dist);
        let delta_star = self.mirror_delta(d_sg, d_rg, self.ground[idx], b_src, a_rcv * d_rg + b_rcv);
        Some(Edge {
            delta,
            delta_favourable: arc_path_difference(d_sb, d_br, self.dsr, self.gamma()),
            delta_star,
        })
    }

    /// `diffraction::compute_single_edge_at` for one vector crossing, as `screening_attenuation` feeds it.
    pub fn crossing_edge(&self, candidate: &CrossingCandidate) -> Edge {
        let (t, n, t_e) = (&self.t, self.t.len(), candidate.t);
        let p = t.partition_point(|&x| x <= t_e).clamp(1, n - 1);
        let frac = if t[p] > t[p - 1] { (t_e - t[p - 1]) / (t[p] - t[p - 1]) } else { 0.0 };
        let top = self.ground[p - 1] + frac * (self.ground[p] - self.ground[p - 1]) + candidate.height_m as f64;
        let los = self.src_e + (self.rcv_e - self.src_e) * t_e;
        let sign = if top >= los { 1.0 } else { -1.0 };
        let (d_sg, d_rg) = (t_e * self.dist, (1.0 - t_e) * self.dist);
        let (d_sb, d_br) = self.legs(t_e, top);
        let p_lo = t.partition_point(|&x| x < t_e);
        let p_hi = t.partition_point(|&x| x <= t_e);
        let i1 = p_hi.clamp(1, n - 1);
        let f1 = if t[i1] > t[i1 - 1] { ((t_e - t[i1 - 1]) / (t[i1] - t[i1 - 1])).clamp(0.0, 1.0) } else { 0.0 };
        let d_top = self.ground[i1 - 1] + f1 * (self.ground[i1] - self.ground[i1 - 1]);
        let (_, b_src) = fit_plane(&t[..p_lo], &self.ground[..p_lo], Some((t_e, d_top)), 0.0, self.dist);
        let (a_rcv, b_rcv) = fit_plane(&t[p_hi..], &self.ground[p_hi..], Some((t_e, d_top)), t_e, self.dist);
        let delta_star = self.mirror_delta(d_sg, d_rg, d_top, b_src, a_rcv * d_rg + b_rcv);
        let gamma = self.gamma();
        let delta_favourable = if sign > 0.0 {
            arc_path_difference(d_sb, d_br, self.dsr, gamma)
        } else {
            let arc = |chord: f64| 2.0 * gamma * (chord / (2.0 * gamma)).asin();
            let d_sa = (d_sg * d_sg + (los - self.src_e).powi(2)).sqrt();
            let d_ar = (d_rg * d_rg + (self.rcv_e - los).powi(2)).sqrt();
            2.0 * arc(d_sa) + 2.0 * arc(d_ar) - arc(d_sb) - arc(d_br) - arc(self.dsr)
        };
        Edge {
            delta: sign * (d_sb + d_br - self.dsr),
            delta_favourable,
            delta_star,
        }
    }

    fn legs(&self, t: f64, top: f64) -> (f64, f64) {
        let (d_sg, d_rg) = (t * self.dist, (1.0 - t) * self.dist);
        (
            (d_sg * d_sg + (top - self.src_e).powi(2)).sqrt(),
            (d_rg * d_rg + (top - self.rcv_e).powi(2)).sqrt(),
        )
    }

    /// Rayleigh δ*: mirror source and receiver across the two per-side mean ground planes.
    fn mirror_delta(&self, d_sg: f64, d_rg: f64, d_top: f64, src_plane: f64, rcv_plane_at_end: f64) -> f64 {
        let n = self.ground.len();
        let s_star = 2.0 * src_plane - (self.ground[0] + self.src_h);
        let r_star = 2.0 * rcv_plane_at_end - (self.ground[n - 1] + self.rcv_h);
        let d_sd = (d_sg * d_sg + (d_top - s_star).powi(2)).sqrt();
        let d_dr = (d_rg * d_rg + (r_star - d_top).powi(2)).sqrt();
        let d_sr = (self.dist * self.dist + (r_star - s_star).powi(2)).sqrt();
        (d_sd + d_dr - d_sr).max(0.0)
    }
}

/// CNOSSOS (2.5.26): each chord replaced by its arc of radius Γ.
fn arc_path_difference(d_sb: f64, d_br: f64, dsr: f64, gamma: f64) -> f64 {
    let arc = |chord: f64| 2.0 * gamma * (chord / (2.0 * gamma)).asin();
    arc(d_sb) + arc(d_br) - arc(dsr)
}

/// Unweighted OLS mean ground plane (`diffraction::fit_mean_ground_plane`, optionally with the
/// diffraction point folded in as `fit_plane_with_point` does).
fn fit_plane(ts: &[f64], zs: &[f64], extra: Option<(f64, f64)>, t_offset: f64, dist: f64) -> (f64, f64) {
    let points = ts.iter().copied().zip(zs.iter().copied()).chain(extra);
    let (mut n, mut sx, mut sz, mut sxx, mut sxz) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (t, z) in points {
        let x = (t - t_offset) * dist;
        n += 1.0;
        sx += x;
        sz += z;
        sxx += x * x;
        sxz += x * z;
    }
    if n < 1.0 {
        return (0.0, 0.0);
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < 1e-9 {
        return (0.0, sz / n);
    }
    let a = (n * sxz - sx * sz) / denom;
    (a, (sz - a * sx) / n)
}

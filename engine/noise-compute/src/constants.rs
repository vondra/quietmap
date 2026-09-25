//! Physical constants from CNOSSOS-EU and ISO 9613-2.

use crate::types::NUM_BANDS;

/// A-weighting per octave band [dB] (IEC 61672-1).
/// Bands: 63, 125, 250, 500, 1000, 2000, 4000, 8000 Hz
pub const A_WEIGHTING: [f64; NUM_BANDS] = [-26.2, -16.1, -8.6, -3.2, 0.0, 1.2, 1.0, -1.1];

/// Atmospheric absorption [dB/km] (ISO 9613-1, 15°C, 70% RH, 101.325 kPa).
pub const ALPHA_ATM: [f64; NUM_BANDS] = [0.1, 0.4, 1.0, 1.9, 3.7, 8.7, 22.0, 58.4];

/// Vegetation attenuation [dB/m] (ISO 9613-2:2024 Annex A.2.2 × 0.5 Central Europe calibration).
/// WHY: ISO values calibrated for dense deciduous foliage in full leaf. ESA WorldCover
/// "tree cover" class 10 includes canopy ≥10% (sparse/coniferous/mixed), averaging ~50%.
pub const ALPHA_VEG: [f64; NUM_BANDS] = [0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.045, 0.06];

/// Maximum vegetation attenuation per band [dB] (ISO 9613-2 Table A.1 × 0.5 Central Europe calibration).
pub const MAX_VEG_ATTEN: [f64; NUM_BANDS] = [2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 9.0, 12.0];

/// Band-mean ground correction factors (CNOSSOS-EU §2.5.15) — one number per
/// octave band standing in for the analytic `A_ground,H(G, f, h_s, h_r, d)`.
/// NEVER used on its own: the term the engine applies is
/// [`crate::propagation::iso9613::ground_atten_db`], which combines this table
/// with [`GROUND_HARD_FLOOR_DB`]. `G = 1 − IMD/100`.
pub const GROUND_CF: [f64; NUM_BANDS] = [-1.5, -0.7, 1.5, 2.5, 2.0, 1.3, 0.7, 0.2];

/// Hard-ground floor of `A_ground` [dB] — CNOSSOS-EU 2015/996 (2.5.15), quoted:
/// *"if Gpath = 0: Aground,H = −3 dB"*, with (2.5.18) `Aground,H,min =
/// −3(1 − Ḡm)` supplying the lower bound of the governing max(). ISO 9613-2
/// Table 3 arrives at the same −3 dB for hard ground (`As + Ar = −1.5 − 1.5`).
///
/// The physics: over a reflective surface the direct ray and its mirror image
/// arrive in phase, so the received level sits ~3 dB ABOVE free field — an
/// attenuation of −3 dB, not zero. Reading `A_ground = CF[i]·G` alone made it
/// zero at G = 0 and cost every layer 3.00 dB in every band over hard ground
/// (verified against the official TC01: 40.81 vs 43.81 dB(A); see
/// `tests/tc_ground.rs`).
///
/// Mirrored into the CUDA kernel by `noise-gpu/build.rs` (`-D` injection), so
/// this line is the only place the number exists.
pub const GROUND_HARD_FLOOR_DB: f64 = -3.0;


/// Octave band center frequencies [Hz].
pub const BAND_FREQ: [f64; NUM_BANDS] = [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// Speed of sound [m/s] at 15°C.
pub const SPEED_OF_SOUND: f64 = 340.0;

/// CNOSSOS-EU §2.5.6(c) penumbra floor: the most NEGATIVE path difference that
/// still attenuates. `diffraction::maekawa_bands` runs the near-miss branch down
/// to δ = −λ/20 and returns zero past it, so at the longest wavelength in the
/// model (63 Hz) this is the deepest near miss that can carry energy in ANY
/// band — and therefore the floor of every candidate loop that keeps
/// below-sight-line obstacles, and of any prune accelerating one.
///
/// # Why the LOWEST band binds — the trap this constant exists to avoid
///
/// The instinct is that the highest band is the permissive one, because its
/// `λ/4` gate is the smallest. It is the opposite, and getting it backwards
/// silently mis-prunes the world. Band *i* is silent when `δ ≤ λ_i/4 − δ*`
/// (the Rayleigh gate) OR when `δ ≤ −λ_i/20` (the penumbra runs out). `δ*` is
/// FITTED DATA and only ever makes the first condition easier, so the only
/// δ*-independent floor is the second — and taking it at the SHORTEST
/// wavelength is not a floor at all:
///
/// ```text
/// maekawa_bands(δ = −λ_8kHz/20 = −0.002125, δ* = 0.1) → 1 kHz band = 4.39 dB
/// ```
///
/// At the LONGEST wavelength (63 Hz) the value is 127× further out, and there
/// every band is silent for every `δ*`. `diffraction::delta_reject_tests`
/// pins both directions, including that trap.
///
/// # Output-INVISIBLE, not merely "returns zero"
///
/// Below this δ the near-miss branch returns exactly zero AND the favourable
/// arm equals the homogeneous one, so a skip changes nothing anywhere in the
/// output — not one band, not one propagation variant. That is stronger than
/// "this term is zero here", and it is what lets a prune written against this
/// floor be gated byte-identical rather than to a dB tolerance.
///
/// Converged on independently three times on 2026-08-04: from the δ*-free
/// floor (this entry), from the CUDA cell prune, and from the favourable-arm
/// identity. Three routes, one number.
///
/// # THE definition — every other appearance derives from this line
///
/// * The CUDA surface kernel's `ARC_PENUMBRA_FLOOR_M` and `ARC_DELTA_REJECT` come from
///   `noise-gpu/build.rs`, which mirrors THIS EXPRESSION (reading
///   [`SPEED_OF_SOUND`] and dividing) and injects the result via `-D`.
/// * `obstacle_index::cell_prune_floor_m` reads it directly.
///
/// It had five hand copies of `340/63/20` across two languages until 2026-08-08.
/// Nine copies of the ground-term formula is how that term went missing from
/// eight of them; do not start a sixth.
pub const PENUMBRA_DELTA_FLOOR_M: f64 = -SPEED_OF_SOUND / 63.0 / 20.0;

/// Default receiver height [m] — END 2002/49/EC facade standard (4.0m).
/// Was 1.5m (human ear). Changed to 4.0m to match EU strategic noise mapping
/// and eliminate systematic -3 dB bias vs SHM across all sources.
pub const DEFAULT_RECEIVER_HEIGHT: f64 = 4.0;

/// Favourable propagation probability (CNOSSOS-EU §2.5.21, Central Europe).
/// One value for all periods (owner 2026-07-28: no per-period p).
pub const P_FAV: f64 = 0.5;

/// Master switch for CNOSSOS long-term favourable/homogeneous mixing
/// (2015/996 formulas (2.5.9), (2.5.24), (2.5.25)). FLIPPED ON
/// 2026-07-28 after the gates passed (G3: 7 anchors moved toward
/// external truth, none regressed beyond pre-existing near-barrier
/// overshoots; G6: GPU gate pass, drift mean 0.004 dB). Flipping raises
/// every terrain/building screened receiver, so any future change here
/// travels with a surface-layer OUTPUT_VER bump + world repaint + the
/// CUDA surface-kernel #define mirror — never alone.
pub const FAVOURABLE_MIXING: bool = true;

/// CNOSSOS-EU (2.5.24) favourable-ray curvature Γ = max(Γ_MIN, Γ_PER_DSR·d),
/// d = slant source→receiver distance.
pub const FAV_RAY_CURVATURE_MIN_M: f64 = 1000.0;
pub const FAV_RAY_CURVATURE_PER_DSR: f64 = 8.0;

/// Diffraction attenuation cap [dB] (single-edge model).
pub const SINGLE_DIFF_CAP: f64 = 20.0;

/// Source heights [m].
pub const SOURCE_HEIGHT_ROAD: f64 = 0.05; // CNOSSOS-EU §2.4.1
pub const SOURCE_HEIGHT_RAIL: f64 = 0.5; // CNOSSOS-EU §2.7.1
pub const SOURCE_HEIGHT_LEISURE: f64 = 1.5;

/// Flat-earth geo math lives only in `grid::geo` (no H3 anywhere) — re-exported
/// here so old `constants::` paths keep working.
pub use grid::geo::{m_per_deg_lon, M_PER_DEG_LAT};

/// Meters per degree of longitude at the equator (multiply by `cos(lat)` for
/// a given latitude via [`m_per_deg_lon`]) — re-exported, single home is
/// `grid::geo`.
pub use grid::geo::M_PER_DEG_LON_EQ;
/// Fallback building height (m) when a footprint has neither a mapped height
/// nor a floor count — the last rung of the building height ladder
/// (`height` → `floors × BUILDING_FLOOR_HEIGHT_M` → this).
/// 8 m ≈ 2–3 storeys at `BUILDING_FLOOR_HEIGHT_M`, the dominant residential
/// building form, and matches the engine's long-standing emission fallback so
/// screening and emission agree on unmapped buildings.
pub const BUILDING_DEFAULT_HEIGHT_M: f64 = 8.0;

/// Maximum physical building height (m). The Burj Khalifa is the tallest
/// building on Earth at 828 m; any larger mapped value is a tag error.
/// Applied during engine normalization so extracted source data stays raw.
pub const BUILDING_HEIGHT_MAX_M: f64 = 828.0;

/// Storey height (m) for `building:levels` / Overture `num_floors` × N
/// height conversions — the middle rung of the building height ladder.
/// Canonical here, mirrored by the shell rasterizer alongside
/// [`BUILDING_DEFAULT_HEIGHT_M`].
pub const BUILDING_FLOOR_HEIGHT_M: f64 = 3.0;

/// Half-edge of the receiver-enclosure 3×3 probe footprint (m) — a metric
/// 150 × 150 m isotropic square. CANONICAL here since the vector enclosure
/// (obstacle_index::enclosure_db) joined the raster probe (raster-reader
/// re-exports it); popup and pipeline must probe the identical footprint.
pub const ENCLOSURE_RADIUS_M: f64 = 75.0;

/// CNOSSOS road emission reference speed [km/h].
pub const V_REF_ROAD: f64 = 70.0;

/// Heavy vehicle speed cap [km/h] (Czech legal requirement, consistent with CNOSSOS).
pub const HEAVY_SPEED_CAP: f64 = 80.0;

/// Road surface corrections [dB] applied to rolling noise only.
pub const SURFACE_CORR: [f64; 5] = [
    0.0, // 0: asphalt (reference)
    4.0, // 1: sett/cobblestone
    4.0, // 2: cobblestone/paving stones
    1.0, // 3: concrete
    2.0, // 4: gravel/unpaved
];

// ── Source max-radius cutoffs (meters) ──────────────────────────
// Single source of truth: pipeline (soa.rs), normalize.rs, and
// source-reader (popup) all reference these constants.

/// Industrial point source max radius.
pub const INDUSTRIAL_MAX_RADIUS: f64 = 4_000.0;

/// Aircraft ground ops max radii. Per-ops_kind reach matches per-source
/// emission decay: cumulative line-source Lden at the cap boundary sits
/// ~5-15 dB (well below the 30 dB renderer floor + the ~25 dB road/rail
/// calibration boundary). Wired via `ground_ops_max_radius()` into both
/// heatmap (`ground_ops.rs`) and popup (`airport_traffic.rs`). Measured
/// LKPR 49-tile bbox: 97× wall speedup vs the prior 16 km blanket;
/// mean drift 5.4 dB, max 26 dB at outermost tile corners (mostly
/// pixels that fall under the 30 dB tile-skip floor).
pub const GROUND_OPS_RUNWAY_MAX_RADIUS: f64 = 5_000.0;
pub const GROUND_OPS_TAXI_MAX_RADIUS: f64 = 3_000.0;
pub const GROUND_OPS_APRON_MAX_RADIUS: f64 = 1_500.0;

/// Per-`ops_kind` reach. Heatmap + popup share this — single source of truth.
/// Unknown / NONE → most generous cap so we never silently drop.
/// APRON arm is reserved: current Stage 2C only emits ops_kind ∈
/// {RUNWAY_ROLL, TAXI} for line features (aprons are area features —
/// see `stage_2c/airport_traffic_writer.rs:69`). Wired anyway so a
/// future area→line synthesis doesn't silently inherit the RUNWAY cap.
#[inline]
pub fn ground_ops_max_radius(ops_kind: u8) -> f64 {
    use crate::emission::aircraft::{
        GROUND_OPS_KIND_APRON_MOVEMENT, GROUND_OPS_KIND_RUNWAY_ROLL, GROUND_OPS_KIND_TAXI,
    };
    match ops_kind {
        GROUND_OPS_KIND_RUNWAY_ROLL => GROUND_OPS_RUNWAY_MAX_RADIUS,
        GROUND_OPS_KIND_TAXI => GROUND_OPS_TAXI_MAX_RADIUS,
        GROUND_OPS_KIND_APRON_MOVEMENT => GROUND_OPS_APRON_MAX_RADIUS,
        _ => GROUND_OPS_RUNWAY_MAX_RADIUS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// At the pole, `cos(lat) = 0` would yield `m_per_deg_lon = 0`,
    /// breaking every caller that divides by it. The floor inside
    /// `m_per_deg_lon` keeps the value finite and positive.
    #[test]
    fn m_per_deg_lon_at_pole_is_finite_positive() {
        let v = m_per_deg_lon(PI / 2.0);
        assert!(v.is_finite() && v > 0.0, "m_per_deg_lon(π/2) = {v}");
    }

    /// Equator value stays unchanged by the floor (cos(0) = 1 ≫ 0.01).
    #[test]
    fn m_per_deg_lon_at_equator_unchanged_by_floor() {
        let v = m_per_deg_lon(0.0);
        assert!((v - M_PER_DEG_LON_EQ).abs() < 1e-9);
    }
}

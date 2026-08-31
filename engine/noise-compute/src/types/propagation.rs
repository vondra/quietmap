//! Propagation result types — per-source summaries and the linear-energy
//! propagation-variant struct (full / free-field / per-effect) the kernels fill.
use super::*;

/// Per-source-type summary.
#[derive(Debug, Clone, Serialize)]
pub struct SourceResult {
    pub source_type: LayerKind,
    pub periods: NoisePeriods,
    /// Free-field counterpart of `periods` (no terrain / screening / vegetation
    /// applied). Populated for road / rail / industrial from
    /// `free_field_energy` in their per-source kernels; for aircraft airborne
    /// equals `periods` (kernel has no terrain/screening); for aircraft ground
    /// ops currently equals `periods` (TODO: real free-field path from
    /// airport_traffic variants — Codex /gg #80). Was missing until 2026-05-24,
    /// causing wire field `lden_free` to always read `null`.
    pub periods_free: NoisePeriods,
    pub segment_count: usize,
    pub displayed_count: usize,
}

/// Propagation variant energies (linear, not dB).
///
/// `no_ground_energy` and `no_atmospheric_energy` are popup-only; the pipeline
/// kernel (`propagate_variants::<false>`) leaves them at 0.0 since tile output
/// never reads them. The popup kernel (`propagate_variants::<true>`) fills all
/// 7 variants so the contributor detail can show A-weighted per-effect deltas.
#[derive(Debug, Clone, Copy, Default)]
pub struct PropagationVariants {
    pub full_energy: f64,              // all effects applied
    pub free_field_energy: f64,        // div + atm + ground only (no terrain/screening/vegetation)
    pub no_terrain_energy: f64,        // full minus terrain diffraction
    pub no_screening_energy: f64,      // full minus building screening
    pub no_vegetation_energy: f64,     // full minus vegetation
    pub no_ground_energy: f64,         // full minus ground effect (popup-only; 0 on pipeline path)
    pub no_atmospheric_energy: f64, // full minus atmospheric absorption (popup-only; 0 on pipeline path)
    pub band_energy: [f64; NUM_BANDS], // per-band received levels (linear energy, A-weighted)
}

impl PropagationVariants {
    /// Element-wise accumulation (energy summation).
    #[inline]
    pub fn add(&mut self, other: &Self) {
        self.full_energy += other.full_energy;
        self.free_field_energy += other.free_field_energy;
        self.no_terrain_energy += other.no_terrain_energy;
        self.no_screening_energy += other.no_screening_energy;
        self.no_vegetation_energy += other.no_vegetation_energy;
        self.no_ground_energy += other.no_ground_energy;
        self.no_atmospheric_energy += other.no_atmospheric_energy;
        for i in 0..NUM_BANDS {
            self.band_energy[i] += other.band_energy[i];
        }
    }

    /// Scale all energy fields by a factor (linear space).
    #[inline]
    pub fn scale(&mut self, factor: f64) {
        self.full_energy *= factor;
        self.free_field_energy *= factor;
        self.no_terrain_energy *= factor;
        self.no_screening_energy *= factor;
        self.no_vegetation_energy *= factor;
        self.no_ground_energy *= factor;
        self.no_atmospheric_energy *= factor;
        for i in 0..NUM_BANDS {
            self.band_energy[i] *= factor;
        }
    }

    /// Convert energy to dB, clamped to avoid -inf.
    #[inline]
    pub fn to_db(energy: f64) -> f64 {
        10.0 * energy.max(1e-12).log10()
    }

    /// Compute Lden from day/eve/night variant energies using a field extractor.
    /// Avoids repeating `compute_lden(to_db(day.field), to_db(eve.field), to_db(night.field))`
    /// for each variant field.
    #[inline]
    pub fn lden_from_periods(day: &Self, eve: &Self, night: &Self, field: fn(&Self) -> f64) -> f64 {
        crate::periods::compute_lden(
            Self::to_db(field(day)),
            Self::to_db(field(eve)),
            Self::to_db(field(night)),
        )
    }

    /// A-weighted ΔL_A per effect for a Contributor accumulator. Terrain,
    /// screening, vegetation, and atmospheric deltas are clamped to ≤ 0
    /// (the physics says they only attenuate); ground is signed because
    /// over soft ground CF[i] < 0 at 63/125 Hz can boost LF energy.
    /// `full_lden_db` is the caller's already-computed full Lden.
    #[inline]
    pub fn impact_deltas(variants: &[Self; 3], full_lden_db: f64) -> ImpactDeltas {
        let delta = |f: fn(&Self) -> f64| {
            full_lden_db - Self::lden_from_periods(&variants[0], &variants[1], &variants[2], f)
        };
        ImpactDeltas {
            terrain: delta(|v| v.no_terrain_energy).min(0.0),
            screening: delta(|v| v.no_screening_energy).min(0.0),
            vegetation: delta(|v| v.no_vegetation_energy).min(0.0),
            atmospheric: delta(|v| v.no_atmospheric_energy).min(0.0),
            ground: delta(|v| v.no_ground_energy),
        }
    }
}

/// A-weighted ΔL_A per propagation effect. `ground` is signed; the others are
/// ≤ 0 (effects only attenuate in the energy-aggregate).
#[derive(Debug, Clone, Copy, Default)]
pub struct ImpactDeltas {
    pub terrain: f64,
    pub screening: f64,
    pub vegetation: f64,
    pub atmospheric: f64,
    pub ground: f64,
}

/// Propagation baseline fields tied to the contributor's CLOSEST segment.
///
/// Per-effect A-weighted impact scalars live on [`Contributor`] itself
/// (`atmospheric_impact_db`, `ground_impact_db`) because they are
/// energy-weighted averages over every grouped segment — not a
/// closest-segment property.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PropagationBaseline {
    pub geometric_db: f64,  // divergence loss at closest segment (negative)
    pub ground_factor: f64, // G value at closest segment (0-1) — display only
}

/// Terrain diffraction metadata (path-profile telemetry). The A-weighted
/// impact scalar lives on [`Contributor::terrain_impact_db`] since it is
/// derived from the variant Lden across all grouped segments.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TerrainBreakdown {
    pub delta_m: f64, // path difference (meters)
    /// Transparency metadata: terrain profile sample count (0 if no hill detected).
    pub profile_points: u32,
}

/// The single exact vector crossing that beat the terrain edge on signed δ.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ObstacleEdge {
    pub kind: &'static str, // "building" | "barrier"
    pub t: f64,             // fractional position along path (0..1)
    pub height_m: f64,      // building or barrier height above ground
    pub screen_h_m: f64,    // edge-top minus line-of-sight (excess above LOS)
    /// Query-local ordinal, deterministic for one on-disk state but not a
    /// durable store identity.
    pub obstacle_id: u32,
}

/// Screening trace for the one vector crossing that beat bare terrain.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScreeningObstacleTrace {
    pub delta_m: f64,
    /// Median terrain-profile sampling step used to interpolate ground under
    /// the exact crossing.
    pub step_m: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge: Option<ObstacleEdge>,
}

/// Building screening metadata. The A-weighted impact scalar lives on
/// [`Contributor::screening_impact_db`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScreeningBreakdown {
    /// Engine-internal obstacle trace. Populated on popup path (compute_path_effects),
    /// not on pipeline hot path (screening_attenuation). Serialized only when Some.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obstacle: Option<ScreeningObstacleTrace>,
}

/// Vegetation path metadata. The A-weighted impact scalar lives on
/// [`Contributor::vegetation_impact_db`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct VegetationBreakdown {
    pub forest_depth_m: f64, // cumulative forest depth (meters)
    /// Transparency: path length actually sampled for forest (metres).
    /// 0 when the segment was beyond the model's applicable range.
    pub sampled_path_m: f64,
}

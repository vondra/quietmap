//! One flattened airborne row through the envelope, reach and terrain gates and the Doc 29 kernel.

use grid::geo::wrapped_longitude_delta;

use crate::compute::aircraft_v6::state::FlightAccum;
use crate::compute::aircraft_v6::views::AirborneSegmentBatch;
use crate::emission::aircraft::{self, AircraftKernelResult, CpaResult};
use crate::propagation::iso9613::fast_exp_f64;
use crate::types::{AircraftSegment, Receiver, SegmentTrace};

/// Receiver-side state every row of one click shares.
pub(super) struct ScatterContext<'a> {
    pub receiver: &'a Receiver,
    pub rx_elev: f64,
    pub npd_luts: &'a aircraft::NpdLuts,
    /// The geographic selection is a periodic 16 km axis envelope, not a
    /// circle or per-class reach: unclamped line CPA can retain corner rows.
    pub envelope: aircraft::AirborneEnvelope,
    /// Receiver-relative metre factors, computed once. `m_per_deg_lon`
    /// expects radians; the lat factor mirrors the kernel's
    /// `aircraft::M_PER_DEG_LAT` so the line-distance prefilter is
    /// bit-identical to `segment_sel_with_overrides`.
    pub rx_m_per_lon: f64,
    pub rx_m_per_lat: f64,
    pub class_weights: &'a aircraft::ClassWeights,
    pub horizon: &'a aircraft::ReceiverHorizon,
    pub buildings: Option<&'a aircraft::BuildingHorizon>,
    pub n_days_f: f64,
}

impl<'a> ScatterContext<'a> {
    pub fn new(
        receiver: &'a Receiver,
        n_days_f: f64,
        class_weights: &'a aircraft::ClassWeights,
        horizon: &'a aircraft::ReceiverHorizon,
        buildings: Option<&'a aircraft::BuildingHorizon>,
    ) -> Self {
        let cos_lat = receiver.lat.to_radians().cos().max(0.2);
        Self {
            receiver,
            rx_elev: receiver.altitude_m(),
            npd_luts: aircraft::NpdLuts::shared(),
            envelope: aircraft::AirborneEnvelope::new(receiver.lat, receiver.lon),
            rx_m_per_lon: aircraft::M_PER_DEG_LAT * cos_lat,
            rx_m_per_lat: aircraft::M_PER_DEG_LAT,
            class_weights,
            horizon,
            buildings,
            n_days_f,
        }
    }
}

/// Kernel result of one row plus the display metrics derived from it.
pub(super) struct RowKernel {
    pub seg: AircraftSegment,
    pub kernel: AircraftKernelResult,
    pub class_idx: usize,
    /// GA hybrid weight of the row's class, already folded into the four
    /// energies below and carried as the flight's count weight.
    pub class_weight: f64,
    pub period: usize,
    pub energy: f64,
    pub free_energy: f64,
    pub no_terrain_energy: f64,
    pub no_screening_energy: f64,
    pub cpa: CpaResult,
    /// CLAMPED CPA — the closest the aircraft actually reaches on this
    /// segment — so a curving track's extrapolated foot cannot report a
    /// phantom near pass (`clamped_display_cpa`); SEL keeps the unclamped CPA.
    pub disp_dist: f64,
    pub disp_alt: f64,
    pub lmax: f64,
}

impl RowKernel {
    /// The 20 dB received floor the popup applies to one event.
    pub fn below_event_floor(&self) -> bool {
        self.kernel.sel < 20.0
    }

    /// Fold this row into its flight's received accumulator: energy, the
    /// peak-Lmax record (strict `>` so the earlier row wins a tie) and the
    /// closest approach.
    pub fn apply_received(&self, acc: &mut FlightAccum) {
        acc.period_energy[self.period] += self.energy;
        if self.lmax > acc.peak_lmax {
            acc.peak_lmax = self.lmax;
            acc.peak_sel = self.kernel.sel;
            acc.peak_altitude_m = self.disp_alt;
            acc.peak_period = self.seg.period;
            acc.peak_date_id = self.seg.date_id;
            acc.peak_seg_start = [self.seg.start_lon, self.seg.start_lat];
            acc.peak_seg_end = [self.seg.end_lon, self.seg.end_lat];
        }
        if self.disp_dist < acc.min_dist_m {
            acc.min_dist_m = self.disp_dist;
        }
    }

    pub fn apply_variants(&self, acc: &mut FlightAccum) {
        acc.free_period_energy[self.period] += self.free_energy;
        acc.no_terrain_period_energy[self.period] += self.no_terrain_energy;
        acc.no_screening_period_energy[self.period] += self.no_screening_energy;
    }
}

/// The flight accumulator a row belongs to, created from the file's flight
/// table when the flight first contributes.
pub(super) fn flight_accumulator<'m>(
    flights: &'m mut std::collections::HashMap<u64, FlightAccum>,
    batch: &AirborneSegmentBatch<'_>,
    row: usize,
    class_weight: f64,
) -> &'m mut FlightAccum {
    let key = batch.flight_key[row] as usize;
    flights.entry(batch.flight_id[row]).or_insert_with(|| {
        FlightAccum::new(
            batch.flights.profile_idx[key],
            class_weight,
            false,
            batch.flights.aircraft_type(key),
            batch.flights.callsign(key).to_string(),
        )
    })
}

/// Evaluate row `i` of `batch`: `None` when the envelope, the class reach,
/// the stale-ground gate or the kernel rejects it. `FLOOR` applies the
/// kernel's 20 dB event floor; a split piece runs unfloored and is floored
/// with its chord (`chords`).
pub(super) fn evaluate_row<const FLOOR: bool>(
    ctx: &ScatterContext<'_>,
    batch: &AirborneSegmentBatch<'_>,
    i: usize,
) -> Option<RowKernel> {
    let identity = &batch.flights;
    let key = batch.flight_key[i] as usize;
    let profile_idx = identity.profile_idx[key];
    // Per-class reach for the per-direction line-distance gate below. The
    // kernel uses the same `REACH_SQ_TABLE[class_idx][is_dep]` internally
    // (`segment_sel.rs:196`), so rejecting earlier on `cross² > reach² ×
    // len²` (unclamped line-distance squared, matches `doc29.rs:359`
    // exactly) cannot false-reject anything the kernel would accept.
    let class_idx = aircraft::noise_class_of(profile_idx) as usize;
    let reach_sq_class = aircraft::REACH_SQ_TABLE[class_idx];
    let class_weight = ctx.class_weights.get(class_idx as u8);
    // Unlike aggregate min/max bounds, these endpoints identify the short
    // arc used by the kernel and by the batch envelope gate.
    let [s_lat_f, s_lon_f] = batch.start_lat_lon(i);
    let [e_lat_f, e_lon_f] = batch.end_lat_lon(i);
    if !ctx
        .envelope
        .intersects_segment([s_lat_f, s_lon_f], [e_lat_f, e_lon_f])
    {
        return None;
    }
    let receiver = ctx.receiver;
    let ax = wrapped_longitude_delta(receiver.lon, s_lon_f as f64) * ctx.rx_m_per_lon;
    let ay = (s_lat_f as f64 - receiver.lat) * ctx.rx_m_per_lat;
    let by = (e_lat_f as f64 - receiver.lat) * ctx.rx_m_per_lat;
    let sdx = wrapped_longitude_delta(s_lon_f as f64, e_lon_f as f64) * ctx.rx_m_per_lon;
    let sdy = by - ay;
    let seg_len_sq = sdx * sdx + sdy * sdy;
    let flags = batch.flags[i];
    let is_departure = flags & 0b001 != 0;
    // Degenerate sub-segments are covered by the envelope check alone.
    if seg_len_sq > 1.0 {
        let cross = ax * sdy - ay * sdx;
        if cross * cross > reach_sq_class[is_departure as usize] * seg_len_sq {
            return None;
        }
    }
    // `ground_context = NONE` for every airborne sub-segment: Stage 1's
    // ground inference already routes near-airport low-AGL points to the
    // ground path (measured 2026-05-23, 0.000 dB on five reference receivers).
    let seg = AircraftSegment {
        flight_id: batch.flight_id[i],
        profile_idx,
        is_departure,
        on_ground: false,
        period: batch.period[i],
        date_id: batch.date_id[i],
        start_lat: s_lat_f as f64,
        start_lon: s_lon_f as f64,
        start_alt_m: f32::from(batch.start_alt_m[i]),
        end_lat: e_lat_f as f64,
        end_lon: e_lon_f as f64,
        end_alt_m: f32::from(batch.end_alt_m[i]),
        speed_kt: batch.speed_kt[i],
        segment_length_m: batch.length_m[i],
        count_weight: 1.0,
        surface_model: false,
        ground_context: aircraft::GROUND_CONTEXT_NONE,
        ground_ops_kind: aircraft::GROUND_OPS_KIND_NONE,
        source_id: identity.source_id[key] as u16,
    };
    // Only start/end terrain elevations are stored: the endpoint stale-ground
    // gate and Filter D's receiver-dependent cuts run from them.
    let start_elev = batch.terrain_start_elev_m[i] as f64;
    let end_elev = batch.terrain_end_elev_m[i] as f64;
    let terrain = aircraft::SegmentTerrain {
        start_elev,
        q1_elev: 0.0,
        mid_elev: 0.0,
        q3_elev: 0.0,
        end_elev,
    };
    if aircraft::is_ground_stale_with_terrain(&seg, &terrain) {
        return None;
    }
    let kernel = aircraft::segment_kernel_with_cuts::<FLOOR>(
        &seg,
        receiver.lat,
        receiver.lon,
        ctx.rx_elev,
        start_elev - 30.0,
        end_elev - 30.0,
        ctx.npd_luts,
        ctx.horizon,
        ctx.buildings,
    )?;
    // The GA hybrid weight is folded into every energy here so each
    // downstream consumer sees the `1/ga_n_days`-scaled value.
    let energy_for_sel =
        |sel: f64| fast_exp_f64(sel * std::f64::consts::LN_10 * 0.1) * class_weight;
    let cpa = CpaResult {
        q_m: kernel.q_m,
        d_p_m: kernel.d_p_m,
        lateral_m: kernel.lateral_m,
        relative_alt_m: kernel.rel_alt_m,
        beta_deg: kernel.beta_deg,
        seg_len_m: kernel.seg_len_m,
        t: kernel.t,
    };
    let sdz = seg.end_alt_m as f64 - seg.start_alt_m as f64;
    let (disp_dist, disp_alt) = aircraft::clamped_display_cpa(&cpa, sdz);
    // log2 × LOG10_2 ≡ log10 at f64; matches the kernel's NPD-lookup idiom.
    let log_d = (disp_dist * aircraft::FT_PER_M).max(100.0).log2() * std::f64::consts::LOG10_2;
    let lmax = ctx.npd_luts.lookup_lmax(class_idx, seg.is_departure, log_d);
    Some(RowKernel {
        period: (seg.period.min(2)) as usize,
        energy: energy_for_sel(kernel.sel),
        free_energy: energy_for_sel(kernel.free_sel),
        no_terrain_energy: energy_for_sel(kernel.sel_no_terrain),
        no_screening_energy: energy_for_sel(kernel.sel_no_screening),
        seg,
        kernel,
        class_idx,
        class_weight,
        cpa,
        disp_dist,
        disp_alt,
        lmax,
    })
}

/// The Noise Segments row of one evaluated sub-segment.
pub(super) fn build_row_trace(
    ctx: &ScatterContext<'_>,
    batch: &AirborneSegmentBatch<'_>,
    i: usize,
    row: &RowKernel,
) -> SegmentTrace {
    let kernel = &row.kernel;
    let mut period_energies = [0.0f64; 3];
    period_energies[row.period] = row.energy;
    let mut free_period_energies = [0.0f64; 3];
    free_period_energies[row.period] = row.free_energy;
    let mut no_terrain_period_energies = [0.0f64; 3];
    no_terrain_period_energies[row.period] = row.no_terrain_energy;
    let mut no_screening_period_energies = [0.0f64; 3];
    no_screening_period_energies[row.period] = row.no_screening_energy;
    let (screening_kind, screening_db) = if kernel.terrain_dz <= 0.0 && kernel.building_dz <= 0.0 {
        ("none", 0.0)
    } else if kernel.terrain_dz >= kernel.building_dz {
        ("terrain", kernel.terrain_dz)
    } else {
        ("building", kernel.building_dz)
    };
    let installation = match kernel.installation {
        aircraft::Installation::Wing => "wing",
        aircraft::Installation::Fuselage => "fuselage",
        aircraft::Installation::Propeller => "propeller",
    };
    let doc29 = crate::types::Doc29Breakdown {
        sel_npd_db: kernel.sel_npd_db,
        delta_v_db: kernel.delta_v_db,
        delta_i_db: kernel.delta_i_db,
        lambda_db: kernel.lambda_db,
        delta_f_db: kernel.delta_f_db,
        d_p_m: row.cpa.d_p_m,
        lateral_m: row.cpa.lateral_m,
        beta_deg: row.cpa.beta_deg,
        seg_len_m: row.seg.segment_length_m as f64,
        d_bar_m: kernel.d_bar_m,
        installation,
        cffk_fast_path: kernel.cffk_fast_path,
        screening_kind,
        screening_db,
    };
    let key = batch.flight_key[i] as usize;
    let aircraft_type = batch.flights.aircraft_type(key);
    crate::traces::build_aircraft_airborne_subsegment_trace(
        crate::traces::BuildAircraftAirborneSubSegmentTrace {
            callsign: batch.flights.callsign(key),
            aircraft_type: &aircraft_type,
            class_name: aircraft::CLASS_NAMES[row.class_idx],
            flight_id: row.seg.flight_id,
            start_lat: row.seg.start_lat,
            start_lon: row.seg.start_lon,
            end_lat: row.seg.end_lat,
            end_lon: row.seg.end_lon,
            cpa_distance_m: row.disp_dist,
            altitude_m_at_cpa: row.disp_alt,
            d_slant_m: row.disp_dist.max(1.0),
            is_departure: row.seg.is_departure,
            period_energies,
            free_period_energies,
            no_terrain_period_energies,
            no_screening_period_energies,
            n_days: ctx.n_days_f,
            doc29,
        },
    )
}

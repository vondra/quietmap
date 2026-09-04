//! Weighted cruise statistics and deterministic bounded candidate unions.

use super::*;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub(super) struct CruiseKey {
    pub(super) cruise_cell_id: u64,
    pub(super) class: u8,
    pub(super) fl_bin: u8,
    pub(super) period: u8,
}

/// Per-bucket worker accumulator (v14). `fid_set` tracks the full
/// `unique_count`; `top` keeps a bounded top-K snapshot ranked by
/// source-side peak Lmax at 25 m. Tail fids beyond K=`CRUISE_TOP_K`
/// drop out of band counters at the popup but still contribute to
/// `unique_count`.
#[derive(Clone)]
pub(super) struct CruiseAccum {
    pub(super) sum_length_m: f32,
    pub(super) rep_len_m: f32, // weighted mean of original segment lengths
    pub(super) rep_len_w: f32, // weight accumulator
    pub(super) rep_alt_m: f32,
    pub(super) rep_speed_kt: f32,
    pub(super) weight: f32,
    pub(super) rep_profile_idx: u8,
    /// Distinct fids that touched this bucket. `.len()` → `unique_count`.
    pub(super) fid_set: std::collections::HashSet<u64>,
    /// Bounded top-K candidates keyed on fid for O(1) re-entrance.
    /// A linear scan over K=50 entries finds the min for eviction —
    /// cheap at this size vs the constant factor of a BTreeMap.
    pub(super) top: HashMap<u64, CruiseTopCandidate>,
    /// Smallest `peak_lmax_25m_db` currently in `top`. Avoids the
    /// full scan when a new candidate is below the cap.
    pub(super) top_min_lmax: f32,
    pub(super) source_id: u8,
    pub(super) origin: u8,
}

impl Default for CruiseAccum {
    fn default() -> Self {
        Self {
            sum_length_m: 0.0,
            rep_len_m: 0.0,
            rep_len_w: 0.0,
            rep_alt_m: 0.0,
            rep_speed_kt: 0.0,
            weight: 0.0,
            rep_profile_idx: 0,
            fid_set: std::collections::HashSet::new(),
            top: HashMap::new(),
            top_min_lmax: f32::NEG_INFINITY,
            source_id: 0,
            origin: 0,
        }
    }
}

fn candidate_rank(a: &CruiseTopCandidate, b: &CruiseTopCandidate) -> std::cmp::Ordering {
    a.peak_lmax_25m_db
        .total_cmp(&b.peak_lmax_25m_db)
        .then_with(|| b.flight_id.cmp(&a.flight_id))
}

impl CruiseAccum {
    pub(super) fn add(&mut self, seg: &FlightSegment, clip_len_m: f32, npd_luts: &NpdLuts) {
        self.sum_length_m += clip_len_m;
        // rep_alt / rep_speed: clip-length-weighted mean.
        let mid_alt = 0.5 * (seg.start_alt_m + seg.end_alt_m);
        self.rep_alt_m += clip_len_m * mid_alt;
        self.rep_speed_kt += clip_len_m * seg.speed_kt;
        self.weight += clip_len_m;
        // rep_len_m: weighted mean of source-segment length, used as
        // ΔF input. We weight by clip-length so a segment slicing many
        // cells contributes its full length to each cell's mean.
        self.rep_len_m += clip_len_m * seg.length_m;
        self.rep_len_w += clip_len_m;
        self.fid_set.insert(seg.flight_id);
        self.rep_profile_idx = seg.profile_idx;
        self.source_id = seg.source_id;
        self.origin = seg.origin;
        // Source-side peak Lmax at 25 m. Doc 29 §A.3.2 — cruise rows
        // use the Departure NPD curve. NPD `lookup_lmax` indexes by
        // log10(d_ft); 25 m → 82 ft → log10 ≈ 1.914.
        let class_idx = noise_class_of(seg.profile_idx) as usize;
        let log_d = log_d_25m_ft();
        let lmax_db = npd_luts.lookup_lmax(class_idx, true, log_d) as f32;
        self.update_top(seg, lmax_db, mid_alt);
    }

    /// Re-entrant top-K maintenance from a live segment. Builds a
    /// `CruiseTopCandidate` from the segment fields and delegates to
    /// [`merge_top_entry`] so the add and merge paths share one
    /// cap-K + re-entrant implementation.
    fn update_top(&mut self, seg: &FlightSegment, lmax_db: f32, altitude_m: f32) {
        self.merge_top_entry(CruiseTopCandidate {
            flight_id: seg.flight_id,
            callsign: seg.callsign.clone(),
            aircraft_type: seg.aircraft_type,
            peak_lmax_25m_db: lmax_db,
            altitude_m,
        });
    }

    /// Symmetric merge for the Stage 2B fold/reduce. Both `add` and
    /// `merge` must produce the same final accumulator state regardless
    /// of split point — tested by `merge_matches_sequential`.
    pub(super) fn merge(&mut self, other: CruiseAccum) {
        self.sum_length_m += other.sum_length_m;
        self.rep_alt_m += other.rep_alt_m;
        self.rep_speed_kt += other.rep_speed_kt;
        self.weight += other.weight;
        self.rep_len_m += other.rep_len_m;
        self.rep_len_w += other.rep_len_w;
        for fid in other.fid_set {
            self.fid_set.insert(fid);
        }
        // Replay other's top entries through the cap-K logic so the
        // final accumulator has the true top-K of the union (rev 2
        // accepts that two capped top-50 lists union to top-50 of
        // top-100 — bounded rank pollution at the Kth slot).
        for cand in other.top.into_values() {
            self.merge_top_entry(cand);
        }
        // `rep_profile_idx` / `source_id` / `origin` are NOT
        // invariant per bucket key — different `profile_idx` can map
        // to the same `class`. Both `add` and `merge` pick
        // arbitrarily; downstream remaps `profile_idx` → class so
        // the pick has no measurable effect.
    }

    /// Cap-K + re-entrant top-K maintenance. If the fid is already in
    /// `top`, lift its Lmax on the max-wins rule (loudest segment of
    /// this fid dominates display). If new and `top` is below capacity,
    /// insert. If new and at capacity, evict the current min — but
    /// only when the incoming Lmax beats it.
    pub(super) fn merge_top_entry(&mut self, cand: CruiseTopCandidate) {
        if let Some(existing) = self.top.get_mut(&cand.flight_id) {
            let candidate_wins = cand
                .peak_lmax_25m_db
                .total_cmp(&existing.peak_lmax_25m_db)
                .then_with(|| existing.altitude_m.total_cmp(&cand.altitude_m))
                .then_with(|| existing.callsign.cmp(&cand.callsign))
                .then_with(|| existing.aircraft_type.cmp(&cand.aircraft_type))
                .is_gt();
            if candidate_wins {
                *existing = cand;
                self.recompute_top_min_lmax();
            }
            return;
        }
        if self.top.len() < CRUISE_TOP_K {
            let lmax = cand.peak_lmax_25m_db;
            self.top.insert(cand.flight_id, cand);
            if lmax < self.top_min_lmax || self.top.len() == 1 {
                self.top_min_lmax = lmax;
            }
            return;
        }
        if cand.peak_lmax_25m_db < self.top_min_lmax {
            return;
        }
        let victim = self
            .top
            .values()
            .min_by(|a, b| candidate_rank(a, b))
            .expect("full top set");
        if candidate_rank(&cand, victim).is_le() {
            return;
        }
        let victim_fid = victim.flight_id;
        self.top.remove(&victim_fid);
        self.top.insert(cand.flight_id, cand);
        self.recompute_top_min_lmax();
    }

    /// Reset `top_min_lmax` to the current minimum across `top`.
    /// Called after any operation that could have removed or
    /// lifted the previous min.
    fn recompute_top_min_lmax(&mut self) {
        self.top_min_lmax = self
            .top
            .values()
            .map(|c| c.peak_lmax_25m_db)
            .fold(f32::INFINITY, f32::min);
    }

    pub(super) fn finalize(self, key: CruiseKey) -> CruiseBucket {
        let w = self.weight.max(1e-6);
        let lw = self.rep_len_w.max(1e-6);
        let unique_count = self.fid_set.len() as u32;
        let mut top_candidates: Vec<CruiseTopCandidate> = self.top.into_values().collect();
        // Sort by Lmax descending (tiebreak fid ascending) so on-disk
        // bytes stay deterministic across re-extracts.
        top_candidates.sort_by(|a, b| {
            b.peak_lmax_25m_db
                .partial_cmp(&a.peak_lmax_25m_db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.flight_id.cmp(&b.flight_id))
        });
        // Doc 29 §A.3.2 — cruise rows always use the Departure NPD curve
        // (en-route flights inherit Departure NPD; no cruise NPD set
        // is published). The kernel hardcodes `is_departure: true` on
        // the synth `AircraftSegment` it builds from each row, so no
        // per-row column is needed.
        CruiseBucket {
            cruise_cell_id: key.cruise_cell_id,
            class: key.class,
            rep_profile_idx: self.rep_profile_idx,
            fl_bin: key.fl_bin,
            period: key.period,
            sum_length_m: self.sum_length_m,
            rep_len_m: self.rep_len_m / lw,
            rep_alt_m: self.rep_alt_m / w,
            rep_speed_kt: self.rep_speed_kt / w,
            unique_count,
            top_candidates,
            source_id: self.source_id,
            origin: self.origin,
        }
    }
}

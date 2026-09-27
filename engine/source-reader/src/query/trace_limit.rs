//! Limit popup traces per source kind and aircraft tab.

pub(crate) fn apply_segment_top_k_with_cap(
    traces: &mut noise_compute::types::TraceCollector,
    per_kind_cap: usize,
) -> noise_compute::types::SegmentTracesSummary {
    use noise_compute::types::{LayerKind, SegmentTracesSummary};

    let mut summary = SegmentTracesSummary {
        total_count: traces.segments.len() as u32,
        truncated: false,
        ..Default::default()
    };

    // Aircraft tabs have independent budgets within the popup-wide cap.
    let aircraft_subtype_bucket = |seg: &noise_compute::types::SegmentTrace| -> Option<u8> {
        if seg.kind != LayerKind::Aircraft {
            return None;
        }
        match seg.aircraft_subtype {
            1 => Some(1),
            2 => Some(2),
            3 => Some(3),
            _ => None,
        }
    };

    let mut per_kind_total: std::collections::HashMap<LayerKind, u32> =
        std::collections::HashMap::new();
    let mut aircraft_ground_total = 0u32;
    let mut aircraft_cruise_total = 0u32;
    for seg in &traces.segments {
        *per_kind_total.entry(seg.kind).or_insert(0) += 1;
        match aircraft_subtype_bucket(seg) {
            Some(1) => aircraft_ground_total += 1,
            // Airborne subseg total comes from `traces.airborne_above_cutoff`
            // (maintained by airborne::scatter). With the bounded min-heap
            // most candidates never reach `traces.segments`, so the Vec
            // length is no longer a valid denominator.
            Some(2) => {}
            Some(3) => aircraft_cruise_total += 1,
            _ => {}
        }
    }
    // Kernels that pre-select (ground always; cruise when `trace_cap` is
    // set; road/rail/point once they do) report kept-row counts; a zero
    // report means pre-selection stayed off and the pushed count stands.
    // `max` is exact either way: reported kept >= pushed survivors.
    let pushed_road = *per_kind_total.get(&LayerKind::Road).unwrap_or(&0);
    let pushed_railway = *per_kind_total.get(&LayerKind::Railway).unwrap_or(&0);
    let pushed_building = *per_kind_total.get(&LayerKind::Building).unwrap_or(&0);
    let pushed_industrial = *per_kind_total.get(&LayerKind::Industrial).unwrap_or(&0);
    let pushed_ship = *per_kind_total.get(&LayerKind::Ship).unwrap_or(&0);
    summary.road_total = traces.road_total.max(pushed_road);
    summary.railway_total = traces.railway_total.max(pushed_railway);
    summary.aircraft_ground_total = traces.aircraft_ground_total.max(aircraft_ground_total);
    summary.building_total = traces.building_total.max(pushed_building);
    summary.industrial_total = traces.industrial_total.max(pushed_industrial);
    summary.ship_total = traces.ship_total.max(pushed_ship);
    summary.aircraft_airborne_total = traces.airborne_above_cutoff;
    summary.aircraft_cruise_total = traces.aircraft_cruise_total.max(aircraft_cruise_total);
    // Kept-totals, not pushed: pre-selected rows never reached the vec.
    // (Ground was already pre-capped at emission, so the base undercounted
    // it here; the denominator fix surfaces the true kept count.)
    summary.total_count = (traces.segments.len() as u32)
        .saturating_add(summary.road_total.saturating_sub(pushed_road))
        .saturating_add(summary.railway_total.saturating_sub(pushed_railway))
        .saturating_add(
            summary
                .aircraft_ground_total
                .saturating_sub(aircraft_ground_total),
        )
        .saturating_add(summary.building_total.saturating_sub(pushed_building))
        .saturating_add(summary.industrial_total.saturating_sub(pushed_industrial))
        .saturating_add(summary.ship_total.saturating_sub(pushed_ship))
        .saturating_add(
            summary
                .aircraft_cruise_total
                .saturating_sub(aircraft_cruise_total),
        );

    // Total order (Lden desc, kind, subtype, stable row index): the exact
    // order the in-kernel pre-selections rank by, so their survivors are
    // precisely this cap's — unlike the old Lden-only unstable sort, whose
    // tied order was input order (a pdqsort artifact). No NaN is possible
    // here (`received_lden.full` is always finite on pushed traces).
    traces
        .segments
        .sort_unstable_by(noise_compute::types::cmp_traces_for_top_k);

    let mut per_kind: std::collections::HashMap<LayerKind, u32> = std::collections::HashMap::new();
    let mut aircraft_ground_count = 0u32;
    let mut aircraft_airborne_subseg_count = 0u32;
    let mut aircraft_cruise_count = 0u32;
    let mut truncated = false;
    traces.segments.retain_mut(|seg| {
        let count = match aircraft_subtype_bucket(seg) {
            Some(1) => &mut aircraft_ground_count,
            Some(2) => &mut aircraft_airborne_subseg_count,
            Some(3) => &mut aircraft_cruise_count,
            _ => per_kind.entry(seg.kind).or_insert(0),
        };
        let cap_ok = (*count as usize) < per_kind_cap;
        if cap_ok {
            *count += 1;
        }
        if !cap_ok {
            truncated = true;
        }
        cap_ok
    });
    summary.truncated = truncated;

    summary.road_count = *per_kind.get(&LayerKind::Road).unwrap_or(&0);
    summary.railway_count = *per_kind.get(&LayerKind::Railway).unwrap_or(&0);
    summary.aircraft_ground_count = aircraft_ground_count;
    summary.building_count = *per_kind.get(&LayerKind::Building).unwrap_or(&0);
    summary.industrial_count = *per_kind.get(&LayerKind::Industrial).unwrap_or(&0);
    summary.ship_count = *per_kind.get(&LayerKind::Ship).unwrap_or(&0);
    summary.aircraft_airborne_count = aircraft_airborne_subseg_count;
    summary.aircraft_cruise_count = aircraft_cruise_count;
    // In-kernel pre-selection drops kept rows before they reach
    // `traces.segments`, so the cap-loop above never sees them and can't
    // flip `truncated` there. Detect it by comparing kept-totals against
    // returned counts (airborne included).
    let reported_total = summary.road_total as u64
        + summary.railway_total as u64
        + summary.aircraft_ground_total as u64
        + summary.building_total as u64
        + summary.industrial_total as u64
        + summary.ship_total as u64
        + summary.aircraft_airborne_total as u64
        + summary.aircraft_cruise_total as u64;
    let returned_total = summary.road_count as u64
        + summary.railway_count as u64
        + summary.aircraft_ground_count as u64
        + summary.building_count as u64
        + summary.industrial_count as u64
        + summary.ship_count as u64
        + summary.aircraft_airborne_count as u64
        + summary.aircraft_cruise_count as u64;
    if reported_total > returned_total {
        summary.truncated = true;
    }

    summary
}

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
    summary.road_total = *per_kind_total.get(&LayerKind::Road).unwrap_or(&0);
    summary.railway_total = *per_kind_total.get(&LayerKind::Railway).unwrap_or(&0);
    summary.aircraft_ground_total = aircraft_ground_total;
    summary.building_total = *per_kind_total.get(&LayerKind::Building).unwrap_or(&0);
    summary.industrial_total = *per_kind_total.get(&LayerKind::Industrial).unwrap_or(&0);
    summary.ship_total = *per_kind_total.get(&LayerKind::Ship).unwrap_or(&0);
    summary.aircraft_airborne_total = traces.airborne_above_cutoff;
    summary.aircraft_cruise_total = aircraft_cruise_total;

    traces.segments.sort_unstable_by(|a, b| {
        b.received_lden
            .full
            .partial_cmp(&a.received_lden.full)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

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
    // Airborne pre-capping in `airborne::scatter` drops most above-cutoff
    // candidates before they reach `traces.segments`, so the cap-loop
    // above never sees them and can't flip `truncated` for that case.
    // Detect it here by comparing total above-cutoff vs returned count.
    if traces.airborne_above_cutoff > summary.aircraft_airborne_count {
        summary.truncated = true;
    }

    summary
}

//! Ground energy and movement unions, with one emitted owner per microsegment.
use super::*;

pub(super) fn accumulate_segment(
    seg: &FlightSegment,
    (owner, line_index): (u64, &mut AirportLineIndex<'_>),
    cache: &SquareCache,
    counters: &mut HashMap<CounterKey, CounterAcc>,
    micro_accs: &mut HashMap<(u64, u16), MovementUnion>,
    airport_aggs: &mut HashMap<String, MovementUnion>,
    budget: &mut AllocationBudget,
) -> Result<()> {
    if cache.lines.is_empty() {
        return Ok(());
    }
    let (intersections, _) = line_index.project(
        [seg.start_lat, seg.start_lon, seg.end_lat, seg.end_lon],
        AIRPORT_LINE_SNAP_BUFFER_M,
    );
    if intersections.is_empty() {
        return Ok(());
    }
    let class_idx = if seg.veh_kind == 1 {
        seg.gse_class
    } else {
        noise_class_of(seg.profile_idx)
    };
    let is_dep = (seg.veh_kind == 0 && seg.is_departure()) as u8;
    let is_ga =
        seg.veh_kind == 0 && noise_compute::emission::aircraft::is_ga_sampled_class(class_idx);

    for hit in intersections.iter() {
        let Some(&line_idx) = cache.line_index.get(&(hit.osm_id, hit.segment_idx)) else {
            continue;
        };
        if cache.owners[line_idx] != owner {
            continue;
        }
        let line = &cache.lines[line_idx];
        let Some(ops_kind) = ops_kind_from_aeroway(line.aeroway_type) else {
            continue;
        };
        if seg.veh_kind == 1
            && (class_idx as usize) >= noise_compute::emission::gse::GSE_LW_BANDS_DB.len()
        {
            continue;
        }
        let bands = if seg.veh_kind == 0 {
            let lw = compute_aircraft_lw_per_meter_lin(
                class_idx,
                ops_kind,
                if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
                    is_dep
                } else {
                    0
                },
                seg.speed_kt,
            );
            let density = (hit.length_within_segment_m as f64) / (line.length_m as f64).max(1e-9);
            let mut out = [0.0f32; NUM_BANDS];
            for i in 0..NUM_BANDS {
                out[i] = (lw[i] as f64 * density) as f32;
            }
            out
        } else {
            compute_gse_band_energy_lin(
                class_idx,
                ops_kind,
                seg.speed_kt,
                hit.length_within_segment_m,
            )
        };
        let airport_key = &cache.airport_keys[line_idx];
        let row_is_dep_value = if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
            is_dep
        } else {
            0
        };
        let key = CounterKey {
            airport_key: airport_key.clone(),
            osm_id: line.osm_id,
            segment_idx: line.segment_idx,
            ops_kind,
            is_departure: row_is_dep_value,
            veh_kind: seg.veh_kind,
            class_idx,
            period: seg.period,
        };
        if !counters.contains_key(&key) {
            budget.reserve_hash_entry::<CounterKey, CounterAcc>(counters.len())?;
            // Sorted counter tuples, output rows, spatial batching and Arrow
            // construction coexist with the accumulator tables.
            budget.reserve(
                4 * (std::mem::size_of::<(CounterKey, CounterAcc)>()
                    + 2 * std::mem::size_of::<AirportTrafficRow>()
                    + airport_key.len()
                    + std::mem::size_of::<[f64; 4]>()
                    + 4 * std::mem::size_of::<usize>()) as u64,
            )?;
        }
        let entry = counters.entry(key).or_insert_with(|| CounterAcc {
            start_gx: line.grid.0 .0,
            start_gy: line.grid.0 .1,
            end_gx: line.grid.1 .0,
            end_gy: line.grid.1 .1,
            length_m: line.length_m,
            ..Default::default()
        });
        if !entry.fid_set.contains(&seg.flight_id) {
            anyhow::ensure!(
                entry.fid_set.len() < u32::MAX as usize,
                "ground counter exceeds UInt32"
            );
            budget.reserve_hash_entry::<u64, ()>(entry.fid_set.len())?;
            entry.fid_set.insert(seg.flight_id);
        }
        for (acc, &band) in entry.band_energy_lin.iter_mut().zip(&bands) {
            *acc += band as f64;
        }

        let identity = (line.osm_id, line.segment_idx);
        if !micro_accs.contains_key(&identity) {
            budget.reserve_hash_entry::<(u64, u16), MovementUnion>(micro_accs.len())?;
        }
        if !airport_aggs.contains_key(airport_key) {
            budget.reserve_hash_entry::<String, MovementUnion>(airport_aggs.len())?;
            budget.reserve(
                4 * (airport_key.len() + std::mem::size_of::<AirportSummaryPartRow>()) as u64,
            )?;
        }
        let flags = movements::hit_flags(is_ga, seg.veh_kind, class_idx, ops_kind, is_dep == 1);
        micro_accs.entry(identity).or_default().insert(
            seg.flight_id,
            flags & movements::MICRO_FLAGS,
            budget,
        )?;
        airport_aggs
            .entry(airport_key.clone())
            .or_default()
            .insert(seg.flight_id, flags & movements::AIRPORT_FLAGS, budget)?;
    }
    Ok(())
}

pub(super) fn counters_to_rows(
    counters: HashMap<CounterKey, CounterAcc>,
    micro_accs: &HashMap<(u64, u16), MovementUnion>,
) -> Vec<AirportTrafficRow> {
    let mut counters: Vec<_> = counters.into_iter().collect();
    counters.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut rows = Vec::with_capacity(counters.len());
    for (key, acc) in counters {
        let bands_lin: [f32; NUM_BANDS] = std::array::from_fn(|i| acc.band_energy_lin[i] as f32);
        let unique_movement_count = acc.fid_set.len() as u32;
        let runway = key.veh_kind == 0 && key.ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL;
        let unique_arr_count = if runway && key.is_departure == 0 {
            unique_movement_count
        } else {
            0
        };
        let unique_dep_count = if runway && key.is_departure == 1 {
            unique_movement_count
        } else {
            0
        };
        let unique_gse_count_per_class = std::array::from_fn(|i| {
            if key.veh_kind == 1 && i == usize::from(key.class_idx) {
                unique_movement_count
            } else {
                0
            }
        });
        let micro = micro_accs.get(&(key.osm_id, key.segment_idx));
        let count = |flag| micro.map_or(0, |m| m.count(flag));
        let microseg_unique_count = count(movements::NON_GA);
        let microseg_unique_arr_count = count(movements::ARRIVAL);
        let microseg_unique_dep_count = count(movements::DEPARTURE);
        let microseg_unique_gse_count_per_class = std::array::from_fn(|i| count(movements::GSE[i]));
        let microseg_unique_ga_count = count(movements::GA);
        let microseg_unique_ga_arr_count = count(movements::GA_ARRIVAL);
        let microseg_unique_ga_dep_count = count(movements::GA_DEPARTURE);
        rows.push(AirportTrafficRow {
            airport_key: key.airport_key,
            osm_id: key.osm_id,
            segment_idx: key.segment_idx,
            geometry_kind: GEOMETRY_KIND_LINE,
            start_gx: acc.start_gx,
            start_gy: acc.start_gy,
            end_gx: acc.end_gx,
            end_gy: acc.end_gy,
            length_m: acc.length_m,
            ops_kind: key.ops_kind,
            is_departure: key.is_departure,
            veh_kind: key.veh_kind,
            class_idx: key.class_idx,
            period: key.period,
            band_energy_lin: bands_lin,
            unique_movement_count,
            unique_arr_count,
            unique_dep_count,
            unique_gse_count_per_class,
            microseg_unique_count,
            microseg_unique_arr_count,
            microseg_unique_dep_count,
            microseg_unique_gse_count_per_class,
            microseg_unique_ga_count,
            microseg_unique_ga_arr_count,
            microseg_unique_ga_dep_count,
        });
    }
    rows
}

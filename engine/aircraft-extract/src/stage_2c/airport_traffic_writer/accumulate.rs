//! Ground energy and movement unions, with one emitted owner per microsegment.
use super::*;

pub(super) fn accumulate_segment(
    seg: &FlightSegment,
    owner: u64,
    cache: &SquareCache,
    counters: &mut HashMap<CounterKey, CounterAcc>,
    micro_accs: &mut HashMap<(u64, u16), MicrosegAcc>,
    airport_aggs: &mut HashMap<String, AirportAggregateAcc>,
) {
    if cache.lines.is_empty() {
        return;
    }
    let intersections = project_leg_onto_airport_lines(
        seg.start_lat,
        seg.start_lon,
        seg.end_lat,
        seg.end_lon,
        &cache.lines,
        AIRPORT_LINE_SNAP_BUFFER_M,
    );
    if intersections.is_empty() {
        return;
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
        let entry = counters.entry(key).or_insert_with(|| CounterAcc {
            start_gx: line.grid.0 .0,
            start_gy: line.grid.0 .1,
            end_gx: line.grid.1 .0,
            end_gy: line.grid.1 .1,
            length_m: line.length_m,
            ..Default::default()
        });
        entry.fid_set.insert(seg.flight_id);
        for (acc, &band) in entry.band_energy_lin.iter_mut().zip(&bands) {
            *acc += band as f64;
        }

        let micro_entry = micro_accs
            .entry((line.osm_id, line.segment_idx))
            .or_default();
        if is_ga {
            micro_entry.fid_set_ga.insert(seg.flight_id);
        } else {
            micro_entry.fid_set.insert(seg.flight_id);
        }
        let airport_entry = airport_aggs.entry(airport_key.clone()).or_default();
        if seg.veh_kind == 0 {
            if ops_kind == GROUND_OPS_KIND_RUNWAY_ROLL {
                if is_dep == 1 {
                    entry.fid_set_dep.insert(seg.flight_id);
                    if is_ga {
                        micro_entry.fid_set_ga_dep.insert(seg.flight_id);
                        airport_entry.ga_dep.insert(seg.flight_id);
                    } else {
                        micro_entry.fid_set_dep.insert(seg.flight_id);
                        airport_entry.dep.insert(seg.flight_id);
                    }
                } else {
                    entry.fid_set_arr.insert(seg.flight_id);
                    if is_ga {
                        micro_entry.fid_set_ga_arr.insert(seg.flight_id);
                        airport_entry.ga_arr.insert(seg.flight_id);
                    } else {
                        micro_entry.fid_set_arr.insert(seg.flight_id);
                        airport_entry.arr.insert(seg.flight_id);
                    }
                }
            }
            let ops_idx = match ops_kind {
                GROUND_OPS_KIND_RUNWAY_ROLL => 0,
                GROUND_OPS_KIND_TAXI => 1,
                GROUND_OPS_KIND_APRON_MOVEMENT => 2,
                _ => continue,
            };
            if is_ga {
                airport_entry.ga_ops_per_kind[ops_idx].insert(seg.flight_id);
            } else {
                airport_entry.ops_per_kind[ops_idx].insert(seg.flight_id);
            }
        } else if seg.veh_kind == 1 {
            let ci = class_idx as usize;
            if ci < NUM_GSE_CLASSES {
                entry.fid_set_gse_per_class[ci].insert(seg.flight_id);
                micro_entry.fid_set_gse_per_class[ci].insert(seg.flight_id);
                airport_entry.gse_per_class[ci].insert(seg.flight_id);
            }
        }
    }
}

pub(super) fn counters_to_rows(
    counters: HashMap<CounterKey, CounterAcc>,
    micro_accs: &HashMap<(u64, u16), MicrosegAcc>,
) -> Vec<AirportTrafficRow> {
    let mut rows = Vec::with_capacity(counters.len());
    for (key, acc) in counters {
        let bands_lin: [f32; NUM_BANDS] = std::array::from_fn(|i| acc.band_energy_lin[i] as f32);
        let unique_movement_count = acc.fid_set.len() as u32;
        let unique_arr_count = acc.fid_set_arr.len() as u32;
        let unique_dep_count = acc.fid_set_dep.len() as u32;
        let unique_gse_count_per_class: [u32; NUM_GSE_CLASSES] =
            std::array::from_fn(|i| acc.fid_set_gse_per_class[i].len() as u32);
        let micro = micro_accs.get(&(key.osm_id, key.segment_idx));
        let microseg_unique_count = micro.map_or(0, |m| m.fid_set.len() as u32);
        let microseg_unique_arr_count = micro.map_or(0, |m| m.fid_set_arr.len() as u32);
        let microseg_unique_dep_count = micro.map_or(0, |m| m.fid_set_dep.len() as u32);
        let microseg_unique_gse_count_per_class: [u32; NUM_GSE_CLASSES] =
            std::array::from_fn(|i| micro.map_or(0, |m| m.fid_set_gse_per_class[i].len() as u32));
        let microseg_unique_ga_count = micro.map_or(0, |m| m.fid_set_ga.len() as u32);
        let microseg_unique_ga_arr_count = micro.map_or(0, |m| m.fid_set_ga_arr.len() as u32);
        let microseg_unique_ga_dep_count = micro.map_or(0, |m| m.fid_set_ga_dep.len() as u32);
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

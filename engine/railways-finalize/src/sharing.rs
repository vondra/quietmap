//! Divide class priors among nearby physical ways, independent of child segmentation.

use crate::encode::{grid_cell_lonlat, Expanded};
use crate::merge::{share_class_defaults, STATUS_ESTIMATED};
use grid::geo::{
    m_per_deg_lon, point_to_segment, wrapped_longitude_delta, wrapped_longitude_midpoint,
    M_PER_DEG_LAT,
};
use std::collections::{HashMap, HashSet};

// Keep the existing local corridor estimate; evidence-backed traffic is never divided.
const CORRIDOR_DISTANCE_M: f64 = 50.0;
type Cell = (i64, i64);
type Point = (i32, i32);

pub(crate) fn apply_default_sharing(rows: &mut [Expanded]) {
    let mut groups: HashMap<(String, u8, u8), Vec<usize>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        if !row.corridor.is_empty() {
            groups
                .entry((row.corridor.clone(), row.rail_type, row.usage))
                .or_default()
                .push(index);
        }
    }
    for indices in groups.into_values() {
        let mut endpoints: HashMap<i64, HashMap<Point, usize>> = HashMap::new();
        let segments: Vec<_> = indices
            .iter()
            .map(|&index| {
                let row = &rows[index];
                let g = row.child.geom;
                for point in [(g.start_gx, g.start_gy), (g.end_gx, g.end_gy)] {
                    *endpoints
                        .entry(row.osm_id)
                        .or_default()
                        .entry(point)
                        .or_default() += 1;
                }
                (
                    grid_cell_lonlat(g.start_gx, g.start_gy),
                    grid_cell_lonlat(g.end_gx, g.end_gy),
                )
            })
            .collect();
        if endpoints.len() <= 1 {
            continue;
        }
        let endpoints: HashMap<_, HashSet<_>> = endpoints
            .into_iter()
            .map(|(way, counts)| {
                (
                    way,
                    counts
                        .into_iter()
                        .filter_map(|(point, count)| (count == 1).then_some(point))
                        .collect(),
                )
            })
            .collect();
        let reference_lon = segments[0].0 .0;
        let max_latitude = segments
            .iter()
            .flat_map(|(a, b)| [a.1.abs(), b.1.abs()])
            .fold(0.0, f64::max);
        // This longitude scale is no larger than any segment's distance scale,
        // so the bucket gate cannot discard a true neighbour within 50 metres.
        let longitude_scale = m_per_deg_lon(max_latitude.to_radians());
        let project = |(lon, lat): (f64, f64)| {
            [
                wrapped_longitude_delta(reference_lon, lon) * longitude_scale,
                lat * M_PER_DEG_LAT,
            ]
        };
        let cell = |value: f64| (value / CORRIDOR_DISTANCE_M).floor() as i64;
        let mut buckets: HashMap<Cell, HashMap<i64, Vec<usize>>> = HashMap::new();
        for (local, &(start, end)) in segments.iter().enumerate() {
            let a = project(start);
            let b = project(end);
            for x in cell(a[0].min(b[0]) - CORRIDOR_DISTANCE_M)
                ..=cell(a[0].max(b[0]) + CORRIDOR_DISTANCE_M)
            {
                for y in cell(a[1].min(b[1]) - CORRIDOR_DISTANCE_M)
                    ..=cell(a[1].max(b[1]) + CORRIDOR_DISTANCE_M)
                {
                    buckets
                        .entry((x, y))
                        .or_default()
                        .entry(rows[indices[local]].osm_id)
                        .or_default()
                        .push(local);
                }
            }
        }
        for (local, &index) in indices.iter().enumerate() {
            let traffic = rows[index].child.traffic;
            if !((traffic.passenger.source_id == 0 && traffic.passenger.status == STATUS_ESTIMATED)
                || (traffic.freight.source_id == 0 && traffic.freight.status == STATUS_ESTIMATED))
            {
                continue;
            }
            let (a, b) = segments[local];
            let midpoint = (wrapped_longitude_midpoint(a.0, b.0), (a.1 + b.1) / 2.0);
            let p = project(midpoint);
            let own_way = rows[index].osm_id;
            let nearby = buckets[&(cell(p[0]), cell(p[1]))]
                .iter()
                .filter(|(other_way, candidates)| {
                    **other_way != own_way
                        && endpoints[&own_way].is_disjoint(&endpoints[other_way])
                        && candidates.iter().any(|&candidate| {
                            let (start, end) = segments[candidate];
                            point_to_segment(midpoint.1, midpoint.0, start.1, start.0, end.1, end.0)
                                .0
                                < CORRIDOR_DISTANCE_M
                        })
                })
                .count()
                + 1;
            share_class_defaults(std::slice::from_mut(&mut rows[index].child.traffic), nearby);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::{CategoryFlow, RowTraffic};
    use crate::split::{ChildGeom, ChildRow};

    fn row(way: i64, start: (f64, f64), end: (f64, f64), source_id: u16) -> Expanded {
        let (start_gx, start_gy) = grid::lonlat_to_grid(start.0, start.1);
        let (end_gx, end_gy) = grid::lonlat_to_grid(end.0, end.1);
        Expanded {
            parent: 0,
            osm_id: way,
            corridor: "corridor".to_owned(),
            rail_type: 0,
            usage: 0,
            child: ChildRow {
                geom: ChildGeom {
                    start_gx,
                    start_gy,
                    end_gx,
                    end_gy,
                    length_m: 100.0,
                },
                traffic: RowTraffic {
                    passenger: CategoryFlow {
                        periods: [80.0, 0.0, 0.0],
                        status: 2,
                        source_id,
                        matching: 0,
                    },
                    freight: CategoryFlow::default(),
                },
            },
        }
    }

    #[test]
    fn physical_way_sharing_ignores_child_count_and_sequential_ways() {
        let evaluate = |parts: usize| {
            let mut rows = vec![row(1, (14.0, 50.0), (14.001, 50.0), 0)];
            for part in 0..parts {
                let from = 14.0 + 0.001 * part as f64 / parts as f64;
                let to = 14.0 + 0.001 * (part + 1) as f64 / parts as f64;
                rows.push(row(2, (from, 50.0001), (to, 50.0001), 0));
            }
            rows.push(row(3, (14.001, 50.0), (14.002, 50.0), 0));
            rows.push(row(4, (15.0, 50.0), (15.001, 50.0), 0));
            rows.push(row(5, (16.0, 50.0), (16.001, 50.0), 100));
            apply_default_sharing(&mut rows);
            assert_eq!(
                rows.last().unwrap().child.traffic.passenger.periods[0],
                80.0
            );
            assert_eq!(
                rows[rows.len() - 2].child.traffic.passenger.periods[0],
                80.0
            );
            rows[0].child.traffic.passenger.periods[0]
        };
        assert_eq!(evaluate(1), 40.0);
        assert_eq!(evaluate(100), 40.0);
        let mut local_pairs = Vec::new();
        for pair in 0..5_000 {
            let longitude = 14.0 + (pair % 100) as f64 * 0.01;
            let latitude = 49.0 + (pair / 100) as f64 * 0.01;
            for track in 0..2 {
                local_pairs.push(row(
                    pair * 2 + track,
                    (longitude, latitude + track as f64 * 0.0001),
                    (longitude + 0.001, latitude + track as f64 * 0.0001),
                    if track == 1 { 100 } else { 0 },
                ));
            }
        }
        let started = std::time::Instant::now();
        apply_default_sharing(&mut local_pairs);
        eprintln!(
            "physical-way sharing: {} rows in {:?}",
            local_pairs.len(),
            started.elapsed()
        );
        for (index, row) in local_pairs.iter().enumerate() {
            assert_eq!(
                row.child.traffic.passenger.periods[0],
                if index % 2 == 0 { 40.0 } else { 80.0 }
            );
        }
    }
}

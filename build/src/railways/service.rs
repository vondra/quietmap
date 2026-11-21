//! Walk each ordered station leg, preserving reversals and quarantining unresolved observations.
use super::{
    alignment,
    leg_matcher::LegMatcher,
    movements::Stop,
    network::distance,
    route::{self, Visit},
    source_routes::Routes,
};

pub struct Passage {
    pub square: [u16; 2],
    pub way: i64,
    pub segment: i16,
    pub from: f64,
    pub to: f64,
    pub occurrence: i64,
}
pub struct Matched {
    pub passages: Vec<Passage>,
    pub matching: u8,
    pub failed: usize,
    pub failed_shapes: Vec<Vec<[f64; 2]>>,
}

fn collapse(stops: &[Stop]) -> Vec<Stop> {
    let mut result: Vec<Stop> = Vec::new();
    for stop in stops {
        if result.last().is_none_or(|last| last.id != stop.id) {
            result.push(stop.clone());
        }
    }
    result
}

fn geometry(stops: &[Stop], shape: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let distinct = |points: Vec<[f64; 2]>| {
        let mut points = points;
        points.dedup();
        points
    };
    let positions = stops
        .iter()
        .map(|stop| [stop.latitude, stop.longitude])
        .collect();
    if shape.len() < 2 || distinct(shape.to_vec()) == distinct(positions) {
        Vec::new()
    } else {
        shape.to_vec()
    }
}

fn anchors_with_cost(shape: &[[f64; 2]], stops: &[Stop]) -> (Vec<usize>, f64) {
    if shape.is_empty() || stops.is_empty() {
        return (Vec::new(), 0.0);
    }
    let mut costs = vec![0.0; shape.len()];
    let mut predecessors = Vec::new();
    for stop in stops {
        let mut next = Vec::new();
        let mut previous = Vec::new();
        let mut best = 0;
        for (index, point) in shape.iter().enumerate() {
            if costs[index] < costs[best] {
                best = index;
            }
            next.push(costs[best] + distance([stop.latitude, stop.longitude], *point));
            previous.push(best);
        }
        costs = next;
        predecessors.push(previous);
    }
    let mut chosen = 0;
    for index in 1..costs.len() {
        if costs[index] < costs[chosen] {
            chosen = index;
        }
    }
    let best_cost = costs[chosen];
    let mut result = Vec::new();
    for previous in predecessors.iter().rev() {
        result.push(chosen);
        chosen = previous[chosen];
    }
    result.reverse();
    (result, best_cost)
}

type OrientedShape = (Vec<[f64; 2]>, Vec<usize>);

fn advancing_legs(shape: &[[f64; 2]], anchors: &[usize], stops: &[Stop]) -> usize {
    (1..stops.len())
        .filter(|&i| {
            let (s0, f0, _) = project_stop_to_anchor(
                shape,
                anchors[i - 1],
                [stops[i - 1].latitude, stops[i - 1].longitude],
            );
            let (s1, f1, _) =
                project_stop_to_anchor(shape, anchors[i], [stops[i].latitude, stops[i].longitude]);
            (s1 as f64 + f1) > (s0 as f64 + f0) + 1e-6
        })
        .count()
}

fn orient_failed_shape(
    shape: &[[f64; 2]],
    fwd_anchors: &[usize],
    stops: &[Stop],
    fwd_cost: f64,
) -> Option<OrientedShape> {
    if shape.len() < 2 || stops.len() < 2 {
        return None;
    }
    let rev_shape: Vec<[f64; 2]> = shape.iter().rev().copied().collect();
    let (rev_anchors, rev_cost) = anchors_with_cost(&rev_shape, stops);
    let rev_wins = rev_cost < fwd_cost - 1e-6
        || ((rev_cost - fwd_cost).abs() <= 1e-6
            && advancing_legs(&rev_shape, &rev_anchors, stops)
                > advancing_legs(shape, fwd_anchors, stops));
    rev_wins.then_some((rev_shape, rev_anchors))
}

fn project_point_on_segment(pt: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64, [f64; 2]) {
    let cos_lat = (0.5 * (a[0] + b[0])).to_radians().cos().max(0.05);
    let dx = (b[1] - a[1]) * 111_320.0 * cos_lat;
    let dy = (b[0] - a[0]) * 110_540.0;
    let len2 = dx * dx + dy * dy;
    let frac = if len2 > 1e-12 {
        let px = (pt[1] - a[1]) * 111_320.0 * cos_lat;
        let py = (pt[0] - a[0]) * 110_540.0;
        ((px * dx + py * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let foot = [a[0] + frac * (b[0] - a[0]), a[1] + frac * (b[1] - a[1])];
    (distance(pt, foot), frac, foot)
}

/// Project a station coordinate onto the archived shape segments adjacent to its anchor vertex,
/// keeping failed-leg geometry strictly on the archived track polyline even when the station pin
/// sits off-track or between widely spaced shape vertices.
fn project_stop_to_anchor(
    shape: &[[f64; 2]],
    anchor: usize,
    stop: [f64; 2],
) -> (usize, f64, [f64; 2]) {
    let mut best: Option<(f64, usize, f64, [f64; 2])> = None;
    if anchor > 0 {
        let (d, frac, foot) = project_point_on_segment(stop, shape[anchor - 1], shape[anchor]);
        best = Some((d, anchor - 1, frac, foot));
    }
    if anchor + 1 < shape.len() {
        let (d, frac, foot) = project_point_on_segment(stop, shape[anchor], shape[anchor + 1]);
        if best.is_none_or(|(bd, _, _, _)| d < bd) {
            best = Some((d, anchor, frac, foot));
        }
    }
    match best {
        Some((_, seg, frac, foot)) => (seg, frac, foot),
        None => (anchor, 0.0, shape[anchor]),
    }
}

fn slice_shape_between_stops(
    shape: &[[f64; 2]],
    a0: usize,
    stop0: [f64; 2],
    a1: usize,
    stop1: [f64; 2],
) -> Vec<[f64; 2]> {
    let (seg0, frac0, pt0) = project_stop_to_anchor(shape, a0, stop0);
    let (seg1, frac1, pt1) = project_stop_to_anchor(shape, a1, stop1);
    let pos0 = seg0 as f64 + frac0;
    let pos1 = seg1 as f64 + frac1;
    if pos1 <= pos0 + 1e-6 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(seg1.saturating_sub(seg0) + 2);
    out.push(pt0);
    for point in &shape[(seg0 + 1)..=seg1.min(shape.len() - 1)] {
        out.push(*point);
    }
    out.push(pt1);
    out.dedup();
    out
}

fn reversal_runs(points: &[[f64; 2]]) -> Vec<Vec<[f64; 2]>> {
    let mut runs = Vec::new();
    let mut run = vec![points[0]];
    for index in 1..points.len() - 1 {
        let previous = *run.last().unwrap();
        let current = points[index];
        let next = points[index + 1];
        run.push(current);
        let cosine = ((current[0] + next[0]) / 2.0 * std::f64::consts::PI / 180.0)
            .cos()
            .max(0.05);
        let ax = (current[1] - previous[1]) * 111_320.0 * cosine;
        let ay = (current[0] - previous[0]) * 110_540.0;
        let bx = (next[1] - current[1]) * 111_320.0 * cosine;
        let by = (next[0] - current[0]) * 110_540.0;
        let spans = ax.hypot(ay) * bx.hypot(by);
        if spans > 0.0 && (ax * bx + ay * by) / spans <= -0.5 {
            runs.push(run);
            run = vec![current];
        }
    }
    run.push(*points.last().unwrap());
    runs.push(run);
    runs
}

fn slice_reversed_subwalk(
    rev_shape: &[[f64; 2]],
    a0: usize,
    stop0: [f64; 2],
    a1: usize,
    stop1: [f64; 2],
    failed_run: &[[f64; 2]],
) -> Vec<[f64; 2]> {
    if failed_run.len() < 2 {
        return Vec::new();
    }
    let pair_slice = slice_shape_between_stops(rev_shape, a0, stop0, a1, stop1);
    if pair_slice.len() < 2 {
        return Vec::new();
    }
    let sub0 = failed_run[0];
    let sub1 = *failed_run.last().unwrap();
    let sub_stops = [sub0, sub1].map(|pt| Stop {
        source_id: String::new(),
        id: String::new(),
        latitude: pt[0],
        longitude: pt[1],
    });
    let (sub_anchors, _) = anchors_with_cost(&pair_slice, &sub_stops);
    let sliced = slice_shape_between_stops(&pair_slice, sub_anchors[0], sub0, sub_anchors[1], sub1);
    if sliced.len() >= 2 {
        sliced
    } else {
        Vec::new()
    }
}

pub fn match_service(
    routes: &Routes,
    matcher: &mut LegMatcher<'_>,
    stops: &[Stop],
    shape: &[[f64; 2]],
) -> Matched {
    let network = matcher.network;
    let shape = geometry(stops, shape);
    let stations = collapse(stops);
    let observations = if shape.is_empty() {
        stations
            .iter()
            .map(|stop| [stop.latitude, stop.longitude])
            .collect()
    } else {
        shape.clone()
    };
    if let Some(route) = routes.unique(stops)
        && let Some(passages) = alignment::align(&observations, route)
    {
        let mut clipped = Vec::new();
        for passage in passages {
            let lower = passage.from.min(passage.to);
            let upper = passage.from.max(passage.to);
            if let Some(pieces) = network.way_pieces.get(&passage.way) {
                let mut visits = Vec::new();
                for edge in pieces {
                    let from = lower.max(edge.extent[0]);
                    let to = upper.min(edge.extent[1]);
                    if to > from {
                        visits.push(Passage {
                            square: edge.square,
                            way: edge.way,
                            segment: edge.segment,
                            from: if passage.from < passage.to { from } else { to },
                            to: if passage.from < passage.to { to } else { from },
                            occurrence: passage.occurrence,
                        });
                    }
                }
                visits.sort_by(|a, b| a.from.min(a.to).total_cmp(&b.from.min(b.to)));
                if passage.from > passage.to {
                    visits.reverse();
                }
                clipped.extend(visits);
            }
        }
        return Matched {
            passages: clipped,
            matching: 1,
            failed: 0,
            failed_shapes: Vec::new(),
        };
    }
    let (anchors, fwd_cost) = anchors_with_cost(&shape, &stations);
    let mut rev_oriented: Option<Option<OrientedShape>> = None;
    let mut visits = Vec::<Visit>::new();
    let mut failed = 0;
    let mut failed_shapes: Vec<Vec<[f64; 2]>> = Vec::new();
    for index in 1..stations.len() {
        let stop0 = [stations[index - 1].latitude, stations[index - 1].longitude];
        let stop1 = [stations[index].latitude, stations[index].longitude];
        let mut points = vec![stop0];
        if !shape.is_empty() {
            points.extend_from_slice(
                &shape[(anchors[index - 1] + 1).min(anchors[index])..anchors[index]],
            );
        }
        points.push(stop1);
        let runs = reversal_runs(&points);
        let run_count = runs.len();
        let mut last_failed_run: Option<usize> = None;
        let mut any_ok = false;
        let mut any_failed = false;
        let mut rev_failed_runs: Vec<Vec<[f64; 2]>> = Vec::new();
        for (run_idx, run) in runs.into_iter().enumerate() {
            match matcher.walk(
                run[0],
                *run.last().unwrap(),
                if shape.is_empty() { &[] } else { &run },
            ) {
                Ok(next) => {
                    any_ok = true;
                    route::stitch(&mut visits, next);
                }
                Err(_) => {
                    failed += 1;
                    any_failed = true;
                    if !shape.is_empty() {
                        let rev = rev_oriented
                            .get_or_insert_with(|| {
                                orient_failed_shape(&shape, &anchors, &stations, fwd_cost)
                            })
                            .as_ref();
                        if rev.is_some() {
                            rev_failed_runs.push(run);
                            continue;
                        }
                        let mut archived = if run_count == 1 {
                            slice_shape_between_stops(
                                &shape,
                                anchors[index - 1],
                                stop0,
                                anchors[index],
                                stop1,
                            )
                        } else {
                            let a0 = anchors[index - 1];
                            let a1 = anchors[index];
                            let mut out = Vec::with_capacity(run.len() + 2);
                            if run_idx == 0 {
                                let (seg0, _, pt0) = project_stop_to_anchor(&shape, a0, stop0);
                                out.push(pt0);
                                if seg0 < a0 {
                                    out.push(shape[a0]);
                                }
                                out.extend_from_slice(&run[1..]);
                            } else {
                                out.extend_from_slice(&run);
                            }
                            if run_idx + 1 == run_count {
                                out.pop();
                                let (seg1, _, pt1) = project_stop_to_anchor(&shape, a1, stop1);
                                if seg1 >= a1 {
                                    out.push(shape[a1]);
                                }
                                out.push(pt1);
                            }
                            out.dedup();
                            out
                        };
                        if archived.len() >= 2 {
                            if last_failed_run == Some(run_idx.wrapping_sub(1))
                                && let Some(prev) = failed_shapes.last_mut()
                                && prev.last() == Some(&archived[0])
                            {
                                prev.extend_from_slice(&archived[1..]);
                            } else {
                                archived.dedup();
                                failed_shapes.push(archived);
                            }
                            last_failed_run = Some(run_idx);
                        }
                    }
                }
            }
        }
        if any_failed && let Some(Some((rev_shape, rev_anchors))) = rev_oriented.as_ref() {
            if !any_ok {
                let archived = slice_shape_between_stops(
                    rev_shape,
                    rev_anchors[index - 1],
                    stop0,
                    rev_anchors[index],
                    stop1,
                );
                if archived.len() >= 2 {
                    failed_shapes.push(archived);
                }
            } else {
                for failed_run in rev_failed_runs {
                    let archived = slice_reversed_subwalk(
                        rev_shape,
                        rev_anchors[index - 1],
                        stop0,
                        rev_anchors[index],
                        stop1,
                        &failed_run,
                    );
                    if archived.len() >= 2 {
                        failed_shapes.push(archived);
                    }
                }
            }
        }
    }
    let passages = visits
        .iter()
        .enumerate()
        .filter_map(|(occurrence, visit)| {
            let edge = &network.edges[visit.edge];
            if edge.traversal_only {
                return None;
            }
            let from = edge.extent[0] + visit.from * (edge.extent[1] - edge.extent[0]);
            let to = edge.extent[0] + visit.to * (edge.extent[1] - edge.extent[0]);
            (from != to).then_some(Passage {
                square: edge.square,
                way: edge.way,
                segment: edge.segment,
                from,
                to,
                occurrence: occurrence as i64,
            })
        })
        .collect();
    Matched {
        passages,
        matching: 2,
        failed,
        failed_shapes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn station_stub_keeps_ambiguity_and_reversal_retains_the_return_leg() {
        let stops = [[0.0, 0.0], [0.0, 0.01]].map(|point| Stop {
            source_id: String::new(),
            id: point[1].to_string(),
            latitude: point[0],
            longitude: point[1],
        });
        assert!(geometry(&stops, &[[0.0, 0.0], [0.0, 0.01]]).is_empty());
        assert_eq!(
            reversal_runs(&[[0.0, 0.0], [0.0, 0.01], [0.0, 0.0]]),
            vec![vec![[0.0, 0.0], [0.0, 0.01]], vec![[0.0, 0.01], [0.0, 0.0]]]
        );
    }

    #[test]
    fn failed_shape_slice_projects_off_track_and_mid_segment_stops_onto_archived_polyline() {
        // Straight north-south archived shape with 200 m vertex spacing at lon = 0.0.
        let shape = [[0.0, 0.0], [0.002, 0.0], [0.004, 0.0], [0.006, 0.0]];
        // Start stop is 30 m off-track (west) and 70 m south of shape[1] (anchor 1);
        // end stop is 30 m off-track (east) and 50 m north of shape[2] (anchor 2).
        let stop0 = [0.0013, -0.00027];
        let stop1 = [0.0045, 0.00027];
        let sliced = slice_shape_between_stops(&shape, 1, stop0, 2, stop1);
        assert_eq!(sliced.len(), 4);
        for pt in &sliced {
            assert!(
                pt[1].abs() < 1e-12,
                "projected archived shape must stay strictly on track polyline, got {pt:?}"
            );
        }
        assert!((sliced[0][0] - 0.0013).abs() < 1e-9);
        assert_eq!(sliced[1], shape[1]);
        assert_eq!(sliced[2], shape[2]);
        assert!((sliced[3][0] - 0.0045).abs() < 1e-9);

        // Reversed archived shape (e.g., westbound Amtrak trip referencing an eastbound shape):
        // forward anchors collapse onto one vertex, while orient_failed_shape reverses the shape
        // and recovers the full interstation leg cleanly.
        let rev_stops = [
            Stop {
                source_id: String::new(),
                id: "C".into(),
                latitude: 0.0045,
                longitude: 0.0,
            },
            Stop {
                source_id: String::new(),
                id: "B".into(),
                latitude: 0.0020,
                longitude: 0.0,
            },
            Stop {
                source_id: String::new(),
                id: "A".into(),
                latitude: 0.0005,
                longitude: 0.0,
            },
        ];
        let (fwd_anchors, fwd_cost) = anchors_with_cost(&shape, &rev_stops);
        assert_eq!(fwd_anchors[0], fwd_anchors[1]);
        let (rev_shape, rev_anchors) =
            orient_failed_shape(&shape, &fwd_anchors, &rev_stops, fwd_cost)
                .expect("reversed shape must win");
        let rev_leg = slice_shape_between_stops(
            &rev_shape,
            rev_anchors[0],
            [rev_stops[0].latitude, rev_stops[0].longitude],
            rev_anchors[1],
            [rev_stops[1].latitude, rev_stops[1].longitude],
        );
        assert_eq!(rev_leg.len(), 3);
        assert!((rev_leg[0][0] - 0.0045).abs() < 1e-9);
        assert_eq!(rev_leg[1], [0.004, 0.0]);
        assert!((rev_leg[2][0] - 0.0020).abs() < 1e-9);

        // Equal vertex-anchor cost tie on a 2-vertex straight shape ([0, 1000 m] with stops at
        // 400 m then 200 m): tie-break by advancing projected positions picks reversed orientation.
        let seg_shape = [[0.0, 0.0], [0.009, 0.0]];
        let tie_stops = [
            Stop {
                source_id: String::new(),
                id: "S1".into(),
                latitude: 0.0036,
                longitude: 0.0,
            },
            Stop {
                source_id: String::new(),
                id: "S2".into(),
                latitude: 0.0018,
                longitude: 0.0,
            },
        ];
        let (tie_fwd_a, tie_fwd_cost) = anchors_with_cost(&seg_shape, &tie_stops);
        let (tie_rev_s, tie_rev_a) =
            orient_failed_shape(&seg_shape, &tie_fwd_a, &tie_stops, tie_fwd_cost)
                .expect("equal-cost reversed stops must pick reversed shape");
        let tie_leg = slice_shape_between_stops(
            &tie_rev_s,
            tie_rev_a[0],
            [tie_stops[0].latitude, tie_stops[0].longitude],
            tie_rev_a[1],
            [tie_stops[1].latitude, tie_stops[1].longitude],
        );
        assert_eq!(tie_leg.len(), 2);
        assert!((tie_leg[0][0] - 0.0036).abs() < 1e-9);
        assert!((tie_leg[1][0] - 0.0018).abs() < 1e-9);

        // Partially matched reversed station pair on a revisiting shape A-B-U-J-S-J-T with stops
        // A, S, T: if S->J succeeds and J->T fails, slice_reversed_subwalk must restrict to the
        // current station pair (S..=T) and pick the second J rather than replaying J->S->J->T.
        let revisit_rev = [
            [0.0, 0.000],    // A
            [0.0, 0.001],    // B
            [0.004, 0.0139], // U (near T, forces forward anchor past second J)
            [0.0, 0.004],    // first J (before S)
            [0.0, 0.014],    // S
            [0.0, 0.004],    // second J (after S)
            [0.004, 0.014],  // T (> 300 m north of S--J so J->T fails walk)
        ];
        let sub_slice = slice_reversed_subwalk(
            &revisit_rev,
            4,
            revisit_rev[4],
            6,
            revisit_rev[6],
            &[revisit_rev[5], revisit_rev[6]],
        );
        assert_eq!(sub_slice, vec![revisit_rev[5], revisit_rev[6]]);

        // End-to-end match_service check for the same reversed revisiting shape: S->J walks on the
        // connected track while J->T fails, so failed_shapes must contain only J->T (never S).
        let mut net = super::super::network::Network::default();
        net.nodes = vec![[0.0, 0.014], [0.0, 0.004]];
        net.edges.push(super::super::network::Edge {
            nodes: [0, 1],
            points: [net.nodes[0], net.nodes[1]],
            length: distance(net.nodes[0], net.nodes[1]),
            way: 1,
            segment: 0,
            square: [100, 100],
            extent: [0.0, distance(net.nodes[0], net.nodes[1])],
            rail_type: 0,
            usage: 0,
            traffic_mode: 0,
            country: *b"US",
            corridor: "Main".into(),
            corridor_name: String::new(),
            traversal_only: false,
        });
        net.index();
        let mut matcher = LegMatcher::new(&net);
        let fwd_reversed_shape: Vec<[f64; 2]> = revisit_rev.iter().rev().copied().collect();
        let stops = [
            ("A", revisit_rev[0]),
            ("S", revisit_rev[4]),
            ("T", revisit_rev[6]),
        ]
        .map(|(id, pt)| Stop {
            source_id: String::new(),
            id: id.into(),
            latitude: pt[0],
            longitude: pt[1],
        });
        let matched = match_service(
            &Routes::default(),
            &mut matcher,
            &stops,
            &fwd_reversed_shape,
        );
        assert!(
            !matched.passages.is_empty(),
            "S->J subwalk must match on the connected track"
        );
        assert_eq!(
            matched.failed_shapes.last(),
            Some(&vec![revisit_rev[5], revisit_rev[6]]),
            "partially matched reversed S->T leg must emit only failed J->T, got {:?}",
            matched.failed_shapes
        );

        // A failed subwalk lying entirely outside the current station pair (both endpoints project
        // onto the same tip of pair_slice) must return empty rather than escaping the station pair.
        let straight_pair = [[0.0, 0.0], [0.0, 0.009]];
        let off_pair_spur = [[0.009, 0.0135], [0.0, 0.009]];
        assert!(
            slice_reversed_subwalk(
                &straight_pair,
                0,
                straight_pair[0],
                1,
                straight_pair[1],
                &off_pair_spur,
            )
            .is_empty(),
            "off-pair failed subwalk projecting to one tip must never be emitted outside S->T"
        );
    }
}

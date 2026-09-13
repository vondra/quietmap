//! Sparse physical road candidates; source-way identity removes longitudinal repeats.

use crate::{allocation::{compatible_alternative, longitudinal_overlap}, input::Road};
use std::collections::{HashMap, HashSet};

const CELL_M: f64 = 256.0;
const REACH_M: f64 = 50.0;

pub struct RoadIndex {
    pub roads: Vec<Road>,
    cells: HashMap<(u8, i32, i32), Vec<usize>>,
}

impl RoadIndex {
    pub fn new(mut roads: Vec<Road>) -> Self {
        // Canonical endpoint wrapping can put adjacent pieces of one owner in
        // opposite coordinate frames. Index only the dateline halo twice.
        let half = grid::EARTH_CIRCUMFERENCE_M / 2.0;
        let halo = grid::EARTH_CIRCUMFERENCE_M / f64::from(grid::Z9_TILES_PER_AXIS);
        let copies = roads.iter().filter_map(|road| {
            let midpoint = road.midpoint().0;
            if (midpoint.abs() - half).abs() > halo { return None; }
            let mut copy = road.clone();
            let shift = -midpoint.signum() * grid::EARTH_CIRCUMFERENCE_M;
            copy.start.0 += shift;
            copy.end.0 += shift;
            Some(copy)
        }).collect::<Vec<_>>();
        roads.extend(copies);
        let mut cells: HashMap<(u8, i32, i32), Vec<usize>> = HashMap::new();
        for (index, road) in roads.iter().enumerate() {
            if road.direction == 0 { continue; }
            let min_x = (road.start.0.min(road.end.0) / CELL_M).floor() as i32;
            let max_x = (road.start.0.max(road.end.0) / CELL_M).floor() as i32;
            let min_y = (road.start.1.min(road.end.1) / CELL_M).floor() as i32;
            let max_y = (road.start.1.max(road.end.1) / CELL_M).floor() as i32;
            for x in min_x..=max_x { for y in min_y..=max_y {
                cells.entry((road.class, x, y)).or_default().push(index);
            }}
        }
        Self { roads, cells }
    }

    pub fn alternatives<'a>(&'a self, road: &Road) -> Vec<&'a Road> {
        let mut queue = vec![road];
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut head = 0;
        while head < queue.len() {
            let current = queue[head];
            head += 1;
            for candidate in self.candidates(current) {
                if compatible_alternative(road, candidate) && longitudinal_overlap(road, candidate)
                    && seen.insert((candidate.way_id, candidate.segment_idx)) {
                    result.push(candidate);
                    queue.push(candidate);
                }
            }
        }
        result
    }

    pub fn candidates<'a>(&'a self, road: &Road) -> Vec<&'a Road> {
        if road.direction == 0 { return Vec::new(); }
        let reach = REACH_M / road.mercator_scale();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for x in ((road.start.0.min(road.end.0) - reach) / CELL_M).floor() as i32..=((road.start.0.max(road.end.0) + reach) / CELL_M).floor() as i32 {
            for y in ((road.start.1.min(road.end.1) - reach) / CELL_M).floor() as i32..=((road.start.1.max(road.end.1) + reach) / CELL_M).floor() as i32 {
                if let Some(indices) = self.cells.get(&(road.class, x, y)) {
                    for index in indices { if seen.insert(*index) { result.push(&self.roads[*index]); } }
                }
            }
        }
        result
    }
}

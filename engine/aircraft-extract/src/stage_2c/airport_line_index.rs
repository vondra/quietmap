//! Conservative frame-interval BVH retaining the scalar projection's exact hit order.
use super::airport_traffic::{
    clipped_overlap_in_frame, collect_intersections, AirportLineSegment, LegIntersection, LineFrame,
};
use crate::geo::{flat_dist, M_PER_DEG_LAT};
use anyhow::{Context, Result};
use grid::geo::wrapped_longitude_delta;
use std::mem::size_of;

#[derive(Clone, Copy, Debug)]
struct Interval(f32, f32);

impl Interval {
    fn point(value: f32) -> Self {
        Self(value, value)
    }
    fn union(self, other: Self) -> Self {
        Self(self.0.min(other.0), self.1.max(other.1))
    }
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0, self.1 + other.1)
    }
    fn sub(self, other: Self) -> Self {
        self.add(other.neg())
    }
    fn neg(self) -> Self {
        Self(-self.1, -self.0)
    }
    fn mul(self, other: Self) -> Self {
        let values = [
            self.0 * other.0,
            self.0 * other.1,
            self.1 * other.0,
            self.1 * other.1,
        ];
        if values.iter().any(|v| !v.is_finite()) {
            return Self(f32::NEG_INFINITY, f32::INFINITY);
        }
        Self(
            values.into_iter().fold(f32::INFINITY, f32::min),
            values.into_iter().fold(f32::NEG_INFINITY, f32::max),
        )
    }
}

#[derive(Clone, Copy)]
struct FrameBounds {
    lat: Interval,
    lon: (f64, f64),
    longitude_scale: Interval,
    east: Interval,
    north: Interval,
    half_length: f32,
}

impl FrameBounds {
    fn from_frame(frame: LineFrame) -> Self {
        Self {
            lat: Interval::point(frame.mid_lat),
            lon: (frame.mid_lon, frame.mid_lon),
            longitude_scale: Interval::point(frame.m_per_deg_lon),
            east: Interval::point(frame.u_e),
            north: Interval::point(frame.u_n),
            half_length: frame.s_half,
        }
    }
    fn union(self, other: Self) -> Self {
        Self {
            lat: self.lat.union(other.lat),
            lon: (self.lon.0.min(other.lon.0), self.lon.1.max(other.lon.1)),
            longitude_scale: self.longitude_scale.union(other.longitude_scale),
            east: self.east.union(other.east),
            north: self.north.union(other.north),
            half_length: self.half_length.max(other.half_length),
        }
    }
    fn to_local(self, lat: f32, lon: f32) -> (Interval, Interval) {
        let low = f64::from(lon) - self.lon.1;
        let high = f64::from(lon) - self.lon.0;
        // normalize_longitude is monotone only inside one branch and wrap interval.
        // An uncertain antipodal cut retains all longitudes; it never removes a line.
        let same_branch = (low >= -180.0 && high < 180.0)
            || ((high < -180.0 || low >= 180.0)
                && ((low + 180.0) / 360.0).floor() == ((high + 180.0) / 360.0).floor());
        let delta = if same_branch {
            Interval(
                wrapped_longitude_delta(self.lon.1, f64::from(lon)) as f32,
                wrapped_longitude_delta(self.lon.0, f64::from(lon)) as f32,
            )
        } else {
            Interval(-180.0, 180.0)
        };
        let east = delta.mul(self.longitude_scale);
        let north = Interval::point(lat)
            .sub(self.lat)
            .mul(Interval::point(M_PER_DEG_LAT));
        (
            east.mul(self.east).add(north.mul(self.north)),
            east.mul(self.north.neg()).add(north.mul(self.east)),
        )
    }
    fn rejects(self, leg: [f32; 4], buffer: f32) -> bool {
        let (x1, y1) = self.to_local(leg[0], leg[1]);
        let (x2, y2) = self.to_local(leg[2], leg[3]);
        // Every bound uses the same rounded f32 operations as LineFrame::to_local.
        // Monotonic round-to-nearest keeps interval endpoints enclosing those values.
        // Both endpoints strictly outside one edge force the scalar clip's t range
        // empty (including a rounded entering/exiting t of exactly 0 or 1).
        (x1.0 > self.half_length && x2.0 > self.half_length)
            || (x1.1 < -self.half_length && x2.1 < -self.half_length)
            || (y1.0 > buffer && y2.0 > buffer)
            || (y1.1 < -buffer && y2.1 < -buffer)
    }
}

struct Node {
    bounds: FrameBounds,
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}

pub struct AirportLineIndex<'a> {
    lines: &'a [AirportLineSegment],
    frames: Vec<Option<LineFrame>>,
    indices: Vec<usize>,
    nodes: Vec<Node>,
    candidates: Vec<usize>,
}

impl<'a> AirportLineIndex<'a> {
    /// Exact requested vector capacities, doubled for allocator overhead. Charged before construction.
    pub fn allocation_allowance(line_count: usize) -> Result<u64> {
        let bytes = line_count as u128
            * (2 * size_of::<Node>() + size_of::<Option<LineFrame>>() + 2 * size_of::<usize>())
                as u128;
        u64::try_from(bytes * 2).context("ground line index allocation overflow")
    }
    pub fn new(lines: &'a [AirportLineSegment], allocation_limit: u64) -> Result<Self> {
        anyhow::ensure!(
            Self::allocation_allowance(lines.len())? <= allocation_limit,
            "ground line index allocation exceeds admission"
        );
        let mut frames = Vec::with_capacity(lines.len());
        let mut indices = Vec::with_capacity(lines.len());
        for (index, line) in lines.iter().enumerate() {
            let frame = LineFrame::new(line);
            if frame.is_some() {
                indices.push(index);
            }
            frames.push(frame);
        }
        let mut result = Self {
            lines,
            frames,
            indices,
            nodes: Vec::with_capacity(
                lines
                    .len()
                    .checked_mul(2)
                    .context("ground line node count overflow")?,
            ),
            candidates: Vec::with_capacity(lines.len()),
        };
        if !result.indices.is_empty() {
            result.build(0, result.indices.len());
        }
        Ok(result)
    }
    fn orientation_partition(frame: LineFrame) -> u8 {
        u8::from(frame.u_e.is_sign_negative())
            | (u8::from(frame.u_n.is_sign_negative()) << 1)
            | (u8::from(frame.u_e.abs() < frame.u_n.abs()) << 2)
    }
    fn build(&mut self, start: usize, end: usize) -> usize {
        let mut bounds = FrameBounds::from_frame(self.frames[self.indices[start]].unwrap());
        for &index in &self.indices[start + 1..end] {
            bounds = bounds.union(FrameBounds::from_frame(self.frames[index].unwrap()));
        }
        let node = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            start,
            end,
            children: None,
        });
        if end - start > 1 {
            let latitude_axis =
                f64::from(bounds.lat.1 - bounds.lat.0) > bounds.lon.1 - bounds.lon.0;
            // Sign and dominant-axis partitions keep interval coefficients from
            // independently spanning zero. They change traversal only, never hits.
            let first_partition =
                Self::orientation_partition(self.frames[self.indices[start]].unwrap());
            let mixed_orientation = self.indices[start + 1..end].iter().any(|&index| {
                Self::orientation_partition(self.frames[index].unwrap()) != first_partition
            });
            let middle = (end - start) / 2;
            self.indices[start..end].select_nth_unstable_by(middle, |&a, &b| {
                let a = self.frames[a].unwrap();
                let b = self.frames[b].unwrap();
                if mixed_orientation {
                    Self::orientation_partition(a).cmp(&Self::orientation_partition(b))
                } else if latitude_axis {
                    a.mid_lat.total_cmp(&b.mid_lat)
                } else {
                    a.mid_lon.total_cmp(&b.mid_lon)
                }
            });
            let left = self.build(start, start + middle);
            let right = self.build(start + middle, end);
            self.nodes[node].children = Some((left, right));
        }
        node
    }
    fn visit(&mut self, node_index: usize, leg: [f32; 4], buffer: f32) -> usize {
        let node = &self.nodes[node_index];
        if node.bounds.rejects(leg, buffer) {
            return 1;
        }
        if let Some((left, right)) = node.children {
            1 + self.visit(left, leg, buffer) + self.visit(right, leg, buffer)
        } else {
            self.candidates
                .extend_from_slice(&self.indices[node.start..node.end]);
            1
        }
    }
    /// Returns ordered hits and measured (visited nodes, scalar candidate calls).
    pub fn project(
        &mut self,
        leg: [f32; 4],
        buffer: f32,
    ) -> (Vec<LegIntersection>, (usize, usize)) {
        self.candidates.clear();
        let visited = if self.nodes.is_empty() {
            0
        } else {
            self.visit(0, leg, buffer)
        };
        self.candidates.sort_unstable();
        let hits = collect_intersections(
            flat_dist(leg[0], leg[1], leg[2], leg[3]),
            self.candidates.len(),
            self.candidates.iter().map(|&index| {
                (
                    &self.lines[index],
                    clipped_overlap_in_frame(
                        leg[0],
                        leg[1],
                        leg[2],
                        leg[3],
                        self.frames[index].unwrap(),
                        buffer,
                    ),
                )
            }),
        );
        (hits, (visited, self.candidates.len()))
    }
}

#[cfg(test)]
#[path = "airport_line_index_tests.rs"]
mod tests;

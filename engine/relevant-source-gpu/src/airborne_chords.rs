//! Scene-constant split-piece geometry and all possible predecessor links, in source row order.
use anyhow::{ensure, Result};
use arrow::record_batch::RecordBatch;
use noise_compute::{
    compute::aircraft_v6::{airborne::CHORD_START, AirborneSegmentBatch},
    emission::aircraft as air,
};
use source_reader::aircraft_v6::AirborneRowAccum;
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ChordSource {
    pub endpoints: [f32; 4],
    pub physical: [f64; 13],
    pub identity: [i32; 6],
}

impl ChordSource {
    pub fn prepare(batch: &AirborneSegmentBatch<'_>, row: usize) -> Result<Self> {
        let checked = crate::airborne_pack::DeviceAirborneSource::prepare(batch, row)?;
        let segment = batch.segment(row);
        let terrain = batch.terrain(row);
        let prepared =
            air::prepare_segment(&segment, terrain.start_elev - 30.0, terrain.end_elev - 30.0);
        Ok(Self {
            endpoints: [
                segment.start_lat as f32,
                segment.start_lon as f32,
                segment.end_lat as f32,
                segment.end_lon as f32,
            ],
            physical: [
                prepared.start_alt_m,
                prepared.d_lon,
                prepared.sdy,
                prepared.sdz,
                prepared.dv,
                prepared.di_a,
                prepared.di_b,
                prepared.di_c,
                prepared.reach_sq,
                prepared.terrain_start_cut_m,
                prepared.terrain_end_cut_m,
                prepared.power_w,
                prepared.heli_db,
            ],
            // Stale ground pieces remain in the graph, but can never be accepted.
            identity: checked.map_or([0, -1, 0, 0, 0, 0], |source| source.identity),
        })
    }
}

pub(crate) struct ChordKey {
    pub flight: u64,
    pub start: (i32, i32),
    pub end: (i32, i32),
    pub flags: u8,
}

#[derive(Default)]
pub(crate) struct ChordSources {
    pub sources: Vec<ChordSource>,
    /// Four u32s per row: predecessor offset/count, connected-component offset/count.
    pub ranges: Vec<u32>,
    pub predecessors: Vec<u32>,
    pub members: Vec<u32>,
}

impl ChordSources {
    pub fn from_batches(batches: &[RecordBatch]) -> Result<Self> {
        let rows = AirborneRowAccum::new(batches).map_err(anyhow::Error::msg)?;
        let mut sources = Vec::new();
        let mut keys = Vec::new();
        for batch in rows.views() {
            for row in 0..batch.len() {
                sources.push(ChordSource::prepare(batch, row)?);
                keys.push(ChordKey {
                    flight: batch.flight_id[row],
                    start: (batch.start_gx[row], batch.start_gy[row]),
                    end: (batch.end_gx[row], batch.end_gy[row]),
                    flags: batch.flags[row],
                });
            }
        }
        Self::new(sources, &keys)
    }

    pub fn new(sources: Vec<ChordSource>, keys: &[ChordKey]) -> Result<Self> {
        ensure!(
            sources.len() == keys.len() && keys.len() <= u32::MAX as usize / 4,
            "invalid airborne chord graph size"
        );
        let mut by_end: HashMap<_, Vec<usize>> = HashMap::new();
        for (row, key) in keys.iter().enumerate() {
            by_end.entry((key.flight, key.end)).or_default().push(row);
        }
        let mut result = Self {
            sources,
            ranges: vec![0; keys.len() * 4],
            ..Self::default()
        };
        let mut parent: Vec<_> = (0..keys.len()).collect();
        for (row, key) in keys.iter().enumerate() {
            result.ranges[row * 4] = result.predecessors.len().try_into()?;
            if key.flags & CHORD_START == 0 {
                if let Some(previous) = by_end.get(&(key.flight, key.start)) {
                    result.ranges[row * 4 + 1] = previous.len().try_into()?;
                    for &candidate in previous {
                        result.predecessors.push(candidate as u32);
                        let left = root(&mut parent, row);
                        let right = root(&mut parent, candidate);
                        parent[left] = right;
                    }
                }
            }
        }
        let mut components = Vec::<Vec<usize>>::new();
        let mut component_of_root = HashMap::new();
        for row in 0..keys.len() {
            let head = root(&mut parent, row);
            let group = *component_of_root.entry(head).or_insert_with(|| {
                components.push(Vec::new());
                components.len() - 1
            });
            components[group].push(row);
        }
        for component in components {
            let first: u32 = result.members.len().try_into()?;
            for &row in &component {
                result.ranges[row * 4 + 2] = first;
                result.ranges[row * 4 + 3] = component.len().try_into()?;
                result.members.push(row as u32);
            }
        }
        // CUDA's owned buffers need an address even when every piece starts a chord.
        if result.predecessors.is_empty() {
            result.predecessors.push(0);
        }
        Ok(result)
    }
}

fn root(parent: &mut [usize], mut row: usize) -> usize {
    while parent[row] != row {
        parent[row] = parent[parent[row]];
        row = parent[row];
    }
    row
}

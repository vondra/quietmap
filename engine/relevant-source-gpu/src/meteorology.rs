//! Upload the global ERA5 table once; propagation owns consumption of this allocation.
use crate::cuda_bridge::DeviceBuffer;
use anyhow::Result;
use raster_reader::meteorology::{Meteorology, MeteorologyNode};

pub struct DeviceMeteorology {
    nodes: DeviceBuffer<MeteorologyNode>,
    pub maximum_probability: [f32; 3],
}

impl DeviceMeteorology {
    pub fn upload(table: &Meteorology) -> Result<Self> {
        Ok(Self {
            nodes: DeviceBuffer::from_slice(table.nodes())?,
            maximum_probability: table.maximum_probability(),
        })
    }

    /// Row-major ERA5 nodes, longitude wraps at 1440; lifetime belongs to this allocation.
    pub fn as_ptr(&self) -> *const MeteorologyNode {
        self.nodes.as_ptr()
    }
}

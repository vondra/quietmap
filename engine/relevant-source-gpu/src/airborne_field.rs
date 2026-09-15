//! Manifest-bound airborne rows, GPU independent events and exact CPU split chords.
use crate::{input_manifest::InputManifest, surface_scene::scene_bounds};
use anyhow::{ensure, Context, Result};
use arrow::{
    array::BooleanArray, compute::filter_record_batch, ipc::reader::FileReader,
    record_batch::RecordBatch,
};
use grid::{bounds::BoundedSquares, Square};
use noise_compute::{compute::aircraft_v6::airborne, emission::aircraft as air};
use raster_reader::RealRasters;
use source_reader::aircraft_v6::{assert_airborne_contract, AirborneRowAccum};
use std::{io::Cursor, path::Path};

#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub struct AirborneScene<'a> {
    pub(crate) owner: Square,
    pub(crate) rasters: &'a RealRasters,
    pub(crate) independent: Vec<RecordBatch>,
    pub(crate) chords: Vec<RecordBatch>,
    pub(crate) days: u16,
    pub(crate) weights: air::ClassWeights,
}
impl<'a> AirborneScene<'a> {
    pub fn load(
        owner: Square,
        prepared: &Path,
        manifest: &InputManifest,
        rasters: &'a RealRasters,
    ) -> Result<Self> {
        ensure!(owner.x < 512 && owner.y < 512, "invalid airborne owner");
        let [south, west, north, east] = scene_bounds(owner);
        // The popup's midpoint-owner support includes half the maximum stored segment.
        let dy = air::meters_to_lat_deg(air::AIRBORNE_QUERY_RADIUS_M);
        let dx = air::meters_to_lon_deg(south.abs().max(north.abs()), air::AIRBORNE_QUERY_RADIUS_M);
        let squares = BoundedSquares::from_degrees(
            (south - dy).next_down(),
            (west - dx).next_down(),
            (north + dy).next_up(),
            (east + dx).next_up(),
        )
        .context("invalid airborne support")?;
        let mut scene = Self {
            owner,
            rasters,
            independent: Vec::new(),
            chords: Vec::new(),
            days: 0,
            weights: air::ClassWeights::uniform(),
        };
        let mut stamp = None;
        let mut owners: Vec<_> = squares.iter().collect();
        owners.sort_by_key(|s| (s.y, s.x));
        for square in owners {
            let relative = format!("z9/{}/{}/airborne.arrow", square.x, square.y);
            let Some((bytes, _)) = manifest.read_arrow(prepared, &relative)? else {
                continue;
            };
            let reader = FileReader::try_new(Cursor::new(bytes), None)?;
            let schema = reader.schema();
            assert_airborne_contract(&relative, &[RecordBatch::new_empty(schema.clone())])
                .map_err(anyhow::Error::msg)?;
            AirborneRowAccum::new(&[RecordBatch::new_empty(schema.clone())])
                .map_err(anyhow::Error::msg)?;
            let days = schema
                .metadata()
                .get("n_days")
                .and_then(|v| v.parse::<u16>().ok())
                .filter(|v| *v > 0)
                .context("airborne n_days missing")?;
            let current = schema
                .metadata()
                .get(air::SAMPLE_DAYS_BY_CLASS_KEY)
                .context("airborne class windows missing")?;
            ensure!(
                stamp.as_ref().is_none_or(|old| old == current)
                    && (scene.days == 0 || scene.days == days),
                "mixed airborne normalization windows"
            );
            scene.weights =
                air::ClassWeights::parse(Some(current), days).map_err(anyhow::Error::msg)?;
            scene.days = days;
            stamp = Some(current.clone());
            for batch in reader {
                let batch = batch?;
                let decoded = AirborneRowAccum::new(std::slice::from_ref(&batch))
                    .map_err(anyhow::Error::msg)?;
                let view = &decoded.views()[0];
                for split in [false, true] {
                    let mask = BooleanArray::from_iter(
                        view.flags
                            .iter()
                            .map(|flags| Some((flags & airborne::SPLIT_PIECE != 0) == split)),
                    );
                    let selected = filter_record_batch(&batch, &mask)?;
                    if selected.num_rows() > 0 {
                        if split {
                            scene.chords.push(selected);
                        } else {
                            scene.independent.push(selected);
                        }
                    }
                }
            }
        }
        Ok(scene)
    }
    pub fn row_counts(&self) -> (usize, usize) {
        (
            self.independent.iter().map(RecordBatch::num_rows).sum(),
            self.chords.iter().map(RecordBatch::num_rows).sum(),
        )
    }
}

#[cfg(feature = "gpu")]
#[path = "airborne_gpu.rs"]
mod gpu;

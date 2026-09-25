//! Bounded receiver batches share exact horizons between independent events and CUDA split chords.
use super::AirborneScene;
use crate::{
    airborne_pack::{
        DeviceAirborneReceiver, DeviceAirborneSource, PackedScreening, ReceiverScreening,
        AIRBORNE_REDUCTION_ROWS,
    },
    cuda_bridge::{check_cuda, DeviceBuffer, RelevantSourceCuda},
    surface_scene::SurfaceScene,
    tile_receivers::TileReceivers,
};
use anyhow::{ensure, Result};
use noise_compute::emission::aircraft as air;
#[path = "airborne_chord_gpu.rs"]
mod chords;
use rayon::prelude::*;
use source_reader::aircraft_v6::AirborneRowAccum;

#[repr(C)]
struct DeviceAirborneScreen {
    terrain: *const u32,
    terrain_max: *const f32,
    buildings: *const u32,
    building_max: *const u16,
    global_max: *const u16,
    records: u32,
}
unsafe extern "C" {
    fn relevant_source_cuda_airborne(
        sources: *const DeviceAirborneSource,
        source_count: u32,
        receivers: *const DeviceAirborneReceiver,
        receiver_count: u32,
        npd: *const f32,
        weights: *const f32,
        screen: *const DeviceAirborneScreen,
        days: f32,
        partial: *mut f32,
        output: *mut f32,
    ) -> std::ffi::c_int;
}

impl AirborneScene<'_> {
    pub fn period_powers(
        &self,
        scene: &SurfaceScene,
        receivers: &TileReceivers,
    ) -> Result<Vec<f32>> {
        ensure!(
            scene.owner == self.owner,
            "airborne scene belongs to another owner"
        );
        let count = receivers.x.len();
        ensure!(
            receivers.y.len() == count && receivers.altitude.len() == count,
            "incomplete airborne receivers"
        );
        let points: Vec<_> = receivers
            .x
            .iter()
            .zip(&receivers.y)
            .map(|(&x, &y)| scene.frame.decode(x, y))
            .collect();
        ensure!(
            points.iter().flatten().all(|v| v.is_finite())
                && receivers.altitude.iter().all(|v| v.is_finite()),
            "nonfinite airborne receivers"
        );
        let mut output = vec![0.0f32; count * 3];
        if count == 0 || (self.independent.is_empty() && self.chords.sources.is_empty()) {
            return Ok(output);
        }
        let _cuda = RelevantSourceCuda::initialize()?;
        // A union box only removes rows outside EVERY receiver's existing periodic envelope.
        let sources = self.selected_sources(&points)?;
        let device_sources = (!sources.is_empty())
            .then(|| DeviceBuffer::from_slice(&sources))
            .transpose()?;
        let npd = DeviceBuffer::from_slice(&air::NpdLuts::shared().device_luts_flat_f32())?;
        let weights = DeviceBuffer::from_slice(&self.weights.as_array().map(|v| v as f32))?;
        let chords = (!self.chords.sources.is_empty())
            .then(|| chords::DeviceChords::new(&self.chords, &self.weights)).transpose()?;
        // Two batches of 256 horizons bound staging memory whatever the tile's pixel count.
        let batches: Vec<_> = (0..count)
            .step_by(256)
            .map(|first| first..(first + 256).min(count))
            .collect();
        let screen = |batch: std::ops::Range<usize>| -> Result<Vec<ReceiverScreening>> {
            batch
                .into_par_iter()
                .map(|i| {
                    ReceiverScreening::build_cached(
                        points[i][0],
                        points[i][1],
                        receivers.altitude[i],
                        self.rasters,
                        &scene.obstacles,
                    )
                })
                .collect()
        };
        let mut screening = screen(batches[0].clone())?;
        for (index, batch) in batches.iter().enumerate() {
            // The CPU builds the next batch's horizons while the card evaluates this one.
            let next = std::thread::scope(|scope| -> Result<_> {
                let next = batches
                    .get(index + 1)
                    .map(|next| scope.spawn(|| screen(next.clone())));
                let uploaded = UploadedScreen::new(&screening)?;
                let totals = &mut output[batch.start * 3..batch.end * 3];
                if let Some(source) = &device_sources {
                    totals.copy_from_slice(&gpu_powers(source, &uploaded, &npd, &weights, self.days)?);
                }
                if let Some(chords) = &chords {
                    for (total, chord) in totals.iter_mut().zip(chords.powers(&uploaded, self.days)?) {
                        *total += chord;
                    }
                }
                next.map(|handle| handle.join().expect("airborne screening thread panicked"))
                    .transpose()
            })?;
            if let Some(next) = next {
                screening = next;
            }
        }
        ensure!(
            output.iter().all(|v| v.is_finite() && *v >= 0.0),
            "invalid airborne mean power"
        );
        Ok(output)
    }

    fn selected_sources(&self, points: &[[f64; 2]]) -> Result<Vec<DeviceAirborneSource>> {
        let envelope = air::AirborneEnvelope::covering_receivers(points)
            .ok_or_else(|| anyhow::anyhow!("airborne receivers are empty"))?;
        let mut sources = Vec::new();
        for batch in &self.independent {
            let decoded =
                AirborneRowAccum::new(std::slice::from_ref(batch)).map_err(anyhow::Error::msg)?;
            let view = &decoded.views()[0];
            for i in 0..view.len() {
                let start = view.start_lat_lon(i);
                let end = view.end_lat_lon(i);
                if !envelope.intersects_segment(start, end) {
                    continue;
                }
                if let Some(source) = DeviceAirborneSource::prepare(view, i)? {
                    sources.push(source);
                }
            }
        }
        ensure!(
            sources.len() <= u32::MAX as usize,
            "airborne source count exceeds CUDA indexing"
        );
        Ok(sources)
    }
}

struct UploadedScreen {
    terrain: DeviceBuffer<u32>,
    terrain_max: DeviceBuffer<f32>,
    buildings: DeviceBuffer<u32>,
    building_max: DeviceBuffer<u16>,
    global_max: DeviceBuffer<u16>,
    points: DeviceBuffer<DeviceAirborneReceiver>,
}
impl UploadedScreen {
    fn new(receivers: &[ReceiverScreening]) -> Result<Self> {
        let packed = PackedScreening::new(receivers);
        Ok(Self {
            terrain: DeviceBuffer::from_slice(&packed.terrain)?,
            terrain_max: DeviceBuffer::from_slice(&packed.terrain_max)?,
            buildings: DeviceBuffer::from_slice(&packed.buildings)?,
            building_max: DeviceBuffer::from_slice(&packed.building_max)?,
            global_max: DeviceBuffer::from_slice(&packed.global_max)?,
            points: DeviceBuffer::from_slice(&receivers.iter().map(|rx| rx.device).collect::<Vec<_>>())?,
        })
    }
    fn pointers(&self) -> DeviceAirborneScreen {
        DeviceAirborneScreen {
            terrain: self.terrain.as_ptr(), terrain_max: self.terrain_max.as_ptr(),
            buildings: self.buildings.as_ptr(), building_max: self.building_max.as_ptr(),
            global_max: self.global_max.as_ptr(), records: self.points.element_count() as u32,
        }
    }
}

fn gpu_powers(
    sources: &DeviceBuffer<DeviceAirborneSource>,
    receivers: &UploadedScreen,
    npd: &DeviceBuffer<f32>,
    weights: &DeviceBuffer<f32>,
    days: u16,
) -> Result<Vec<f32>> {
    let count = receivers.points.element_count();
    let parts = sources.element_count().div_ceil(AIRBORNE_REDUCTION_ROWS);
    let partial = DeviceBuffer::<f32>::uninitialized(
        count.checked_mul(parts)
            .and_then(|n| n.checked_mul(3))
            .ok_or_else(|| anyhow::anyhow!("airborne reduction overflow"))?,
    )?;
    let output = DeviceBuffer::<f32>::uninitialized(count * 3)?;
    check_cuda(unsafe {
        relevant_source_cuda_airborne(
            sources.as_ptr(),
            sources.element_count().try_into()?,
            receivers.points.as_ptr(),
            count.try_into()?,
            npd.as_ptr(),
            weights.as_ptr(),
            &receivers.pointers(),
            f32::from(days),
            partial.as_mut_ptr(),
            output.as_mut_ptr(),
        )
    })?;
    output.copy_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise_compute::{propagation::obstacle_index::ObstacleSet, types::RasterSampler};

    struct Flat;
    impl RasterSampler for Flat {
        fn elevation(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn ground_g(&self, _: f64, _: f64) -> f64 {
            0.0
        }
        fn building_enclosure(&self, _: f64, _: f64) -> f64 {
            0.0
        }
    }

    /// A departure of `class` passing `lateral` metres east of the receiver at `relative_alt`.
    fn source_beside(
        metres_per_longitude_degree: f64,
        class: usize,
        departure: bool,
        lateral: f64,
        relative_alt: f64,
    ) -> DeviceAirborneSource {
        let profile = &air::PROFILES[air::CLASS_REP_PROFILE_IDX[class] as usize];
        let (installation, a, b, c) = air::delta_i_constants(profile.installation);
        let endpoints = [
            (50.0 - 500.0 / air::M_PER_DEG_LAT) as f32,
            (14.0 + lateral / metres_per_longitude_degree) as f32,
            (50.0 + 500.0 / air::M_PER_DEG_LAT) as f32,
            (14.0 + lateral / metres_per_longitude_degree) as f32,
        ];
        let dy = (f64::from(endpoints[2]) - f64::from(endpoints[0])) * air::M_PER_DEG_LAT;
        DeviceAirborneSource {
            endpoints,
            physical: [
                (relative_alt + 4.0) as f32,
                0.0,
                dy as f32,
                0.0,
                0.0,
                a as f32,
                b as f32,
                c as f32,
                16_000.0 * 16_000.0,
                -100_000.0,
                -100_000.0,
                0.0,
                0.0,
            ],
            identity: [
                match installation {
                    air::Installation::Wing => 0,
                    air::Installation::Fuselage => 1,
                    air::Installation::Propeller => 2,
                },
                class as i32,
                i32::from(departure),
                0,
                0,
                0,
            ],
        }
    }

    #[test]
    fn independent_rows_reduce_across_parts_and_receiver_blocks() -> Result<()> {
        let _cuda = RelevantSourceCuda::initialize()?;
        let empty = ObstacleSet { indexes: vec![] };
        let receivers = (0..40)
            .map(|i| ReceiverScreening::build(50.0 + f64::from(i) * 1e-4, 14.0, 4.0, &Flat, &empty))
            .collect::<Result<Vec<_>>>()?;
        let mpdl = receivers[0].device.metres_per_longitude_degree;
        let source = source_beside(mpdl, 0, true, 200.0, 500.0);
        let rows: Vec<_> = (0..=AIRBORNE_REDUCTION_ROWS)
            .map(|i| DeviceAirborneSource {
                identity: [
                    source.identity[0],
                    source.identity[1],
                    source.identity[2],
                    (i % 3) as i32,
                    0,
                    0,
                ],
                ..source
            })
            .collect();
        let npd = DeviceBuffer::from_slice(&air::NpdLuts::shared().device_luts_flat_f32())?;
        let weights = DeviceBuffer::from_slice(&[1.0f32, 1.0])?;
        let all = gpu_powers(&DeviceBuffer::from_slice(&rows)?, &UploadedScreen::new(&receivers)?, &npd, &weights, 1)?;
        for (index, rx) in receivers.iter().enumerate() {
            let single = gpu_powers(
                &DeviceBuffer::from_slice(&[source])?,
                &UploadedScreen::new(std::slice::from_ref(rx))?,
                &npd,
                &weights,
                1,
            )?;
            ensure!(single[0] > 0.0, "receiver {index} hears no source");
            for period in 0..3 {
                let copies = rows.iter().filter(|row| row.identity[3] == period as i32).count() as f64;
                let expected = copies * f64::from(single[0]) * air::PERIOD_SECONDS[0] / air::PERIOD_SECONDS[period];
                let error = (10.0 * (f64::from(all[index * 3 + period]) / expected).log10()).abs();
                ensure!(error < 0.01, "receiver {index} period {period}: {error} dB");
            }
        }
        Ok(())
    }

    #[test]
    fn aircraft_kernel_cpu_cuda_parity_across_classes_operations_and_slants() -> Result<()> {
        let _cuda = RelevantSourceCuda::initialize()?;
        let rx =
            ReceiverScreening::build(50.0, 14.0, 4.0, &Flat, &ObstacleSet { indexes: vec![] })?;
        let luts = air::NpdLuts::shared();
        let npd = DeviceBuffer::from_slice(&luts.device_luts_flat_f32())?;
        let weights = DeviceBuffer::from_slice(&[1.0f32, 1.0])?;
        let mpdl = rx.device.metres_per_longitude_degree;
        let mut cases = 0;
        for class in 0..air::NUM_CLASSES {
            let profile = &air::PROFILES[air::CLASS_REP_PROFILE_IDX[class] as usize];
            let (installation, a, b, c) = air::delta_i_constants(profile.installation);
            for departure in [false, true] {
                for lateral in [0.0, 200.0, 1_000.0, 8_000.0] {
                    for relative_alt in [-50.0, 100.0, 500.0, 9_000.0] {
                        // (power_row, power_w, heli_db): pinned row, an interior
                        // Eq. 4-3 lerp, and a helicopter correction. Row 0 with
                        // w 0.5 is safe on every class: pinned rows repeat the
                        // anchor curve, thrust classes have ≥ 2 rows.
                        for (power_row, power_w, heli_db) in
                            [(0u8, 0.0, 0.0), (0, 0.5, 0.0), (0, 0.0, -5.0)]
                        {
                            let mut source =
                                source_beside(mpdl, class, departure, lateral, relative_alt);
                            source.physical[11] = power_w as f32;
                            source.physical[12] = heli_db as f32;
                            source.identity[5] = i32::from(power_row);
                            cases += parity_case(
                                &source,
                                mpdl,
                                luts,
                                class,
                                departure,
                                lateral,
                                relative_alt,
                                installation,
                                a,
                                b,
                                c,
                                power_row,
                                power_w,
                                heli_db,
                                &rx,
                                &npd,
                                &weights,
                            )?;
                        }
                    }
                }
            }
        }
        eprintln!("aircraft CPU/CUDA parity: {cases} cases below 0.1 dB");
        Ok(())
    }

    /// One CPU-vs-CUDA comparison; returns 1 on success.
    #[allow(clippy::too_many_arguments)]
    fn parity_case(
        source: &DeviceAirborneSource,
        mpdl: f64,
        luts: &air::NpdLuts,
        class: usize,
        departure: bool,
        lateral: f64,
        relative_alt: f64,
        installation: air::Installation,
        a: f64,
        b: f64,
        c: f64,
        power_row: u8,
        power_w: f64,
        heli_db: f64,
        rx: &ReceiverScreening,
        npd: &DeviceBuffer<f32>,
        weights: &DeviceBuffer<f32>,
    ) -> Result<u32> {
        let endpoints = source.endpoints;
        let dy = f64::from(source.physical[2]);
        let expected = air::segment_energy_kernel::<false>(
            (f64::from(endpoints[1]) - 14.0) * mpdl,
            (f64::from(endpoints[0]) - 50.0) * air::M_PER_DEG_LAT,
            0.0,
            dy,
            0.0,
            relative_alt + 4.0,
            1.0 / (dy * dy),
            dy,
            4.0,
            luts,
            class,
            departure,
            0.0,
            installation,
            power_row,
            power_w,
            heli_db,
            a,
            b,
            c,
            false,
            16_000.0 * 16_000.0,
            -100_000.0,
            -100_000.0,
            Some(&rx.terrain),
            Some(&rx.buildings),
        )
        .map(|result| {
            noise_compute::propagation::iso9613::fast_exp_f64(
                result.sel * std::f64::consts::LN_10 * 0.1,
            ) / air::PERIOD_SECONDS[0]
        })
        .unwrap_or(0.0);
        let sources = DeviceBuffer::from_slice(&[*source])?;
        let actual = f64::from(
            gpu_powers(
                &sources,
                &UploadedScreen::new(std::slice::from_ref(rx))?,
                npd,
                weights,
                1,
            )?[0],
        );
        let difference = if actual == 0.0 && expected == 0.0 {
            0.0
        } else {
            (10.0 * (actual / expected).log10()).abs()
        };
        assert!(difference < 0.1, "class={class} departure={departure} lateral={lateral} altitude={relative_alt} row={power_row} w={power_w} heli={heli_db}: {difference} dB, {actual} vs {expected}");
        Ok(1)
    }
}

#[cfg(test)]
#[path = "airborne_chord_tests.rs"]
mod chord_tests;

//! Aircraft box energies at the painter's lattice points on the card, the popup's box physics in f32.
//!
//! A square's boxes, decoded once (`DeviceFlightBoxes`), and per request its point's horizons and
//! its segments: one tile's boxes of one file and band, a block each (kernels/aircraft.cuh).

use crate::device::{Gpu, driver, read_back};
use cudarc::driver::{CudaSlice, DeviceRepr, LaunchAsync, LaunchConfig};
use paint::flight_boxes::{FlightBoxes, FlightRequest, FlightWeight};
use paint::flights::{FlightSums, tiles_heard};
use paint::square::Square;
use physics::bands::{PERIOD_HOURS, PERIODS, energy};
use physics::doc29::atmosphere::receiver_impedance_db;
use physics::doc29::box_sums::INSTALLATION_SHARE_SLANTS_M;
use physics::doc29::boxes::average_length_m;
use physics::doc29::npd::{
    METRES_PER_FOOT, NPD_DISTANCES, NPD_DISTANCES_FT, NPD_LAST_DISTANCE_M, NPD_NEAREST_SLANT_M,
    TAIL_ANCHOR_M,
};
use physics::doc29::screening::{
    GRAZING_LOSS_DB, PATH_DIFFERENCE_SLOPE_PER_M, SCREENING_CAP_DB,
    SCREENING_CEILING_ABOVE_GROUND_M,
};
use popup::aircraft::boxes::{AIRCRAFT_REACH_M, FLIGHT_SPEED_M_S};
use popup::aircraft::horizons::{
    BUILDING_SECTORS, HORIZON_BANDS, HORIZON_FLOATS, Horizons, TERRAIN_HORIZON_FLOATS,
    TERRAIN_SECTORS,
};
use rayon::prelude::*;
use tiles::aircraft::Group;

/// Threads of a segment's block (kernels/aircraft.cuh FLIGHT_THREADS).
const FLIGHT_THREADS: u32 = 128;

/// kernels/aircraft.cuh FlightBox.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DeviceFlightBox {
    east_m: f32,
    north_m: f32,
    centroid_altitude_m: f32,
    sin_axis: f32,
    cos_axis: f32,
    gradient: f32,
    gradient_spread: f32,
    average_length_m: f32,
    piece_length_m: [f32; PERIODS],
    levels_db: [[f32; NPD_DISTANCES]; PERIODS],
    tail_levels_db: [f32; PERIODS],
    lg_scaled_distance: [f32; NPD_DISTANCES],
    installation_shares: [[[f32; 3]; 2]; PERIODS],
    lambda_per_slant: [f32; PERIODS],
    screened: u32,
    helicopter: u32,
    silent: [u32; PERIODS],
}

/// kernels/aircraft.cuh FlightPoint.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DeviceFlightPoint {
    x: f32,
    y: f32,
    altitude_m: f32,
    impedance_gain: f32,
    horizons: u64,
}

/// kernels/aircraft.cuh FlightSegment.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DeviceFlightSegment {
    point: u32,
    sum: u32,
    first: u32,
    count: u32,
    weight: u32,
    within_m: f32,
    coarse_m: f32,
}

// SAFETY: plain records of f32, u32 and u64 fields in the kernel's order (checked by
// `flight_struct_sizes` when the module loads).
unsafe impl DeviceRepr for DeviceFlightBox {}
unsafe impl DeviceRepr for DeviceFlightPoint {}
unsafe impl DeviceRepr for DeviceFlightSegment {}

/// Sizes of the records above, in kernels/aircraft.cuh flight_struct_sizes's order.
pub(crate) const FLIGHT_STRUCT_SIZES: [usize; 3] = [
    size_of::<DeviceFlightBox>(),
    size_of::<DeviceFlightPoint>(),
    size_of::<DeviceFlightSegment>(),
];

/// The Rust constants kernels/aircraft.cuh reads, rounded once to f32.
pub(crate) fn aircraft_constants() -> String {
    let logs: Vec<String> = NPD_DISTANCES_FT
        .iter()
        .map(|&ft| format!("{:?}f", ft.log10() as f32))
        .collect();
    let seconds: Vec<String> = PERIOD_HOURS
        .iter()
        .map(|&hours| format!("{:?}f", (hours * 3_600.0) as f32))
        .collect();
    let define = |name: &str, value: f64| format!("#define {name} {:?}f\n", value as f32);
    [
        format!("#define NPD_DISTANCES {NPD_DISTANCES}\n"),
        format!(
            "__constant__ float LOG10_NPD_DISTANCES_FT[NPD_DISTANCES] = {{{}}};\n",
            logs.join(", ")
        ),
        format!(
            "__constant__ float PERIOD_SECONDS[PERIODS] = {{{}}};\n",
            seconds.join(", ")
        ),
        define("METRES_PER_FOOT", METRES_PER_FOOT),
        define("NPD_NEAREST_SLANT_M", NPD_NEAREST_SLANT_M),
        define("NPD_LAST_DISTANCE_M", NPD_LAST_DISTANCE_M),
        define("TAIL_ANCHOR_M", TAIL_ANCHOR_M),
        define("INSTALLATION_SHARE_NEAR_M", INSTALLATION_SHARE_SLANTS_M[0]),
        define("INSTALLATION_SHARE_FAR_M", INSTALLATION_SHARE_SLANTS_M[1]),
        define("PATH_DIFFERENCE_SLOPE_PER_M", PATH_DIFFERENCE_SLOPE_PER_M),
        define("GRAZING_LOSS_DB", GRAZING_LOSS_DB),
        define("SCREENING_CAP_DB", SCREENING_CAP_DB),
        define("AIRCRAFT_REACH_M", AIRCRAFT_REACH_M),
        format!("#define HORIZON_BANDS {HORIZON_BANDS}\n"),
        format!("#define TERRAIN_SECTORS {TERRAIN_SECTORS}\n"),
        format!("#define BUILDING_SECTORS {BUILDING_SECTORS}\n"),
        format!("#define TERRAIN_HORIZON_FLOATS {TERRAIN_HORIZON_FLOATS}\n"),
        format!("#define FLIGHT_THREADS {FLIGHT_THREADS}\n"),
    ]
    .concat()
}

/// A square's listed boxes on the card: per aircraft tile, file and band where its boxes begin.
pub struct DeviceFlightBoxes {
    boxes: CudaSlice<DeviceFlightBox>,
    first: Vec<[[u32; 2]; 2]>,
}

impl DeviceFlightBoxes {
    /// Decodes and uploads every box `flight_boxes` lists.
    pub fn upload(gpu: &Gpu, square: &Square, flight_boxes: &FlightBoxes) -> Result<Self, String> {
        let mut first = Vec::with_capacity(square.aircraft.len());
        let mut lists = Vec::new();
        let mut count = 0u32;
        for (tile, files) in square.aircraft.iter().zip(&flight_boxes.lists) {
            first.push(std::array::from_fn(|file| {
                std::array::from_fn(|band| {
                    let at = count;
                    if let Some(boxes) = [&tile.fine, &tile.far][file] {
                        lists.push((tile.tile, *boxes, &files[file][band]));
                        count += files[file][band].len() as u32;
                    }
                    at
                })
            }));
        }
        let boxes: Vec<DeviceFlightBox> = lists
            .par_iter()
            .flat_map_iter(|(tile, boxes, indices)| {
                indices.iter().map(move |&index| {
                    let record = boxes.aircraft_box(index);
                    let global = tile.global(record.place.centroid);
                    let [east_m, north_m] = square
                        .frame
                        .metres_of_steps([global.x as f64, global.y as f64]);
                    let (sin_axis, cos_axis) = record.axis_rad.sin_cos();
                    let f = |values: [f64; PERIODS]| values.map(|value| value as f32);
                    DeviceFlightBox {
                        east_m: east_m as f32,
                        north_m: north_m as f32,
                        centroid_altitude_m: record.centroid_altitude_m as f32,
                        sin_axis: sin_axis as f32,
                        cos_axis: cos_axis as f32,
                        gradient: record.gradient as f32,
                        gradient_spread: record.gradient_spread as f32,
                        average_length_m: average_length_m(
                            &record.energy_db,
                            &record.piece_length_m,
                        ) as f32,
                        piece_length_m: f(record.piece_length_m),
                        levels_db: record.energy_db.map(|levels| levels.map(|l| l as f32)),
                        tail_levels_db: f(record.tail_energy_db),
                        lg_scaled_distance: record.lg_scaled_distance.map(|l| l as f32),
                        installation_shares: record
                            .installation_shares
                            .map(|slants| slants.map(|shares| shares.map(|s| s as f32))),
                        lambda_per_slant: std::array::from_fn(|period| {
                            (record.flights_per_day[period]
                                / (PERIOD_HOURS[period] * 3_600.0)
                                / FLIGHT_SPEED_M_S) as f32
                        }),
                        screened: u32::from(
                            record.centroid_altitude_m - record.place.ground_m
                                < SCREENING_CEILING_ABOVE_GROUND_M,
                        ),
                        helicopter: u32::from(record.place.group == Group::Helicopter),
                        silent: record.energy_db.map(|levels| {
                            u32::from(levels.iter().all(|&l| l == f64::NEG_INFINITY))
                        }),
                    }
                })
            })
            .collect();
        let boxes = driver(gpu.device.htod_sync_copy(&boxes))?;
        Ok(DeviceFlightBoxes { boxes, first })
    }
}

impl Gpu {
    /// The flights of each request on the card (paint::flight_boxes::FlightBoxes::sums on the
    /// cores): per band its boxes' energy and energy times lambda.
    pub fn flight_sums(
        &self,
        square: &Square,
        flight_boxes: &FlightBoxes,
        device_boxes: &DeviceFlightBoxes,
        requests: &[FlightRequest],
    ) -> Result<Vec<[FlightSums; 2]>, String> {
        // Each request whose boxes are heard: its point with its horizons, and its segments.
        let prepared: Vec<Option<(Vec<f32>, Vec<DeviceFlightSegment>)>> = requests
            .par_iter()
            .enumerate()
            .map(|(slot, request)| -> Result<_, String> {
                let point = request.point;
                let heard = tiles_heard(square, point.position, request.within_m);
                if !flight_boxes.hears_any(&heard, request.weights) {
                    return Ok(None);
                }
                let horizons = Horizons::build(
                    &square.ground,
                    &square.obstacles,
                    point.position,
                    point.altitude_m,
                    point.own_footprint,
                )?;
                let mut floats = Vec::with_capacity(HORIZON_FLOATS);
                horizons.write_f32(&mut floats);
                let mut segments = Vec::new();
                for tile in heard {
                    let file = usize::from(!tile.fine);
                    for band in 0..2 {
                        let Some(weight) = request.weights[band] else {
                            continue;
                        };
                        let count = flight_boxes.lists[tile.number][file][band].len() as u32;
                        if count == 0 {
                            continue;
                        }
                        segments.push(DeviceFlightSegment {
                            point: slot as u32,
                            sum: (2 * slot + band) as u32,
                            first: device_boxes.first[tile.number][file][band],
                            count,
                            weight: match weight {
                                FlightWeight::One => 0,
                                FlightWeight::Far => 1,
                                FlightWeight::Near => 2,
                            },
                            within_m: request.within_m as f32,
                            coarse_m: flight_boxes.coarse_m as f32,
                        });
                    }
                }
                Ok(Some((floats, segments)))
            })
            .collect::<Result<_, String>>()?;
        let (mut horizons, mut points, mut segments) = (Vec::new(), Vec::new(), Vec::new());
        for (request, prepared) in requests.iter().zip(prepared) {
            let point = request.point;
            points.push(DeviceFlightPoint {
                x: point.position[0] as f32,
                y: point.position[1] as f32,
                altitude_m: point.altitude_m as f32,
                impedance_gain: energy(receiver_impedance_db(point.altitude_m)) as f32,
                horizons: horizons.len() as u64,
            });
            if let Some((floats, request_segments)) = prepared {
                horizons.extend(floats);
                segments.extend(request_segments);
            }
        }
        let mut sums = vec![[[[0.0; PERIODS]; 2]; 2]; requests.len()];
        if segments.is_empty() {
            return Ok(sums);
        }
        let device = &self.device;
        let horizons = driver(device.htod_sync_copy(&horizons))?;
        let points = driver(device.htod_sync_copy(&points))?;
        let segment_count = segments.len() as u32;
        let segments = driver(device.htod_sync_copy(&segments))?;
        let totals = driver(device.alloc_zeros::<f64>(4 * PERIODS * requests.len()))?;
        let stream = self.fork()?;
        let kernel = device
            .get_func("aircraft", "flight_sums")
            .ok_or("kernel flight_sums missing")?;
        let config = LaunchConfig {
            grid_dim: (segment_count, 1, 1),
            block_dim: (FLIGHT_THREADS, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: every segment's boxes lie in `boxes` (counted when they were uploaded), its point
        // in `points`, its horizons in `horizons`, its sums (2 x PERIODS doubles) in `totals`.
        driver(unsafe {
            kernel.launch_on_stream(
                &stream,
                config,
                (&device_boxes.boxes, &points, &horizons, &segments, &totals),
            )
        })?;
        let totals = read_back(&stream, &totals)?;
        for (slot, request_sums) in sums.iter_mut().enumerate() {
            for (band, band_sums) in request_sums.iter_mut().enumerate() {
                let at = (2 * slot + band) * 2 * PERIODS;
                for (kind, values) in band_sums.iter_mut().enumerate() {
                    for (period, value) in values.iter_mut().enumerate() {
                        *value = totals[at + kind * PERIODS + period];
                    }
                }
            }
        }
        Ok(sums)
    }
}

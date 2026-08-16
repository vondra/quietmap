//! Actual prepared-store CPU/CUDA parity gate for one H0 source/receiver pair.

#[path = "h0_pair_diagnostic_cpu.rs"]
mod cpu_replay;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use cudarc::driver::{CudaDevice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::Ptx;
use h3o::CellIndex;
use noise_compute::admin;
use noise_compute::compute::element::{LineLayer, LinePiece};
use noise_compute::constants::RAILWAY_REACH_CEILING;
use noise_compute::propagation::geo::point_to_segment_full;
use noise_compute::propagation::h0_streaming_reduction::{
    collect_h0_semantic_candidates, reduce_h0, H0Candidate, H0Node, H0SemanticCandidateRecord,
};
use noise_compute::propagation::streaming_reduction::{
    source_frame_mlon, source_frame_vector_from_world, MetricVector, SourceId64, SpanDecision,
};
use noise_gpu::{
    flatten_obstacles, pack_barrier_rows, pack_sources, upload_obstacle_flat,
    H0_PAIR_DIAGNOSTIC_ABI_VERSION, H0_PAIR_DIAGNOSTIC_HEADER_WORDS, H0_PAIR_DIAGNOSTIC_MAGIC,
    H0_PAIR_DIAGNOSTIC_NODE_BASE, H0_PAIR_DIAGNOSTIC_NODE_WORDS, H0_PAIR_DIAGNOSTIC_RECORD_BASE,
    H0_PAIR_DIAGNOSTIC_RECORD_WORDS,
};
use raster_reader::fused_tile_z13::{default_batch_size, TileBatch, TILE_PX};
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::h0_pair_reference::evaluate_h0_pair;
use tile_painter::region_runner::tile_centre_r4;
use tile_painter::source_line::LineRow;
use tile_painter::source_loader_barrier::BarrierData;
use tile_painter::source_loader_obstacle::ObstacleData;
use tile_painter::source_loader_rail::RailData;
use tile_painter::source_loader_road::RoadData;
use tile_painter::wire_hm3::collapse_lden_surface_u8;

const SCATTER_PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/scatter.ptx"));
const MAX_DIAGNOSTIC_RAW_RECORDS: usize = 4_000_000;
const REACH_BOUNDARY_GUARD_M: f64 = 10.0;

#[derive(Clone, Copy)]
enum DiagnosticLayer {
    Road,
    Rail,
}

impl DiagnosticLayer {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "road" => Ok(Self::Road),
            "rail" => Ok(Self::Rail),
            _ => bail!("layer must be road or rail, got {value:?}"),
        }
    }

    const fn h0_layer(self) -> LineLayer {
        match self {
            Self::Road => LineLayer::Road,
            Self::Rail => LineLayer::Rail,
        }
    }

    const fn halo_m(self) -> f64 {
        match self {
            Self::Road => 10_000.0,
            Self::Rail => RAILWAY_REACH_CEILING,
        }
    }

    const fn tag(self) -> f64 {
        match self {
            Self::Road => 0.0,
            Self::Rail => 1.0,
        }
    }

    fn load_rows(self, h3r4: &Path, ring: &[u64], centre: CellIndex) -> Result<Vec<LineRow>> {
        let centre = h3o::LatLng::from(centre);
        let admin = admin::admin_for_latlng(centre.lat(), centre.lng());
        Ok(match self {
            Self::Road => RoadData::load_for_r4s(h3r4, ring, admin)?.into_rows(),
            Self::Rail => RailData::load_for_r4s(h3r4, ring, admin)?.into_rows(),
        })
    }
}

struct Arguments {
    h3r4: PathBuf,
    output: PathBuf,
    zoom: u8,
    tile_x: u32,
    tile_y: u32,
    layer: DiagnosticLayer,
    pixel_y: u32,
    pixel_x: u32,
}

fn parse_arguments() -> Result<Arguments> {
    let values: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        values.len() == 6 || values.len() == 8,
        "usage: h0-pair-diagnostic H3R4_DIR OUTPUT_DIR Z X Y road|rail [PY PX]"
    );
    let parse_u32 = |index: usize, name: &str| {
        values[index]
            .parse::<u32>()
            .with_context(|| format!("invalid {name}: {:?}", values[index]))
    };
    let pixel_y = if values.len() == 8 {
        parse_u32(6, "PY")?
    } else {
        256
    };
    let pixel_x = if values.len() == 8 {
        parse_u32(7, "PX")?
    } else {
        256
    };
    ensure!(pixel_y < TILE_PX as u32 && pixel_x < TILE_PX as u32);
    Ok(Arguments {
        h3r4: PathBuf::from(&values[0]),
        output: PathBuf::from(&values[1]),
        zoom: values[2]
            .parse::<u8>()
            .with_context(|| format!("invalid Z: {:?}", values[2]))?,
        tile_x: parse_u32(3, "X")?,
        tile_y: parse_u32(4, "Y")?,
        layer: DiagnosticLayer::parse(&values[5])?,
        pixel_y,
        pixel_x,
    })
}

fn select_source(
    rows: &[LineRow],
    receiver_latitude: f64,
    receiver_longitude: f64,
) -> Result<(usize, &LineRow, f64)> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let distance = point_to_segment_full(
                receiver_latitude,
                receiver_longitude,
                line.start_lat,
                line.start_lon,
                line.end_lat,
                line.end_lon,
            )
            .d_endpoint_m;
            (distance + REACH_BOUNDARY_GUARD_M <= line.max_distance_m)
                .then_some((index, line, distance))
        })
        .min_by(|left, right| {
            left.2
                .total_cmp(&right.2)
                .then_with(|| left.0.cmp(&right.0))
        })
        .context("no source reaches the selected receiver")
}

fn node_words(node: &H0Node) -> [u64; H0_PAIR_DIAGNOSTIC_NODE_WORDS] {
    [
        node.line_node.position_m[0].to_bits(),
        node.line_node.position_m[1].to_bits(),
        node.line_node.piece_fraction.to_bits(),
        node.line_node.weight.to_bits(),
        node.line_node.placement_distance_m.to_bits(),
        node.line_node.node_x_m.to_bits(),
        node.node_distance_m.to_bits(),
        node.line_node.arm as i64 as u64,
    ]
}

fn write_words(path: &Path, words: impl IntoIterator<Item = u64>) -> Result<()> {
    let mut output = BufWriter::new(File::create(path)?);
    for word in words {
        output.write_all(&word.to_le_bytes())?;
    }
    output.flush()?;
    Ok(())
}

fn semantic_words(record: &H0SemanticCandidateRecord) -> [u64; 7] {
    [
        record.source_id_bits,
        record.endpoint0_x_bits,
        record.endpoint0_y_bits,
        record.endpoint1_x_bits,
        record.endpoint1_y_bits,
        u64::from(record.near_f32_bits),
        u64::from(record.height_f32_bits),
    ]
}

fn candidate_from_record(record: &[u64]) -> Result<H0Candidate> {
    ensure!(record.len() == H0_PAIR_DIAGNOSTIC_RECORD_WORDS);
    let near_bits = record[5] as u32;
    let height_bits = (record[5] >> 32) as u32;
    let candidate = H0Candidate::from_metric_segment(
        SourceId64::from_bits(record[0]),
        MetricVector::new(f64::from_bits(record[1]), f64::from_bits(record[2])),
        MetricVector::new(f64::from_bits(record[3]), f64::from_bits(record[4])),
        f32::from_bits(height_bits),
    )
    .map_err(|error| anyhow::anyhow!("device candidate is invalid: {error:?}"))?;
    ensure!(candidate.near_f32().to_bits() == near_bits);
    Ok(candidate)
}

fn device_semantic_record(record: &[u64]) -> Result<Option<H0SemanticCandidateRecord>> {
    ensure!(record.len() == H0_PAIR_DIAGNOSTIC_RECORD_WORDS);
    match record[6] {
        value if value == SpanDecision::Outside as u64 => Ok(None),
        value if value == SpanDecision::NearGuardedDegenerate as u64 => Ok(None),
        value if value == SpanDecision::HardFault as u64 => {
            bail!("device S_pair verdict contains a hard fault")
        }
        value if value == SpanDecision::Overlaps as u64 => {
            // Gate the verdict emitted by CUDA. Do not feed the device geometry
            // back through the Rust constructor and call that device parity.
            let candidate = candidate_from_record(record)?;
            Ok(Some(H0SemanticCandidateRecord::from(candidate)))
        }
        value => bail!("unknown device S_pair verdict {value}"),
    }
}

fn main() -> Result<()> {
    let arguments = parse_arguments()?;
    ensure!(arguments.h3r4.is_dir(), "missing H3R4_DIR");
    fs::create_dir(&arguments.output).with_context(|| {
        format!(
            "diagnostic output must be a fresh directory: {}",
            arguments.output.display()
        )
    })?;

    let r4 = tile_centre_r4(arguments.zoom, arguments.tile_x, arguments.tile_y)
        .context("tile centre is outside H3")?;
    let centre = CellIndex::try_from(r4)?;
    let ring: Vec<u64> = centre
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let prepared = std::env::var("NOISE_GPU_PREPARED")
        .context("NOISE_GPU_PREPARED must name the manifest-pinned prepared root")?;
    let rasters = RealRasters::new(Path::new(&prepared));
    let batch_size = default_batch_size();
    let batch_x = (arguments.tile_x / batch_size) * batch_size;
    let batch_y = (arguments.tile_y / batch_size) * batch_size;
    let mut batch = TileBatch::build(
        arguments.zoom,
        batch_x,
        batch_y,
        batch_size,
        arguments.layer.halo_m(),
        &rasters,
    );
    let tile_index =
        ((arguments.tile_y - batch_y) * batch_size + arguments.tile_x - batch_x) as usize;
    let rows = arguments.layer.load_rows(&arguments.h3r4, &ring, centre)?;
    let barrier_data = BarrierData::load_for_r4s(&arguments.h3r4, &ring)?;
    let obstacle_data = ObstacleData::load_for_r4s(&arguments.h3r4, r4, &ring)?;
    let obstacle_set = obstacle_data
        .set()
        .context("actual-store parity requires vector obstacles")?;
    tile_painter::source_loader_obstacle::bake_tile_vector_rx_refl(
        &mut batch.tiles[tile_index],
        obstacle_set,
    );
    let tile = &batch.tiles[tile_index];
    let receiver_latitude = tile.rx_lat[arguments.pixel_y as usize];
    let receiver_longitude = tile.rx_lon[arguments.pixel_x as usize];
    let (source_index, source, source_distance_m) =
        select_source(&rows, receiver_latitude, receiver_longitude)?;
    let barriers = barrier_data.for_tile(&tile.bbox, arguments.layer.halo_m());
    let flat = flatten_obstacles(obstacle_set);
    let raw_cpu = cpu_replay::collect_raw_candidates(
        &flat,
        &barriers,
        source,
        receiver_latitude,
        receiver_longitude,
    )?;
    ensure!(
        raw_cpu.len() <= MAX_DIAGNOSTIC_RAW_RECORDS / 2,
        "CPU raw walk leaves no bounded CUDA diagnostic headroom: {} > {}",
        raw_cpu.len(),
        MAX_DIAGNOSTIC_RAW_RECORDS / 2
    );
    let device_record_capacity = raw_cpu
        .len()
        .saturating_mul(2)
        .saturating_add(4096)
        .clamp(65_536, MAX_DIAGNOSTIC_RAW_RECORDS);
    ensure!(
        device_record_capacity <= MAX_DIAGNOSTIC_RAW_RECORDS,
        "diagnostic-only record ceiling exceeded: {} > {}",
        device_record_capacity,
        MAX_DIAGNOSTIC_RAW_RECORDS
    );

    let source_mlon = source_frame_mlon(source.start_lat, source.end_lat)
        .map_err(|error| anyhow::anyhow!("source mlon: {error:?}"))?;
    let source0 = source_frame_vector_from_world(
        source.start_lat,
        source.start_lon,
        receiver_latitude,
        receiver_longitude,
        source_mlon,
    )
    .map_err(|error| anyhow::anyhow!("source start frame: {error:?}"))?;
    let source1 = source_frame_vector_from_world(
        source.end_lat,
        source.end_lon,
        receiver_latitude,
        receiver_longitude,
        source_mlon,
    )
    .map_err(|error| anyhow::anyhow!("source end frame: {error:?}"))?;
    let piece = LinePiece {
        start_m: [source0.x, source0.y],
        end_m: [source1.x, source1.y],
        emission_per_m: 1.0,
    };
    let cpu_reduction = reduce_h0(
        piece,
        arguments.layer.h0_layer(),
        raw_cpu.iter().map(|entry| entry.candidate),
    )
    .map_err(|error| anyhow::anyhow!("CPU H0 reduction: {error:?}"))?;
    let cpu_semantic =
        collect_h0_semantic_candidates(piece, raw_cpu.iter().map(|entry| entry.candidate))
            .map_err(|error| anyhow::anyhow!("CPU S_pair: {error:?}"))?;
    let cpu_path = evaluate_h0_pair(
        tile,
        source,
        &barriers,
        Some(obstacle_set),
        arguments.pixel_y as usize,
        arguments.pixel_x as usize,
        arguments.layer.h0_layer(),
        raw_cpu.iter().map(|entry| entry.candidate),
        Some(noise_compute::propagation::path_profile::CoarseMid {
            src_zone_m: 600.0,
            rx_zone_m: 600.0,
            mid_stride: 3,
        }),
    )
    .map_err(|error| anyhow::anyhow!("CPU H0 physical path: {error:?}"))?;
    ensure!(
        cpu_path.reduction.nodes().len() == cpu_reduction.nodes().len()
            && cpu_path.reduction.admitted_mask_words() == cpu_reduction.admitted_mask_words(),
        "CPU physical reference did not consume the landed reducer result"
    );

    let dev = CudaDevice::new(0).context("CUDA device")?;
    dev.load_ptx(
        Ptx::from_src(SCATTER_PTX),
        "h0-pair-diagnostic",
        &["h0_pair_diagnostic", "h0_pair_path_diagnostic"],
    )
    .context("load diagnostic PTX")?;
    let function = dev
        .get_func("h0-pair-diagnostic", "h0_pair_diagnostic")
        .context("h0_pair_diagnostic entry")?;
    let path_function = dev
        .get_func("h0-pair-diagnostic", "h0_pair_path_diagnostic")
        .context("h0_pair_path_diagnostic entry")?;
    let obstacle_dev = upload_obstacle_flat(&dev, flat)?;
    let barrier_count = barriers.len();
    let barrier_dev = dev.htod_copy(pack_barrier_rows(&barriers))?;
    let noise_gpu::SourceBuffers { seg, sp, semis } = pack_sources(std::slice::from_ref(source));
    let segment_dev = dev.htod_copy(seg)?;
    let source_parameters_dev = dev.htod_copy(sp)?;
    let emission_dev = dev.htod_copy(semis)?;
    let output_words = H0_PAIR_DIAGNOSTIC_RECORD_BASE
        .checked_add(
            device_record_capacity
                .checked_mul(H0_PAIR_DIAGNOSTIC_RECORD_WORDS)
                .context("diagnostic output size overflow")?,
        )
        .context("diagnostic output size overflow")?;
    let mut output_dev = dev.alloc_zeros::<u64>(output_words)?;
    unsafe {
        function.launch(
            LaunchConfig {
                grid_dim: (1, 1, 1),
                block_dim: (1, 1, 1),
                shared_mem_bytes: 0,
            },
            (
                &obstacle_dev.table,
                &barrier_dev,
                barrier_count as i32,
                &segment_dev,
                receiver_latitude,
                receiver_longitude,
                arguments.layer.h0_layer().d_floor_m(),
                &mut output_dev,
                device_record_capacity as u64,
            ),
        )?;
    }
    let output = dev.dtoh_sync_copy(&output_dev)?;
    ensure!(output.len() >= H0_PAIR_DIAGNOSTIC_HEADER_WORDS);
    ensure!(output[0] == H0_PAIR_DIAGNOSTIC_MAGIC);
    ensure!(output[1] == H0_PAIR_DIAGNOSTIC_ABI_VERSION as u64);
    ensure!(output[2] == output[3], "device raw capture is incomplete");
    ensure!(output[4] == 0, "device diagnostic record overflow");
    ensure!(output[5] == 0, "device hard geometry fault");
    ensure!(output[7] == cpu_reduction.nodes().len() as u64);
    ensure!(output[10] == source0.x.to_bits());
    ensure!(output[11] == source0.y.to_bits());
    ensure!(output[12] == source1.x.to_bits());
    ensure!(output[13] == source1.y.to_bits());
    ensure!(output[18] == source_mlon.to_bits());
    ensure!(
        output[19] == output[2],
        "device mask walk visit count mismatch"
    );
    ensure!(output[21] == 0, "device node iterator fault");
    ensure!(output[22] == 1, "device refused diagnostic output");

    let device_node_words = &output[H0_PAIR_DIAGNOSTIC_NODE_BASE
        ..H0_PAIR_DIAGNOSTIC_NODE_BASE
            + cpu_reduction.nodes().len() * H0_PAIR_DIAGNOSTIC_NODE_WORDS];
    let cpu_node_words: Vec<u64> = cpu_reduction.nodes().iter().flat_map(node_words).collect();
    ensure!(device_node_words == cpu_node_words);
    let mut cpu_mask = [0_u64; 2];
    for (index, word) in cpu_reduction.admitted_mask_words().iter().enumerate() {
        cpu_mask[index] = *word;
    }
    ensure!(output[8..10] == cpu_mask);

    let device_raw_count = output[3] as usize;
    let device_record_words = &output[H0_PAIR_DIAGNOSTIC_RECORD_BASE
        ..H0_PAIR_DIAGNOSTIC_RECORD_BASE + device_raw_count * H0_PAIR_DIAGNOSTIC_RECORD_WORDS];
    let cpu_record_words: Vec<u64> = raw_cpu.iter().flat_map(|entry| entry.record).collect();
    let raw_store_exact = device_record_words == cpu_record_words;
    let mut device_semantic_records: Vec<_> = device_record_words
        .chunks_exact(H0_PAIR_DIAGNOSTIC_RECORD_WORDS)
        .map(device_semantic_record)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    device_semantic_records.sort_unstable();
    for records in device_semantic_records.windows(2) {
        ensure!(
            records[0].source_id_bits != records[1].source_id_bits || records[0] == records[1],
            "device S_pair maps one provenience to different geometry"
        );
    }
    device_semantic_records.dedup();
    ensure!(device_semantic_records == cpu_semantic.records);

    let halo = &tile.halo;
    let (latitude_min, longitude_min, inverse_cell_degrees, halo_rows, halo_columns) = halo.geom();
    let meta = vec![
        halo_rows as f64,
        halo_columns as f64,
        latitude_min,
        longitude_min,
        inverse_cell_degrees,
        tile.bbox.north_lat,
        tile.bbox.south_lat,
        tile.bbox.west_lon,
        tile.bbox.east_lon,
        0.0,
        16.0,
        barrier_count as f64,
        1.0,
        noise_gpu::OUT_SLOTS_H0 as f64,
        arguments.layer.tag(),
        0.0,
        arguments.pixel_y as f64,
        arguments.pixel_x as f64,
    ];
    let elevation: Vec<f32> = halo.pixels().iter().map(|pixel| pixel.elevation).collect();
    let mut cover = Vec::with_capacity(halo_rows * halo_columns * 3);
    for pixel in halo.pixels() {
        cover.extend_from_slice(&[pixel.building, pixel.forest, pixel.imd]);
    }
    let mut receiver_latlon = Vec::with_capacity(TILE_PX * 2);
    receiver_latlon.extend_from_slice(&tile.rx_lat);
    receiver_latlon.extend_from_slice(&tile.rx_lon);
    let mut receiver_alt_reflection = Vec::with_capacity(TILE_PX * TILE_PX * 2);
    for index in 0..TILE_PX * TILE_PX {
        receiver_alt_reflection.push(tile.rx_alt_m[index]);
        receiver_alt_reflection.push(tile.rx_refl_db[index]);
    }
    let elevation_dev = dev.htod_copy(elevation)?;
    let inner_dev = dev.htod_copy(tile.inner_elev_m.clone())?;
    let cover_dev = dev.htod_copy(cover)?;
    let meta_dev = dev.htod_copy(meta)?;
    let receiver_latlon_dev = dev.htod_copy(receiver_latlon)?;
    let receiver_alt_reflection_dev = dev.htod_copy(receiver_alt_reflection)?;
    let mut path_output_dev = dev.alloc_zeros::<u64>(40)?;
    unsafe {
        path_function.launch(
            LaunchConfig {
                grid_dim: (1, 1, 1),
                block_dim: (1, 1, 1),
                shared_mem_bytes: 0,
            },
            noise_gpu::line_kernel_arguments!(
                &elevation_dev,
                &inner_dev,
                &cover_dev,
                &meta_dev,
                &segment_dev,
                &source_parameters_dev,
                &emission_dev,
                &receiver_latlon_dev,
                &receiver_alt_reflection_dev,
                &barrier_dev,
                &obstacle_dev.table,
                &mut path_output_dev,
            ),
        )?;
    }
    let path_output = dev.dtoh_sync_copy(&path_output_dev)?;
    ensure!(path_output[0] == H0_PAIR_DIAGNOSTIC_MAGIC);
    ensure!(path_output[1] == 2, "unexpected H0 physical diagnostic ABI");
    ensure!(
        path_output[2..5] == [0, 0, 0],
        "H0 physical diagnostic hard fault: {:?}",
        &path_output[2..5]
    );
    ensure!(
        path_output[39] == 1,
        "device physical output was not accepted"
    );
    let mut device_period_band_power = [[0.0_f64; 8]; 3];
    let mut maximum_band_delta_db = 0.0_f64;
    for period in 0..3 {
        for band in 0..8 {
            let device = f64::from_bits(path_output[10 + period * 8 + band]);
            let cpu = cpu_path.period_band_power[period][band];
            ensure!(
                (device > 0.0) == (cpu > 0.0),
                "per-band presence mismatch at period {period}, band {band}: cpu={cpu:e}, device={device:e}"
            );
            if device > 0.0 {
                let delta_db = (10.0 * (device / cpu).log10()).abs();
                maximum_band_delta_db = maximum_band_delta_db.max(delta_db);
            }
            device_period_band_power[period][band] = device;
        }
    }
    ensure!(
        maximum_band_delta_db <= 0.5,
        "per-band H0 CPU/CUDA delta {maximum_band_delta_db:.6} dB exceeds 0.5 dB"
    );
    let device_period_power_f32 = [
        f32::from_bits(path_output[34] as u32),
        f32::from_bits(path_output[35] as u32),
        f32::from_bits(path_output[36] as u32),
    ];
    let mut cpu_accumulator = TileAccumulator::new();
    let mut device_accumulator = TileAccumulator::new();
    cpu_accumulator.energy[..3].copy_from_slice(&cpu_path.period_power_f32);
    device_accumulator.energy[..3].copy_from_slice(&device_period_power_f32);
    let cpu_hm3_byte = collapse_lden_surface_u8(&cpu_accumulator)[0];
    let device_hm3_byte = collapse_lden_surface_u8(&device_accumulator)[0];
    ensure!(
        cpu_hm3_byte == device_hm3_byte,
        "one-pair H0 HM3 byte mismatch: CPU={cpu_hm3_byte}, CUDA={device_hm3_byte}"
    );

    write_words(
        &arguments.output.join("cpu-raw-records.bin"),
        cpu_record_words.iter().copied(),
    )?;
    write_words(
        &arguments.output.join("device-raw-records.bin"),
        device_record_words.iter().copied(),
    )?;
    write_words(
        &arguments.output.join("cpu-nodes.bin"),
        cpu_node_words.iter().copied(),
    )?;
    write_words(
        &arguments.output.join("device-nodes.bin"),
        device_node_words.iter().copied(),
    )?;
    write_words(
        &arguments.output.join("semantic-set.bin"),
        cpu_semantic.records.iter().flat_map(semantic_words),
    )?;
    write_words(
        &arguments.output.join("device-header.bin"),
        output[..H0_PAIR_DIAGNOSTIC_HEADER_WORDS].iter().copied(),
    )?;
    write_words(
        &arguments.output.join("cpu-period-band-power.bin"),
        cpu_path
            .period_band_power
            .iter()
            .flatten()
            .map(|value| value.to_bits()),
    )?;
    write_words(
        &arguments.output.join("device-period-band-power.bin"),
        device_period_band_power
            .iter()
            .flatten()
            .map(|value| value.to_bits()),
    )?;
    write_words(
        &arguments.output.join("device-path-header.bin"),
        path_output.iter().copied(),
    )?;
    let mut report = BufWriter::new(File::create(arguments.output.join("report.txt"))?);
    writeln!(report, "H0_PAIR_DIAGNOSTIC=PASS")?;
    writeln!(report, "r4={r4:015x}")?;
    writeln!(
        report,
        "tile=z{}/{}/{}",
        arguments.zoom, arguments.tile_x, arguments.tile_y
    )?;
    writeln!(report, "pixel={},{}", arguments.pixel_y, arguments.pixel_x)?;
    writeln!(
        report,
        "receiver_lat_bits={:016x}",
        receiver_latitude.to_bits()
    )?;
    writeln!(
        report,
        "receiver_lon_bits={:016x}",
        receiver_longitude.to_bits()
    )?;
    writeln!(report, "region_source_rows={}", rows.len())?;
    writeln!(report, "selected_source_index={source_index}")?;
    writeln!(
        report,
        "selected_source_distance_m={source_distance_m:.17e}"
    )?;
    writeln!(
        report,
        "selected_source_reach_margin_m={:.17e}",
        source.max_distance_m - source_distance_m
    )?;
    writeln!(report, "barrier_rows={barrier_count}")?;
    writeln!(report, "cpu_raw_candidate_visits={}", raw_cpu.len())?;
    writeln!(report, "device_raw_candidate_visits={device_raw_count}")?;
    writeln!(
        report,
        "cpu_mask_guarded_visits={}",
        cpu_reduction.guarded_degenerate_candidate_count()
    )?;
    writeln!(report, "device_mask_guarded_visits={}", output[20])?;
    writeln!(
        report,
        "cpu_span_guarded_visits={}",
        cpu_semantic.guarded_degenerate_visits
    )?;
    writeln!(report, "device_span_guarded_visits={}", output[6])?;
    writeln!(
        report,
        "semantic_candidate_count={}",
        cpu_semantic.records.len()
    )?;
    writeln!(report, "node_count={}", cpu_reduction.nodes().len())?;
    writeln!(report, "mask0={:016x}", cpu_mask[0])?;
    writeln!(report, "mask1={:016x}", cpu_mask[1])?;
    writeln!(report, "hard_faults=0")?;
    writeln!(
        report,
        "raw_store_cpu_cuda_exact={}",
        if raw_store_exact {
            "YES"
        } else {
            "NO_NOT_A_GATE"
        }
    )?;
    writeln!(report, "device_span_verdict_is_gate=YES")?;
    writeln!(report, "semantic_set_cpu_cuda_exact=YES")?;
    writeln!(report, "nodes_mask_cpu_cuda_exact=YES")?;
    writeln!(report, "per_band_cpu_cuda_presence_exact=YES")?;
    writeln!(
        report,
        "per_band_cpu_cuda_max_delta_db={maximum_band_delta_db:.9}"
    )?;
    writeln!(report, "one_pair_hm3_byte_cpu={cpu_hm3_byte}")?;
    writeln!(report, "one_pair_hm3_byte_cuda={device_hm3_byte}")?;
    writeln!(report, "one_pair_hm3_byte_exact=YES")?;
    report.flush()?;
    Ok(())
}

//! Popup–painter parity on real receivers, both lanes against a converged line quadrature: the
//! popup in process (`noise_compute::compute_at_point` at the façade receiver), the painter's
//! pair kernel at the same receiver, and the painter again with every road and rail line within
//! 400 m cut into pieces no longer than one metre (or the requested convergence-check spacing). Reads `lat lon [cohort]` lines on stdin,
//! writes one tab-separated row per receiver and the ladder per layer on stderr. Optional
//! five popup layer levels after the cohort mean lat/lon is an already resolved façade; this
//! allows the CPU lane to run beside its prepared inputs while the GPU uses staged support.
use anyhow::{ensure, Context, Result};
use noise_compute::types::{LayerKind, RasterSampler, Receiver};
use relevant_source_gpu::{
    cuda_bridge::RelevantSourceCuda,
    input_manifest::{parse_digest, InputManifest},
    native_sources::SurfaceSource,
    surface_gpu::SurfaceGpu,
    surface_scene::SurfaceScene,
};
use std::{io::{BufRead, Write}, path::PathBuf};

/// Layers both lanes paint: road, rail, industrial, building (with leisure), ship.
const LAYERS: [(LayerKind, u8); 5] = [
    (LayerKind::Road, 0),
    (LayerKind::Railway, 1),
    (LayerKind::Industrial, 2),
    (LayerKind::Building, 3),
    (LayerKind::Ship, 5),
];
/// Lines within this distance of a receiver are cut finer for the converged lane (w2-parity).
const CONVERGED_RADIUS_M: f32 = 400.0;
/// Maximum reference piece length; rerun at 0.25 m to check convergence.
const CONVERGED_PIECE_LENGTH_M: f32 = 1.0;
/// Receivers per converged batch (their cut lines replace the scene's sources for one launch).
const CONVERGED_BATCH: usize = 32;
/// A layer counts at a receiver when either side reaches the display floor (w2-parity).
const COUNTED_FLOOR_DB: f64 = 30.0;

type InputReceiver = (f64, f64, String, Option<[f64; 5]>);

struct Probe {
    cohort: String,
    facade: [f64; 2],
    popup: [f64; 5],
    painter: [f64; 5],
    converged: [f64; 5],
}

fn lden(periods: [f64; 3]) -> f64 {
    let db = |energy: f64| 10.0 * energy.max(1e-30).log10();
    noise_compute::periods::compute_lden(db(periods[0]), db(periods[1]), db(periods[2]))
}

fn layer_lden(energies: &tile_painter::corner_store::CornerEnergy) -> [f64; 5] {
    LAYERS.map(|(_, layer)| {
        let mut periods = [0.0_f64; 3];
        for entry in energies.0.iter().filter(|entry| entry.layer == layer) {
            for (sum, value) in periods.iter_mut().zip(entry.periods) {
                *sum += f64::from(value);
            }
        }
        lden(periods)
    })
}

fn segment_distance_m(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (sx, sy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = sx * sx + sy * sy;
    let t = if length_sq > 0.0 {
        (((p[0] - a[0]) * sx + (p[1] - a[1]) * sy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * sx).hypot(p[1] - a[1] - t * sy)
}

/// The scene's sources with every road and rail line near one of `receivers` cut into pieces.
fn converged_sources(original: &[SurfaceSource], receivers: &[[f32; 2]], piece_m: f32) -> Vec<SurfaceSource> {
    let mut sources = Vec::with_capacity(original.len());
    for source in original {
        let d = source.device;
        let (a, b) = ([d.start_x_m, d.start_y_m], [d.end_x_m, d.end_y_m]);
        let nearest = receivers.iter().map(|r| segment_distance_m(*r, a, b)).fold(f32::INFINITY, f32::min);
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if source.layer > 1 || nearest > CONVERGED_RADIUS_M || length <= piece_m {
            sources.push(source.clone());
            continue;
        }
        let pieces = (length / piece_m).ceil() as usize;
        for k in 0..pieces {
            let (f0, f1) = (k as f32 / pieces as f32, (k + 1) as f32 / pieces as f32);
            let mut piece = source.clone();
            piece.device.start_x_m = a[0] + (b[0] - a[0]) * f0;
            piece.device.start_y_m = a[1] + (b[1] - a[1]) * f0;
            piece.device.end_x_m = a[0] + (b[0] - a[0]) * f1;
            piece.device.end_y_m = a[1] + (b[1] - a[1]) * f1;
            piece.device.extent_m = d.extent_m / pieces as f32;
            sources.push(piece);
        }
    }
    sources
}

/// The popup physics at the pixel's own outdoor receiver and its per-layer Lden, in process
/// (the painter evaluates the same point; building pixels copy a stored façade row instead).
fn popup(prepared: &std::path::Path, rasters: &raster_reader::RealRasters, lat: f64, lon: f64) -> Result<([f64; 2], [f64; 5])> {
    let obstacles = source_reader::structure_store::load_obstacle_set(prepared, lat, lon).map_err(anyhow::Error::msg)?;
    let sources = source_reader::collect_sources_at_point(prepared, lat, lon).map_err(anyhow::Error::msg)?;
    ensure!(sources.unavailable_layers.iter().all(|layer| *layer == "aircraft"),
        "popup surface layers unavailable: {:?}", sources.unavailable_layers);
    let checked = raster_reader::CheckedRasters::new(rasters);
    let sampler = noise_compute::propagation::obstacle_index::VectorReflectionSampler { inner: &checked, set: &obstacles, own_footprint: None };
    let receiver = Receiver::new(lat, lon, sampler.elevation(lat, lon));
    let result = noise_compute::compute_at_point(
        &receiver,
        &sources.roads,
        &sources.railways,
        &sources.buildings,
        &sources.industrial,
        &sources.ships,
        &obstacles,
        &sampler,
        None,
    );
    checked.ensure_valid().map_err(|error| anyhow::anyhow!("{error}"))?;
    let levels = LAYERS.map(|(kind, _)| {
        result.sources.iter().find(|layer| layer.source_type == kind).map_or(-113.6, |layer| layer.periods.lden_db)
    });
    Ok(([lat, lon], levels))
}

/// Share of counted receivers whose |a − b| exceeds each rung.
fn ladder(probes: &[&Probe], a: impl Fn(&Probe) -> f64, b: impl Fn(&Probe) -> f64) -> String {
    let counted: Vec<f64> = probes
        .iter()
        .filter(|p| a(p).max(b(p)) >= COUNTED_FLOOR_DB)
        .map(|p| (a(p) - b(p)).abs())
        .collect();
    let share = |rung: f64| 100.0 * counted.iter().filter(|&&d| d > rung).count() as f64 / counted.len().max(1) as f64;
    let count = |rung: f64| counted.iter().filter(|&&d| d > rung).count();
    format!(
        "n {:5}  >0.5 {:5.1} %  >1 {:5.2} %  >3 {} ({:.3} %)  >6 {} ({:.4} %)",
        counted.len(),
        share(0.5),
        share(1.0),
        count(3.0),
        share(3.0),
        count(6.0),
        share(6.0)
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!((6..=8).contains(&args.len()), "usage: parity-probe PREPARED RASTERS MANIFEST SHA256 z9/X/Y [reference_piece_m] [batch_size] < receivers");
    let piece_m: f32 = args.get(6).map(|value| value.parse()).transpose()?.unwrap_or(CONVERGED_PIECE_LENGTH_M);
    ensure!(piece_m.is_finite() && (0.01..=CONVERGED_PIECE_LENGTH_M).contains(&piece_m),
        "reference piece length must be between 0.01 and 1 m");
    let batch_size: usize = args.get(7).map(|value| value.parse()).transpose()?.unwrap_or(CONVERGED_BATCH);
    ensure!((1..=CONVERGED_BATCH).contains(&batch_size), "batch size must be between 1 and {CONVERGED_BATCH}");
    let prepared = PathBuf::from(&args[1]);
    let rasters = raster_reader::RealRasters::new(&PathBuf::from(&args[2]));
    noise_compute::square_country_city::set_square_country_city_prepared_directory(&prepared);
    let manifest = InputManifest::open(&PathBuf::from(&args[3]), parse_digest(&args[4])?)?;
    let owner = grid::parse_square_name(&args[5]).context("owner z9/X/Y")?;
    let input = std::io::stdin();
    let mut lines = input.lock().lines();
    let started = std::time::Instant::now();
    let cuda = RelevantSourceCuda::initialize()?;
    let host = SurfaceScene::load(owner, &prepared, &manifest, &rasters)?;
    let mut scene = SurfaceGpu::upload(host)?;
    let original: Vec<_> = scene.host.sources.iter().filter(|s| LAYERS.iter().any(|(_, layer)| *layer == s.layer)).cloned().collect();
    eprintln!("pair-kernel probe: {} sources, reference pieces <= {piece_m} m within {CONVERGED_RADIUS_M} m; excludes tile partition/interpolation/quantization", original.len());
    let mut probes = Vec::new();
    println!("index\tcohort\tfacade_lat\tfacade_lon\tlayer\tpopup\tpainter\tconverged");
    loop {
        let batch: Vec<InputReceiver> = lines.by_ref().take(batch_size).map(|line| {
            let line = line?;
            let mut fields = line.split_whitespace();
            let lat: f64 = fields.next().context("lat")?.parse()?;
            let lon: f64 = fields.next().context("lon")?.parse()?;
            ensure!(lat.is_finite() && lon.is_finite() && lat.abs() <= 85.0 && lon.abs() <= 180.0, "invalid receiver");
            let cohort = fields.next().unwrap_or("all").to_owned();
            let levels: Vec<f64> = fields.map(str::parse).collect::<std::result::Result<_, _>>()?;
            ensure!(levels.is_empty() || (levels.len() == 5 && levels.iter().all(|value| value.is_finite())), "supply all five finite popup layer levels, or none");
            let levels = if levels.is_empty() { None } else { Some(levels.try_into().unwrap()) };
            Ok((lat, lon, cohort, levels))
        }).collect::<Result<_>>()?;
        if batch.is_empty() { break; }
        let popups: Vec<([f64; 2], [f64; 5])> = batch.iter()
            .map(|(lat, lon, _, levels)| match levels { Some(levels) => Ok(([*lat, *lon], *levels)), None => popup(&prepared, &rasters, *lat, *lon) })
            .collect::<Result<_>>()?;
        let facades: Vec<_> = popups.iter().map(|(facade, _)| *facade).collect();
        let encoded: Vec<_> = facades.iter().map(|p| scene.host.frame.encode(p[0], p[1])).collect();
        let batch_sources: Vec<_> = original.iter().filter(|source| encoded.iter().any(|position|
            segment_distance_m(*position, [source.device.start_x_m, source.device.start_y_m],
                [source.device.end_x_m, source.device.end_y_m]) <= source.device.max_distance_m)).cloned().collect();
        scene.replace_sources(batch_sources.clone())?;
        let painted = scene.evaluate_positions(&cuda, &facades).context("painter lane")?;
        scene.replace_sources(converged_sources(&batch_sources, &encoded, piece_m))?;
        let converged = scene.evaluate_positions(&cuda, &facades).context("reference lane")?;
        for (((_, _, cohort, _), (facade, popup)), (painted, converged)) in batch.iter().zip(&popups).zip(painted.iter().zip(&converged)) {
            let probe = Probe { cohort: cohort.clone(), facade: *facade, popup: *popup,
                painter: layer_lden(painted), converged: layer_lden(converged) };
            for (index, (kind, _)) in LAYERS.iter().enumerate() {
                println!("{}\t{}\t{:.10}\t{:.10}\t{}\t{:.6}\t{:.6}\t{:.6}",
                    probes.len(), probe.cohort, probe.facade[0], probe.facade[1], kind.as_str(),
                    probe.popup[index], probe.painter[index], probe.converged[index]);
            }
            probes.push(probe);
        }
        std::io::stdout().flush()?;
        eprintln!("completed {} receivers in {:.1} s", probes.len(), started.elapsed().as_secs_f64());
    }
    ensure!(!probes.is_empty(), "no receivers");
    let mut cohorts: Vec<String> = probes.iter().flat_map(|p| p.cohort.split(';').map(str::to_owned)).collect();
    cohorts.push("all".to_owned());
    cohorts.sort();
    cohorts.dedup();
    for cohort in &cohorts {
        let members: Vec<&Probe> = probes.iter().filter(|p| cohort == "all" || p.cohort.split(';').any(|tag| tag == cohort)).collect();
        for (index, (kind, _)) in LAYERS.iter().enumerate() {
            let kind = kind.as_str();
            eprintln!("{cohort:>10} {kind:>10} painter−converged {}", ladder(&members, |p| p.painter[index], |p| p.converged[index]));
            eprintln!("{cohort:>10} {kind:>10}   popup−converged {}", ladder(&members, |p| p.popup[index], |p| p.converged[index]));
            eprintln!("{cohort:>10} {kind:>10}     popup−painter {}", ladder(&members, |p| p.popup[index], |p| p.painter[index]));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relevant_source_gpu::source_frame::DeviceLineSource;
    use tile_painter::corner_store::SourceIdentity;

    #[test]
    fn reference_spacing_is_metric_and_preserves_line_power_and_coverage() {
        let source = SurfaceSource {
            identity: SourceIdentity([0; 32]), layer: 1,
            device: DeviceLineSource { start_x_m: -125.0, end_x_m: 125.0,
                extent_m: 250.0, emission_linear: [2.0; 24], ..DeviceLineSource::default() },
        };
        for spacing in [1.0, 0.25] {
            // The old distance-dependent rule used 8 m at this receiver.
            let split = converged_sources(std::slice::from_ref(&source), &[[0.0, 400.0]], spacing);
            assert_eq!(split.len(), (250.0 / spacing) as usize);
            assert_eq!(split.first().unwrap().device.start_x_m, -125.0);
            assert_eq!(split.last().unwrap().device.end_x_m, 125.0);
            for pair in split.windows(2) { assert_eq!(pair[0].device.end_x_m, pair[1].device.start_x_m); }
            let power: f32 = split.iter().map(|part| {
                assert!((part.device.end_x_m - part.device.start_x_m) <= spacing + 1e-4);
                part.device.extent_m * part.device.emission_linear[0]
            }).sum();
            assert!((power - 500.0).abs() < 1e-3);
        }
        assert_eq!(converged_sources(&[source], &[[0.0, 401.0]], 1.0).len(), 1);
    }
}

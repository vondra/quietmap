//! Popup–painter parity on real receivers, both lanes against a converged line quadrature: the
//! popup in process (`noise_compute::compute_at_point` at the façade receiver), the painter's
//! pair kernel at the same receiver, and the painter again with every road and rail line within
//! 400 m cut into pieces subtending about a degree. Reads `lat lon [cohort]` lines on stdin,
//! writes one tab-separated row per receiver and the ladder per layer on stderr.
use anyhow::{ensure, Context, Result};
use noise_compute::types::{LayerKind, RasterSampler, Receiver};
use rayon::prelude::*;
use relevant_source_gpu::{
    cuda_bridge::RelevantSourceCuda,
    input_manifest::{parse_digest, InputManifest},
    native_sources::SurfaceSource,
    surface_gpu::SurfaceGpu,
    surface_scene::SurfaceScene,
};
use std::{io::BufRead, path::PathBuf};

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
/// A converged piece subtends at most about this angle at its nearest receiver, never less
/// than a metre long.
const CONVERGED_PIECE_ANGLE_RAD: f32 = 0.02;
/// Receivers per converged batch (their cut lines replace the scene's sources for one launch).
const CONVERGED_BATCH: usize = 32;
/// A layer counts at a receiver when either side reaches the display floor (w2-parity).
const COUNTED_FLOOR_DB: f64 = 30.0;

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
fn converged_sources(original: &[SurfaceSource], receivers: &[[f32; 2]]) -> Vec<SurfaceSource> {
    let mut sources = Vec::with_capacity(original.len());
    for source in original {
        let d = source.device;
        let (a, b) = ([d.start_x_m, d.start_y_m], [d.end_x_m, d.end_y_m]);
        let nearest = receivers.iter().map(|r| segment_distance_m(*r, a, b)).fold(f32::INFINITY, f32::min);
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        let piece_m = (nearest * CONVERGED_PIECE_ANGLE_RAD).max(1.0);
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

/// The popup's façade receiver and its per-layer Lden, in process.
fn popup(prepared: &std::path::Path, rasters: &raster_reader::RealRasters, lat: f64, lon: f64) -> Result<([f64; 2], [f64; 5])> {
    let load = |lat, lon| source_reader::structure_store::load_obstacle_set(prepared, lat, lon).map_err(anyhow::Error::msg);
    let mut obstacles = load(lat, lon)?;
    let (facade_lat, facade_lon, _) = source_reader::structure_store::locate_facade_receiver(&obstacles, lat, lon);
    if (facade_lat, facade_lon) != (lat, lon) {
        obstacles = load(facade_lat, facade_lon)?;
    }
    let sources = source_reader::collect_sources_at_point(prepared, facade_lat, facade_lon).map_err(anyhow::Error::msg)?;
    let checked = raster_reader::CheckedRasters::new(rasters);
    let sampler = noise_compute::propagation::obstacle_index::VectorReflectionSampler { inner: &checked, set: &obstacles };
    let receiver = Receiver::new(facade_lat, facade_lon, sampler.elevation(facade_lat, facade_lon));
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
    Ok(([facade_lat, facade_lon], levels))
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
    ensure!(args.len() == 6, "usage: parity-probe PREPARED RASTERS MANIFEST SHA256 z9/X/Y < receivers");
    let prepared = PathBuf::from(&args[1]);
    let rasters = raster_reader::RealRasters::new(&PathBuf::from(&args[2]));
    noise_compute::square_country_city::set_square_country_city_prepared_directory(&prepared);
    let manifest = InputManifest::open(&PathBuf::from(&args[3]), parse_digest(&args[4])?)?;
    let owner = grid::parse_square_name(&args[5]).context("owner z9/X/Y")?;
    let receivers: Vec<(f64, f64, String)> = std::io::stdin()
        .lock()
        .lines()
        .map(|line| {
            let line = line?;
            let mut fields = line.split_whitespace();
            let lat: f64 = fields.next().context("lat")?.parse()?;
            let lon: f64 = fields.next().context("lon")?.parse()?;
            Ok((lat, lon, fields.next().unwrap_or("all").to_owned()))
        })
        .collect::<Result<_>>()?;
    let started = std::time::Instant::now();
    let popups: Vec<([f64; 2], [f64; 5])> = receivers
        .par_iter()
        .map(|(lat, lon, _)| popup(&prepared, &rasters, *lat, *lon))
        .collect::<Result<_>>()?;
    eprintln!("popup lane: {} receivers in {:.0} s", popups.len(), started.elapsed().as_secs_f64());
    let cuda = RelevantSourceCuda::initialize()?;
    let host = SurfaceScene::load(owner, &prepared, &manifest, &rasters)?;
    let mut scene = SurfaceGpu::upload(host)?;
    let facades: Vec<[f64; 2]> = popups.iter().map(|(facade, _)| *facade).collect();
    let painted = scene.evaluate_positions(&cuda, &facades)?;
    let original = scene.host.sources.clone();
    let mut converged = Vec::with_capacity(facades.len());
    for batch in facades.chunks(CONVERGED_BATCH) {
        let encoded: Vec<[f32; 2]> = batch.iter().map(|p| scene.host.frame.encode(p[0], p[1])).collect();
        scene.replace_sources(converged_sources(&original, &encoded))?;
        converged.extend(scene.evaluate_positions(&cuda, batch)?);
    }
    eprintln!("painter lanes done at {:.0} s", started.elapsed().as_secs_f64());
    let probes: Vec<Probe> = receivers
        .iter()
        .zip(&popups)
        .zip(painted.iter().zip(&converged))
        .map(|(((_, _, cohort), (facade, popup)), (painted, converged))| Probe {
            cohort: cohort.clone(),
            facade: *facade,
            popup: *popup,
            painter: layer_lden(painted),
            converged: layer_lden(converged),
        })
        .collect();
    println!("cohort\tfacade_lat\tfacade_lon\tlayer\tpopup\tpainter\tconverged");
    for probe in &probes {
        for (index, (kind, _)) in LAYERS.iter().enumerate() {
            println!(
                "{}\t{:.7}\t{:.7}\t{}\t{:.3}\t{:.3}\t{:.3}",
                probe.cohort, probe.facade[0], probe.facade[1], kind.as_str(), probe.popup[index], probe.painter[index], probe.converged[index]
            );
        }
    }
    let mut cohorts: Vec<String> = probes.iter().map(|p| p.cohort.clone()).collect();
    cohorts.sort();
    cohorts.dedup();
    cohorts.insert(0, "all".to_owned());
    for cohort in &cohorts {
        let members: Vec<&Probe> = probes.iter().filter(|p| cohort == "all" || &p.cohort == cohort).collect();
        for (index, (kind, _)) in LAYERS.iter().enumerate() {
            let kind = kind.as_str();
            eprintln!("{cohort:>10} {kind:>10} painter−converged {}", ladder(&members, |p| p.painter[index], |p| p.converged[index]));
            eprintln!("{cohort:>10} {kind:>10}   popup−converged {}", ladder(&members, |p| p.popup[index], |p| p.converged[index]));
            eprintln!("{cohort:>10} {kind:>10}     popup−painter {}", ladder(&members, |p| p.popup[index], |p| p.painter[index]));
        }
    }
    Ok(())
}

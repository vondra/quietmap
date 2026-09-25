//! Painter parity through the actual `paint_tile` path: one z13 tile painted with production
//! sources and again with every road and rail line near the tile cut into converged pieces.
//! Compares outdoor Lden per pixel per layer (partition and background interpolation included,
//! display-byte quantization excluded) and reports the ladder per layer on stderr.
use anyhow::{ensure, Context, Result};
use noise_compute::types::LayerKind;
use relevant_source_gpu::{
    cuda_bridge::RelevantSourceCuda, input_manifest::{parse_digest, InputManifest},
    native_sources::SurfaceSource, paint_tile::paint_tile, surface_gpu::SurfaceGpu,
    surface_scene::SurfaceScene,
};
use std::path::PathBuf;

/// Layers compared: road, rail, industrial, building (with leisure), ship (ground ops excluded).
const LAYERS: [(LayerKind, u8); 5] = [
    (LayerKind::Road, 0),
    (LayerKind::Railway, 1),
    (LayerKind::Industrial, 2),
    (LayerKind::Building, 3),
    (LayerKind::Ship, 5),
];
/// Lines within this distance of the tile are cut finer for the converged lane.
const CONVERGED_RADIUS_M: f32 = 400.0;
/// Tile bbox margin covering the pixel grid (400 m plus half a diagonal cell).
const TILE_MARGIN_M: f32 = 405.0;
/// Maximum reference piece length; rerun at 0.25 m to check convergence.
const CONVERGED_PIECE_LENGTH_M: f32 = 1.0;
/// A pixel counts for a layer when either side reaches the display floor.
const COUNTED_FLOOR_DB: f64 = 30.0;

fn lden(periods: [f64; 3]) -> f64 {
    let db = |energy: f64| 10.0 * energy.max(1e-30).log10();
    noise_compute::periods::compute_lden(db(periods[0]), db(periods[1]), db(periods[2]))
}

fn point_segment_distance_m(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (sx, sy) = (b[0] - a[0], b[1] - a[1]);
    let length_sq = sx * sx + sy * sy;
    let t = if length_sq > 0.0 {
        (((p[0] - a[0]) * sx + (p[1] - a[1]) * sy) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * sx).hypot(p[1] - a[1] - t * sy)
}

fn point_bbox_distance_m(p: [f32; 2], bbox: [f32; 4]) -> f32 {
    let dx = if p[0] < bbox[0] { bbox[0] - p[0] } else if p[0] > bbox[2] { p[0] - bbox[2] } else { 0.0 };
    let dy = if p[1] < bbox[1] { bbox[1] - p[1] } else if p[1] > bbox[3] { p[1] - bbox[3] } else { 0.0 };
    dx.hypot(dy)
}

fn orient(p: [f32; 2], q: [f32; 2], r: [f32; 2]) -> f32 {
    (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
}

fn on_segment(p: [f32; 2], q: [f32; 2], r: [f32; 2]) -> bool {
    q[0] >= p[0].min(r[0]) && q[0] <= p[0].max(r[0]) && q[1] >= p[1].min(r[1]) && q[1] <= p[1].max(r[1])
}

fn segments_intersect(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    if o1 == 0.0 && on_segment(a, c, b) { return true; }
    if o2 == 0.0 && on_segment(a, d, b) { return true; }
    if o3 == 0.0 && on_segment(c, a, d) { return true; }
    if o4 == 0.0 && on_segment(c, b, d) { return true; }
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0)
}

/// Distance from segment AB to axis-aligned bbox [min_x, min_y, max_x, max_y].
fn segment_bbox_distance_m(a: [f32; 2], b: [f32; 2], bbox: [f32; 4]) -> f32 {
    if a == b {
        return point_bbox_distance_m(a, bbox);
    }
    if point_bbox_distance_m(a, bbox) == 0.0 || point_bbox_distance_m(b, bbox) == 0.0 {
        return 0.0;
    }
    let corners = [[bbox[0], bbox[1]], [bbox[2], bbox[1]], [bbox[2], bbox[3]], [bbox[0], bbox[3]]];
    for i in 0..4 {
        if segments_intersect(a, b, corners[i], corners[(i + 1) % 4]) {
            return 0.0;
        }
    }
    let mut best = point_bbox_distance_m(a, bbox).min(point_bbox_distance_m(b, bbox));
    for corner in corners {
        best = best.min(point_segment_distance_m(corner, a, b));
    }
    best
}

/// Sources with every road and rail line near `bbox` cut into pieces of at most `piece_m`.
fn converged_sources_for_tile(original: &[SurfaceSource], bbox: [f32; 4], piece_m: f32) -> Vec<SurfaceSource> {
    let mut sources = Vec::with_capacity(original.len());
    for (index, source) in original.iter().enumerate() {
        let d = source.device;
        let (a, b) = ([d.start_x_m, d.start_y_m], [d.end_x_m, d.end_y_m]);
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if source.layer > 1 || segment_bbox_distance_m(a, b, bbox) > TILE_MARGIN_M || length <= piece_m {
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
            // Split pieces share the parent digest; tag each with (source, piece)
            // so paint_tile's (layer, identity) map stays injective.
            piece.identity.0[24..28].copy_from_slice(&(index as u32).to_le_bytes());
            piece.identity.0[28..32].copy_from_slice(&(k as u32).to_le_bytes());
            sources.push(piece);
        }
    }
    sources
}

/// Share of counted pixels whose |a − b| exceeds each rung, with denominators.
fn ladder(diffs: &[(f64, f64)]) -> String {
    let counted: Vec<f64> = diffs.iter()
        .filter(|(a, b)| a.max(*b) >= COUNTED_FLOOR_DB)
        .map(|(a, b)| (a - b).abs())
        .collect();
    let share = |rung: f64| 100.0 * counted.iter().filter(|&&d| d > rung).count() as f64 / counted.len().max(1) as f64;
    let count = |rung: f64| counted.iter().filter(|&&d| d > rung).count();
    format!(
        "n {:6}  >0.5 {:5.1} %  >1 {:5.2} %  >3 {} ({:.3} %)  >6 {} ({:.4} %)",
        counted.len(), share(0.5), share(1.0), count(3.0), share(3.0), count(6.0), share(6.0),
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(args.len() == 7 || args.len() == 8,
        "usage: tile-parity-probe PREPARED RASTERS MANIFEST SHA256 z9/X/Y z13/X/Y [reference_piece_m]");
    let piece_m: f32 = args.get(7).map(|value| value.parse()).transpose()?.unwrap_or(CONVERGED_PIECE_LENGTH_M);
    ensure!(piece_m.is_finite() && (0.01..=CONVERGED_PIECE_LENGTH_M).contains(&piece_m),
        "reference piece length must be between 0.01 and 1 m");
    let prepared = PathBuf::from(&args[1]);
    let rasters = raster_reader::RealRasters::new(&PathBuf::from(&args[2]));
    let manifest = InputManifest::open(&PathBuf::from(&args[3]), parse_digest(&args[4])?)?;
    let owner = grid::parse_square_name(&args[5]).context("owner z9/X/Y")?;
    let tile: Vec<&str> = args[6].split('/').collect();
    ensure!(tile.len() == 3 && tile[0] == "z13", "tile z13/X/Y");
    let tile_x: u32 = tile[1].parse().context("tile X")?;
    let tile_y: u32 = tile[2].parse().context("tile Y")?;
    ensure!(tile_x / 16 == u32::from(owner.x) && tile_y / 16 == u32::from(owner.y), "tile outside owner");
    let started = std::time::Instant::now();
    let cuda = RelevantSourceCuda::initialize()?;
    let host = SurfaceScene::load(owner, &prepared, &manifest, &rasters)?;
    let mut scene = SurfaceGpu::upload(host)?;
    let original: Vec<_> = scene.host.sources.iter()
        .filter(|s| LAYERS.iter().any(|(_, layer)| *layer == s.layer)).cloned().collect();
    eprintln!("tile probe: {} sources, tile z13/{tile_x}/{tile_y}, reference pieces <= {piece_m} m within {CONVERGED_RADIUS_M} m",
        original.len());
    let receivers = relevant_source_gpu::tile_receivers::TileReceivers::prepare(&scene.host, tile_x, tile_y)?;
    let pixels = receivers.x.len();
    let bbox = receivers.x.iter().zip(&receivers.y).fold(
        [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY],
        |[min_x, min_y, max_x, max_y], (&x, &y)| {
            [min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]
        },
    );
    // Audible sources only, as the pair probe does per batch.
    let audible: Vec<_> = original.iter().filter(|source| {
        let d = source.device;
        segment_bbox_distance_m([d.start_x_m, d.start_y_m], [d.end_x_m, d.end_y_m], bbox) <= d.max_distance_m
    }).cloned().collect();
    eprintln!("audible sources for tile: {} (from {})", audible.len(), original.len());
    scene.replace_sources(audible.clone())?;
    let corners = grid::surface_corner::tile_corners(tile_x, tile_y).context("tile corners")?;
    let painted_corners = scene.evaluate_corners(&cuda, &corners).context("painter corners")?;
    let painter = paint_tile(&cuda, &scene, tile_x, tile_y, &receivers, &painted_corners).context("painter lane")?;
    eprintln!("painter lane done in {:.1} s", started.elapsed().as_secs_f64());
    let converged = converged_sources_for_tile(&audible, bbox, piece_m);
    eprintln!("reference sources: {} (from {})", converged.len(), audible.len());
    scene.replace_sources(converged)?;
    let reference_corners = scene.evaluate_corners(&cuda, &corners).context("reference corners")?;
    let reference = paint_tile(&cuda, &scene, tile_x, tile_y, &receivers, &reference_corners).context("reference lane")?;
    eprintln!("reference lane done in {:.1} s", started.elapsed().as_secs_f64());
    println!("pixel\tlayer\tpainter\tconverged");
    let mut diffs: [Vec<(f64, f64)>; 5] = Default::default();
    for pixel in 0..pixels {
        for (index, (_, layer)) in LAYERS.iter().enumerate() {
            let plane_painter = &painter[*layer as usize];
            let plane_reference = &reference[*layer as usize];
            let painter_periods = [f64::from(plane_painter[pixel * 3]), f64::from(plane_painter[pixel * 3 + 1]), f64::from(plane_painter[pixel * 3 + 2])];
            let reference_periods = [f64::from(plane_reference[pixel * 3]), f64::from(plane_reference[pixel * 3 + 1]), f64::from(plane_reference[pixel * 3 + 2])];
            let (a, b) = (lden(painter_periods), lden(reference_periods));
            diffs[index].push((a, b));
            // Only print counted pixels to keep output small (quiet pixels dominate).
            if a.max(b) >= COUNTED_FLOOR_DB {
                println!("{pixel}\t{layer}\t{a:.6}\t{b:.6}");
            }
        }
    }
    for (index, (kind, _)) in LAYERS.iter().enumerate() {
        eprintln!("{:>10} painter−converged {}", kind.as_str(), ladder(&diffs[index]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relevant_source_gpu::source_frame::DeviceLineSource;
    use tile_painter::corner_store::SourceIdentity;

    #[test]
    fn segment_bbox_distance_is_zero_when_crossing() {
        let bbox = [0.0, 0.0, 100.0, 100.0];
        // Long asymmetric crossing, midpoint outside: still 0.
        assert_eq!(segment_bbox_distance_m([-10.0, 50.0], [1000.0, 50.0], bbox), 0.0);
        // Parallel 5 m below: 5.
        assert!((segment_bbox_distance_m([-10.0, -5.0], [1000.0, -5.0], bbox) - 5.0).abs() < 1e-4);
    }

    #[test]
    fn tile_reference_cuts_lines_near_the_bbox_and_preserves_power() {
        let line = |x0: f32, x1: f32| SurfaceSource {
            identity: SourceIdentity([0; 32]), layer: 0,
            device: DeviceLineSource { start_x_m: x0, start_y_m: 0.0, end_x_m: x1, end_y_m: 0.0,
                extent_m: x1 - x0, emission_linear: [2.0; 24], ..DeviceLineSource::default() },
        };
        let bbox = [0.0, 0.0, 100.0, 100.0];
        // Crossing the tile: cut into 1 m pieces, power preserved.
        let crossing = line(-50.0, 150.0);
        let split = converged_sources_for_tile(std::slice::from_ref(&crossing), bbox, 1.0);
        assert_eq!(split.len(), 200);
        let power: f32 = split.iter().map(|p| p.device.extent_m * p.device.emission_linear[0]).sum();
        assert!((power - 400.0).abs() < 1e-2);
        // Far from the tile: kept whole.
        let far = line(1000.0, 1100.0);
        assert_eq!(converged_sources_for_tile(std::slice::from_ref(&far), bbox, 1.0).len(), 1);
        // Points and non-road layers: kept whole even near.
        let mut point = line(10.0, 10.0);
        point.layer = 2;
        assert_eq!(converged_sources_for_tile(std::slice::from_ref(&point), bbox, 1.0).len(), 1);
    }
}

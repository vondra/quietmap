//! Airborne GPU validation + benchmark against the CPU painter's shared screened path.
//!
//! The GPU compute lives in `noise_gpu::airborne` (shared with the `gpu-airborne` production
//! builder); this bin only drives it over an R4's tiles and checks parity + times it. E5
//! (region-resident): `prepare_segment` + pack + upload the R4's candidates ONCE, then per tile
//! only a cheap classify into index lists. Reports the amortised region speedup vs CPU-prod.
//!
//!   NOISE_GPU_PREPARED=/dev/shm/qmap/prepared DATA_YEAR=2026 e2-airborne <x> <y>
//!   QM_E2_EXACT=1 … e2-airborne <x> <y>   — add a CPU-exact ground-truth pass (mountain gate)
use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use h3o::CellIndex;
use noise_compute::emission::aircraft::{RECEIVER_HORIZON_MAX_M, RECEIVER_HORIZON_RANGE_SCALE};
use noise_gpu::airborne::{region_candidates, AirborneGpu};
use raster_reader::fused_tile_z13::{default_batch_size, TileBatch, TILE_PX};
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::airborne::scatter_tile;
use tile_painter::region_runner::{region_tiles, tile_centre_r4};
use tile_painter::source_loader_airborne::AirborneData;
use tile_painter::source_loader_obstacle::ObstacleData;

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (x, y): (u32, u32) = (a[1].parse()?, a[2].parse()?);
    let z = 12u8; // 512@z12 base (the old z13@256 lattice)
    let prepared = env("NOISE_GPU_PREPARED", "/dev/shm/qmap/prepared");
    let year = env("DATA_YEAR", "2026");
    let h3r4 = format!("{prepared}/{year}/h3r4");

    let r4 = tile_centre_r4(z, x, y).context("tile centre R4")?;
    let ring: Vec<u64> = CellIndex::try_from(r4)?
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let rasters = RealRasters::new(Path::new(&prepared));
    let air = AirborneData::load_for_r4s(Path::new(&h3r4), &ring)?;
    let obstacles = ObstacleData::load_for_r4s(Path::new(&h3r4), r4, &ring)?;
    let views = air.views();
    let bn = default_batch_size();
    let n = TILE_PX * TILE_PX;

    // GA hybrid weight LUT: uniform here so this parity gate validates the
    // GPU vs CPU kernel MATH (the per-class weight is an identical post-
    // multiply on both paths; w=1.0 keeps the comparison about the kernel).
    let cw = noise_compute::emission::aircraft::ClassWeights::uniform();
    // Device + kernels + NPD: one-time startup, excluded from the per-region prep cost below.
    let gpu = AirborneGpu::new(&cw);
    // Regression for conservative range packing: with the old nearest-metre
    // rule, 123.25 m stored as 123 m would put this source in front of the
    // edge. The shared CPU/GPU ceiling must keep the edge out of the query.
    let probe_true_range_m = 123.25_f32;
    let probe_source_range_m = 123.125_f32;
    let probe_packed_range = (f64::from(probe_true_range_m) * RECEIVER_HORIZON_RANGE_SCALE).ceil();
    if probe_packed_range <= f64::from(probe_source_range_m) {
        anyhow::bail!(
            "CPU terrain range probe did not pack away from source: packed={probe_packed_range}, source={probe_source_range_m}"
        );
    }
    let probe_dz =
        gpu.terrain_range_quantization_probe(probe_true_range_m, probe_source_range_m)?;
    if probe_dz != 0.0 {
        anyhow::bail!(
            "GPU terrain range probe screened a source in front of the edge: Dz={probe_dz} dB"
        );
    }
    eprintln!(
        "GPU terrain range quantization probe: true={probe_true_range_m:.3} m, source={probe_source_range_m:.3} m, packed={probe_packed_range:.0} m, Dz={probe_dz:.3} dB — PASS"
    );

    // ---- region-prep ONCE: candidates + prepare_segment + pack + upload (the amortised wall) ----
    // The envelope is derived from the R4 geometry (see `region_candidates`), so it is a safe
    // superset for every tile of the R4 — independent of which tile (x,y) was passed.
    let t_prep = std::time::Instant::now();
    let region = region_candidates(&views, r4, z);
    let resident = gpu.load_region(region).expect("load_region");
    let t_prep_ms = t_prep.elapsed().as_secs_f64() * 1e3;
    let nreg = resident.len();

    // ---- loop the R4's tiles, region SoA resident ----
    let tiles = region_tiles(r4, z);
    let mut batches: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for &(tx, ty) in &tiles {
        batches
            .entry(((tx / bn) * bn, (ty / bn) * bn))
            .or_default()
            .push((tx, ty));
    }

    // QM_E2_EXACT=1 adds a CPU-exact (FORCE_EXACT) ground-truth pass per tile and reports
    // GPU-vs-exact + adaptive-vs-exact drift — the mountain parity gate. The coarse far-field
    // lattice was tuned on gentle Prague terrain; steep tiles swing rx_alt sharply across the
    // tile, so the far field is less smooth — validate on LOWI (Innsbruck) before "whole world".
    let e2_exact = env("QM_E2_EXACT", "0") == "1";
    let (mut tot_cpu, mut tot_gpu) = (0.0f64, 0.0f64);
    let (mut worst_db, mut tot_zero, mut n_done) = (0.0f64, 0usize, 0usize);
    let mut tot_non_finite = 0usize;
    let (mut worst_gx, mut zero_gx) = (0.0f64, 0usize); // GPU vs exact (the mountain gate)
    let (mut worst_ax, mut zero_ax) = (0.0f64, 0usize); // adaptive vs exact (coarse error alone)
                                                        // Severity of the GPU-vs-exact drift: how localised is it (cells over 0.5/1.0 dB, tiles
                                                        // affected, where the worst sits) — distinguishes one freak cell from systematic mountain bias.
    let (mut n_gx_over_half, mut n_gx_over_1, mut n_tiles_over_half) = (0usize, 0usize, 0usize);
    let mut worst_gx_tile = (0u32, 0u32);
    for ((bx, by), btiles) in &batches {
        let batch = TileBatch::build_receiver_altitude_with_halo(
            z,
            *bx,
            *by,
            bn,
            RECEIVER_HORIZON_MAX_M,
            &rasters,
        );
        for &(tx, ty) in btiles {
            let tile = &batch.tiles[((ty - by) * bn + (tx - bx)) as usize];
            let interiors = [obstacles.interior_estimate(tile)];
            let interior = &interiors[0];

            // CPU-prod (the real CPU result + baseline)
            std::env::remove_var("QM_AIRBORNE_FORCE_EXACT");
            let mut accum_prod = TileAccumulator::new();
            let tc = std::time::Instant::now();
            let _ = scatter_tile(
                tile,
                &views,
                &cw,
                obstacles.set(),
                interior,
                &mut accum_prod,
            );
            tot_cpu += tc.elapsed().as_secs_f64() * 1e3;

            // GPU production path: classify → receiver horizons → near/far → expand.
            let tg = std::time::Instant::now();
            let fine = gpu
                .scatter_region(&resident, &[tile], obstacles.set(), &interiors)?
                .pop()
                .expect("one GPU tile");
            tot_gpu += tg.elapsed().as_secs_f64() * 1e3;

            // parity vs CPU-prod (both buffers are n*3 = TileAccumulator energy len).
            // A NaN or infinite cell would slip past both comparisons (NaN compares
            // false, `f64::max` ignores it), so it is counted as its own failure.
            for (&g, &c) in fine.energy.iter().zip(accum_prod.energy.iter()) {
                if !g.is_finite() || !c.is_finite() {
                    tot_non_finite += 1;
                    continue;
                }
                if (g > 0.0) != (c > 0.0) {
                    tot_zero += 1;
                }
                if g > 0.0 && c > 0.0 {
                    let d = (10.0 * (g as f64 / c as f64).log10()).abs();
                    worst_db = worst_db.max(d);
                }
            }

            // Mountain gate: CPU-exact ground truth (FORCE_EXACT = per-pixel everywhere, no
            // coarse lattice) vs the GPU result AND vs CPU-adaptive — isolates the far-field
            // coarsening error on steep terrain.
            if e2_exact {
                std::env::set_var("QM_AIRBORNE_FORCE_EXACT", "1");
                let mut accum_exact = TileAccumulator::new();
                let _ = scatter_tile(
                    tile,
                    &views,
                    &cw,
                    obstacles.set(),
                    interior,
                    &mut accum_exact,
                );
                std::env::remove_var("QM_AIRBORNE_FORCE_EXACT");
                let mut tile_worst = 0.0f64;
                for ((&g, &a), &x) in fine
                    .energy
                    .iter()
                    .zip(accum_prod.energy.iter())
                    .zip(accum_exact.energy.iter())
                {
                    if !g.is_finite() || !a.is_finite() || !x.is_finite() {
                        tot_non_finite += 1;
                        continue;
                    }
                    if (g > 0.0) != (x > 0.0) {
                        zero_gx += 1;
                    }
                    if g > 0.0 && x > 0.0 {
                        let d = (10.0 * (g as f64 / x as f64).log10()).abs();
                        tile_worst = tile_worst.max(d);
                        if d > 0.5 {
                            n_gx_over_half += 1;
                        }
                        if d > 1.0 {
                            n_gx_over_1 += 1;
                        }
                    }
                    if (a > 0.0) != (x > 0.0) {
                        zero_ax += 1;
                    }
                    if a > 0.0 && x > 0.0 {
                        worst_ax = worst_ax.max((10.0 * (a as f64 / x as f64).log10()).abs());
                    }
                }
                if tile_worst > worst_gx {
                    worst_gx = tile_worst;
                    worst_gx_tile = (tx, ty);
                }
                if tile_worst > 0.5 {
                    n_tiles_over_half += 1;
                }
            }
            n_done += 1;
        }
    }

    let gpu_total = t_prep_ms + tot_gpu;
    eprintln!(
        "region R4 {r4:015x} | {n_done} tiles | {nreg} region candidates (prepare_segment ONCE)"
    );
    eprintln!("GPU vs CPU-adaptive: worst max {worst_db:.4} dB, {tot_zero} zero-sided total");
    let production_failed =
        !worst_db.is_finite() || worst_db >= 0.5 || tot_zero != 0 || tot_non_finite != 0;
    if !production_failed {
        eprintln!("✓ region GPU airborne within 0.5 dB, 0 zero-sided across {n_done} tiles");
    } else {
        eprintln!(
            "✗ parity FAILED (worst {worst_db:.3} dB, {tot_zero} zero-sided, {tot_non_finite} non-finite)"
        );
    }
    if e2_exact {
        let cells = (n_done * n * 3).max(1);
        eprintln!(
            "GPU vs CPU-EXACT (ground truth): worst {worst_gx:.4} dB @tile {}/{} | {n_gx_over_half} cells >0.5 dB, {n_gx_over_1} >1.0 dB (of {cells}), {n_tiles_over_half}/{n_done} tiles affected, {zero_gx} zero-sided",
            worst_gx_tile.0, worst_gx_tile.1
        );
        eprintln!(
            "  adaptive vs exact: worst {worst_ax:.4} dB, {zero_ax} zero-sided — the coarse-lattice error; GPU≈adaptive ({worst_db:.4} dB) so GPU inherits it, does not introduce it"
        );
        let exact_failed =
            !worst_gx.is_finite() || worst_gx >= 0.5 || zero_gx != 0 || tot_non_finite != 0;
        if !exact_failed {
            eprintln!("✓ MOUNTAIN GATE PASS — GPU within 0.5 dB of exact across {n_done} tiles");
        } else {
            eprintln!(
                "✗ MOUNTAIN GATE over 0.5 dB — worst {worst_gx:.3} dB on {n_gx_over_half} cell(s); pre-existing CPU coarse-lattice limit, not a GPU-port defect"
            );
        }
        if exact_failed {
            anyhow::bail!(
                "airborne exact-oracle parity gate failed: worst={worst_gx:.4} dB, zero-sided={zero_gx}, non-finite={tot_non_finite}"
            );
        }
    }
    eprintln!("--- TIMING (same box, whole R4) ---");
    eprintln!(
        "  CPU prod total {tot_cpu:.0} ms  ({:.1} ms/tile)",
        tot_cpu / n_done.max(1) as f64
    );
    eprintln!(
        "  GPU total      {gpu_total:.0} ms  ({:.1} ms/tile) = region-prep {t_prep_ms:.0} (once) + per-tile {tot_gpu:.0}",
        gpu_total / n_done.max(1) as f64,
    );
    eprintln!(
        "  → region speedup (amortised): {:.1}×",
        tot_cpu / gpu_total.max(0.001)
    );
    if production_failed {
        anyhow::bail!(
            "airborne production parity gate failed: worst={worst_db:.4} dB, zero-sided={tot_zero}, non-finite={tot_non_finite}"
        );
    }
    Ok(())
}

//! E2c step 2 — validate the FULL surface path port WITH the energy-budget skip:
//! the `rail` kernel now mirrors scatter_band exactly. Two checks:
//!   (1) GPU vs a CPU reference that reuses the real engine path-effect fns
//!       (terrain/screening/veg/ground) AND the same per-pixel skip, f32 energy
//!       accumulation (matching TileAccumulator) — byte-exact isolates the port.
//!   (2) GPU energy → collapse_lden_surface_u8 → diff vs the production baseline
//!       tile (/root/baseline/rail/12/x/y.bin), the true scatter_tile output.
//!
//!   NOISE_GPU_PREPARED=/dev/shm/qmap/prepared NOISE_GPU_BASELINE=/root/baseline \
//!   NOISE_GPU_HALO_M=10000 e2-full <tile_x> <tile_y>
use std::path::Path;

use anyhow::{Context, Result};
use cudarc::driver::{CudaDevice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::Ptx;
use h3o::CellIndex;
use noise_compute::propagation::streaming_reduction::source_frame_mlon;
use noise_compute::types::Barrier;
use noise_gpu::{BIN_W, N_BINS, SOURCE_SEGMENT_STRIDE};
use raster_reader::fused_tile_z13::{default_batch_size, TileBatch, TILE_PX};
use raster_reader::RealRasters;
use tile_painter::accumulator::TileAccumulator;
use tile_painter::region_runner::tile_centre_r4;
use tile_painter::source_loader_rail::RailData;
use tile_painter::wire_hm3::{collapse_lden_surface_u8, read_tile};

const SCATTER_PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/scatter.ptx"));
const NO_DATA: u8 = 255;

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn main() -> Result<()> {
    // This binary IS the lane-parity gate, so it is the one place a CPU-only
    // lever must never pass silently — it would compare two different kernels.
    //
    // Since the 2026-08-19 §3.5e port the kernel paints the CPU painter's OWN
    // rule — 5 uniform buckets per microsegment fan, the 3° bucket gate on the
    // nested arc query, both compiled in from the CPU's own constants — so the
    // honest comparison runs the CPU reference at its production DEFAULTS, with
    // no pins at all. (Before the port the kernel arc-screened every pair from
    // `ARC_DEGENERATE_SPAN` = 1e-4 rad up with no quadrature, and this binary
    // pinned the CPU back onto that rule with `QM_SEG_SAMPLES=1
    // QM_ARC_MIN_SPAN_DEG=0`; a bare run therefore PASSED the guard while
    // comparing two different algorithms — the fork, measured: 69,005 cells,
    // 88.5 % CPU-louder. The port closes it; the residual this gate reports is
    // the analysis below.)
    //
    // The pre-port pins were also MEASURED to not be the explanation for this
    // gate's historical 4469-cell / 17.63 dB failure — the run already had both
    // pins set, and changing them moved zero cells. The fault was later isolated
    // to one footprint-wide range standing in for its differently ranged edges
    // in the CUDA hull. Per-edge arcs reduced that residual to 2298 cells / 5.91
    // dB; it remains a separate vector-candidate evaluation fork, documented at
    // `arc_screen_bands`, rather than a quadrature-port or pinning effect.
    noise_gpu::ensure_no_cpu_only_arc_levers()?;
    let a: Vec<String> = std::env::args().collect();
    let (x, y): (u32, u32) = (a[1].parse()?, a[2].parse()?);
    let z = 12u8; // 512@z12 base (the old z13@256 lattice)
    let prepared = env("NOISE_GPU_PREPARED", "/dev/shm/qmap/prepared");
    let baseline = env("NOISE_GPU_BASELINE", "/root/baseline");
    let year = env("DATA_YEAR", "2026");
    let halo_m: f64 = env("NOISE_GPU_HALO_M", "10000").parse()?;
    // The old η slot, now the surface kernel's byte-stop ON/OFF (see
    // `noise_gpu::pack_tile`): 0 = skip nothing, non-zero = stop once the byte is
    // decided. Kept reading SURFACE_BUDGET_ETA so `=0` still selects the exact
    // reference on BOTH lanes with one env, which is what the parity gate needs.
    let eta: f64 = env("SURFACE_BUDGET_ETA", "0.40")
        .parse::<f64>()
        .unwrap_or(0.40)
        .clamp(0.0, 0.40);
    // Skip the (slow) CPU reference — keep only GPU kernel timing + the GPU→u8
    // baseline diff. For fast fp32 drift/perf iteration, where byte-exact-vs-CPU
    // is moot anyway.
    let gpu_only = std::env::var("NOISE_GPU_ONLY").is_ok();
    // NOISE_GPU_PERF_ONLY=1: kernel-timing loop for the GPU optimisation track —
    // drops the CPU reference EVEN in vector mode, where the parity gate below
    // normally forbids it (the fix-pack's arc CPU reference costs ~45 min per
    // tile, which no A/B loop can pay). It is deliberately a SEPARATE variable
    // from NOISE_GPU_ONLY so the parity gate keeps failing closed for anyone who
    // just wanted to skip a slow reference; a perf run prints its own banner and
    // its output is a TIME, never a P4 verdict.
    let perf_only = std::env::var("NOISE_GPU_PERF_ONLY").is_ok_and(|v| v == "1");
    let gpu_only = gpu_only || perf_only;
    if perf_only {
        eprintln!(
            "!! NOISE_GPU_PERF_ONLY — CPU reference and the vector parity gate are OFF. \
             This run measures kernel time ONLY and is NOT a P4 result."
        );
    }
    // Swizzle tile width (must divide TILE_PX): the cache-blocking knob. 8×8 measured
    // best on the 4060 (rays overlap most in a ~96 m tile).
    let tw: f64 = env("NOISE_GPU_TW", "8").parse::<f64>().unwrap_or(8.0);
    let h3r4 = format!("{prepared}/{year}/h3r4");

    let r4 = tile_centre_r4(z, x, y).context("tile centre")?;
    let cell = CellIndex::try_from(r4)?;
    let ring: Vec<u64> = cell
        .grid_disk::<Vec<_>>(1)
        .into_iter()
        .map(u64::from)
        .collect();
    let rasters = RealRasters::new(Path::new(&prepared));
    // C1 rail per-region period split needs the admin table. The bench loads rail
    // directly (no surface binary in the loop), so init it here too — else the
    // reference run resolves Admin::UNKNOWN → world split and drifts vs a
    // real-admin production tile (Codex delta 1).
    let _ = noise_compute::admin::init_admin_table(&noise_compute::admin::default_admin_path(
        Path::new(&h3r4),
    ));
    let ll = h3o::LatLng::from(cell);
    let admin = noise_compute::admin::admin_for_latlng(ll.lat(), ll.lng());
    let rail = RailData::load_for_r4s(Path::new(&h3r4), &ring, admin)?.into_rows();
    // The region's obstacle store loads into BOTH lanes — the CPU reference
    // threads the set through scatter_tile_with_cfg, the GPU gets the flattened
    // grids — so this validator IS the vector-parity gate.
    let obstacle_data = tile_painter::source_loader_obstacle::ObstacleData::load_for_r4s(
        Path::new(&h3r4),
        r4,
        &ring,
    )?;
    // NOISE_GPU_FEATURES=1: print the A-PRIORI cost features for this tile and
    // exit before any GPU work. Everything here is available from the prepared
    // tree WITHOUT painting, which is the whole point — the hub has to order a
    // repaint queue before it knows what a cell costs.
    if std::env::var("NOISE_GPU_FEATURES").is_ok_and(|v| v == "1") {
        let bn = default_batch_size();
        let (bx, by) = ((x / bn) * bn, (y / bn) * bn);
        let batch = TileBatch::build(z, bx, by, bn, halo_m, &rasters);
        let tile = &batch.tiles[((y - by) * bn + (x - bx)) as usize];
        let bb = &tile.bbox;
        let (mut n_edges, mut n_foot, mut occ, mut cells) = (0usize, 0usize, 0usize, 0usize);
        {
            let set = obstacle_data.set();
            let flat = noise_gpu::flatten_obstacles(set);
            n_edges = flat.edges.len() / 5;
            n_foot = flat.foot_box.len() / noise_gpu::FOOT_BOX_STRIDE;
            cells = flat.cell_max_h.len();
            occ = flat.cell_max_h.iter().filter(|&&h| h > 0.0).count();
        }
        // Footprints NEAR this tile — the a-priori proxy for obstacle work per
        // ray. Ring-level counts cannot discriminate between tiles of the same
        // R4 cell (they share the ring), so this is deliberately tile-local: the
        // tile bbox grown by a typical audibility margin, in each index's frame.
        const NEAR_M: f64 = 2000.0;
        let mut foot_near = 0usize;
        {
            let set = obstacle_data.set();
            for idx in &set.indexes {
                let v = idx.gpu_view();
                let x0 = (bb.west_lon - v.origin_lon) * v.m_per_deg_lon - NEAR_M;
                let x1 = (bb.east_lon - v.origin_lon) * v.m_per_deg_lon + NEAR_M;
                let y0 = (bb.south_lat - v.origin_lat) * noise_compute::constants::M_PER_DEG_LAT
                    - NEAR_M;
                let y1 = (bb.north_lat - v.origin_lat) * noise_compute::constants::M_PER_DEG_LAT
                    + NEAR_M;
                let fl = noise_gpu::flatten_obstacles(
                    &noise_compute::propagation::obstacle_index::ObstacleSet {
                        indexes: vec![std::sync::Arc::clone(idx)],
                    },
                );
                let st = noise_gpu::FOOT_BOX_STRIDE;
                for f in 0..fl.foot_box.len() / st {
                    let b = &fl.foot_box[f * st..f * st + 4];
                    if (b[0] as f64) <= x1
                        && (b[2] as f64) >= x0
                        && (b[1] as f64) <= y1
                        && (b[3] as f64) >= y0
                    {
                        foot_near += 1;
                    }
                }
            }
        }
        // Sources whose audibility disk reaches this tile — the a-priori proxy
        // for the (source × receiver) pair count the kernel actually pays for.
        let (clat, clon) = (
            (bb.north_lat + bb.south_lat) * 0.5,
            (bb.west_lon + bb.east_lon) * 0.5,
        );
        let half = noise_compute::constants::M_PER_DEG_LAT * (bb.north_lat - bb.south_lat) * 0.5;
        let in_reach = rail
            .iter()
            .filter(|r| {
                let d = noise_compute::propagation::geo::flat_dist(
                    clat,
                    clon,
                    r.start_lat,
                    r.start_lon,
                );
                d <= r.max_distance_m + half * 1.5
            })
            .count();
        println!(
            "FEAT\t{x}/{y}\tedges={n_edges}\tfoot={n_foot}\tcells={cells}\tocc={occ}\t\
             occ_frac={:.4}\tfoot_per_occ={:.3}\trail_rows={}\tin_reach={in_reach}\tfoot_near={foot_near}\twork={:.0}",
            if cells > 0 { occ as f64 / cells as f64 } else { 0.0 },
            if occ > 0 { n_foot as f64 / occ as f64 } else { 0.0 },
            rail.len(),
            in_reach as f64 * foot_near as f64 / 1e6
        );
        return Ok(());
    }
    let bn = default_batch_size();
    let (bx, by) = ((x / bn) * bn, (y / bn) * bn);
    let mut batch = TileBatch::build(z, bx, by, bn, halo_m, &rasters);
    {
        let set = obstacle_data.set();
        let tile = &mut batch.tiles[((y - by) * bn + (x - bx)) as usize];
        tile_painter::source_loader_obstacle::bake_tile_vector_rx_refl(tile, set);
        eprintln!("vector obstacles: {} edges", set.edge_count());
    }
    let tile = &batch.tiles[((y - by) * bn + (x - bx)) as usize];
    let halo = &tile.halo;
    let (lat_min, lon_min, inv, rows, cols) = halo.geom();
    let nsrc = rail.len();
    eprintln!("tile {x}/{y} R4 {r4:015x} | rail rows {nsrc} | halo {rows}×{cols}");

    // ---- packed device buffers ----
    let n = TILE_PX * TILE_PX;
    let meta: Vec<f64> = vec![
        rows as f64,
        cols as f64,
        lat_min,
        lon_min,
        inv,
        tile.bbox.north_lat,
        tile.bbox.south_lat,
        tile.bbox.west_lon,
        tile.bbox.east_lon,
        eta,
        tw,
        0.0, // nbarr — the e2 CPU reference below is barrier-free (`no_barriers`)
        nsrc as f64,
        // meta[13] = the `out` length allocated below. This validator takes the
        // PROF_COUNTERS block, so a counter-instrumented PTX writes it here and
        // writes nothing of it under the production painter (`OUT_SLOTS_PROD`).
        noise_gpu::OUT_SLOTS_PROF as f64,
        1.0, // meta[14] = rail H0 placement-floor tag
    ];
    // sp = 12/source: length/reach/height/bridge ++ 8 host-precomputed Lden band
    // weights (Σ_p LDEN_W[p]·emission_lin[p][i]) — mirrors pack_sources so the shared
    // `line`/`line_binned_fused` kernels read sp[4+i] for the energy-budget UB.
    const LDEN_W: [f64; 3] = [12.0, 12.649110640673518, 80.0];
    let mut seg = Vec::with_capacity(nsrc * SOURCE_SEGMENT_STRIDE);
    let mut sp = Vec::with_capacity(nsrc * 12);
    let mut semis = Vec::with_capacity(nsrc * 24);
    for r in &rail {
        seg.extend_from_slice(&[
            r.start_lat,
            r.start_lon,
            r.end_lat,
            r.end_lon,
            source_frame_mlon(r.start_lat, r.end_lat)
                .expect("non-finite source geometry cannot enter the GPU ABI"),
        ]);
        sp.extend_from_slice(&[
            r.length_m as f64,
            r.max_distance_m,
            r.source_height_m,
            if r.bridge { 1.0 } else { 0.0 },
        ]);
        for i in 0..8 {
            sp.push(
                LDEN_W[0] * r.emission_lin[0][i] as f64
                    + LDEN_W[1] * r.emission_lin[1][i] as f64
                    + LDEN_W[2] * r.emission_lin[2][i] as f64,
            );
        }
        for p in 0..3 {
            for i in 0..8 {
                semis.push(r.emission_lin[p][i]);
            }
        }
    }
    let mut rxll = Vec::with_capacity(2 * TILE_PX);
    rxll.extend_from_slice(&tile.rx_lat);
    rxll.extend_from_slice(&tile.rx_lon);
    let mut rxar = Vec::with_capacity(n * 2);
    for i in 0..n {
        rxar.push(tile.rx_alt_m[i]);
        rxar.push(tile.rx_refl_db[i]);
    }
    let elev: Vec<f32> = halo.pixels().iter().map(|p| p.elevation).collect();
    let inner: Vec<f32> = tile.inner_elev_m.clone();
    // cover = halo [forest, imd] per cell, interleaved
    let mut cover = Vec::with_capacity(rows * cols * 2);
    for p in halo.pixels() {
        cover.push(p.forest);
        cover.push(p.imd);
    }

    // ---- CPU reference: the PRODUCTION unified kernel, not a hand-copied mirror
    // (AGENTS.md anti-mirror rule). scatter_line::scatter_tile_with_cfg builds the
    // same per-pixel terms (cylindrical divergence + clamped-foot FLC), stops on
    // the identical byte-space rule, and accumulates f32 into a TileAccumulator
    // whose `.energy` is the exact [py][px][period] layout of
    // `cpu`. The cadence MUST match the kernel's compile-time coarse-middle
    // mirror (scatter.cu SHADOW_MID_STRIDE 3 / 600 m zones — the CPU env
    // DEFAULTS): a `None` (exact) reference conflates cadence differences with
    // GPU drift (gg review 2026-07-28 #1; candidate terrain LERP and δ*
    // partitions ride the samples). Barrier-free (meta nbarr = 0 above), so an
    // empty barrier slice. Skipped under NOISE_GPU_ONLY — the GPU→u8 baseline
    // diff stands alone.
    let kernel_cadence = Some(noise_compute::propagation::path_profile::CoarseMid {
        src_zone_m: 600.0,
        rx_zone_m: 600.0,
        mid_stride: 3,
    });
    let mut cpu = vec![0f32; n * 3];
    if !gpu_only {
        let no_barriers: &[Barrier] = &[];
        let mut accum = TileAccumulator::new();
        tile_painter::scatter_line::scatter_tile_with_cfg(
            tile,
            &rail,
            no_barriers,
            obstacle_data.set(),
            &mut accum,
            kernel_cadence,
        );
        cpu.copy_from_slice(&accum.energy);
    }
    // Vector-mode gate hardening (gg review 2026-07-28 #2): the parity claim
    // requires (a) the store actually LOADED — a missing ring silently
    // returning off() must fail the run, not validate raster-vs-raster; (b) a
    // CPU reference to compare against; (c) proof the candidate lane FIRED —
    // the same CPU scatter WITHOUT obstacles must differ somewhere, else the
    // tile exercises nothing and the gate is vacuous.
    if !perf_only {
        anyhow::ensure!(
            !gpu_only,
            "NOISE_GPU_ONLY removes the CPU reference — the vector parity gate \
             needs both lanes"
        );
        let mut raster_accum = TileAccumulator::new();
        tile_painter::scatter_line::scatter_tile_with_cfg(
            tile,
            &rail,
            &[],
            &noise_compute::propagation::obstacle_index::ObstacleSet::empty(),
            &mut raster_accum,
            kernel_cadence,
        );
        let n_diff = cpu
            .iter()
            .zip(raster_accum.energy.iter())
            .filter(|(a, b)| a != b)
            .count();
        anyhow::ensure!(
            n_diff > 0,
            "vector CPU reference is identical to the raster reference — no candidate \
             fired on this tile; pick a tile with obstacle coverage"
        );
        eprintln!("vector lane active: {n_diff} CPU cells differ from raster mode");
    }

    // ---- GPU ----
    let dev = CudaDevice::new(0).expect("cuda");
    dev.load_ptx(
        Ptx::from_src(SCATTER_PTX),
        "s",
        &["line", "line_binned_fused"],
    )
    .expect("ptx");
    // NOISE_GPU_KERNEL: line (scan-all, default) | line_binned_fused (GPU-side binning).
    // line_binned_fused uses the 1024×64 binned launch; line is pixel-major. The two must
    // agree byte-for-byte — the conservative-cull + ordered-replay parity check.
    let mode = env("NOISE_GPU_KERNEL", "line");
    let binned_launch = mode == "line_binned_fused";
    let f = dev
        .get_func(
            "s",
            if binned_launch {
                "line_binned_fused"
            } else {
                "line"
            },
        )
        .expect("fn");
    let d_elev = dev.htod_copy(elev).expect("elev");
    let d_inner = dev.htod_copy(inner).expect("inner");
    let d_cover = dev.htod_copy(cover).expect("cover");
    let d_meta = dev.htod_copy(meta).expect("meta");
    let d_seg = dev.htod_copy(seg).expect("seg");
    let d_sp = dev.htod_copy(sp).expect("sp");
    let d_semis = dev.htod_copy(semis).expect("semis");
    let d_rxll = dev.htod_copy(rxll).expect("rxll");
    let d_rxar = dev.htod_copy(rxar).expect("rxar");
    // One zero row — meta nbarr = 0, the kernel never reads it (cuMemAlloc
    // rejects 0-byte buffers).
    let d_barr = dev
        .htod_copy(vec![0.0f64; noise_gpu::BARRIER_STRIDE])
        .expect("barr");
    let obst = noise_gpu::upload_obstacles(&dev, obstacle_data.set()).expect("obstacles");
    // Energies + the ARC FAULT slot + TEN per-pixel counters the kernel fills
    // only when built with -DPROF_COUNTERS=1: quadrature pairs, pairs with an
    // escalation, marched bucket rays, escalating buckets, hull lookups/hits,
    // clip survivors, emitted arcs and interval overflows. Always allocated, so
    // one host binary serves both PTX builds: 512²·10·4 B = 10.0 MiB on top of
    // the 3.0 MiB of energies, once per process, in THIS
    // validator only — and meta[13] above is what tells the kernel it may use it.
    let mut d_out = dev
        .alloc_zeros::<f32>(noise_gpu::OUT_SLOTS_PROF)
        .expect("out");
    let block: u32 = env("NOISE_GPU_BLOCK", "128").parse().unwrap_or(128);
    let cfg = LaunchConfig {
        grid_dim: if binned_launch {
            (N_BINS as u32, 1, 1)
        } else {
            ((n as u32).div_ceil(block), 1, 1)
        },
        block_dim: (
            if binned_launch {
                (BIN_W * BIN_W) as u32
            } else {
                block
            },
            1,
            1,
        ),
        shared_mem_bytes: 0,
    };
    let t = std::time::Instant::now();
    unsafe {
        // line (pixel-major) and line_binned_fused (binned) share the ARG tuple
        // (…, barr, obst, out); only the launch GEOMETRY differs (binned_launch
        // above). nsrc rides in meta[12].
        f.launch(
            cfg,
            noise_gpu::line_kernel_arguments!(
                &d_elev,
                &d_inner,
                &d_cover,
                &d_meta,
                &d_seg,
                &d_sp,
                &d_semis,
                &d_rxll,
                &d_rxar,
                &d_barr,
                &obst.table,
                &mut d_out,
            ),
        )
        .expect("launch");
    }
    dev.synchronize().expect("sync");
    let gpu_ms = t.elapsed().as_secs_f64() * 1e3;
    let gpu = dev.dtoh_sync_copy(&d_out).expect("dtoh");
    // ARC FAULT — filled by EVERY build, counters or not (kernel `arc_drops`).
    // Read BEFORE the raw dump below so a dump can fail closed on it: a run
    // that dropped arcs under-screens, and its dump would silently compare two
    // different rules. The !gpu_only gate at the bottom re-checks it fatally.
    let arc_drops = gpu[noise_gpu::OUT_FAULT_SLOT] as f64;
    eprintln!("ARC FAULT dropped_arcs={arc_drops:.0} (ARC_MAX_MERGED overflow)");
    // NOISE_GPU_DUMP_RAW=<path>: the raw per-period f32 energies, so two KERNEL
    // variants can be diffed against EACH OTHER instead of against the CPU. That
    // is the right instrument for a precision change and the wrong one for a
    // parity claim: it isolates the change's delta from every other lane
    // difference, which is exactly what you want while the CPU↔GPU arc parity
    // crack is open and must not be mixed into the measurement. It also costs no
    // CPU reference, so it runs under NOISE_GPU_PERF_ONLY on tiles whose
    // reference is 45 minutes. Sibling of NOISE_GPU_WRITE_TILE below, one level
    // rawer: that one quantises to the HM3 byte (0.5 dB), which is coarser than
    // the drift being measured here.
    if let Ok(p) = std::env::var("NOISE_GPU_DUMP_RAW") {
        anyhow::ensure!(
            arc_drops == 0.0,
            "NOISE_GPU_DUMP_RAW refused: {arc_drops:.0} dropped arcs — this arm \
             under-screens and its dump would poison the A/B evidence"
        );
        let path = Path::new(&p);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut raw = Vec::with_capacity(n * 3 * 4);
        for v in &gpu[..n * 3] {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, raw)?;
        eprintln!("dumped {} raw f32 energies to {p}", n * 3);
    }
    eprintln!("GPU kernel {gpu_ms:.1} ms");
    // Port-shape census (only non-zero under -DPROF_COUNTERS=1). These units are
    // deliberately separate: mixing escalating BUCKETS with quadrature PAIRS
    // produces neither the pair nor the bucket eligibility rate. This unbinned
    // e2 source-order population is diagnostic; the production-path rail census
    // owns the reviewed ≈1.2 % acceptance check.
    let mut c = [0f64; noise_gpu::OUT_ARCSTAT_COUNTERS];
    for px in gpu[noise_gpu::OUT_ARCSTAT_BASE..].chunks_exact(noise_gpu::OUT_ARCSTAT_COUNTERS) {
        for (acc, v) in c.iter_mut().zip(px.iter()) {
            *acc += *v as f64;
        }
    }
    let (quad_pairs, escalating_pairs, bucket_rays, escalating_buckets) = (c[0], c[1], c[2], c[3]);
    let (look, hit, clipped, emitted) = (c[4], c[5], c[6], c[7]);
    let (ovf, ovf_calls) = (c[8], c[9]);
    let r = |a: f64, b: f64| if b > 0.0 { a / b } else { 0.0 };
    let buckets_per_pair = r(bucket_rays, quad_pairs);
    let pair_escalation_frac = r(escalating_pairs, quad_pairs);
    let bucket_escalation_frac = r(escalating_buckets, bucket_rays);
    if quad_pairs > 0.0 {
        eprintln!(
            "ARCSTAT quadrature_pairs={quad_pairs:.0} bucket_rays={bucket_rays:.0} \
             buckets_per_pair={:.6} escalating_pairs={escalating_pairs:.0} \
             pair_escalation_frac={:.6} escalating_buckets={escalating_buckets:.0} \
             bucket_escalation_frac={:.6} hull_lookups={look:.0} hull_hit_rate={:.4} \
             clipped={clipped:.0} clip_rate={:.4} emitted={emitted:.0} emit_rate={:.4} \
             iv_overflow={ovf:.0} arc_calls_with_overflow={ovf_calls:.0}",
            buckets_per_pair,
            pair_escalation_frac,
            bucket_escalation_frac,
            r(hit, look),
            r(clipped, look),
            r(emitted, look)
        );
    }

    // ---- compare vs CPU ref (skipped under NOISE_GPU_ONLY): exact f32 inequality +
    // zero-sided mismatches + both-positive dB.
    //
    // WHAT THIS COMPARES, AND WHY THE BYTE-STOP DOES NOT COVER IT: the loop below
    // reads PER-PERIOD energies, not the HM3 byte. The byte-stop's guarantee is
    // only ever about the byte of the Lden COLLAPSE — `Σ_p W_p·e_p` — so it does
    // NOT bound a single period's relative error: a period carrying a small share
    // of that weighted sum can absorb the whole dropped tail and read several dB
    // off, with the collapsed byte still identical. Both lanes stop by default
    // (η = 0.40 ⇒ meta[9] non-zero) and they drop DIFFERENT tails, because the CPU
    // walks loudest-bound-first and the kernel walks source order. So the maxdb
    // below is real GPU drift PLUS legitimate stop divergence, and the two cannot
    // be separated from this number alone: re-run with SURFACE_BUDGET_ETA=0, which
    // puts both lanes on the exact path, before attributing a residual to the
    // kernel. (Direction matters when reading a failure: the CPU skips ~50 % of
    // pairs and the kernel 9-11 %, so the stop makes the CPU the QUIETER lane —
    // it cannot manufacture a CPU-louder asymmetry, only mask one.)
    if !gpu_only {
        let (mut maxdb, mut nover, mut nbit, mut nzero) = (0f64, 0usize, 0usize, 0usize);
        // SIGN of each divergence, which is what separates fp32 noise from a lane
        // FORK. Noise is symmetric; a rule that differs is one-sided. On
        // 2026-08-04 this tile sat at 87.2 % "CPU louder" while the gate below
        // called the same numbers benign — three dropped-sign defects in
        // scatter.cu. After fixing them the split fell to 55.4 %, i.e. a coin
        // flip, and the cell count dropped 70 %. The asymmetry was the signal;
        // nothing was measuring it.
        let (mut n_gpu_louder, mut n_cpu_louder) = (0usize, 0usize);
        for i in 0..n * 3 {
            let (g, c) = (gpu[i], cpu[i]);
            if g != c {
                nbit += 1;
            }
            if (g > 0.0) != (c > 0.0) {
                nzero += 1;
            }
            if g > 0.0 && c > 0.0 {
                let signed = 10.0 * (g as f64 / c as f64).log10();
                let d = signed.abs();
                if d > maxdb {
                    maxdb = d;
                }
                if d > 0.5 {
                    nover += 1;
                    if signed > 0.0 {
                        n_gpu_louder += 1;
                    } else {
                        n_cpu_louder += 1;
                    }
                }
            }
        }
        eprintln!(
            "GPU vs CPU ref: {nbit}/{} f32-unequal, {nzero} zero-sided, max {maxdb:.4} dB, {nover} >0.5 dB",
            n * 3
        );
        if nbit == 0 {
            eprintln!("✓ full path + budget skip port BYTE-EXACT vs CPU ref");
        } else if maxdb < 0.5 && nzero == 0 {
            eprintln!("✓ port within 0.5 dB ({nbit} sub-0.5 dB f32 drifts, 0 zero-sided)");
        } else if nzero == 0 {
            let share = if nover == 0 {
                0.0
            } else {
                100.0 * n_gpu_louder as f64 / nover as f64
            };
            eprintln!(
                "lane drift: {nover} cells >0.5 dB (max {maxdb:.2} dB), 0 presence-flips, \
                 {n_gpu_louder} GPU-louder / {n_cpu_louder} CPU-louder ({share:.1}% GPU)"
            );
        }
        // HARD GATE (gg review 2026-07-28 #2: mismatch stats must be able to
        // FAIL the run, not just print). Three independent ways to fail:
        //
        // 1. A zero-sided cell — the lanes disagree on audibility itself.
        // 2. MAGNITUDE. Until 2026-08-04 this branch printed "benign
        //    (scattered, popup-corrected)" for ANY max, gated on `nzero == 0`
        //    alone, and its own comment expected "max ~5 dB". It therefore
        //    reported 14.91 dB as benign and returned success, for three
        //    successive eras, while three dropped-sign defects sat in
        //    scatter.cu. A check that explains its own failure away is not a
        //    check. The bar is the owner's: 1.0 dB anywhere.
        // 3. ASYMMETRY. fp32-vs-f64 noise has no preferred direction, so a
        //    lopsided split is a RULE difference however small its dB. This is
        //    the test that actually identified the defect (87.2 % → 55.4 %
        //    after the fix), and it catches forks that hide under the dB bar.
        //    Only applied once the sample is big enough to mean anything.
        const LANE_MAX_DB: f64 = 1.0;
        const ASYMMETRY_MIN_CELLS: usize = 200;
        const ASYMMETRY_MAX_SHARE: f64 = 0.75;
        // 4. ARC DROPS. The CPU's twin of ARC_MAX_MERGED is `usize::MAX`, so a
        //    single dropped arc means the kernel answered a question the reference
        //    was never asked — under-screening by construction, in whatever cells
        //    it hit. Checked here rather than left to the dB bar because the drops
        //    are counted exactly while their dB cost is diluted over 262 144 cells.
        anyhow::ensure!(
            arc_drops == 0.0,
            "e2 GATE FAILED: {arc_drops:.0} blocked arcs DROPPED for ARC_MAX_MERGED \
             overflow — the kernel under-screens those pairs while the uncapped CPU \
             reference does not, so this comparison is between two different rules. \
             Raise it: NOISE_GPU_DEFINES=\"-DARC_MAX_MERGED=<bigger>\""
        );
        anyhow::ensure!(
            nzero == 0,
            "e2 GATE FAILED: {nzero} zero-sided cells (one lane audible, the other silent)"
        );
        anyhow::ensure!(
            maxdb <= LANE_MAX_DB,
            "e2 GATE FAILED: lanes differ by {maxdb:.2} dB (limit {LANE_MAX_DB:.1}) on \
             {nover} cells — fp32 rounding does not produce that; look for a rule fork"
        );
        if nover >= ASYMMETRY_MIN_CELLS {
            let share = n_gpu_louder.max(n_cpu_louder) as f64 / nover as f64;
            anyhow::ensure!(
                share <= ASYMMETRY_MAX_SHARE,
                "e2 GATE FAILED: {:.1}% of {nover} divergent cells lean one way \
                 ({n_gpu_louder} GPU-louder / {n_cpu_louder} CPU-louder) — noise is \
                 symmetric, so this is a rule difference, not precision",
                100.0 * share
            );
        }
    }

    // ---- (2) GPU energy → collapse → u8, diff vs the production baseline ----
    // gpu is pix*3+period, the exact TileAccumulator.energy layout.
    let mut accum = TileAccumulator::new();
    accum.energy.copy_from_slice(&gpu[..n * 3]);
    let cells = collapse_lden_surface_u8(&accum);
    // NOISE_GPU_WRITE_TILE=<path>: dump this run's GPU tile in the production
    // HM3 wire format so two kernel variants can be diffed with the owner's
    // acceptance instrument (`compare_hm3 exact.bin candidate.bin`) at the cost
    // of ONE tile instead of a whole cell repaint. Same collapse the production
    // painter uses, so the bytes are the ones the world would ship.
    if let Ok(p) = std::env::var("NOISE_GPU_WRITE_TILE") {
        let path = Path::new(&p);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let n_bytes = tile_painter::wire_hm3::write_tile(path, &cells, 0, false)?;
        eprintln!("wrote GPU tile {p} ({n_bytes} bytes)");
    }
    let base_path = Path::new(&baseline)
        .join(format!("rail/{z}"))
        .join(x.to_string())
        .join(format!("{y}.bin"));
    if base_path.exists() {
        let b = read_tile(&base_path)?;
        let (mut maxb, mut nd, mut presence) = (0i32, 0usize, 0usize);
        for i in 0..cells.len().min(b.len()) {
            let (c, bb) = (cells[i], b[i]);
            if c == NO_DATA && bb == NO_DATA {
                continue;
            }
            if (c == NO_DATA) != (bb == NO_DATA) {
                presence += 1;
                continue;
            }
            let d = (c as i32 - bb as i32).abs();
            maxb = maxb.max(d);
            if d > 0 {
                nd += 1;
            }
        }
        eprintln!(
            "GPU→u8 vs production baseline: max {maxb}B ({:.1} dB), {nd} differ, {presence} presence-changed",
            maxb as f64 * 0.5
        );
    } else {
        eprintln!("(no baseline tile at {base_path:?} — skipping u8 parity)");
    }
    Ok(())
}

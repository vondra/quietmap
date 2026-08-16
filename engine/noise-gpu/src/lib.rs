//! Shared host-side helpers for the GPU scatter, used by all four bins in
//! `Cargo.toml`: the `gpu-surface` / `gpu-airborne` production batch runners
//! and the `e2-full` / `e2-airborne` parity validators. Surface geometry and
//! obstacle upload live here; the airborne path is [`airborne`].

/// Region-resident GPU airborne scatter, shared by the `e2-airborne` validator and the
/// `gpu-airborne` production builder (cudarc-backed, so gated on the `gpu` feature).
#[cfg(feature = "gpu")]
pub mod airborne;
#[cfg(feature = "h0-v3")]
pub mod h0_v3_field;

use noise_compute::emission::aircraft::{Installation, SegmentPrepared, M_PER_DEG_LAT};
use noise_compute::propagation::streaming_reduction::{source_frame_mlon, SourceId64};
use noise_compute::types::Barrier;
use raster_reader::fused_tile_z13::{FusedTileZ13, TILE_PX};
use tile_painter::source_line::LineRow;

/// Build the frozen twelve-argument surface tuple. Adding or removing an
/// argument is a compile-time ABI error at every launch site.
#[macro_export]
macro_rules! line_kernel_arguments {
    (
        $arg0:expr, $arg1:expr, $arg2:expr, $arg3:expr,
        $arg4:expr, $arg5:expr, $arg6:expr, $arg7:expr,
        $arg8:expr, $arg9:expr, $arg10:expr, $arg11:expr $(,)?
    ) => {{
        const _: [(); $crate::LINE_KERNEL_ARGUMENT_COUNT] = [(); 12];
        (
            $arg0, $arg1, $arg2, $arg3, $arg4, $arg5, $arg6, $arg7, $arg8, $arg9, $arg10, $arg11,
        )
    }};
}
fn inst_code(inst: Installation) -> i32 {
    match inst {
        Installation::Wing => 0,
        Installation::Fuselage => 1,
        Installation::Propeller => 2,
    }
}

/// Pack a tile's receiver lattice (rll = lat|lon|m_per_deg_lon, rxa = elevation) —
/// uploaded ONCE; near and every far level index into the same lattice.
pub fn pack_airborne_receivers(tile: &FusedTileZ13) -> (Vec<f64>, Vec<f32>) {
    let n = TILE_PX;
    let mut rll = Vec::with_capacity(3 * n);
    rll.extend_from_slice(&tile.rx_lat); // [0..n] rx lat
    rll.extend_from_slice(&tile.rx_lon); // [n..2n] rx lon
    for py in 0..n {
        // Mirror airborne.rs:175 — m_per_deg_lon per receiver row.
        rll.push(M_PER_DEG_LAT * tile.rx_lat[py].to_radians().cos().max(0.2));
    }
    (rll, tile.rx_alt_m.clone())
}

/// Concatenate a block's tiles' receiver lattices for the M3 batched kernels: `rll`
/// is per-tile `[lat | lon | m_per_deg_lon]` (3·TPX each), `rxa` is per-tile elevation
/// (TPX·TPX each), tile `ti` at stride `ti·3·TPX` / `ti·TPX·TPX`. Same per-tile content
/// as `pack_airborne_receivers`, just concatenated so one launch covers the block.
pub fn pack_airborne_receivers_batch(tiles: &[&FusedTileZ13]) -> (Vec<f64>, Vec<f32>) {
    let n = TILE_PX;
    let mut rll = Vec::with_capacity(3 * n * tiles.len());
    let mut rxa = Vec::with_capacity(n * n * tiles.len());
    for tile in tiles {
        rll.extend_from_slice(&tile.rx_lat);
        rll.extend_from_slice(&tile.rx_lon);
        for py in 0..n {
            rll.push(M_PER_DEG_LAT * tile.rx_lat[py].to_radians().cos().max(0.2));
        }
        rxa.extend_from_slice(&tile.rx_alt_m);
    }
    (rll, rxa)
}

/// Pack a sub-seg list into the per-seg device SoA (sll, sf, si) — shared by the
/// near launch and each far level (receivers reused, segs per level).
pub fn pack_airborne_segs(segs: &[(SegmentPrepared, u8)]) -> (Vec<f64>, Vec<f32>, Vec<i32>) {
    let nseg = segs.len();
    let mut sll = vec![0.0f64; 2 * nseg];
    let mut sf = Vec::with_capacity(12 * nseg);
    let mut si = Vec::with_capacity(4 * nseg);
    for (s, (p, period)) in segs.iter().enumerate() {
        sll[s] = p.start_lat;
        sll[nseg + s] = p.start_lon;
        sf.extend_from_slice(&[
            p.start_alt_m as f32,
            p.d_lon as f32,
            p.sdy as f32,
            p.sdz as f32,
            p.dv as f32,
            p.d_bar_m as f32,
            p.di_a as f32,
            p.di_b as f32,
            p.di_c as f32,
            p.reach_sq as f32,
            p.terrain_start_cut_m as f32,
            p.terrain_end_cut_m as f32,
        ]);
        si.extend_from_slice(&[
            inst_code(p.inst),
            p.class_idx as i32,
            p.is_departure as i32,
            (*period).min(2) as i32,
        ]);
    }
    (sll, sf, si)
}

/// Pixel-bin edge: a 16×16 receiver patch = one CUDA block in `line_binned`.
pub const BIN_W: usize = 16;
/// Bins per axis (an internal step for `N_BINS`; not part of the public binning API like `BIN_W`).
const BIN_TILES: usize = TILE_PX / BIN_W;
/// Bins per tile (BIN_TILES², 1024 at 512 px) — `line_binned`'s grid dim.
pub const N_BINS: usize = BIN_TILES * BIN_TILES;

/// f32 slots of per-pixel energy in the `line`/`line_binned_fused` `out` buffer
/// (three Lden periods per pixel) — mirrored by `OUT_ENERGY_SLOTS` in
/// `kernels/scatter.cu`.
pub const OUT_ENERGY_SLOTS: usize = TILE_PX * TILE_PX * 3;
/// Index of the ARC FAULT slot: blocked arcs the kernel had to DROP because the
/// merged-arc list hit `ARC_MAX_MERGED`, summed over the whole launch (and, since
/// `out` is allocated once per block, over every tile that launch painted).
///
/// Nonzero means the tiles it counts UNDER-screen — a genuinely blocked direction
/// was painted clear. It is one f32, allocated in production too, because for as
/// long as the only overflow counter lived behind `-DPROF_COUNTERS=1` there was no
/// way to tell "never happened" from "never measured", and the answer turned out
/// to be 1.1 M dropped arcs per dense tile. Being an f32 it saturates well past
/// 2^24, so read it as a fault flag with an order of magnitude; the exact census
/// is `arcstat[6]` under `-DPROF_COUNTERS=1`.
pub const OUT_FAULT_SLOT: usize = OUT_ENERGY_SLOTS;
/// First slot of the per-pixel `-DPROF_COUNTERS=1` block (8 f32 per pixel).
pub const OUT_ARCSTAT_BASE: usize = OUT_ENERGY_SLOTS + 1;
/// `out` length the PRODUCTION painter allocates: energies + the fault slot.
pub const OUT_SLOTS_PROD: usize = OUT_ENERGY_SLOTS + 1;
/// `out` length a counter-instrumented build needs (+8 MiB at 512 px). The kernel
/// writes the counter block ONLY when `meta[13]` says the buffer is at least this
/// long, so a `-DPROF_COUNTERS=1` PTX run under the production painter — which
/// allocates [`OUT_SLOTS_PROD`] — writes none of it instead of running off the end
/// of the buffer.
pub const OUT_SLOTS_PROF: usize = OUT_ARCSTAT_BASE + TILE_PX * TILE_PX * 8;

/// Byte-aligned start of the exact H0 u64 counters in the existing `out`
/// allocation. The obstacle pointer table has no spare slot; keeping these
/// counters in `out` also leaves the barrier candidate-tail authority untouched.
pub const OUT_H0_COUNTER_BYTE_OFFSET: usize = 3_145_736;
const _: () = assert!(
    OUT_H0_COUNTER_BYTE_OFFSET
        == (OUT_SLOTS_PROD * std::mem::size_of::<f32>())
            .next_multiple_of(std::mem::size_of::<u64>())
);
/// Exact u64 channels: node overflow, hard geometry, ABI/layout, guarded legal
/// degenerates, completed pairs, raw candidate visits, generated nodes and
/// admitted nodes. They are review evidence, never acoustic accumulation.
pub const OUT_H0_COUNTERS: usize = 8;
/// f32 allocation length for the compile-time H0 arm, including aligned u64s.
pub const OUT_SLOTS_H0: usize = 786_450;
const _: () = assert!(
    OUT_SLOTS_H0
        == (OUT_H0_COUNTER_BYTE_OFFSET + OUT_H0_COUNTERS * std::mem::size_of::<u64>())
            .div_ceil(std::mem::size_of::<f32>())
);

/// Versioned word layout of the diagnostic-only actual-store H0 pair dump.
/// This buffer is a separate kernel argument; it never enters the production
/// 12-argument line launch or its `out` allocation.
pub const H0_PAIR_DIAGNOSTIC_ABI_VERSION: usize = 1;
pub const H0_PAIR_DIAGNOSTIC_HEADER_WORDS: usize = 24;
pub const H0_PAIR_DIAGNOSTIC_NODE_WORDS: usize = 8;
pub const H0_PAIR_DIAGNOSTIC_RECORD_WORDS: usize = 7;
pub const H0_PAIR_DIAGNOSTIC_NODE_BASE: usize = 24;
pub const H0_PAIR_DIAGNOSTIC_RECORD_BASE: usize = 552;
pub const H0_PAIR_DIAGNOSTIC_MAGIC: u64 = 0x514d_4830_5041_4952;
const _: () = assert!(
    H0_PAIR_DIAGNOSTIC_RECORD_BASE
        == H0_PAIR_DIAGNOSTIC_NODE_BASE
            + noise_compute::compute::element::H0_NODE_CAP * H0_PAIR_DIAGNOSTIC_NODE_WORDS
);

/// Versioned surface metadata layout. Slot 14 carries the line-layer tag used
/// to select the frozen road/rail H0 placement floor.
pub const SURFACE_META_ABI_VERSION: usize = 2;
pub const SURFACE_META_SLOTS: usize = 15;
pub const SURFACE_META_LAYER_SLOT: usize = 14;

/// Per-tile non-halo buffers packed for the `line`/`line_binned_fused` kernels (the halo
/// elev/cover are uploaded once per batch and shared; the line SOURCES are uploaded
/// once per layer — see [`SourceBuffers`]). `meta` carries the SHARED halo geom +
/// this tile's bbox + eta + swizzle width + barrier count. `barr` is the tile's
/// vector noise-wall slice, nbarr×[`BARRIER_STRIDE`]
/// `{start_lat, start_lon, end_lat, end_lon, height_m, dist_m, source_id_bits}` in
/// `BarrierData::for_tile` order (dist_m a conservative lower bound, sorted
/// ascending — the kernel's early-break key). The kernel intersects the
/// ENDPOINTS with each propagation ray, exactly as `path_effects` §1 does.
pub struct TileBuffers {
    pub inner: Vec<f32>,
    pub meta: Vec<f64>,
    pub rxll: Vec<f64>,
    pub rxar: Vec<f32>,
    pub barr: Vec<f64>,
}

/// A layer's line sources, packed ONCE per (region, layer) and uploaded once; the
/// per-tile bins (`PixelBins.indices`) index into these arrays, so they are tile-
/// invariant. Previously re-packed and re-uploaded per tile — ~160 MB/tile on a
/// dense (LKPR-class) layer, ~30× redundant PCIe + CPU work across a region.
pub struct SourceBuffers {
    pub seg: Vec<f64>,
    pub sp: Vec<f64>,
    pub semis: Vec<f32>,
}

/// Pack a layer's line sources (tile-invariant): `seg` (4 coords + the exact
/// source-frame longitude scale), `sp` (12 =
/// length/reach/height/bridge ++ 8 host-precomputed Lden band weights), `semis`
/// (3 periods × 8 emission bands). The 8 `sp[4+i]` = `Σ_p LDEN_W[p]·emission_lin[p][i]`
/// — the energy-budget-skip UB loop's per-band Lden weight, hoisted off the GPU
/// (was 24 f64 mul-adds + casts per source×receiver inside `line_source`; the kernel
/// now reads `sp[4+i]`). Byte-identical: same f64 FMA, just evaluated once on the host.
const LDEN_W: [f64; 3] = [12.0, 12.649110640673518, 80.0]; // 4·√10 (mirror scatter.cu LDEN_W)
pub fn pack_sources(lines: &[LineRow]) -> SourceBuffers {
    let (mut seg, mut sp, mut semis) = (
        Vec::with_capacity(lines.len() * SOURCE_SEGMENT_STRIDE),
        Vec::with_capacity(lines.len() * 12),
        Vec::with_capacity(lines.len() * 24),
    );
    for r in lines {
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
    SourceBuffers { seg, sp, semis }
}

/// Pack one physical barrier slice with the versioned stride and mandatory
/// zero-count dummy row. Exposed for the actual-store H0 diagnostic so it
/// launches the same bytes as [`pack_tile`].
pub fn pack_barrier_rows(barriers: &[Barrier]) -> Vec<f64> {
    let physical_slots = barrier_candidate_tail_slot_offset(barriers.len());
    let mut packed = Vec::with_capacity(physical_slots);
    for barrier in barriers {
        packed.extend_from_slice(&[
            barrier.start_lat,
            barrier.start_lon,
            barrier.end_lat,
            barrier.end_lon,
            barrier.height_m as f64,
            barrier.dist_m,
            SourceId64::wall(barrier.osm_id, barrier.segment_idx)
                .expect("barrier provenience outside the GPU ABI")
                .as_f64_bits(),
        ]);
    }
    if packed.is_empty() {
        packed.resize(physical_slots, 0.0);
    }
    debug_assert_eq!(packed.len(), physical_slots);
    packed
}

/// Pack one tile's per-tile device buffers (inner DEM + meta + receivers +
/// barriers). The line sources are NOT here — they are layer-invariant (see
/// [`pack_sources`]). `halo_geom` = (lat_min, lon_min, inv, rows, cols) of the
/// SHARED batch halo. `barriers` is this tile's `BarrierData::for_tile` slice;
/// an empty slice packs one zero row (cuMemAlloc rejects 0 bytes) with
/// `meta[11] = 0` so the kernel never reads it.
///
/// `eta` keeps its name and its slot (`meta[9]`) but not its meaning: the surface
/// kernel replaced the energy-budget skip with the exact byte-space stop
/// (`scatter.cu`'s `bs_decided`, mirroring `tile_painter::byte_stop`), which has
/// no tolerance to set. The kernel now reads the slot as ON/OFF, and `0` — what
/// `SURFACE_BUDGET_ETA=0` has always produced — still means "skip nothing, exact
/// reference", so every caller and harness keeps working unchanged.
pub fn pack_tile(
    tile: &FusedTileZ13,
    halo_geom: (f64, f64, f64, usize, usize),
    eta: f64,
    tw: f64,
    barriers: &[Barrier],
    nsrc: usize,
    out_slots: usize,
    line_layer_tag: usize,
) -> TileBuffers {
    let (lat_min, lon_min, inv, rows, cols) = halo_geom;
    let n = TILE_PX * TILE_PX;
    // meta[12] = nsrc: the kernel reads the source count from meta because
    // the freed launch-arg slot carries the obstacle pointer table (cudarc's
    // tuple launch caps at 12 args).
    // meta[13] = out_slots: the f32 length the CALLER allocated for `out`. The
    // kernel's optional regions (fault slot, PROF_COUNTERS block) are entered
    // only when this proves the room exists, so the buffer size and the writes
    // to it cannot drift apart — the shape of bug that let a counter build write
    // 8 MiB past the end of the production painter's buffer.
    assert!(
        line_layer_tag <= 1,
        "surface line-layer tag must be road=0 or rail=1"
    );
    let meta = vec![
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
        barriers.len() as f64,
        nsrc as f64,
        out_slots as f64,
        line_layer_tag as f64,
    ];
    debug_assert_eq!(meta.len(), SURFACE_META_SLOTS);
    let mut rxll = Vec::with_capacity(2 * TILE_PX);
    rxll.extend_from_slice(&tile.rx_lat);
    rxll.extend_from_slice(&tile.rx_lon);
    let mut rxar = Vec::with_capacity(n * 2);
    for i in 0..n {
        rxar.push(tile.rx_alt_m[i]);
        rxar.push(tile.rx_refl_db[i]);
    }
    let barr = pack_barrier_rows(barriers);
    TileBuffers {
        inner: tile.inner_elev_m.clone(),
        meta,
        rxll,
        rxar,
        barr,
    }
}

/// Host-flat obstacle store for the CUDA lane (geodata-v2 1.6): every
/// per-cell [`ObstacleIndex`] of a region's [`ObstacleSet`] flattened into
/// uploadable arrays. Per index, `metas` carries [`META_STRIDE`] f64 — the
/// authoritative slot list is at the `extend_from_slice` in
/// [`flatten_obstacles`], which is where a new slot gets added:
/// `[origin_lat, origin_lon, m_per_deg_lon, cell_m, min_x, min_y, cols,
/// rows, starts_off, refs_off, edges_off, n_starts, maxh_off,
/// foot_off, cell_foot_starts_off, cell_foot_ids_off, n_foot,
/// foot_edge_starts_off, foot_edge_refs_off]` where the offsets index the
/// SHARED `starts`/`refs`/`edges`/`cell_max_h` and footprint-CSR arrays
/// (edges stride 5: x0,y0,x1,y1,height — `GpuGridView` order). The kernel
/// DDA mirrors `ObstacleIndex::crossings` per index; e2-full is the parity
/// gate. `cell_max_h` powers the exact branch-and-bound cell prune (z13
/// plan E2): `detour` is convex in t for a fixed top, so the ABOVE-sight-line
/// branch is bounded at the chainage endpoints — the below-line branch peaks
/// at the reflection point instead, see `CellPrune::max_delta`.
pub struct ObstacleFlat {
    pub n_indexes: usize,
    pub metas: Vec<f64>,
    pub starts: Vec<u32>,
    pub refs: Vec<u32>,
    pub edges: Vec<f32>,
    /// Per grid cell: max edge height in that cell (0 for empty cells) —
    /// the kernel's branch-and-bound prune (a cell whose best possible
    /// candidate cannot beat the running max-δ is skipped whole).
    pub cell_max_h: Vec<f32>,
    /// Per edge: the owning footprint's id, parallel to `edges` (stride 1).
    /// Arc screening (fix-pack Fix 1) builds ONE angular hull per footprint, so
    /// the kernel needs the owner identity and `gpu_view` therefore materialises
    /// it — see [`GpuGridView::edge_ids`]. It reaches the device as `obst` slot
    /// 6; only `ObstacleKind` is still host-only.
    pub edge_ids: Vec<u32>,

    // ---- Footprint-CSR (arc screening, TRACK C 2026-08-03). The edge-CSR
    // above forces the kernel to discover footprints EDGE BY EDGE: it keeps a
    // per-(segment, receiver) table of partial hulls, searches it linearly per
    // edge, and CAPS it (ARC_MAX_IV) because a thread cannot grow a Vec — which
    // costs local memory and, worse, diverges from the CPU's uncapped grouping
    // on any dense tile. With the footprint as the unit each one is visited
    // once, completed in a single pass and retired, so the table and its cap
    // both disappear.
    /// Per footprint, the CSR extent of [`Self::foot_edge_refs`]. A CSR rather
    /// than a `[first, end)` range because a footprint id's edges are USUALLY
    /// contiguous in build order but not always (287 of them are not on the
    /// Dobříš store), and a range would silently split those hulls in two —
    /// under-blocking a real building.
    pub foot_edge_starts: Vec<u32>,
    /// Edge indices per footprint, grouped by owner (index-local).
    pub foot_edge_refs: Vec<u32>,
    /// Per footprint: `[min_x, min_y, max_x, max_y]` in the index's local frame.
    /// It feeds the wedge prune, so a footprint that cannot lie inside the
    /// segment's angular span is rejected by sign tests alone before any `atan2`
    /// touches its vertices.
    ///
    /// It USED to carry two more slots, `max_height_m` and a `convex` flag, for a
    /// walk that emitted one angular HULL arc per convex footprint. Both went with
    /// that walk (2026-08-09): a hull arc can carry only one RANGE, and handing a
    /// footprint's minimum range to all of its directions was a measured 17.6 dB
    /// parity fault against the CPU lane, so every footprint now emits one arc per
    /// obstacle EDGE — the rule `ObstacleIndex::skyline_arcs_within` follows — and
    /// each arc reads its own edge's range and height (`edges` slot 4). Neither the
    /// footprint-wide height nor its convexity has anything left to decide.
    pub foot_box: Vec<f32>,
    /// Per grid cell, the CSR extent of [`Self::cell_foot_ids`].
    pub cell_foot_starts: Vec<u32>,
    /// Footprint index per (cell, footprint) pair — the edge lists deduplicated
    /// by owner, so it names exactly the footprints the edge-CSR would yield.
    pub cell_foot_ids: Vec<u32>,
    /// For each entry of [`Self::cell_foot_ids`], the previous ENTRY listing the
    /// same footprint (`u32::MAX` for the first) — a back-link CHAIN, walked by
    /// the kernel to decide whether an earlier listing of this footprint falls
    /// inside the cell set the current query actually visits.
    ///
    /// It links entries, not cells, because a single back-link is not enough:
    /// a footprint listed in cells c1 < c2 < c3 where only c1 and c3 are inside
    /// the walked set would see `prev(c3) = c2` outside and process itself a
    /// SECOND time. Idempotent (the interval union absorbs the duplicate, which
    /// is why the outputs still matched byte-for-byte), but pure wasted work —
    /// and it got worse, not better, as the walked set shrank to the triangle.
    pub cell_foot_prev: Vec<u32>,
    /// The cell each entry of [`Self::cell_foot_ids`] belongs to, so a chain hop
    /// can test that cell for membership of the walked set.
    pub cell_foot_cell: Vec<u32>,
}

/// f32 slots per footprint in [`ObstacleFlat::foot_box`] — the bbox alone; see
/// that field for the two the per-edge arc walk retired.
pub const FOOT_BOX_STRIDE: usize = 4;

/// One index's footprint-CSR, built from the `gpu_view` edge order + `edge_ids`.
struct FootprintCsr {
    foot_edge_starts: Vec<u32>,
    foot_edge_refs: Vec<u32>,
    foot_box: Vec<f32>,
    cell_foot_starts: Vec<u32>,
    cell_foot_ids: Vec<u32>,
    cell_foot_prev: Vec<u32>,
    cell_foot_cell: Vec<u32>,
    n_foot: usize,
}

/// Group one index's edges by footprint id, then list every footprint once per
/// grid cell its edges occupy. Footprint indices follow FIRST-APPEARANCE order
/// in the edge array, so the grouping is deterministic and independent of the
/// hash iteration order.
fn footprint_csr_for_index(
    v: &noise_compute::propagation::obstacle_index::GpuGridView<'_>,
    edge_ids: &[u32],
) -> FootprintCsr {
    let n_edges = edge_ids.len();
    // 1. id ⇒ footprint index (first appearance), and the per-edge owner.
    let mut edge_foot = vec![0u32; n_edges];
    let mut of_id: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut n_foot = 0usize;
    for (e, &id) in edge_ids.iter().enumerate() {
        let f = *of_id.entry(id).or_insert_with(|| {
            let f = n_foot as u32;
            n_foot += 1;
            f
        });
        edge_foot[e] = f;
    }
    // 2. counting sort of the edges by owner ⇒ the per-footprint CSR, plus each
    //    footprint's local-frame bbox.
    let mut foot_edge_starts = vec![0u32; n_foot + 1];
    for &f in &edge_foot {
        foot_edge_starts[f as usize + 1] += 1;
    }
    for f in 0..n_foot {
        foot_edge_starts[f + 1] += foot_edge_starts[f];
    }
    let mut cursor = foot_edge_starts[..n_foot].to_vec();
    let mut foot_edge_refs = vec![0u32; n_edges];
    let mut foot_box = vec![0.0f32; n_foot * FOOT_BOX_STRIDE];
    for f in 0..n_foot {
        foot_box[f * FOOT_BOX_STRIDE] = f32::INFINITY;
        foot_box[f * FOOT_BOX_STRIDE + 1] = f32::INFINITY;
        foot_box[f * FOOT_BOX_STRIDE + 2] = f32::NEG_INFINITY;
        foot_box[f * FOOT_BOX_STRIDE + 3] = f32::NEG_INFINITY;
    }
    for (e, &f) in edge_foot.iter().enumerate() {
        let f = f as usize;
        foot_edge_refs[cursor[f] as usize] = e as u32;
        cursor[f] += 1;
        let g = &v.edges_xyxyh[e * 5..e * 5 + 4];
        let b = &mut foot_box[f * FOOT_BOX_STRIDE..f * FOOT_BOX_STRIDE + FOOT_BOX_STRIDE];
        b[0] = b[0].min(g[0]).min(g[2]);
        b[1] = b[1].min(g[1]).min(g[3]);
        b[2] = b[2].max(g[0]).max(g[2]);
        b[3] = b[3].max(g[1]).max(g[3]);
    }

    // 2. per cell, the footprints its edge list names — deduplicated, each
    //    carrying the back-link to that footprint's previous listing.
    let n_cells = v.cell_starts.len().saturating_sub(1);
    let mut cell_foot_starts = Vec::with_capacity(n_cells + 1);
    let mut cell_foot_ids: Vec<u32> = Vec::new();
    let mut cell_foot_prev: Vec<u32> = Vec::new();
    let mut last_entry = vec![u32::MAX; n_foot];
    let mut cell_foot_cell: Vec<u32> = Vec::new();
    let mut scratch: Vec<u32> = Vec::new();
    cell_foot_starts.push(0u32);
    for c in 0..n_cells {
        let (lo, hi) = (v.cell_starts[c] as usize, v.cell_starts[c + 1] as usize);
        scratch.clear();
        scratch.extend(v.edge_refs[lo..hi].iter().map(|&r| edge_foot[r as usize]));
        scratch.sort_unstable();
        scratch.dedup();
        for &f in &scratch {
            let entry = cell_foot_ids.len() as u32;
            cell_foot_ids.push(f);
            cell_foot_prev.push(last_entry[f as usize]);
            cell_foot_cell.push(c as u32);
            last_entry[f as usize] = entry;
        }
        cell_foot_starts.push(cell_foot_ids.len() as u32);
    }
    FootprintCsr {
        foot_edge_starts,
        foot_edge_refs,
        foot_box,
        cell_foot_starts,
        cell_foot_ids,
        cell_foot_prev,
        cell_foot_cell,
        n_foot,
    }
}

pub fn flatten_obstacles(
    set: &noise_compute::propagation::obstacle_index::ObstacleSet,
) -> ObstacleFlat {
    let mut flat = ObstacleFlat {
        n_indexes: set.indexes.len(),
        metas: Vec::with_capacity(set.indexes.len() * META_STRIDE),
        starts: Vec::new(),
        refs: Vec::new(),
        edges: Vec::new(),
        cell_max_h: Vec::new(),
        edge_ids: Vec::new(),
        foot_edge_starts: Vec::new(),
        foot_edge_refs: Vec::new(),
        foot_box: Vec::new(),
        cell_foot_starts: Vec::new(),
        cell_foot_ids: Vec::new(),
        cell_foot_prev: Vec::new(),
        cell_foot_cell: Vec::new(),
    };
    for idx in &set.indexes {
        let v = idx.gpu_view();
        let csr = footprint_csr_for_index(&v, &v.edge_ids);
        flat.edge_ids.extend_from_slice(&v.edge_ids);
        let (foot_off, cfs_off, cfi_off, fes_off, fer_off) = (
            flat.foot_box.len() / FOOT_BOX_STRIDE,
            flat.cell_foot_starts.len(),
            flat.cell_foot_ids.len(),
            flat.foot_edge_starts.len(),
            flat.foot_edge_refs.len(),
        );
        flat.metas.extend_from_slice(&[
            v.origin_lat,
            v.origin_lon,
            v.m_per_deg_lon,
            v.cell_m,
            v.min_x,
            v.min_y,
            v.cols as f64,
            v.rows as f64,
            flat.starts.len() as f64,
            flat.refs.len() as f64,
            (flat.edges.len() / 5) as f64,
            // slot 11: this index's starts extent — the kernel never reads
            // it (its walk is bounded by cols×rows), but the flatten test
            // proves the shared arrays tile exactly with it.
            v.cell_starts.len() as f64,
            // slot 12: this index's offset into the shared cell_max_h.
            flat.cell_max_h.len() as f64,
            // slots 13..=16: the footprint-CSR offsets (foot_range/foot_box
            // share `foot_off`, both being per-footprint) and the footprint
            // count. `cell_foot_starts` carries n_cells+1 entries per index,
            // so its offset is NOT cfi_off.
            foot_off as f64,
            cfs_off as f64,
            cfi_off as f64,
            csr.n_foot as f64,
            // slots 17..=18: the per-footprint edge CSR (n_foot+1 starts and
            // one ref per edge for THIS index).
            fes_off as f64,
            fer_off as f64,
        ]);
        flat.foot_edge_starts
            .extend_from_slice(&csr.foot_edge_starts);
        flat.foot_edge_refs.extend_from_slice(&csr.foot_edge_refs);
        flat.foot_box.extend_from_slice(&csr.foot_box);
        flat.cell_foot_starts
            .extend_from_slice(&csr.cell_foot_starts);
        flat.cell_foot_ids.extend_from_slice(&csr.cell_foot_ids);
        flat.cell_foot_prev.extend_from_slice(&csr.cell_foot_prev);
        flat.cell_foot_cell.extend_from_slice(&csr.cell_foot_cell);
        flat.cell_max_h.extend_from_slice(v.cell_max_h);
        flat.starts.extend_from_slice(v.cell_starts);
        flat.refs.extend_from_slice(v.edge_refs);
        flat.edges.extend_from_slice(&v.edges_xyxyh);
    }
    flat
}

/// Barrier layout version injected into every CUDA translation unit.
pub const BARRIER_ABI_VERSION: usize = 2;
/// f64 slots per barrier in the `barr` buffer.
pub const BARRIER_STRIDE: usize = 7;
/// Source-segment layout version injected into every CUDA translation unit.
pub const SOURCE_SEGMENT_ABI_VERSION: usize = 2;
/// f64 slots per source segment: four coordinates plus authoritative `mlon`.
pub const SOURCE_SEGMENT_STRIDE: usize = 5;
/// Cudarc's physical tuple remains frozen at twelve kernel arguments.
pub const LINE_KERNEL_ARGUMENT_COUNT: usize = 12;

/// First f64 slot after the physical barrier rows, including the mandatory
/// dummy row for `barrier_count == 0`. Indexed replay must append here.
pub fn barrier_candidate_tail_slot_offset(barrier_count: usize) -> usize {
    barrier_count
        .max(1)
        .checked_mul(BARRIER_STRIDE)
        .expect("barrier candidate-tail slot offset overflow")
}

/// Byte form of [`barrier_candidate_tail_slot_offset`].
pub fn barrier_candidate_tail_byte_offset(barrier_count: usize) -> usize {
    barrier_candidate_tail_slot_offset(barrier_count)
        .checked_mul(std::mem::size_of::<f64>())
        .expect("barrier candidate-tail byte offset overflow")
}

/// f64 slots per index in [`ObstacleFlat::metas`].
pub const META_STRIDE: usize = 19;

/// Refuse to run the GPU lane while a CPU-ONLY tuning lever is set.
///
/// `noise_compute::propagation::arc_screening` reads these at runtime
/// (`ArcBounds::from_env`). The kernel CANNOT: its constants are
/// compiled in — `build.rs` injects them from the Rust source precisely so they
/// cannot drift — and device code never reads the environment. So setting one of
/// these moves the CPU rule and leaves the GPU on the shipped value, and any
/// comparison made in that state is rigged without saying so.
///
/// This is not hypothetical. On 2026-08-04 a CPU sweep turned the arc span gate
/// off (`min_span_rad = 0`) while `scatter.cu` kept `ARC_MIN_SPAN = 0.01` for two
/// hours, and earlier an ordered A/B measured the DEFAULT build twice because the
/// `-D` passthrough had been dropped. Both were silent. A knob that reaches one
/// lane is a divergence generator, so the honest behaviour is to stop.
///
/// The kernel's own levers go through `NOISE_GPU_DEFINES="-DNAME=value"`, which
/// `build.rs` forwards to nvcc and which therefore moves the GPU rule for real.
/// To sweep the CPU alone, run a CPU-only binary — not this one.
pub fn ensure_no_cpu_only_arc_levers() -> anyhow::Result<()> {
    /// Every environment name that moves the CPU's ANSWER and not the kernel's.
    /// Keep in step with `ArcBounds::from_env` and `scatter_band::seg_samples` —
    /// a lever added there and missed here is exactly the silent fork this guard
    /// exists to stop.
    ///
    /// Deliberately NOT listed: `QM_ARC_DISABLE_CELL_PRUNE` and
    /// `QM_OBSTACLE_INDEX_CACHE`. Those gate a prune and a cache that are meant to
    /// be exact by construction (skip only strictly-worse candidates; re-read the
    /// same bytes), so a mismatch should cost time and never change a level.
    /// Listing them would fire on honest cost measurements, and a guard that cries
    /// wolf gets switched off. NOTE that "meant to be" is doing work here: on
    /// 2026-08-08 `CellPrune::max_delta` was found underbounding the below-sight-
    /// line branch, i.e. the prune WAS changing levels, which is why the lever
    /// stays even though nothing else reads it.
    /// `(name, value that PINS the CPU to the kernel's rule)`. A lever set to its
    /// pinning value is the OPPOSITE of a fork — it is how an honest lane
    /// comparison is made — so only other values are refused. Filtering by name
    /// alone banned the very procedure `scatter_band::seg_samples` documents as
    /// mandatory, which made `e2-full` unrunnable as specified (2026-08-08).
    const CPU_ONLY: [(&str, Option<&str>); 7] = [
        ("QM_ARC_EXACT", None),
        // 0 makes the CPU arc-screen from the same floor the kernel does
        // (`build.rs` injects it as ARC_DEGENERATE_SPAN, 1e-4 rad). NOT the same
        // as leaving it unset: since 2026-08-08 `seg_arc_bounds()` raises the gate
        // to 3° when this is absent, so absent is a FORK and only the explicit 0
        // pins the lanes. `e2-full` sets it for exactly that reason.
        ("QM_ARC_MIN_SPAN_DEG", Some("0")),
        ("QM_ARC_RADIUS_M", None),
        ("QM_ARC_DELTA_MIN_M", None),
        ("QM_ARC_MAX_ARCS", None),
        ("QM_ARC_MAX_INTERVALS", None),
        // The tile painter's angular quadrature. CUDA has no sampling at all, so
        // the shipped default (N=5) already forks the lanes BY DESIGN — worth up
        // to 1.5 dB on the CPU painter and exactly 0 on the kernel (SPEC §3.5d).
        // N=1 is the one value that restores the kernel's single cp ray.
        ("QM_SEG_SAMPLES", Some("1")),
    ];
    let set: Vec<&str> = CPU_ONLY
        .iter()
        // Compare the pinning value NUMERICALLY: `0.0` and `0` pin identically,
        // and rejecting one of them for its spelling is a false alarm.
        .filter(|(k, pin)| match (std::env::var(k), pin) {
            (Ok(v), Some(p)) => match (v.trim().parse::<f64>(), p.parse::<f64>()) {
                (Ok(got), Ok(want)) => got != want,
                _ => v.trim() != *p,
            },
            (Ok(_), None) => true,
            (Err(_), _) => false,
        })
        .map(|(k, _)| *k)
        .collect();
    anyhow::ensure!(
        set.is_empty(),
        "CPU-ONLY arc lever(s) set while running the GPU lane: {}. \
         The kernel compiles its constants in and never reads the environment, so this \
         would run the CPU under one rule and the GPU under another — every number out \
         of such a run is a comparison of two different kernels. Move the GPU with \
         NOISE_GPU_DEFINES=\"-DARC_...=value\", unset these, or — to compare the \
         lanes — set them to the value that pins the CPU to the kernel's rule \
         (QM_SEG_SAMPLES=1, QM_ARC_MIN_SPAN_DEG=0).",
        set.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod streaming_abi_tests {
    use super::*;

    #[test]
    fn abi_tail_offsets_are_exact_for_0_1_2_17_barriers() {
        for barrier_count in [0_usize, 1, 2, 17] {
            let rows = barrier_count.max(1);
            assert_eq!(
                barrier_candidate_tail_slot_offset(barrier_count),
                rows * BARRIER_STRIDE
            );
            assert_eq!(
                barrier_candidate_tail_byte_offset(barrier_count),
                rows * BARRIER_STRIDE * std::mem::size_of::<f64>()
            );
        }
    }

    #[test]
    fn h0_exact_counters_are_aligned_and_disjoint_from_candidate_tail() {
        assert_eq!(OUT_H0_COUNTER_BYTE_OFFSET % std::mem::size_of::<u64>(), 0);
        assert!(OUT_H0_COUNTER_BYTE_OFFSET >= OUT_SLOTS_PROD * std::mem::size_of::<f32>());
        assert_eq!(OUT_H0_COUNTERS, 8);
        assert_eq!(
            OUT_SLOTS_H0 * std::mem::size_of::<f32>(),
            OUT_H0_COUNTER_BYTE_OFFSET + OUT_H0_COUNTERS * std::mem::size_of::<u64>()
        );
        assert_eq!(SURFACE_META_ABI_VERSION, 2);
        assert_eq!(SURFACE_META_SLOTS, SURFACE_META_LAYER_SLOT + 1);
        for barrier_count in [0_usize, 1, 2, 17] {
            assert_eq!(
                barrier_candidate_tail_byte_offset(barrier_count),
                barrier_candidate_tail_slot_offset(barrier_count) * std::mem::size_of::<f64>()
            );
        }
    }

    #[test]
    fn source_stride_five_preserves_the_first_four_coordinate_lanes() {
        let row = |start_lat, start_lon, end_lat, end_lon| LineRow {
            start_lat,
            start_lon,
            end_lat,
            end_lon,
            length_m: 180.0,
            max_distance_m: 10_000.0,
            source_height_m: 0.5,
            bridge: false,
            emission_lin: [[0.0; noise_compute::types::NUM_BANDS]; 3],
        };
        let coordinates = [
            [-0.0, 179.9995, 0.0, -179.9995],
            [90.0, 0.0, 90.0, 0.0],
            [80.0, 179.999, 80.001, -179.999],
        ];
        let rows: Vec<_> = coordinates
            .iter()
            .map(|c| row(c[0], c[1], c[2], c[3]))
            .collect();
        let packed = pack_sources(&rows);
        assert_eq!(packed.seg.len(), rows.len() * SOURCE_SEGMENT_STRIDE);
        for (expected, actual) in coordinates
            .iter()
            .zip(packed.seg.chunks_exact(SOURCE_SEGMENT_STRIDE))
        {
            let actual_coordinate_bits: Vec<_> =
                actual[..4].iter().map(|value| value.to_bits()).collect();
            assert_eq!(
                actual_coordinate_bits.as_slice(),
                &expected.map(f64::to_bits),
                "coordinate bits changed during packing"
            );
            assert_eq!(
                actual[4].to_bits(),
                source_frame_mlon(expected[0], expected[2])
                    .unwrap()
                    .to_bits()
            );
        }
    }

    #[test]
    fn barrier_stride_seven_preserves_the_first_six_numeric_lanes() {
        let barrier = Barrier {
            osm_id: 42,
            segment_idx: -7,
            height_m: 3.5,
            start_lat: 50.0,
            start_lon: 14.0,
            end_lat: 50.001,
            end_lon: 14.002,
            dist_m: 123.0,
        };
        let packed = pack_barrier_rows(&[barrier]);
        assert_eq!(packed.len(), BARRIER_STRIDE);
        assert_eq!(&packed[..6], &[50.0, 14.0, 50.001, 14.002, 3.5, 123.0]);
        assert_eq!(
            packed[6].to_bits(),
            SourceId64::wall(42, -7).unwrap().bits()
        );
        assert_eq!(pack_barrier_rows(&[]).len(), BARRIER_STRIDE);
    }
}

#[cfg(test)]
mod cpu_only_lever_tests {
    /// The guard must refuse a fork and ALLOW the pinning value — banning the
    /// pinning value bans the documented lane-comparison procedure itself, which
    /// is how this guard made `e2-full` unrunnable as specified (2026-08-08).
    ///
    /// One test, run serially inside itself: the environment is process-global and
    /// the harness is threaded, so splitting these into separate `#[test]` fns
    /// would let them race each other's `set_var`.
    #[test]
    fn pinning_values_pass_and_every_other_value_fails() {
        let restore = |k: &str| std::env::var(k).ok();
        let (prev_seg, prev_span) = (restore("QM_SEG_SAMPLES"), restore("QM_ARC_MIN_SPAN_DEG"));
        // SAFETY: single-threaded within this test; every other test in the crate
        // is pure arithmetic over local buffers and reads no environment.
        unsafe {
            std::env::remove_var("QM_SEG_SAMPLES");
            std::env::remove_var("QM_ARC_MIN_SPAN_DEG");
            assert!(
                super::ensure_no_cpu_only_arc_levers().is_ok(),
                "unset must pass"
            );

            std::env::set_var("QM_SEG_SAMPLES", "1");
            assert!(
                super::ensure_no_cpu_only_arc_levers().is_ok(),
                "QM_SEG_SAMPLES=1 pins the CPU to the kernel's single cp ray"
            );
            std::env::set_var("QM_SEG_SAMPLES", " 1 ");
            assert!(
                super::ensure_no_cpu_only_arc_levers().is_ok(),
                "whitespace around the pinning value must not fork the lanes"
            );

            std::env::set_var("QM_SEG_SAMPLES", "5");
            let err = super::ensure_no_cpu_only_arc_levers()
                .unwrap_err()
                .to_string();
            assert!(err.contains("QM_SEG_SAMPLES"), "N=5 forks the lanes: {err}");
            assert!(
                err.contains("QM_SEG_SAMPLES=1"),
                "error must name the way out"
            );

            std::env::remove_var("QM_SEG_SAMPLES");
            std::env::set_var("QM_ARC_MIN_SPAN_DEG", "0");
            assert!(
                super::ensure_no_cpu_only_arc_levers().is_ok(),
                "0 is the value build.rs injects into the kernel; unset is the FORK \
                 (the CPU gate rises to 3°), which is why only the explicit 0 passes"
            );
            std::env::set_var("QM_ARC_MIN_SPAN_DEG", "1000000");
            assert!(
                super::ensure_no_cpu_only_arc_levers().is_err(),
                "arc off on CPU only"
            );

            std::env::remove_var("QM_ARC_MIN_SPAN_DEG");
            // A lever with no pinning value is refused whatever it says.
            std::env::set_var("QM_ARC_EXACT", "0");
            assert!(
                super::ensure_no_cpu_only_arc_levers().is_err(),
                "no pinning value"
            );
            std::env::remove_var("QM_ARC_EXACT");

            for (k, v) in [
                ("QM_SEG_SAMPLES", prev_seg),
                ("QM_ARC_MIN_SPAN_DEG", prev_span),
            ] {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

#[cfg(test)]
mod obstacle_flat_tests {
    /// Offsets must tile the shared arrays exactly — a mis-offset walks a
    /// neighbouring cell-index's edges (silent physics corruption).
    #[test]
    fn obstacle_source_ids_match_flattened_gpu_ordinals_across_indexes() {
        use noise_compute::propagation::obstacle_index::{
            ObstacleIndex, ObstacleKind, ObstacleSet,
        };
        let mut sets = Vec::new();
        for (olat, olon) in [(50.0, 14.0), (50.5, 14.5)] {
            let mut b = ObstacleIndex::builder(olat, olon);
            let ring: Vec<(f64, f64)> = [(0.0, 0.0), (0.001, 0.0), (0.001, 0.001), (0.0, 0.001)]
                .iter()
                .map(|(dlat, dlon)| (olat + dlat, olon + dlon))
                .collect();
            b.add_ring(&ring, 10.0, ObstacleKind::Building, 0);
            sets.push(std::sync::Arc::new(b.build()));
        }
        let set = ObstacleSet { indexes: sets };
        let flat = super::flatten_obstacles(&set);
        let mut source_ids = std::collections::BTreeSet::new();
        set.skyline_arcs_within(50.25, 14.25, 0.0, 100_000.0, 0.0, 0.0, None, &mut |arc| {
            source_ids.insert(arc.source_id.bits());
        });
        assert_eq!(
            source_ids,
            (0..8).collect(),
            "CPU IDs equal eoff + local edge ref in the flattened GPU store"
        );
        assert_eq!(flat.n_indexes, 2);
        assert_eq!(flat.metas.len(), 2 * super::META_STRIDE);
        assert_eq!(
            source_ids.len(),
            flat.edges.len() / 5,
            "one CPU source ID per edge in the flattened GPU store"
        );
        assert_eq!(flat.metas[10] as usize, 0, "first GPU edge offset");
        // Second index's offsets start exactly where the first index ends,
        // and its extents tile the shared arrays completely.
        let (starts_off2, refs_off2, edges_off2, n_cells2) = (
            flat.metas[super::META_STRIDE + 8] as usize,
            flat.metas[super::META_STRIDE + 9] as usize,
            flat.metas[super::META_STRIDE + 10] as usize,
            flat.metas[super::META_STRIDE + 11] as usize,
        );
        assert_eq!(flat.metas[11] as usize, starts_off2, "starts contiguous");
        assert_eq!(starts_off2 + n_cells2, flat.starts.len(), "starts tiled");
        assert_eq!(edges_off2, 4, "first ring = 4 edges");
        assert_eq!((edges_off2 + 4) * 5, flat.edges.len(), "edges tiled");
        // per-index refs extent = its last cell_start (CSR total)
        assert_eq!(
            refs_off2 + *flat.starts.last().unwrap() as usize,
            flat.refs.len(),
            "refs tiled"
        );
        assert!(flat.edges.len().is_multiple_of(5) && !flat.refs.is_empty());
        // Arc-screening hull grouping (fix-pack Fix 1): every edge carries its
        // footprint id, recovered from the endpoint bits — one ring per index
        // here, so id 0 across the board and never the high-bit "unmatched"
        // sentinel that would silently split a hull.
        assert_eq!(flat.edge_ids.len(), flat.edges.len() / 5, "one id per edge");
        assert!(
            flat.edge_ids.iter().all(|&i| i == 0),
            "single ring per index ⇒ id 0, got {:?}",
            flat.edge_ids
        );
        // E2 per-cell max heights: offsets (slot 12) tile the shared array
        // exactly like starts/refs, and every value equals the true max over
        // that cell's edge heights (here every edge is the same 10 m ring, so
        // occupied cells carry exactly 10.0 and empty cells 0.0).
        let (maxh_off1, maxh_off2) = (
            flat.metas[12] as usize,
            flat.metas[super::META_STRIDE + 12] as usize,
        );
        assert_eq!(maxh_off1, 0, "first index's maxh starts the array");
        // metas[11] is n_starts (= cells + 1, CSR); maxh carries one entry
        // per CELL, so the second offset is the first index's cell count.
        let n_cells1 = flat.metas[11] as usize - 1;
        assert_eq!(maxh_off2, n_cells1, "maxh contiguous across indexes");
        assert_eq!(
            maxh_off2 + (n_cells2 - 1),
            flat.cell_max_h.len(),
            "maxh tiled"
        );
        for (c, &mx) in flat.cell_max_h.iter().enumerate() {
            let (idx_off, cell) = if c < n_cells1 {
                (0, c)
            } else {
                (1, c - n_cells1)
            };
            let (lo, hi) = {
                let so = flat.metas[idx_off * super::META_STRIDE + 8] as usize;
                (
                    flat.starts[so + cell] as usize,
                    flat.starts[so + cell + 1] as usize,
                )
            };
            assert_eq!(
                mx,
                if hi > lo { 10.0 } else { 0.0 },
                "cell {c}: maxh equals the max edge height"
            );
        }
    }
}

// ---- Vector-obstacle GPU upload (geodata-v2 1.6) — shared by gpu-surface
// and the e2-full validator, so it lives here behind the gpu feature.
#[cfg(feature = "gpu")]
mod obstacle_upload {
    use anyhow::{Context, Result};
    use cudarc::driver::{CudaDevice, CudaSlice};
    use std::sync::Arc;

    /// The region's vector obstacles resident on the GPU. `table` is the 14-slot
    /// pointer table the kernel dereferences — `0` n_indexes, `1` metas, `2`
    /// starts, `3` refs, `4` edges, `5` cell_max_h, `6` edge_ids, then the
    /// footprint-CSR half: `7` foot_edge_starts, `8` foot_box, `9`
    /// cell_foot_starts, `10` cell_foot_ids, `11` cell_foot_prev, `12`
    /// foot_edge_refs, `13` cell_foot_cell; the `_`-prefixed slices (and
    /// `_keep`) only exist to keep those device allocations alive for as long as
    /// the table can be launched with. Raster mode = an all-zero 14-slot table
    /// (the kernel reads slots 1..=13 only when slot 0 is non-zero).
    pub struct ObstDev {
        pub table: CudaSlice<u64>,
        _metas: Option<CudaSlice<f64>>,
        _starts: Option<CudaSlice<u32>>,
        _refs: Option<CudaSlice<u32>>,
        _edges: Option<CudaSlice<f32>>,
        _maxh: Option<CudaSlice<f32>>,
        _ids: Option<CudaSlice<u32>>,
        _keep: Vec<Box<dyn std::any::Any + Send + Sync>>,
    }

    pub fn upload_obstacles(
        dev: &Arc<CudaDevice>,
        set: Option<&noise_compute::propagation::obstacle_index::ObstacleSet>,
    ) -> Result<ObstDev> {
        let Some(set) = set else {
            // Raster mode: the kernel reads slots 1..=13 only when slot 0 is
            // non-zero, but the table must still be long enough that a slot
            // read is never out of bounds.
            return Ok(ObstDev {
                table: dev.htod_copy(vec![0u64; 14]).context("obst off-table")?,
                _metas: None,
                _starts: None,
                _refs: None,
                _edges: None,
                _maxh: None,
                _ids: None,
                _keep: Vec::new(),
            });
        };
        let flat = crate::flatten_obstacles(set);
        upload_obstacle_flat(dev, flat)
    }

    /// Upload one already flattened store. The H0 diagnostic first walks these
    /// exact host bytes for its CPU authority, then moves the same vectors to
    /// CUDA; production uses [`upload_obstacles`] and remains unchanged.
    pub fn upload_obstacle_flat(
        dev: &Arc<CudaDevice>,
        flat: crate::ObstacleFlat,
    ) -> Result<ObstDev> {
        use cudarc::driver::DevicePtr;
        let metas = dev.htod_copy(flat.metas).context("obst metas")?;
        let starts = dev.htod_copy(flat.starts).context("obst starts")?;
        let refs = dev.htod_copy(flat.refs).context("obst refs")?;
        let edges = dev.htod_copy(flat.edges).context("obst edges")?;
        let maxh = dev.htod_copy(flat.cell_max_h).context("obst maxh")?;
        let ids = dev.htod_copy(flat.edge_ids).context("obst edge ids")?;
        // NOISE_GPU_DISABLE_PRUNE=1: publish a NULL maxh pointer — the kernel
        // then walks every non-empty cell's edge list (pre-E2 behavior), an
        // incident A/B lever that needs no rebuild. Output is identical
        // either way (the prune is exact); only the cost differs.
        let prune_disabled = std::env::var("NOISE_GPU_DISABLE_PRUNE").is_ok_and(|v| v == "1");
        // Footprint-CSR (slots 7..=13) — the arc-screening walk's own view of
        // the same edges. Uploaded unconditionally: it costs ~20 MB on the
        // densest region measured (0.97 M polygon points) and the kernel picks
        // the walk by `#if ARC_FOOTPRINT_CSR`, not by a null pointer, so both
        // walks stay A/B-able from one binary set.
        let fes = dev
            .htod_copy(flat.foot_edge_starts)
            .context("obst foot edge starts")?;
        let fer = dev
            .htod_copy(flat.foot_edge_refs)
            .context("obst foot edge refs")?;
        let foot_box = dev.htod_copy(flat.foot_box).context("obst foot box")?;
        let cfs = dev
            .htod_copy(flat.cell_foot_starts)
            .context("obst cell foot starts")?;
        let cfi = dev
            .htod_copy(flat.cell_foot_ids)
            .context("obst cell foot ids")?;
        let cfp = dev
            .htod_copy(flat.cell_foot_prev)
            .context("obst cell foot prev")?;
        let cfc = dev
            .htod_copy(flat.cell_foot_cell)
            .context("obst cell foot cell")?;
        let table = dev
            .htod_copy(vec![
                flat.n_indexes as u64,
                *metas.device_ptr(),
                *starts.device_ptr(),
                *refs.device_ptr(),
                *edges.device_ptr(),
                if prune_disabled {
                    0
                } else {
                    *maxh.device_ptr()
                },
                *ids.device_ptr(),
                *fes.device_ptr(),
                *foot_box.device_ptr(),
                *cfs.device_ptr(),
                *cfi.device_ptr(),
                *cfp.device_ptr(),
                *fer.device_ptr(),
                *cfc.device_ptr(),
            ])
            .context("obst table")?;
        Ok(ObstDev {
            table,
            _metas: Some(metas),
            _starts: Some(starts),
            _refs: Some(refs),
            _edges: Some(edges),
            _maxh: Some(maxh),
            _ids: Some(ids),
            _keep: vec![
                Box::new(fes),
                Box::new(fer),
                Box::new(foot_box),
                Box::new(cfs),
                Box::new(cfi),
                Box::new(cfp),
                Box::new(cfc),
            ],
        })
    }
}
#[cfg(feature = "gpu")]
pub use obstacle_upload::{upload_obstacle_flat, upload_obstacles, ObstDev};

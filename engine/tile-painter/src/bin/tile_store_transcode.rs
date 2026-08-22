//! tile-store-transcode — ingest ONE layer's loose `{z}/{x}/{y}.bin` tree into
//! the [`tile_store`] container, then prove parity before anyone deletes the
//! tree. Born as the 2026-07 migration tool; stays as the SINGLE-HOST glue
//! between a kernel's loose output and the store (build-heatmap.sh) — the
//! fleet's equivalent is `tile-store-ingest` driven by the hub.
//!
//! Pure blob copy: every loose tile is already a whole-file-Brotli HM3 image,
//! so it is stored VERBATIM ([`TileCodec::BrotliHm3`]) at every zoom — no
//! re-encode, no decoded-value drift possible. Combine and pyramid rewrites
//! also emit Brotli HM3; zstd remains legacy-read compatibility only.
//!
//! Everything is derived, no tuning flags (project rule): source zoom levels
//! from the layer dir listing, `source_id` from tile headers. The complete
//! source tree is decoded and cross-zoom-validated before destination mutation;
//! production requires base z12 and finishes with exactly z2..z12 sharing one
//! source id and tile size.
//!
//! Parity uses three independent gates in one run:
//!   1. count — store scan total == loose census total (catches extras)
//!   2. exhaustive — EVERY census tile byte-compared loose vs store. This is
//!      the deletion-grade proof; a sampled gate would let unsampled
//!      corruption exit green.
//!   3. reference — every tile in the Dobříš + Ruzyně H3 R4 cells compared by
//!      decoded cells at every zoom (semantic smoke test on the project anchors)
//!
//! PRECONDITION: the loose input tree itself is QUIESCENT — the gates prove
//! equality against the census taken at start, not against a moving input.
//! Destination writers require no operator shutdown: this process takes the
//! bounded master→ingest pair for its entire multi-zoom replacement and full
//! pyramid. A durable `.rebuild-incomplete` fence survives any failed mutation
//! and blocks pack until this same path succeeds. On success the store is
//! fsynced (files + dir) BEFORE the green exit: exit 0 is
//! the license to delete the loose tree, so it must mean durable-on-disk, not
//! page-cache.
//!
//! Usage: tile-store-transcode <loose-layer-dir> <store-layer-dir>
//!   e.g.  tile-store-transcode data/tiles/2026/build/road \
//!                              data/tiles/2026/store/road

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use tile_painter::grid::{tile_range, TILE_PX};
use tile_painter::pyramid::{build_pyramid_with_existing_rebuild_fence, RebuildScope};
use tile_painter::tile_store::{
    detect_zooms as detect_store_zooms, expected_source_id, LooseTree, StoreMasterIngestLocks,
    StoreRebuildFence, TileCodec, TileStore, PUBLISHED_BASE_ZOOM, PUBLISHED_MIN_ZOOM,
};
use tile_painter::wire_hm3::read_tile_bytes_source_id;

/// Project reference cells (CLAUDE.md): Dobříš + Ruzyně H3 R4. Every tile they
/// cover is decoded-compared — the same anchors every parity check uses.
const REFERENCE_CELLS: [&str; 2] = ["841e309ffffffff", "841e355ffffffff"];
const STORE_LOCK_WAIT: Duration = Duration::from_secs(300);

struct SourceZoomPlan {
    zoom: u8,
    coords: Vec<(u32, u32)>,
    source_id: u8,
}

struct SourcePlan {
    zooms: Vec<SourceZoomPlan>,
    source_id: u8,
    base_zoom: u8,
}

fn acquire_transcode_writer_locks(
    store_dir: &Path,
    timeout: Duration,
) -> Result<StoreMasterIngestLocks> {
    let store_root = store_dir
        .parent()
        .with_context(|| format!("store layer {} has no store root", store_dir.display()))?;
    StoreMasterIngestLocks::acquire_bounded(store_root, timeout)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(loose_dir), Some(store_dir), None) = (args.next(), args.next(), args.next()) else {
        bail!("usage: tile-store-transcode <loose-layer-dir> <store-layer-dir>");
    };
    let loose_dir = PathBuf::from(loose_dir);
    let store_dir = PathBuf::from(store_dir);

    transcode_layer(&loose_dir, &store_dir, STORE_LOCK_WAIT)
}

fn transcode_layer(loose_dir: &Path, store_dir: &Path, timeout: Duration) -> Result<()> {
    transcode_layer_for_base(loose_dir, store_dir, timeout, PUBLISHED_BASE_ZOOM)
}

fn transcode_layer_for_base(
    loose_dir: &Path,
    store_dir: &Path,
    timeout: Duration,
    expected_base_zoom: u8,
) -> Result<()> {
    // Validate every source zoom and blob before the first destructive create. The loose source
    // is required to be quiescent; the write pass still revalidates so violating that precondition
    // leaves a durable marker instead of blessing a moving tree.
    let plan = preflight_source(loose_dir, expected_base_zoom)?;
    let layer = store_dir
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("store layer path {} has no UTF-8 name", store_dir.display()))?;
    let required_source_id =
        expected_source_id(layer).with_context(|| format!("{layer}: unknown store layer"))?;
    if plan.source_id != required_source_id {
        bail!(
            "{layer}: source_id {} differs from required {required_source_id}",
            plan.source_id
        );
    }
    execute_source_plan(loose_dir, store_dir, timeout, plan)
}

fn execute_source_plan(
    loose_dir: &Path,
    store_dir: &Path,
    timeout: Duration,
    plan: SourcePlan,
) -> Result<()> {
    eprintln!(
        "transcode {} → {} — source zooms {:?}, final z{}..z{}",
        loose_dir.display(),
        store_dir.display(),
        plan.zooms.iter().map(|zoom| zoom.zoom).collect::<Vec<_>>(),
        PUBLISHED_MIN_ZOOM,
        plan.base_zoom
    );

    // One transaction owns both outer writer domains from the first live truncate through the
    // last pyramid fsync and marker removal. There is no publishable new-base/old-pyramid gap.
    let _writer_locks = acquire_transcode_writer_locks(store_dir, timeout)?;
    let marker = StoreRebuildFence::begin(store_dir, "transcode")?;

    let (mut total_tiles, mut total_bytes) = (0u64, 0u64);
    for zoom in &plan.zooms {
        let (tiles, bytes) = transcode_zoom(loose_dir, store_dir, zoom)?;
        total_tiles += tiles;
        total_bytes += bytes;
    }
    let pyramid_tiles = build_pyramid_with_existing_rebuild_fence(
        store_dir,
        plan.base_zoom,
        PUBLISHED_MIN_ZOOM,
        RebuildScope::Full,
    )?;
    remove_store_levels_outside(store_dir, PUBLISHED_MIN_ZOOM, plan.base_zoom)?;
    validate_complete_layer(store_dir, plan.base_zoom, plan.source_id)?;
    File::open(store_dir)?.sync_all()?;
    marker.finish()?;
    eprintln!(
        "DONE: {total_tiles} source tiles + {pyramid_tiles} pyramid tiles, {:.1} GiB source, all parity gates green",
        total_bytes as f64 / (1 << 30) as f64,
    );
    Ok(())
}

fn preflight_source(loose_dir: &Path, expected_base_zoom: u8) -> Result<SourcePlan> {
    let zooms = detect_zooms(loose_dir)?;
    if zooms.is_empty() {
        bail!("no numeric zoom dirs under {}", loose_dir.display());
    }
    if zooms[0] < PUBLISHED_MIN_ZOOM {
        bail!(
            "source carries z{}, below published floor z{}",
            zooms[0],
            PUBLISHED_MIN_ZOOM
        );
    }
    let base_zoom = *zooms.last().expect("non-empty");
    if base_zoom != expected_base_zoom {
        bail!("source base z{base_zoom} differs from required z{expected_base_zoom}");
    }

    let mut plans = Vec::with_capacity(zooms.len());
    let mut layer_source_id = None;
    for zoom in zooms {
        let tree = LooseTree::new(loose_dir, zoom);
        let mut coords = Vec::new();
        tree.for_each_present(|x, y| {
            coords.push((x, y));
            Ok(())
        })?;
        coords.sort_unstable();
        let &(first_x, first_y) = coords
            .first()
            .with_context(|| format!("z{zoom}: source zoom is empty"))?;
        let first = tree
            .get_blob(first_x, first_y)?
            .context("censused first tile disappeared during preflight")?;
        let source_id = read_tile_bytes_source_id(&first)
            .with_context(|| format!("z{zoom}/{first_x}/{first_y}: invalid first HM3 tile"))?;
        if let Some(expected) = layer_source_id {
            if source_id != expected {
                bail!("z{zoom}: source_id {source_id} ≠ layer's {expected}");
            }
        } else {
            layer_source_id = Some(source_id);
        }
        coords.par_iter().try_for_each(|&(x, y)| -> Result<()> {
            let blob = tree
                .get_blob(x, y)?
                .with_context(|| format!("z{zoom}/{x}/{y} vanished during preflight"))?;
            let sid = read_tile_bytes_source_id(&blob)
                .with_context(|| format!("z{zoom}/{x}/{y}: invalid HM3 tile"))?;
            if sid != source_id {
                bail!("z{zoom}/{x}/{y}: source_id {sid} ≠ zoom {source_id}");
            }
            Ok(())
        })?;
        plans.push(SourceZoomPlan {
            zoom,
            coords,
            source_id,
        });
    }
    Ok(SourcePlan {
        zooms: plans,
        source_id: layer_source_id.expect("non-empty source plans"),
        base_zoom,
    })
}

/// Numeric subdirs of the layer dir = the zoom levels it carries.
fn detect_zooms(loose_dir: &Path) -> Result<Vec<u8>> {
    let mut zooms: Vec<u8> = std::fs::read_dir(loose_dir)
        .with_context(|| format!("read {}", loose_dir.display()))?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    zooms.sort_unstable();
    Ok(zooms)
}

fn transcode_zoom(loose_dir: &Path, store_dir: &Path, plan: &SourceZoomPlan) -> Result<(u64, u64)> {
    let t0 = Instant::now();
    let z = plan.zoom;
    let tree = LooseTree::new(loose_dir, z);
    let store = TileStore::create(store_dir, z, plan.source_id, TILE_PX as u16)?;
    let bytes = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    plan.coords
        .par_iter()
        .try_for_each(|&(x, y)| -> Result<()> {
            let blob = tree
                .get_blob(x, y)?
                .with_context(|| format!("z{z}/{x}/{y} vanished mid-transcode"))?;
            // Validate EVERY blob (decode + magic/version/size + layer id) — the
            // store ships BrotliHm3 verbatim into pmtiles, so a stale/corrupt
            // loose tile must be rejected here, not published.
            let sid = read_tile_bytes_source_id(&blob)
                .with_context(|| format!("z{z}/{x}/{y}: invalid HM3 tile"))?;
            if sid != plan.source_id {
                bail!(
                    "z{z}/{x}/{y}: source_id {sid} ≠ preflight {}",
                    plan.source_id
                );
            }
            store.put_blob(x, y, TileCodec::BrotliHm3, &blob)?;
            bytes.fetch_add(blob.len() as u64, Ordering::Relaxed);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(1_000_000) {
                eprintln!("z{z}: {n}/{} …", plan.coords.len());
            }
            Ok(())
        })?;

    // Durability BEFORE the gates: a green exit licenses deletion, so what the
    // gates verified must be what's on disk (files + their dir entries).
    store.sync_all()?;
    std::fs::File::open(store_dir)?.sync_all()?;

    verify_zoom(&tree, &store, &plan.coords, z)?;
    eprintln!(
        "z{z}: {} tiles, {:.2} GiB in {:.0?} — parity OK (exhaustive)",
        plan.coords.len(),
        bytes.load(Ordering::Relaxed) as f64 / (1 << 30) as f64,
        t0.elapsed()
    );
    Ok((plan.coords.len() as u64, bytes.load(Ordering::Relaxed)))
}

fn remove_store_levels_outside(layer_dir: &Path, min_zoom: u8, max_zoom: u8) -> Result<()> {
    for entry in fs::read_dir(layer_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let zoom = [".qtsi", ".qtsd"].into_iter().find_map(|suffix| {
            name.strip_prefix('z')?
                .strip_suffix(suffix)?
                .parse::<u8>()
                .ok()
        });
        if zoom.is_some_and(|zoom| zoom < min_zoom || zoom > max_zoom) {
            fs::remove_file(entry.path())?;
            eprintln!("removed stale store level {}", entry.path().display());
        }
    }
    Ok(())
}

fn validate_complete_layer(layer_dir: &Path, base_zoom: u8, expected_source_id: u8) -> Result<()> {
    let actual = detect_store_zooms(layer_dir)?;
    let expected: Vec<u8> = (PUBLISHED_MIN_ZOOM..=base_zoom).collect();
    if actual != expected {
        bail!(
            "{}: final zoom set {:?} ≠ expected {:?}",
            layer_dir.display(),
            actual,
            expected
        );
    }
    for zoom in expected {
        let store = TileStore::open(layer_dir, zoom, false)?;
        if store.source_id() != expected_source_id {
            bail!(
                "z{zoom}: final source_id {} ≠ {expected_source_id}",
                store.source_id()
            );
        }
        if store.tile_px() as usize != TILE_PX {
            bail!("z{zoom}: final tile_px {} ≠ {TILE_PX}", store.tile_px());
        }
    }
    Ok(())
}

/// The three parity gates for one zoom. Any failure aborts the whole run —
/// nothing may delete loose trees unless this binary exited 0.
fn verify_zoom(tree: &LooseTree, store: &TileStore, coords: &[(u32, u32)], z: u8) -> Result<()> {
    // Gate 1: exact presence count (store scan vs loose walk).
    let mut store_count = 0u64;
    store.for_each_present(|_, _, _| {
        store_count += 1;
        Ok(())
    })?;
    if store_count != coords.len() as u64 {
        bail!(
            "z{z}: store has {store_count} tiles, loose has {}",
            coords.len()
        );
    }

    // Gate 2: EXHAUSTIVE byte-for-byte over the whole census — the
    // deletion-grade proof (with gate 1 this is exact set equality).
    coords.par_iter().try_for_each(|&(x, y)| -> Result<()> {
        let loose_blob = tree
            .get_blob(x, y)?
            .with_context(|| format!("z{z}/{x}/{y} vanished during verify"))?;
        let (codec, store_blob) = store
            .get_blob(x, y)?
            .with_context(|| format!("z{z}/{x}/{y} missing from store"))?;
        if codec != TileCodec::BrotliHm3 || store_blob != loose_blob {
            bail!("z{z}/{x}/{y}: stored bytes differ from loose tile");
        }
        Ok(())
    })?;

    // Gate 3: reference cells, decoded-cell equality over the full tile range.
    for cell_str in REFERENCE_CELLS {
        let cell: h3o::CellIndex = cell_str.parse().context("reference cell")?;
        let (mut s, mut w, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
        for ll in cell.boundary().iter() {
            s = s.min(ll.lat());
            n = n.max(ll.lat());
            w = w.min(ll.lng());
            e = e.max(ll.lng());
        }
        let (xr, yr) = tile_range(z, s, w, n, e);
        for x in xr {
            for y in yr.clone() {
                let a = tree.get_cells(x, y)?;
                let b = store.get_cells(x, y)?;
                if a != b {
                    bail!("z{z}/{x}/{y}: reference-cell decode mismatch ({cell_str})");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tile_painter::tile_store::REBUILD_INCOMPLETE_MARKER;
    use tile_painter::tile_store::{ingest_store_lock_path, master_store_lock_path, StoreFileLock};
    use tile_painter::wire_hm3::{write_tile, NO_DATA, SOURCE_ID_RAIL, SOURCE_ID_ROAD};

    fn cells(value: u8) -> Vec<u8> {
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[0] = value;
        cells
    }

    fn write_loose(
        loose_dir: &Path,
        zoom: u8,
        x: u32,
        y: u32,
        source_id: u8,
        value: u8,
    ) -> Result<PathBuf> {
        let path = loose_dir.join(format!("{zoom}/{x}/{y}.bin"));
        write_tile(&path, &cells(value), source_id, false)?;
        Ok(path)
    }

    fn test_store_dir(root: &Path) -> PathBuf {
        root.join("tiles/2026/store/road")
    }

    #[test]
    fn transcode_excludes_both_writer_domains_for_its_whole_run() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store_root = dir.path().join("tiles/2026/store");
        let store_dir = store_root.join("road");
        let master_path = master_store_lock_path(&store_root)?;
        let ingest_path = ingest_store_lock_path(&store_root);

        let transcode = acquire_transcode_writer_locks(&store_dir, Duration::ZERO)?;
        assert!(StoreFileLock::acquire_bounded(&master_path, Duration::ZERO).is_err());
        assert!(StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO).is_err());
        drop(transcode);

        StoreFileLock::acquire_bounded(&master_path, Duration::ZERO)?;
        StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO)?;
        Ok(())
    }

    #[test]
    fn preflight_rejects_cross_zoom_layer_mix_before_destination_mutation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let loose_dir = dir.path().join("loose/road");
        let store_dir = test_store_dir(dir.path());
        write_loose(&loose_dir, 5, 5, 5, SOURCE_ID_ROAD, 40)?;
        write_loose(&loose_dir, 6, 10, 10, SOURCE_ID_RAIL, 50)?;

        let existing = TileStore::create(&store_dir, 6, SOURCE_ID_ROAD, TILE_PX as u16)?;
        existing.put_cells_hm3(1, 1, &cells(77))?;
        existing.sync_all()?;
        drop(existing);

        let error =
            transcode_layer_for_base(&loose_dir, &store_dir, Duration::ZERO, 6).unwrap_err();
        assert!(
            error.to_string().contains("source_id"),
            "unexpected error: {error:#}"
        );
        assert!(!store_dir.join(REBUILD_INCOMPLETE_MARKER).exists());
        let unchanged = TileStore::open(&store_dir, 6, false)?
            .get_cells(1, 1)?
            .context("pre-existing destination tile disappeared")?;
        assert_eq!(unchanged[0], 77);
        Ok(())
    }

    #[test]
    fn transcode_rejects_wrong_layer_source_id_before_destination_mutation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let loose_dir = dir.path().join("loose/rail");
        let store_dir = test_store_dir(dir.path());
        write_loose(&loose_dir, 6, 10, 10, SOURCE_ID_RAIL, 50)?;

        let error =
            transcode_layer_for_base(&loose_dir, &store_dir, Duration::ZERO, 6).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("road: source_id 2 differs from required 1"),
            "unexpected error: {error:#}"
        );
        assert!(
            !store_dir.exists(),
            "a rejected transcode must not create the destination"
        );
        Ok(())
    }

    #[test]
    fn full_transaction_finishes_with_exact_zoom_band_and_no_fence() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let loose_dir = dir.path().join("loose/road");
        let store_dir = test_store_dir(dir.path());
        write_loose(&loose_dir, 6, 32, 32, SOURCE_ID_ROAD, 60)?;

        TileStore::create(&store_dir, 1, SOURCE_ID_ROAD, TILE_PX as u16)?.sync_all()?;
        TileStore::create(&store_dir, 7, SOURCE_ID_ROAD, TILE_PX as u16)?.sync_all()?;
        transcode_layer_for_base(&loose_dir, &store_dir, Duration::from_secs(1), 6)?;

        assert_eq!(
            detect_store_zooms(&store_dir)?,
            (PUBLISHED_MIN_ZOOM..=6).collect::<Vec<_>>()
        );
        assert!(!store_dir.join(REBUILD_INCOMPLETE_MARKER).exists());
        assert!(!store_dir.join("z1.qtsi").exists());
        assert!(!store_dir.join("z7.qtsi").exists());
        validate_complete_layer(&store_dir, 6, SOURCE_ID_ROAD)?;
        Ok(())
    }

    #[test]
    fn failure_after_mutation_leaves_fence_until_a_successful_retry() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let loose_dir = dir.path().join("loose/road");
        let store_dir = test_store_dir(dir.path());
        write_loose(&loose_dir, 6, 32, 32, SOURCE_ID_ROAD, 60)?;
        let second = write_loose(&loose_dir, 6, 33, 32, SOURCE_ID_ROAD, 61)?;
        let plan = preflight_source(&loose_dir, 6)?;

        fs::write(&second, b"corrupt after preflight")?;
        assert!(execute_source_plan(&loose_dir, &store_dir, Duration::from_secs(1), plan).is_err());
        assert!(store_dir.join(REBUILD_INCOMPLETE_MARKER).exists());

        write_tile(&second, &cells(61), SOURCE_ID_ROAD, false)?;
        transcode_layer_for_base(&loose_dir, &store_dir, Duration::from_secs(1), 6)?;
        assert!(!store_dir.join(REBUILD_INCOMPLETE_MARKER).exists());
        validate_complete_layer(&store_dir, 6, SOURCE_ID_ROAD)?;
        Ok(())
    }
}

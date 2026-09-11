//! tile-store-fsck — health check and explicit unused-block reclamation for `(layer, zoom)`
//! tile stores: decode-validates every present entry, and flags any pair of
//! entries whose `[offset, offset+len)` byte ranges overlap in the data log.
//!
//! Non-overlap is an invariant of a correctly functioning store (`put_blob`
//! reserves offsets via an atomic tail bump — no two entries should ever
//! collide), so an overlap is corruption BY CONSTRUCTION, and decode success
//! alone cannot catch this class: a corrupt entry can be a valid Brotli
//! stream for a DIFFERENT tile plus trailing garbage, which decodes
//! "successfully" to the wrong tile's data. Found live, 2026-07-14: 88
//! duplicate-offset groups in `industrial/z12`, 43 of them this exact
//! silent-wrong-data pattern — this tool generalizes that forensic scan into
//! a standing, scriptable check.
//!
//! MANDATORY pre-publish gate since 2026-07-16: `tile-store-pack` copies exact
//! entries + open data handles under outer writer locks plus every selected
//! per-z lock, then calls the SAME [`validate_captured_store`] core used here.
//! A dirty snapshot
//! aborts before any archive is created. This is what let `tile-store-pack`'s
//! own ship-out (`TileStore::get_hm3_by_entry`) drop its per-tile decode-
//! validation of `BrotliHm3` blobs and go back to a verbatim copy: the
//! decode-validate work this tool already did per entry only needs to run
//! once before a publish, not once per read inside the packer.
//! Standalone fsck uses the same master→ingest→canonical-per-z order and retains
//! every guard through validation and reporting, so neither a normal writer nor
//! a destructive `TileStore::create` can change a pinned data-file inode while
//! it is being checked.
//!
//! Default is read-only; --reclaim frees unreachable blocks after every selected store passes.
//! Usage: tile-store-fsck <store-root> [--layer L] [--zoom N] [--reclaim]
//!        Exit 0 = clean. Exit 1 = problems found.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use tile_painter::grid::TILE_PX;
use tile_painter::tile_store::fsck::{validate_captured_store, CapturedTileRef, StoreFsckReport};
use tile_painter::tile_store::{
    detect_layers, detect_zooms, reject_incomplete_store_transactions, zoom_store_lock_path,
    StoreFileLocks, StoreMasterIngestLocks, TileStore,
};

const STORE_LOCK_WAIT: Duration = Duration::from_secs(300);

struct FsckSnapshot {
    layer: String,
    zoom: u8,
    store: TileStore,
    captured_data_len: u64,
    entries: Vec<CapturedTileRef>,
}

struct LockedFsckSnapshots {
    _outer_locks: StoreMasterIngestLocks,
    _snapshot_locks: StoreFileLocks,
    snapshots: Vec<FsckSnapshot>,
}

fn main() -> Result<()> {
    let mut positional: Vec<String> = Vec::new();
    let mut only_layer: Option<String> = None;
    let mut only_zoom: Option<u8> = None;
    let mut reclaim = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--reclaim" => reclaim = true,
            "--layer" => only_layer = Some(args.next().context("--layer needs a value")?),
            "--zoom" => {
                only_zoom = Some(
                    args.next()
                        .context("--zoom needs a value")?
                        .parse()
                        .context("--zoom must be a number")?,
                )
            }
            _ => positional.push(a),
        }
    }
    let [store_root]: [String; 1] = positional.try_into().map_err(|_| {
        anyhow::anyhow!("usage: tile-store-fsck <store-root> [--layer L] [--zoom N] [--reclaim]")
    })?;
    let store_root = Path::new(&store_root);

    let locked = capture_locked_snapshots(
        store_root,
        only_layer.as_deref(),
        only_zoom,
        STORE_LOCK_WAIT,
    )?;
    let reports: Vec<StoreFsckReport> = locked.snapshots.iter().map(validate_snapshot).collect();

    let mut problems = 0usize;
    for r in &reports {
        println!(
            "{}/z{}: {} entries, {} decode failures, {} overlap groups{}",
            r.layer,
            r.zoom,
            r.entries,
            r.decode_failures.len(),
            r.overlaps.len(),
            if r.tile_px_mismatch.is_some() {
                ", TILE_PX MISMATCH"
            } else {
                ""
            }
        );
        if let Some(actual) = r.tile_px_mismatch {
            println!(
                "  TILE_PX MISMATCH: store is {actual}px, current build expects {TILE_PX}px — \
                 any ZstdCells entry here will panic the real pmtiles packer (get_hm3_by_entry \
                 → encode_tile_bytes asserts against the GLOBAL TILE_PX, not this store's own)"
            );
        }
        for (t, err) in &r.decode_failures {
            println!(
                "  DECODE FAIL {}/{}/{} @{}: {err}",
                r.layer, t.x, t.y, t.entry.offset
            );
        }
        for (a, b) in &r.overlaps {
            println!(
                "  OVERLAP {}/{}/{} @{}+{} vs {}/{}/{} @{}+{}",
                r.layer,
                a.x,
                a.y,
                a.entry.offset,
                a.entry.len,
                r.layer,
                b.x,
                b.y,
                b.entry.offset,
                b.entry.len
            );
        }
        problems +=
            r.decode_failures.len() + r.overlaps.len() + usize::from(r.tile_px_mismatch.is_some());
    }
    println!("---");
    println!("{problems} problem(s) across {} store(s)", reports.len());
    if problems > 0 {
        std::process::exit(1);
    }
    if reclaim {
        for snapshot in &locked.snapshots {
            let bytes = snapshot
                .store
                .reclaim_unreferenced_blocks(&locked._snapshot_locks)?;
            println!(
                "{}/z{}: reclaimed {bytes} bytes",
                snapshot.layer, snapshot.zoom
            );
        }
    }
    Ok(())
}

fn capture_locked_snapshots(
    store_root: &Path,
    only_layer: Option<&str>,
    only_zoom: Option<u8>,
    timeout: Duration,
) -> Result<LockedFsckSnapshots> {
    let outer_locks = StoreMasterIngestLocks::acquire_bounded(store_root, timeout)?;
    let marker_scope = only_layer
        .map(|layer| vec![layer.to_string()])
        .unwrap_or_default();
    // Check before store detection: a failed destructive create may have left a marker before
    // even one qtsi survived, and that is itself an unhealthy store state.
    reject_incomplete_store_transactions(store_root, &marker_scope)?;
    let mut layers = detect_layers(store_root)?;
    if let Some(layer) = only_layer {
        layers.retain(|candidate| candidate == layer);
    }
    if layers.is_empty() {
        anyhow::bail!("no layer stores under {}", store_root.display());
    }

    let mut selected = Vec::new();
    for layer in &layers {
        let layer_dir = store_root.join(layer);
        let mut zooms = detect_zooms(&layer_dir)?;
        if let Some(zoom) = only_zoom {
            zooms.retain(|candidate| *candidate == zoom);
        }
        selected.extend(zooms.into_iter().map(|zoom| (layer.clone(), zoom)));
    }
    if selected.is_empty() {
        anyhow::bail!(
            "no store matched --layer/--zoom under {} — nothing was checked",
            store_root.display()
        );
    }

    // An open fd pins an inode across rename/unlink, but not across truncate. Acquire the
    // complete per-z set only after the outer domains have frozen the directory shape.
    let snapshot_locks = StoreFileLocks::acquire_canonical(
        selected
            .iter()
            .map(|(layer, zoom)| zoom_store_lock_path(&store_root.join(layer), *zoom)),
        timeout,
    )?;
    // A direct Rust writer need not take the outer master. Recheck only after every selected
    // per-z lock is held, matching the packer's race-closing snapshot protocol.
    reject_incomplete_store_transactions(store_root, &marker_scope)?;
    let snapshots = selected
        .iter()
        .map(|(layer, zoom)| capture_store(&store_root.join(layer), layer, *zoom))
        .collect::<Result<Vec<_>>>()?;
    Ok(LockedFsckSnapshots {
        _outer_locks: outer_locks,
        _snapshot_locks: snapshot_locks,
        snapshots,
    })
}

#[cfg(test)]
fn fsck_one(layer_dir: &Path, layer: &str, zoom: u8) -> Result<StoreFsckReport> {
    Ok(validate_snapshot(&capture_store(layer_dir, layer, zoom)?))
}

fn capture_store(layer_dir: &Path, layer: &str, zoom: u8) -> Result<FsckSnapshot> {
    let store =
        TileStore::open(layer_dir, zoom, false).with_context(|| format!("open {layer}/z{zoom}"))?;
    let captured_data_len = store.data_file_len()?;
    let mut entries: Vec<CapturedTileRef> = Vec::new();
    store.for_each_present(|x, y, entry| {
        entries.push(CapturedTileRef { x, y, entry });
        Ok(())
    })?;
    Ok(FsckSnapshot {
        layer: layer.to_string(),
        zoom,
        store,
        captured_data_len,
        entries,
    })
}

fn validate_snapshot(snapshot: &FsckSnapshot) -> StoreFsckReport {
    validate_captured_store(
        &snapshot.store,
        &snapshot.layer,
        snapshot.zoom,
        snapshot.captured_data_len,
        &snapshot.entries,
        |tile| *tile,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tile_painter::tile_store::fsck::MAX_PLAUSIBLE_BLOB_LEN;
    use tile_painter::tile_store::{
        ingest_store_lock_path, master_store_lock_path, StoreFileLock, StoreRebuildFence,
        StoreUpdateFence, TileCodec,
    };
    use tile_painter::wire_hm3::{write_tile, NO_DATA, SOURCE_ID_INDUSTRIAL, SOURCE_ID_RAIL};

    #[test]
    fn standalone_scan_retains_outer_and_per_zoom_locks_through_validation() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store_root = tmp.path().join("tiles/2026/store");
        let layer_dir = store_root.join("rail");
        TileStore::create(&layer_dir, 6, SOURCE_ID_RAIL, TILE_PX as u16)?.sync_all()?;
        let master_path = master_store_lock_path(&store_root)?;
        let ingest_path = ingest_store_lock_path(&store_root);
        let zoom_path = zoom_store_lock_path(&layer_dir, 6);

        let locked = capture_locked_snapshots(&store_root, None, None, Duration::ZERO)?;
        assert!(StoreFileLock::acquire_bounded(&master_path, Duration::ZERO).is_err());
        assert!(StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO).is_err());
        assert!(StoreFileLock::acquire_bounded(&zoom_path, Duration::ZERO).is_err());
        for snapshot in &locked.snapshots {
            validate_snapshot(snapshot).ensure_clean()?;
        }
        drop(locked);

        StoreFileLock::acquire_bounded(&master_path, Duration::ZERO)?;
        StoreFileLock::acquire_bounded(&ingest_path, Duration::ZERO)?;
        StoreFileLock::acquire_bounded(&zoom_path, Duration::ZERO)?;
        Ok(())
    }

    #[test]
    fn standalone_scan_rejects_an_incomplete_layer_rebuild() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store_root = tmp.path().join("tiles/2026/store");
        let layer_dir = store_root.join("rail");
        TileStore::create(&layer_dir, 6, SOURCE_ID_RAIL, TILE_PX as u16)?.sync_all()?;
        let fence = StoreRebuildFence::begin(&layer_dir, "pyramid:bbox-a")?;
        drop(fence);

        let error = capture_locked_snapshots(&store_root, None, None, Duration::ZERO)
            .err()
            .expect("an incomplete layer rebuild must block fsck");
        assert!(error.to_string().contains("layer transaction incomplete"));
        Ok(())
    }

    #[test]
    fn standalone_scan_rejects_an_incomplete_scoped_update_before_detection() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store_root = tmp.path().join("tiles/2026/store");
        StoreUpdateFence::begin(&store_root, "z12|bbox-a|rail")?;

        let error = capture_locked_snapshots(&store_root, None, None, Duration::ZERO)
            .err()
            .expect("an incomplete scoped update must block fsck");
        assert!(error.to_string().contains("scoped store update"));
        Ok(())
    }

    #[test]
    fn clean_store_reports_zero_problems() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        let cells = vec![NO_DATA; tile_painter::grid::TILE_PX * tile_painter::grid::TILE_PX];
        let p = dir.join("z12/10/11.bin");
        write_tile(&p, &cells, SOURCE_ID_RAIL, false).unwrap();
        // Promote the loose tile into a real store the same way tile-store-ingest does.
        let blob = std::fs::read(&p).unwrap();
        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store.put_blob(10, 11, TileCodec::BrotliHm3, &blob).unwrap();
        store.sync_all().unwrap();

        let r = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(r.entries, 1);
        assert!(r.decode_failures.is_empty());
        assert!(r.overlaps.is_empty());
    }

    #[test]
    fn corrupt_blob_is_reported_as_decode_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store
            .put_blob(3, 4, TileCodec::BrotliHm3, b"not a valid HM3 blob")
            .unwrap();
        store.sync_all().unwrap();

        let r = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(r.entries, 1);
        assert_eq!(r.decode_failures.len(), 1);
        assert_eq!((r.decode_failures[0].0.x, r.decode_failures[0].0.y), (3, 4));
        assert!(r.overlaps.is_empty());
    }

    /// Direct regression test for the live 2026-07-14 finding: two entries
    /// whose declared `[offset, offset+len)` ranges overlap must be flagged
    /// even though BOTH blobs decode successfully — decode-validation alone
    /// cannot catch a valid-stream-for-a-different-tile-plus-garbage entry.
    #[test]
    fn overlapping_entries_are_reported_even_when_both_decode_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("industrial");
        let cells = vec![NO_DATA; tile_painter::grid::TILE_PX * tile_painter::grid::TILE_PX];
        let loose = tmp.path().join("loose.bin");
        write_tile(&loose, &cells, SOURCE_ID_RAIL, false).unwrap();
        let blob = std::fs::read(&loose).unwrap();

        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store
            .put_blob(524, 997, TileCodec::BrotliHm3, &blob)
            .unwrap();
        store
            .put_blob(1232, 2500, TileCodec::BrotliHm3, &blob)
            .unwrap();
        store.sync_all().unwrap();
        drop(store);

        // Simulate the live corruption class directly: alias the second
        // tile's index entry onto the first tile's exact offset+len+codec
        // bytes — both now point at the SAME valid blob, so both decode
        // cleanly, but their ranges are identical (the sharpest overlap).
        use std::os::unix::fs::FileExt;
        let index_path = dir.join("z12.qtsi");
        let entry_pos =
            |x: u32, y: u32| -> u64 { 32 + (u64::from(x) * (1u64 << 12) + u64::from(y)) * 16 };
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&index_path)
            .unwrap();
        let mut first_entry = [0u8; 16];
        f.read_exact_at(&mut first_entry, entry_pos(524, 997))
            .unwrap();
        f.write_all_at(&first_entry, entry_pos(1232, 2500)).unwrap();

        let r = fsck_one(&dir, "industrial", 12).unwrap();
        assert_eq!(r.entries, 2);
        assert!(
            r.decode_failures.is_empty(),
            "both entries point at the same valid blob — decode alone must NOT catch this"
        );
        assert_eq!(
            r.overlaps.len(),
            1,
            "aliased offset must be reported as an overlap"
        );
    }

    /// Regression test for the altitude-review finding (2026-07-14): decode_cells alone
    /// accepts an entry whose blob decodes cleanly but carries a DIFFERENT layer's
    /// source_id — the exact class validate_hm3_by_entry rejects, as the MANDATORY
    /// pre-publish check tile-store-pack now runs over every captured snapshot.
    /// fsck must use the same check, or it reports "clean" on a store publish must refuse.
    #[test]
    fn source_id_mismatch_is_reported_even_though_the_blob_decodes_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        let cells = vec![NO_DATA; tile_painter::grid::TILE_PX * tile_painter::grid::TILE_PX];
        let loose = tmp.path().join("loose.bin");
        // A blob honestly tagged for a DIFFERENT source — a valid BrotliHm3 stream in its
        // own right, decode_cells has no reason to reject it.
        write_tile(&loose, &cells, SOURCE_ID_INDUSTRIAL, false).unwrap();
        let blob = std::fs::read(&loose).unwrap();

        // Store's own header says rail — put_blob trusts the caller, so this plants a
        // mismatch directly (mirrors how a stale/cross-layer ingest could corrupt a store).
        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store.put_blob(1, 1, TileCodec::BrotliHm3, &blob).unwrap();
        store.sync_all().unwrap();

        let r = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(r.entries, 1);
        assert_eq!(
            r.decode_failures.len(),
            1,
            "a source_id mismatch must be reported even though the blob decodes cleanly"
        );
    }

    /// Regression test for the /gg finding (2026-07-14): a legitimately self-consistent
    /// smaller store (pre-2026-07-shift 256px, still creatable via the API) decodes cleanly
    /// here, but `get_hm3_by_entry`'s ZstdCells arm would panic the real packer via
    /// `encode_tile_bytes`'s `assert_eq!` against the GLOBAL TILE_PX. fsck must flag the
    /// mismatch itself, not attempt the panicking call to find out.
    #[test]
    fn tile_px_mismatch_is_reported_even_with_zero_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        TileStore::create(&dir, 12, SOURCE_ID_RAIL, 256).unwrap();

        let r = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(r.tile_px_mismatch, Some(256));
    }

    #[test]
    fn matching_tile_px_reports_no_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        TileStore::create(&dir, 12, SOURCE_ID_RAIL, tile_painter::grid::TILE_PX as u16).unwrap();

        let r = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(r.tile_px_mismatch, None);
    }

    /// Regression test for the /gg finding (2026-07-14): Entry.len is an untrusted u32 read
    /// straight off disk — a corrupted index claiming an implausible length must be reported
    /// as a problem, not trigger a same-sized allocation before fsck ever gets to say so.
    #[test]
    fn implausible_len_is_reported_without_attempting_the_read() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        let cells = vec![NO_DATA; tile_painter::grid::TILE_PX * tile_painter::grid::TILE_PX];
        let p = dir.join("z12/10/11.bin");
        write_tile(&p, &cells, SOURCE_ID_RAIL, false).unwrap();
        let blob = std::fs::read(&p).unwrap();
        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store.put_blob(10, 11, TileCodec::BrotliHm3, &blob).unwrap();
        store.sync_all().unwrap();
        drop(store);

        // Corrupt the entry's len field in place to something far beyond any real tile —
        // simulates a corrupted index without needing gigabytes of backing data on disk.
        use std::os::unix::fs::FileExt;
        let index_path = dir.join("z12.qtsi");
        let entry_pos =
            |x: u32, y: u32| -> u64 { 32 + (u64::from(x) * (1u64 << 12) + u64::from(y)) * 16 };
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&index_path)
            .unwrap();
        let mut entry = [0u8; 16];
        f.read_exact_at(&mut entry, entry_pos(10, 11)).unwrap();
        entry[8..12].copy_from_slice(&(MAX_PLAUSIBLE_BLOB_LEN + 1).to_le_bytes());
        f.write_all_at(&entry, entry_pos(10, 11)).unwrap();

        let r = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(r.entries, 1);
        assert_eq!(r.decode_failures.len(), 1);
        assert!(r.decode_failures[0].1.contains("implausible len"));
    }

    #[test]
    fn entry_past_the_captured_data_length_is_reported_as_out_of_bounds() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        let cells = vec![NO_DATA; tile_painter::grid::TILE_PX * tile_painter::grid::TILE_PX];
        let loose = tmp.path().join("loose.bin");
        write_tile(&loose, &cells, SOURCE_ID_RAIL, false).unwrap();
        let blob = std::fs::read(&loose).unwrap();
        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store.put_blob(10, 11, TileCodec::BrotliHm3, &blob).unwrap();
        store.sync_all().unwrap();
        drop(store);

        use std::os::unix::fs::FileExt;
        let data_len = std::fs::metadata(dir.join("z12.qtsd")).unwrap().len();
        let index_path = dir.join("z12.qtsi");
        let entry_pos = 32 + (u64::from(10u32) * (1u64 << 12) + u64::from(11u32)) * 16;
        let index = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(index_path)
            .unwrap();
        let mut entry = [0u8; 16];
        index.read_exact_at(&mut entry, entry_pos).unwrap();
        entry[0..8].copy_from_slice(&data_len.to_le_bytes());
        index.write_all_at(&entry, entry_pos).unwrap();

        let report = fsck_one(&dir, "rail", 12).unwrap();
        assert_eq!(report.decode_failures.len(), 1);
        assert!(report.decode_failures[0]
            .1
            .contains("outside captured data file"));
    }

    /// Regression test for the /gg finding (2026-07-14): the overlap scan's `offset + len`
    /// must not panic (debug) or silently wrap (release) on a corrupted near-u64::MAX offset
    /// — the exact untrusted-index input class this tool exists to survive. Two entries are
    /// corrupted to adjacent near-MAX offsets so the smaller (the "a" of the sorted window)
    /// overflows when its len is added — proving `checked_add`, not a bare `+`, is in place.
    #[test]
    fn overflowing_offset_addition_is_reported_not_panicked() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("rail");
        let cells = vec![NO_DATA; tile_painter::grid::TILE_PX * tile_painter::grid::TILE_PX];
        let p = dir.join("z12/1/1.bin");
        write_tile(&p, &cells, SOURCE_ID_RAIL, false).unwrap();
        let blob = std::fs::read(&p).unwrap();
        let store = TileStore::create(&dir, 12, SOURCE_ID_RAIL, 512).unwrap();
        store.put_blob(1, 1, TileCodec::BrotliHm3, &blob).unwrap();
        store.put_blob(2, 2, TileCodec::BrotliHm3, &blob).unwrap();
        store.sync_all().unwrap();
        drop(store);

        use std::os::unix::fs::FileExt;
        let index_path = dir.join("z12.qtsi");
        let entry_pos =
            |x: u32, y: u32| -> u64 { 32 + (u64::from(x) * (1u64 << 12) + u64::from(y)) * 16 };
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&index_path)
            .unwrap();
        let plant = |x: u32, y: u32, offset: u64| {
            let mut entry = [0u8; 16];
            f.read_exact_at(&mut entry, entry_pos(x, y)).unwrap();
            entry[0..8].copy_from_slice(&offset.to_le_bytes());
            f.write_all_at(&entry, entry_pos(x, y)).unwrap();
        };
        // a's offset must be within its own (small but nonzero) len of u64::MAX for
        // offset+len to actually overflow — u64::MAX-1 leaves only 1 byte of headroom,
        // comfortably less than any real compressed tile's length.
        plant(1, 1, u64::MAX - 1); // sorts first; +len overflows past u64::MAX
        plant(2, 2, u64::MAX); // sorts second

        let r = fsck_one(&dir, "rail", 12).unwrap(); // must return, not panic
        assert_eq!(r.entries, 2);
        assert_eq!(
            r.overlaps.len(),
            1,
            "an unrepresentable range must be reported, not silently ignored"
        );
    }
}

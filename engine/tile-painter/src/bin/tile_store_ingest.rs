//! tile-store-ingest — the hub's promote step: move ONE extracted cell push
//! (`{layer}/{z}/{x}/{y}.bin` under a temp dir) into the per-(layer,zoom) tile
//! stores, verbatim ([`TileCodec::BrotliHm3`] blobs, entry-write-is-commit).
//!
//! Replaces the loose-tree `renameSync` loop in `scripts/world/hub.mjs`
//! (storage redesign 2026-07). The container format stays owned by ONE crate;
//! the hub just spawns this helper per validated push.
//!
//! Atomicity contract: per-TILE commit is the store's index-entry write. Before
//! this helper starts, the Hub durably records every possible write/tombstone;
//! a crash makes that whole scope dirty and keeps the layer unpublished. The
//! unsealed task must rebuild before goal finalization, and its later push
//! overwrites the same coordinates. Intermediate combine work may therefore
//! be conservative and repeated, but an incomplete cell cannot be published.
//!
//! A layer whose store does not exist yet is created with that layer's canonical HM3
//! `source_id`; every staged blob is decoded and checked against it before the first write.
//!
//! Usage: tile-store-ingest <store-root> <extracted-dir>
//!        [--rebuilt-bbox S,W,N,E] [--rebuilt-scope FILE]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use tile_painter::grid::{tile_range, TILE_PX};
use tile_painter::tile_store::{expected_source_id, TileCodec, TileStore};
use tile_painter::wire_hm3::read_tile_bytes_source_id;

const MAX_REBUILT_SCOPE_BYTES: u64 = 64 << 20;
const MAX_REBUILT_SCOPE_ENTRIES: usize = 1_000_000;
type TileCoordinates = HashSet<(u32, u32)>;
type RebuiltScope = BTreeMap<(String, u8), TileCoordinates>;

fn main() -> Result<()> {
    // usage: tile-store-ingest <store-root> <extracted-dir>
    //        [--rebuilt-bbox S,W,N,E] [--rebuilt-scope FILE]
    // --rebuilt-bbox declares "the staging is a COMPLETE rebuild of this box":
    // kernels never write all-silent tiles (skip_if_empty), so within the box
    // absence-from-staging MEANS silent — the sweep tombstones those store
    // tiles, or a tile that went quiet would keep its stale loud bytes and the
    // combine would keep summing them (/gg Codex CRITICAL, 2026-07-10).
    let mut positional: Vec<String> = Vec::new();
    let mut bbox: Option<[f64; 4]> = None;
    let mut rebuilt_scope: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--rebuilt-bbox" {
            let v: Vec<f64> = args
                .next()
                .context("--rebuilt-bbox needs S,W,N,E")?
                .split(',')
                .map(|p| p.parse())
                .collect::<Result<_, _>>()
                .context("--rebuilt-bbox parse")?;
            let [s, w, n, e]: [f64; 4] = v
                .try_into()
                .map_err(|_| anyhow::anyhow!("--rebuilt-bbox needs 4 numbers"))?;
            bbox = Some([s, w, n, e]);
        } else if a == "--rebuilt-scope" {
            if rebuilt_scope.is_some() {
                bail!("--rebuilt-scope may be specified only once");
            }
            rebuilt_scope = Some(PathBuf::from(
                args.next().context("--rebuilt-scope needs a file")?,
            ));
        } else {
            positional.push(a);
        }
    }
    let [store_root, extracted]: [String; 2] = positional.try_into().map_err(|_| {
        anyhow::anyhow!(
            "usage: tile-store-ingest <store-root> <extracted-dir> \
             [--rebuilt-bbox S,W,N,E] [--rebuilt-scope FILE]"
        )
    })?;
    if bbox.is_some() && rebuilt_scope.is_some() {
        bail!("--rebuilt-bbox and --rebuilt-scope are mutually exclusive");
    }
    // CROSS-PROCESS OUTER LOCK — coarser than, and now belt-and-suspenders on top of, the store's own
    // per-(layer,zoom) flock. Originally the P0 fix (audit 2026-07-13): the hub spawns up to 8 concurrent
    // tile-store-ingest and TileStore's append tail is a process-local AtomicU64, so two ingests into the
    // same .qtsd claimed the SAME offset and corrupted blobs (437 invalid HM3 blobs in 80 cells). Since
    // 2026-07-15 every writable TileStore::open/create self-locks its (layer,zoom)
    // (store.rs::acquire_write_lock), closing the hole for EVERY writer (tile-store-transcode too, not
    // just ingest×ingest). This root-level flock is kept as the OUTER serialiser: it is taken BEFORE any
    // store opens (the per-store lock is the innermost), so the ordering has no cycle — never a deadlock.
    // Same-root only, which is exactly the collision domain (one store root).
    let _ingest_lock = {
        fs::create_dir_all(&store_root).ok();
        let lock = fs::File::create(Path::new(&store_root).join(".ingest.lock"))
            .context("create .ingest.lock")?;
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&lock), libc::LOCK_EX) } != 0 {
            bail!("flock .ingest.lock: {}", std::io::Error::last_os_error());
        }
        lock
    };
    let (tiles, bytes, tombstoned) = ingest_dir(
        Path::new(&store_root),
        Path::new(&extracted),
        bbox,
        rebuilt_scope.as_deref(),
    )?;
    eprintln!("ingested {tiles} tiles / {bytes} B, tombstoned {tombstoned}");
    Ok(())
}

/// Walk `{layer}/{z}/{x}/{y}.bin` under `extracted` and put every blob into
/// `store_root/{layer}/z{z}`. Returns (tiles, bytes).
/// One tile file inside the extracted push: `(x, y, path)`.
type TileFile = (u32, u32, PathBuf);

/// Read the Hub-owned exact tombstone contract. Each line is `layer/zoom/x/y`; the Hub derives
/// it from the rebuilt cell's stale layers crossed with that cell's exact owned z12 tiles.
fn read_rebuilt_scope(path: &Path) -> Result<RebuiltScope> {
    let metadata = fs::metadata(path).with_context(|| path.display().to_string())?;
    if metadata.len() > MAX_REBUILT_SCOPE_BYTES {
        bail!(
            "rebuilt scope {} is {} bytes, above the {}-byte limit",
            path.display(),
            metadata.len(),
            MAX_REBUILT_SCOPE_BYTES
        );
    }
    let text = fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let mut scope = RebuiltScope::new();
    let mut entries = 0usize;
    for (line_index, line) in text.lines().enumerate() {
        let fields: Vec<&str> = line.split('/').collect();
        let [layer, zoom, x, y]: [&str; 4] = fields.try_into().map_err(|_| {
            anyhow::anyhow!(
                "rebuilt scope line {} is not layer/zoom/x/y",
                line_index + 1
            )
        })?;
        let zoom: u8 = zoom
            .parse()
            .with_context(|| format!("rebuilt scope line {} has invalid zoom", line_index + 1))?;
        let x: u32 = x
            .parse()
            .with_context(|| format!("rebuilt scope line {} has invalid x", line_index + 1))?;
        let y: u32 = y
            .parse()
            .with_context(|| format!("rebuilt scope line {} has invalid y", line_index + 1))?;
        expected_source_id(layer)
            .with_context(|| format!("rebuilt scope line {} has unknown layer", line_index + 1))?;
        let n = 1u32.checked_shl(u32::from(zoom)).with_context(|| {
            format!("rebuilt scope line {} has unsupported zoom", line_index + 1)
        })?;
        if x >= n || y >= n {
            bail!(
                "rebuilt scope line {} tile {x}/{y} is out of z{zoom} range",
                line_index + 1
            );
        }
        if scope
            .entry((layer.to_string(), zoom))
            .or_default()
            .insert((x, y))
        {
            entries += 1;
            if entries > MAX_REBUILT_SCOPE_ENTRIES {
                bail!("rebuilt scope exceeds {MAX_REBUILT_SCOPE_ENTRIES} unique entries");
            }
        }
    }
    Ok(scope)
}

fn ingest_dir(
    store_root: &Path,
    extracted: &Path,
    rebuilt_bbox: Option<[f64; 4]>,
    rebuilt_scope_path: Option<&Path>,
) -> Result<(u64, u64, u64)> {
    // Group files per (layer, zoom) so each store opens exactly once.
    let mut groups: BTreeMap<(String, u8), Vec<TileFile>> = BTreeMap::new();
    for layer_entry in fs::read_dir(extracted).with_context(|| extracted.display().to_string())? {
        let layer_entry = layer_entry?;
        if !layer_entry.file_type()?.is_dir() {
            continue;
        }
        let layer = layer_entry.file_name().to_string_lossy().into_owned();
        for z_entry in fs::read_dir(layer_entry.path())? {
            let z_entry = z_entry?;
            let Some(z) = z_entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u8>().ok())
            else {
                continue;
            };
            // The staging directory itself is the expected-layer contract. Builders create this
            // numeric zoom directory even when every rebuilt tile is silent, so an empty group
            // still reaches the tombstone sweep without another operator/user override.
            groups.entry((layer.clone(), z)).or_default();
            for x_entry in fs::read_dir(z_entry.path())? {
                let x_entry = x_entry?;
                let Some(x) = x_entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                for y_entry in fs::read_dir(x_entry.path())? {
                    let y_entry = y_entry?;
                    let name = y_entry.file_name();
                    let Some(y) = name
                        .to_str()
                        .and_then(|s| s.strip_suffix(".bin"))
                        .and_then(|s| s.parse::<u32>().ok())
                    else {
                        continue;
                    };
                    groups
                        .entry((layer.clone(), z))
                        .or_default()
                        .push((x, y, y_entry.path()));
                }
            }
        }
    }
    let rebuilt_scope = match rebuilt_scope_path {
        Some(path) => read_rebuilt_scope(path)?,
        None => RebuiltScope::new(),
    };
    for key in rebuilt_scope.keys() {
        groups.entry(key.clone()).or_default();
    }
    if groups.is_empty() && rebuilt_scope.is_empty() {
        bail!("no tiles under {}", extracted.display());
    }

    preflight_ingest(store_root, &groups, rebuilt_bbox, &rebuilt_scope)?;

    let (mut tiles, bytes) = (0u64, AtomicU64::new(0));
    let mut tombstoned = 0u64;
    let keys: BTreeSet<(String, u8)> = groups.keys().chain(rebuilt_scope.keys()).cloned().collect();
    for (layer, z) in keys {
        let files = groups.remove(&(layer.clone(), z)).unwrap_or_default();
        let exact_scope = rebuilt_scope.get(&(layer.clone(), z));
        let required_source_id =
            expected_source_id(&layer).with_context(|| format!("{layer}: unknown store layer"))?;
        let dir = store_root.join(&layer);
        // Stale-binary tripwire: the world base is z12 (512-px tiles) since
        // the 2026-07 shift — a z13 push can only come from a fleet box still
        // running pre-shift binaries, and must not plant a parallel 256 world
        // next to the real one (the pack would publish both). Probing the
        // pushed layer OR total/ covers a first-push-of-a-new-layer too.
        if z == 13
            && (dir.join("z12.qtsi").exists() || store_root.join("total").join("z12.qtsi").exists())
        {
            bail!(
                "{layer}: push is z13 but the store world is 512@z12 — \
                 this worker still runs pre-shift binaries; re-provision it"
            );
        }
        let store = if dir.join(format!("z{z}.qtsi")).exists() {
            TileStore::open(&dir, z, true)?
        } else {
            if files.is_empty() {
                // A newly introduced layer whose first rebuilt cell is silent has nothing to
                // tombstone yet. Do not allocate an empty global index merely to prove absence.
                continue;
            }
            TileStore::create(&dir, z, required_source_id, TILE_PX as u16)?
        };
        let store_sid = store.source_id();
        if store_sid != required_source_id {
            bail!(
                "{layer}: store source_id {store_sid} differs from required {required_source_id}"
            );
        }
        files
            .par_iter()
            .try_for_each(|(x, y, path)| -> Result<()> {
                let blob = fs::read(path).with_context(|| path.display().to_string())?;
                // Validate EVERY blob (decode + magic/version/size) — the
                // store ships BrotliHm3 verbatim into pmtiles, so this is the
                // last gate a corrupt or pre-shift push can fail (/gg Codex).
                let sid = read_tile_bytes_source_id(&blob)
                    .with_context(|| format!("{layer} {x}/{y}: invalid HM3 push"))?;
                if sid != store_sid {
                    bail!("{layer} {x}/{y}: source_id {sid} ≠ store {store_sid}");
                }
                store.put_blob(*x, *y, TileCodec::BrotliHm3, &blob)?;
                bytes.fetch_add(blob.len() as u64, Ordering::Relaxed);
                Ok(())
            })?;

        // Rebuilt-bbox sweep: within the declared box, a store tile ABSENT
        // from this staging went silent in the rebuild (kernels skip empty
        // tiles) — tombstone it or its stale bytes keep serving + summing.
        // The sweep stays strictly inside tile_range(bbox): builders cover at
        // least that range, so absence there is a definite verdict, never a
        // guess about fringe tiles the kernel didn't attempt.
        if let Some([s, w, n, e]) = rebuilt_bbox {
            let staged: HashSet<(u32, u32)> = files.iter().map(|(x, y, _)| (*x, *y)).collect();
            let (xr, yr) = tile_range(z, s, w, n, e);
            let mut present: Vec<(u32, u32)> = Vec::new();
            if !xr.is_empty() {
                // Guard the degenerate empty range (same lesson as combine's
                // scan): end-1 on an empty range would walk a full column.
                store.for_each_present_in_x_range(xr.start, xr.end - 1, |x, y, _| {
                    if yr.contains(&y) {
                        present.push((x, y));
                    }
                    Ok(())
                })?;
            }
            for (x, y) in present {
                if !staged.contains(&(x, y)) {
                    store.delete(x, y)?;
                    tombstoned += 1;
                }
            }
        }
        if let Some(scope) = exact_scope {
            let staged: HashSet<(u32, u32)> = files.iter().map(|(x, y, _)| (*x, *y)).collect();
            for &(x, y) in scope {
                if !staged.contains(&(x, y)) && store.present(x, y)? {
                    store.delete(x, y)?;
                    tombstoned += 1;
                }
            }
        }
        store.sync_all()?;
        tiles += files.len() as u64;
    }
    Ok((tiles, bytes.load(Ordering::Relaxed), tombstoned))
}

/// Validate the complete staging set and every destination header before the first store write.
/// The write pass validates each blob again to fail closed if a caller violates the staging-tree
/// quiescence contract, but a static corrupt push can never leave a prefix committed.
fn preflight_ingest(
    store_root: &Path,
    groups: &BTreeMap<(String, u8), Vec<TileFile>>,
    rebuilt_bbox: Option<[f64; 4]>,
    rebuilt_scope: &RebuiltScope,
) -> Result<()> {
    for ((layer, zoom), files) in groups {
        let required_source_id =
            expected_source_id(layer).with_context(|| format!("{layer}: unknown store layer"))?;
        if let Some(scope) = rebuilt_scope.get(&(layer.clone(), *zoom)) {
            for (x, y, _) in files {
                if !scope.contains(&(*x, *y)) {
                    bail!("{layer} z{zoom} pushed tile {x}/{y} is outside the exact rebuilt scope");
                }
            }
        }
        if let Some([south, west, north, east]) = rebuilt_bbox {
            let (x_range, y_range) = tile_range(*zoom, south, west, north, east);
            for (x, y, _) in files {
                if !x_range.contains(x) || !y_range.contains(y) {
                    bail!("{layer} z{zoom} pushed tile {x}/{y} is outside the rebuilt bbox");
                }
            }
        }

        files
            .par_iter()
            .try_for_each(|(x, y, path)| -> Result<()> {
                let blob = fs::read(path).with_context(|| path.display().to_string())?;
                let source_id = read_tile_bytes_source_id(&blob).with_context(|| {
                    format!("{layer} {x}/{y}: invalid HM3 push during preflight")
                })?;
                if source_id != required_source_id {
                    bail!(
                        "{layer}: source_id {source_id} differs from required {required_source_id}"
                    );
                }
                Ok(())
            })?;

        let directory = store_root.join(layer);
        let index_exists = directory.join(format!("z{zoom}.qtsi")).try_exists()?;
        let data_exists = directory.join(format!("z{zoom}.qtsd")).try_exists()?;
        if index_exists != data_exists {
            bail!("{layer} z{zoom}: index/data store pair is incomplete before ingest");
        }
        if index_exists {
            let store = TileStore::open(&directory, *zoom, false)?;
            if store.source_id() != required_source_id {
                bail!(
                    "{layer}: store source_id {} differs from required {required_source_id}",
                    store.source_id()
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tile_painter::wire_hm3::{write_tile, NO_DATA, SOURCE_ID_RAIL};

    fn bbox_inside_tile(zoom: u8, x: u32, y: u32) -> [f64; 4] {
        let scale = f64::from(1u32 << zoom);
        let inside = 1e-7;
        let longitude = |tile_x: f64| tile_x / scale * 360.0 - 180.0;
        let latitude = |tile_y: f64| {
            (std::f64::consts::PI * (1.0 - 2.0 * tile_y / scale))
                .sinh()
                .atan()
                .to_degrees()
        };
        [
            latitude(f64::from(y) + 1.0 - inside),
            longitude(f64::from(x) + inside),
            latitude(f64::from(y) + inside),
            longitude(f64::from(x) + 1.0 - inside),
        ]
    }

    #[test]
    fn ingest_creates_store_and_reingest_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, ex) = (tmp.path().join("stores"), tmp.path().join("x"));
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[7] = 90;
        for (x, y) in [(10u32, 11u32), (10, 12)] {
            let p = ex
                .join("rail/12")
                .join(x.to_string())
                .join(format!("{y}.bin"));
            write_tile(&p, &cells, SOURCE_ID_RAIL, false).unwrap();
        }

        let (n, b, _) = ingest_dir(&root, &ex, None, None).unwrap();
        assert_eq!(n, 2);
        assert!(b > 0);
        let s = TileStore::open(&root.join("rail"), 12, false).unwrap();
        assert_eq!(s.source_id(), SOURCE_ID_RAIL);
        assert_eq!(s.get_cells(10, 11).unwrap().unwrap(), cells);

        // A requeued cell re-pushes the same coords: same count, new bytes win.
        cells[7] = 92;
        let p = ex.join("rail/12/10/11.bin");
        write_tile(&p, &cells, SOURCE_ID_RAIL, false).unwrap();
        ingest_dir(&root, &ex, None, None).unwrap();
        let s = TileStore::open(&root.join("rail"), 12, false).unwrap();
        assert_eq!(
            s.get_cells(10, 11).unwrap().unwrap(),
            cells,
            "overwrite wins"
        );
        let mut count = 0;
        s.for_each_present(|_, _, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 2, "no duplicate entries after re-ingest");
    }

    #[test]
    fn exact_rebuilt_scope_tombstones_a_tile_that_became_silent() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, ex) = (tmp.path().join("stores"), tmp.path().join("push"));
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[7] = 90;
        for y in [11u32, 12] {
            write_tile(
                &ex.join(format!("rail/12/10/{y}.bin")),
                &cells,
                SOURCE_ID_RAIL,
                false,
            )
            .unwrap();
        }
        ingest_dir(&root, &ex, None, None).unwrap();

        fs::remove_file(ex.join("rail/12/10/12.bin")).unwrap();
        let scope = tmp.path().join("scope.txt");
        fs::write(&scope, "rail/12/10/11\nrail/12/10/12\n").unwrap();
        let (written, _, tombstoned) = ingest_dir(&root, &ex, None, Some(&scope)).unwrap();
        assert_eq!(written, 1);
        assert_eq!(tombstoned, 1);
        let store = TileStore::open(&root.join("rail"), 12, false).unwrap();
        assert!(store.present(10, 11).unwrap());
        assert!(!store.present(10, 12).unwrap());
    }

    #[test]
    fn exact_rebuilt_scope_can_tombstone_an_entire_silent_push() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, ex) = (tmp.path().join("stores"), tmp.path().join("push"));
        let path = ex.join("rail/12/10/11.bin");
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[7] = 90;
        write_tile(&path, &cells, SOURCE_ID_RAIL, false).unwrap();
        ingest_dir(&root, &ex, None, None).unwrap();
        fs::remove_file(&path).unwrap();
        let scope = tmp.path().join("scope.txt");
        fs::write(&scope, "rail/12/10/11\n").unwrap();

        let (written, _, tombstoned) = ingest_dir(&root, &ex, None, Some(&scope)).unwrap();
        assert_eq!((written, tombstoned), (0, 1));
        let store = TileStore::open(&root.join("rail"), 12, false).unwrap();
        assert!(!store.present(10, 11).unwrap());
    }

    #[test]
    fn empty_expected_zoom_directory_tombstones_an_entire_silent_bbox_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, extracted) = (tmp.path().join("stores"), tmp.path().join("push"));
        let cells = vec![90; TILE_PX * TILE_PX];
        let store =
            TileStore::create(&root.join("rail"), 6, SOURCE_ID_RAIL, TILE_PX as u16).unwrap();
        store.put_cells(10, 11, &cells).unwrap();
        store.sync_all().unwrap();
        drop(store);
        fs::create_dir_all(extracted.join("rail/6")).unwrap();

        let (written, _, tombstoned) =
            ingest_dir(&root, &extracted, Some(bbox_inside_tile(6, 10, 11)), None).unwrap();
        assert_eq!((written, tombstoned), (0, 1));
        let store = TileStore::open(&root.join("rail"), 6, false).unwrap();
        assert!(!store.present(10, 11).unwrap());
    }

    #[test]
    fn bbox_preflight_rejects_a_staged_tile_outside_scope_before_store_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, extracted) = (tmp.path().join("stores"), tmp.path().join("push"));
        write_tile(
            &extracted.join("rail/6/12/11.bin"),
            &vec![90; TILE_PX * TILE_PX],
            SOURCE_ID_RAIL,
            false,
        )
        .unwrap();

        let error =
            ingest_dir(&root, &extracted, Some(bbox_inside_tile(6, 10, 11)), None).unwrap_err();
        assert!(error.to_string().contains("outside the rebuilt bbox"));
        assert!(!root.join("rail/z6.qtsi").exists());
    }

    #[test]
    fn corrupt_later_blob_fails_preflight_before_an_earlier_tile_is_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, extracted) = (tmp.path().join("stores"), tmp.path().join("push"));
        let old_cells = vec![80; TILE_PX * TILE_PX];
        let new_cells = vec![100; TILE_PX * TILE_PX];
        let store =
            TileStore::create(&root.join("rail"), 6, SOURCE_ID_RAIL, TILE_PX as u16).unwrap();
        store.put_cells(10, 11, &old_cells).unwrap();
        store.sync_all().unwrap();
        drop(store);
        write_tile(
            &extracted.join("rail/6/10/11.bin"),
            &new_cells,
            SOURCE_ID_RAIL,
            false,
        )
        .unwrap();
        fs::create_dir_all(extracted.join("rail/6/10")).unwrap();
        fs::write(extracted.join("rail/6/10/12.bin"), b"corrupt").unwrap();

        assert!(ingest_dir(&root, &extracted, None, None).is_err());
        let store = TileStore::open(&root.join("rail"), 6, false).unwrap();
        assert_eq!(store.get_cells(10, 11).unwrap().unwrap(), old_cells);
        assert!(!store.present(10, 12).unwrap());
    }

    #[test]
    fn ingest_rejects_wrong_layer_source_id_before_store_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, extracted) = (tmp.path().join("stores"), tmp.path().join("push"));
        let path = extracted.join("road/12/10/11.bin");
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[7] = 90;
        write_tile(&path, &cells, SOURCE_ID_RAIL, false).unwrap();

        let error = ingest_dir(&root, &extracted, None, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("road: source_id 2 differs from required 1"),
            "unexpected error: {error:#}"
        );
        assert!(!root.join("road/z12.qtsi").exists());
    }
}

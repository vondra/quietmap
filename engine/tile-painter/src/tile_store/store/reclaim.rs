//! Reclaim unreachable log blocks without changing the index, live bytes, or file length.

use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;

use anyhow::{bail, Context, Result};

use super::TileStore;
use crate::tile_store::{format::HEADER_BYTES, StoreFileLocks};

impl TileStore {
    /// Caller retains the outer writer domains and this store's lock through validation and
    /// reclamation. Read the complete current index here: a caller's filtered feed cannot
    /// authorize deleting a tile. Shared logs have another lock/index domain and are skipped.
    pub fn reclaim_unreferenced_blocks(&self, _locks: &StoreFileLocks) -> Result<u64> {
        let before = self.data.metadata()?;
        if before.nlink() != 1 || self.index.metadata()?.nlink() != 1 {
            return Ok(0);
        }
        if before.len() > i64::MAX as u64 {
            bail!("data log exceeds fallocate offset range");
        }
        let mut ranges = Vec::new();
        self.for_each_present(|_, _, entry| {
            let end = entry
                .offset
                .checked_add(u64::from(entry.len))
                .context("indexed range overflows during reclamation")?;
            if entry.offset < HEADER_BYTES || end > before.len() {
                bail!("indexed range outside data log during reclamation");
            }
            ranges.push((entry.offset, end));
            Ok(())
        })?;
        ranges.sort_unstable();
        let mut filesystem = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::fstatvfs(self.data.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error()).context("stat data-log filesystem");
        }
        let block = unsafe { filesystem.assume_init() }.f_frsize;
        if block == 0 {
            bail!("data-log filesystem has no allocation block size");
        }
        let mut gaps = Vec::new();
        let mut previous_end = HEADER_BYTES;
        // Validate every range before the first mutation, including overlaps late in the log.
        for (start, end) in ranges.into_iter().chain([(before.len(), before.len())]) {
            if start < previous_end {
                bail!("overlapping indexed ranges during reclamation");
            }
            let aligned_start = previous_end.div_ceil(block) * block;
            let aligned_end = start / block * block;
            if aligned_start < aligned_end {
                gaps.push((aligned_start, aligned_end));
            }
            previous_end = end;
        }
        if gaps.is_empty() {
            return Ok(0);
        }
        // Reopen the pinned inode, not a pathname a rename could replace. The ordinary read
        // handle stays read-only; only this explicitly locked maintenance operation can write.
        let writable = OpenOptions::new()
            .write(true)
            .open(format!("/proc/self/fd/{}", self.data.as_raw_fd()))?;
        let reopened = writable.metadata()?;
        if (reopened.dev(), reopened.ino(), reopened.len())
            != (before.dev(), before.ino(), before.len())
        {
            bail!("data-log identity changed during reclamation");
        }
        self.sync_all()?;
        for (start, end) in gaps {
            let result = unsafe {
                libc::fallocate(
                    writable.as_raw_fd(),
                    libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
                    start as libc::off_t,
                    (end - start) as libc::off_t,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error()).context("reclaim data-log blocks");
            }
        }
        writable.sync_all()?;
        Ok(before
            .blocks()
            .saturating_sub(writable.metadata()?.blocks())
            * 512)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile_store::{format::ENTRY_BYTES, zoom_store_lock_path, TileCodec};
    use std::fs;
    use std::os::unix::fs::FileExt;
    use std::time::Duration;

    fn fixture() -> Result<(tempfile::TempDir, Vec<Vec<u8>>)> {
        let directory = tempfile::tempdir()?;
        let store = TileStore::create(directory.path(), 4, 1, 512)?;
        let blobs: Vec<Vec<u8>> = (0..12)
            .map(|i| vec![i as u8 + 1; 16381 + i * 123])
            .collect();
        for (i, blob) in blobs.iter().enumerate() {
            store.put_blob(i as u32, 0, TileCodec::BrotliHm3, blob)?;
        }
        for (i, blob) in blobs.iter().enumerate().step_by(2) {
            store.put_blob(i as u32, 0, TileCodec::BrotliHm3, blob)?;
        }
        store.delete(11, 0)?;
        store.sync_all()?;
        Ok((directory, blobs))
    }

    fn lock(directory: &std::path::Path) -> Result<StoreFileLocks> {
        StoreFileLocks::acquire_canonical([zoom_store_lock_path(directory, 4)], Duration::ZERO)
    }

    #[test]
    fn reclaim_preserves_header_index_every_live_byte_and_later_appends() -> Result<()> {
        let (directory, blobs) = fixture()?;
        let root = directory.path();
        let index = fs::read(root.join("z4.qtsi"))?;
        let before = fs::read(root.join("z4.qtsd"))?;
        {
            let locks = lock(root)?;
            let store = TileStore::open(root, 4, false)?;
            assert!(store.reclaim_unreferenced_blocks(&locks)? > 0);
            assert_eq!(store.reclaim_unreferenced_blocks(&locks)?, 0);
            assert_eq!(fs::read(root.join("z4.qtsi"))?, index);
            let after = fs::read(root.join("z4.qtsd"))?;
            assert_eq!(after.len(), before.len());
            assert_eq!(
                &after[..HEADER_BYTES as usize],
                &before[..HEADER_BYTES as usize]
            );
            for (i, blob) in blobs.iter().enumerate().take(11) {
                assert_eq!(store.get_blob(i as u32, 0)?.unwrap().1, *blob);
            }
            assert!(store.get_blob(11, 0)?.is_none());
        }
        let writer = TileStore::open(root, 4, true)?;
        writer.put_blob(15, 0, TileCodec::BrotliHm3, b"after reclaim")?;
        assert_eq!(writer.get_blob(15, 0)?.unwrap().1, b"after reclaim");
        for (i, blob) in blobs.iter().enumerate().take(11) {
            assert_eq!(writer.get_blob(i as u32, 0)?.unwrap().1, *blob);
        }
        Ok(())
    }

    #[test]
    fn malformed_index_cannot_authorize_any_deallocation() -> Result<()> {
        for offset in [0, HEADER_BYTES + 16381, u64::MAX - 1, 1 << 40] {
            let (directory, _) = fixture()?;
            let root = directory.path();
            let index = OpenOptions::new().write(true).open(root.join("z4.qtsi"))?;
            index.write_all_at(&offset.to_le_bytes(), HEADER_BYTES + 10 * 16 * ENTRY_BYTES)?;
            let before = fs::read(root.join("z4.qtsd"))?;
            let locks = lock(root)?;
            let store = TileStore::open(root, 4, false)?;
            assert!(store.reclaim_unreferenced_blocks(&locks).is_err());
            assert_eq!(fs::read(root.join("z4.qtsd"))?, before);
        }
        Ok(())
    }

    #[test]
    fn shared_index_or_data_is_left_untouched() -> Result<()> {
        for extension in ["qtsi", "qtsd"] {
            let (directory, _) = fixture()?;
            let root = directory.path();
            fs::hard_link(
                root.join(format!("z4.{extension}")),
                root.join("other-domain"),
            )?;
            let before = fs::read(root.join("z4.qtsd"))?;
            let locks = lock(root)?;
            let store = TileStore::open(root, 4, false)?;
            assert_eq!(store.reclaim_unreferenced_blocks(&locks)?, 0);
            assert_eq!(fs::read(root.join("z4.qtsd"))?, before);
        }
        Ok(())
    }
}

//! Reserve simultaneous file writes against observed filesystem capacity before issuing them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::sync::{LazyLock, Mutex};

static PENDING: LazyLock<Mutex<HashMap<u64, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct Reservation<'a> {
    pending: &'a Mutex<HashMap<u64, u64>>,
    device: u64,
    bytes: u64,
}

impl<'a> Reservation<'a> {
    fn acquire(
        pending: &'a Mutex<HashMap<u64, u64>>,
        device: u64,
        available: impl FnOnce() -> io::Result<u64>,
        bytes: u64,
    ) -> io::Result<Self> {
        let mut reservations = pending
            .lock()
            .map_err(|_| io::Error::other("disk reservation lock poisoned"))?;
        let available = available()?;
        let reserved = reservations.entry(device).or_default();
        if bytes > available.saturating_sub(*reserved) {
            return Err(io::Error::new(io::ErrorKind::StorageFull, format!("aircraft disk admission: {bytes} B write, {available} B available, {reserved} B pending")));
        }
        *reserved += bytes;
        Ok(Self {
            pending,
            device,
            bytes,
        })
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Ok(mut reservations) = self.pending.lock() {
            *reservations
                .get_mut(&self.device)
                .expect("live disk reservation") -= self.bytes;
        }
    }
}

pub(super) struct DiskCheckedFile {
    file: File,
    device: u64,
    block_bytes: u64,
}

impl DiskCheckedFile {
    pub fn new(file: File) -> io::Result<Self> {
        let device = file.metadata()?.dev();
        super::disk_watermark::reserve_receipt_row(device)?;
        let block_bytes = filesystem_space(&file)?.1.max(1);
        Ok(Self {
            file,
            device,
            block_bytes,
        })
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.file.sync_all()
    }
}

impl Write for DiskCheckedFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let bytes = (buffer.len() as u64).div_ceil(self.block_bytes) * self.block_bytes;
        let _reservation = Reservation::acquire(
            &PENDING,
            self.device,
            || super::disk_watermark::writable_bytes(self.device, filesystem_space(&self.file)?.0),
            bytes,
        )?;
        self.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub(super) fn filesystem_space(file: &File) -> io::Result<(u64, u64)> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // The owned File keeps this descriptor valid through fstatvfs.
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Successful fstatvfs initialized the complete structure.
    let stats = unsafe { stats.assume_init() };
    Ok((
        stats.f_bavail.saturating_mul(stats.f_frsize),
        stats.f_frsize,
    ))
}

pub(crate) fn create_directory_all_synced(path: &std::path::Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut ancestor = path;
    while !ancestor.as_os_str().is_empty() && !ancestor.exists() {
        missing.push(ancestor);
        ancestor = ancestor
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
    }
    // Another worker may have created the existing ancestor but not yet synced
    // its parent. Publish that entry before relying on it for durable children.
    if let Some(parent) = ancestor.parent() {
        let parent = if parent.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            parent
        };
        File::open(parent)?.sync_all()?;
    }
    for directory in missing.into_iter().rev() {
        match std::fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && directory.is_dir() => {}
            Err(error) => return Err(error),
        }
        let parent = directory
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_writes_cannot_each_claim_the_same_free_bytes_and_drop_releases() {
        let pending = Mutex::new(HashMap::new());
        let first = Reservation::acquire(&pending, 1, || Ok(100), 60).unwrap();
        assert!(Reservation::acquire(&pending, 1, || Ok(100), 50).is_err());
        let other_disk = Reservation::acquire(&pending, 2, || Ok(100), 50).unwrap();
        drop(first);
        assert!(Reservation::acquire(&pending, 1, || Ok(100), 100).is_ok());
        drop(other_disk);
    }
    #[test]
    fn durable_directory_creation_preserves_existing_files_and_accepts_repeated_paths() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("new/child");
        create_directory_all_synced(&nested).unwrap();
        create_directory_all_synced(&nested).unwrap();
        let file = nested.join("input");
        std::fs::write(&file, b"original").unwrap();
        assert!(create_directory_all_synced(&file.join("child")).is_err());
        assert_eq!(std::fs::read(file).unwrap(), b"original");
    }
}

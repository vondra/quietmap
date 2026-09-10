//! A scoped spill watermark protects net filesystem headroom, including its pending receipt.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

struct Watermark {
    start_free: u64,
    minimum_free: u64,
    receipt_bytes: u64,
}
static WATERMARKS: LazyLock<Mutex<HashMap<u64, Watermark>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct SpillDiskReservation {
    device: u64,
}
impl SpillDiskReservation {
    pub fn new(directory: &Path, bytes: u64, input_count: usize) -> io::Result<Self> {
        super::create_directory_all_synced(directory)?;
        let file = File::open(directory)?;
        let device = file.metadata()?.dev();
        let start_free = super::disk::filesystem_space(&file)?.0;
        if bytes > start_free {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "spill disk reservation exceeds current free capacity",
            ));
        }
        // Main DB schema/input pages plus rollback journal and a rounded page.
        let receipt_bytes =
            2 * crate::stage_2b::spill_receipt_page_limit(0, input_count, 4096) * 4096 + 4096;
        if bytes < receipt_bytes {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "spill disk reservation cannot hold its receipt",
            ));
        }
        let mut values = WATERMARKS
            .lock()
            .map_err(|_| io::Error::other("spill watermark lock poisoned"))?;
        if values.contains_key(&device) {
            return Err(io::Error::other(
                "spill disk already reserved by this process",
            ));
        }
        values.insert(
            device,
            Watermark {
                start_free,
                minimum_free: start_free - bytes,
                receipt_bytes,
            },
        );
        Ok(Self { device })
    }
}
impl Drop for SpillDiskReservation {
    fn drop(&mut self) {
        if let Ok(mut values) = WATERMARKS.lock() {
            values.remove(&self.device);
        }
    }
}

pub(super) fn reserve_receipt_row(device: u64) -> io::Result<()> {
    if let Some(value) = WATERMARKS
        .lock()
        .map_err(|_| io::Error::other("spill watermark lock poisoned"))?
        .get_mut(&device)
    {
        value.receipt_bytes += 1024;
    }
    Ok(())
}

fn remaining(free: u64, minimum_free: u64, receipt_bytes: u64) -> io::Result<u64> {
    free.checked_sub(minimum_free).and_then(|value| value.checked_sub(receipt_bytes)).ok_or_else(|| io::Error::new(io::ErrorKind::StorageFull, format!("spill disk watermark: {free} B free, {minimum_free} B protected, {receipt_bytes} B reserved for receipt")))
}

pub(super) fn writable_bytes(device: u64, free: u64) -> io::Result<u64> {
    let values = WATERMARKS
        .lock()
        .map_err(|_| io::Error::other("spill watermark lock poisoned"))?;
    match values.get(&device) {
        Some(value) => remaining(free, value.minimum_free, value.receipt_bytes),
        None => Ok(free),
    }
}

pub(crate) fn receipt_watermark(directory: &Path) -> io::Result<Option<(u64, u64)>> {
    let file = File::open(directory)?;
    let values = WATERMARKS
        .lock()
        .map_err(|_| io::Error::other("spill watermark lock poisoned"))?;
    if let Some(value) = values.get(&file.metadata()?.dev()) {
        remaining(
            super::disk::filesystem_space(&file)?.0,
            value.minimum_free,
            value.receipt_bytes,
        )?;
        Ok(Some((value.start_free, value.minimum_free)))
    } else {
        Ok(None)
    }
}

pub(crate) fn receipt_committed(directory: &Path) -> io::Result<()> {
    let file = File::open(directory)?;
    let mut values = WATERMARKS
        .lock()
        .map_err(|_| io::Error::other("spill watermark lock poisoned"))?;
    if let Some(value) = values.get_mut(&file.metadata()?.dev()) {
        value.receipt_bytes = 0;
        remaining(
            super::disk::filesystem_space(&file)?.0,
            value.minimum_free,
            0,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_growth_and_protected_free_space_are_unavailable_to_raw_writes() {
        assert_eq!(remaining(1000, 500, 200).unwrap(), 300);
        assert!(remaining(699, 500, 200).is_err());
        assert_eq!(remaining(1200, 500, 200).unwrap(), 500);
        assert!(remaining(499, 500, 0).is_err());
    }
}

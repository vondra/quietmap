//! One provider-day archive: split TAR parts checked and read as one stream, trace members parsed in
//! parallel batches, one whole trace kept per address. A broken archive fails the day; a corrupt
//! member is only recorded.

use super::filters::point_is_sane;
use super::readsb::parse_trace;
use super::trace::{AircraftTrace, address_identity};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Members and compressed bytes parsed together: a batch bounds what is held beside the traces.
const PARSE_BATCH_MEMBERS: usize = 4096;
const PARSE_BATCH_BYTES: usize = 256 << 20;

/// A trace member whose gzip or JSON failed to decode.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CorruptMember {
    pub member: String,
    pub error: String,
    /// Another export of the same day holds an intact trace of this address.
    pub recovered: bool,
}

/// Every aircraft trace of one provider-day plus the members that failed to decode.
pub struct DayArchive {
    pub traces: Vec<AircraftTrace>,
    pub corrupt_members: Vec<CorruptMember>,
}

/// Group TAR parts into streams (`x.tar` whole or `x.tar.aa`, `x.tar.ab`, ..) and require every
/// split stream contiguous and every stream ending in the TAR end marker.
pub fn checked_archive_parts(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let mut streams: BTreeMap<PathBuf, Vec<(String, PathBuf)>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            return Err(format!("duplicate TAR part {}", path.display()));
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{}: not a UTF-8 file name", path.display()))?;
        let (stem, suffix) = name
            .rsplit_once(".tar")
            .ok_or_else(|| format!("{}: not a TAR part", path.display()))?;
        let split_suffix = suffix.len() == 3
            && suffix.starts_with('.')
            && suffix[1..].bytes().all(|b| b.is_ascii_lowercase());
        if !(suffix.is_empty() || split_suffix) {
            return Err(format!(
                "unfinished or unsupported TAR part {}",
                path.display()
            ));
        }
        streams
            .entry(path.with_file_name(stem))
            .or_default()
            .push((suffix.to_string(), path));
    }
    if streams.is_empty() {
        return Err("no TAR archive".into());
    }
    let mut ordered = Vec::new();
    for (stem, mut parts) in streams {
        parts.sort();
        let whole = parts
            .first()
            .is_some_and(|(suffix, _)| suffix.is_empty())
            .then(|| parts.remove(0).1);
        for (index, (suffix, _)) in parts.iter().enumerate() {
            let expected = format!(
                ".{}{}",
                (b'a' + (index / 26) as u8) as char,
                (b'a' + (index % 26) as u8) as char
            );
            if *suffix != expected {
                return Err(format!("missing TAR part {}.tar{expected}", stem.display()));
            }
        }
        if !parts.is_empty() {
            let split: Vec<PathBuf> = parts.into_iter().map(|(_, path)| path).collect();
            require_end_marker(&split)?;
            ordered.extend(split);
        }
        if let Some(path) = whole {
            require_end_marker(std::slice::from_ref(&path))?;
            ordered.push(path);
        }
    }
    Ok(ordered)
}

/// A complete TAR stream is a whole number of 512-byte blocks ending in 1,024 zero bytes.
fn require_end_marker(parts: &[PathBuf]) -> Result<(), String> {
    let mut sizes = Vec::with_capacity(parts.len());
    for path in parts {
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!("{}: not a non-empty archive file", path.display()));
        }
        sizes.push(metadata.len());
    }
    let total: u64 = sizes.iter().sum();
    if total < 1024 || !total.is_multiple_of(512) {
        return Err(format!("{}: incomplete TAR length", parts[0].display()));
    }
    let mut remaining = 1024u64;
    for (path, size) in parts.iter().zip(sizes).rev() {
        let take = remaining.min(size);
        let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut tail = vec![0u8; take as usize];
        file.seek(SeekFrom::End(-(take as i64)))
            .and_then(|_| file.read_exact(&mut tail))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if tail.iter().any(|&byte| byte != 0) {
            return Err(format!("{}: missing TAR end marker", path.display()));
        }
        remaining -= take;
        if remaining == 0 {
            break;
        }
    }
    Ok(())
}

/// Read every trace member of checked parts ([`checked_archive_parts`]). A member's bytes are read
/// before decoding, so an I/O or TAR error fails the day while a corrupt member is recorded.
pub fn read_archive(parts: &[PathBuf]) -> Result<DayArchive, String> {
    let files = parts
        .iter()
        .map(|path| File::open(path).map_err(|error| format!("{}: {error}", path.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let mut archive =
        tar::Archive::new(BufReader::with_capacity(1 << 20, Concatenated::new(files)));
    archive.set_ignore_zeros(true);
    let where_ = || parts[0].display().to_string();
    let mut traces = Vec::new();
    let mut corrupt = Vec::new();
    let mut batch: Vec<(String, Vec<u8>)> = Vec::with_capacity(PARSE_BATCH_MEMBERS);
    let mut batch_bytes = 0;
    let mut parse = |batch: &mut Vec<(String, Vec<u8>)>| {
        let parsed: Vec<_> = std::mem::take(batch)
            .into_par_iter()
            .map(|(member, bytes)| (member, parse_trace(&bytes)))
            .collect();
        for (member, result) in parsed {
            match result {
                Ok(Some(trace)) => traces.push(trace),
                Ok(None) => {}
                Err(error) => corrupt.push((member, error)),
            }
        }
    };
    let entries = archive
        .entries()
        .map_err(|error| format!("{}: {error}", where_()))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("TAR entry in {}: {error}", where_()))?;
        let member = entry
            .path()
            .map_err(|error| format!("TAR path in {}: {error}", where_()))?
            .to_string_lossy()
            .into_owned();
        if member_address(&member).is_none() {
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{member} in {}: {error}", where_()))?;
        batch_bytes += bytes.len();
        batch.push((member, bytes));
        if batch.len() >= PARSE_BATCH_MEMBERS || batch_bytes >= PARSE_BATCH_BYTES {
            parse(&mut batch);
            batch_bytes = 0;
        }
    }
    parse(&mut batch);
    let traces = select_whole_traces(traces);
    let intact: HashSet<(bool, u32)> = traces
        .iter()
        .filter_map(|trace| address_identity(&trace.address))
        .collect();
    let corrupt_members = corrupt
        .into_iter()
        .map(|(member, error)| CorruptMember {
            recovered: member_address(&member)
                .and_then(address_identity)
                .is_some_and(|identity| intact.contains(&identity)),
            member,
            error,
        })
        .collect();
    Ok(DayArchive {
        traces,
        corrupt_members,
    })
}

/// The address a `.../trace_full_<address>.json[.gz]` member names.
fn member_address(member: &str) -> Option<&str> {
    let name = member.rsplit('/').next()?.strip_prefix("trace_full_")?;
    name.strip_suffix(".json.gz")
        .or_else(|| name.strip_suffix(".json"))
}

/// One trace per address: of alternative exports the one with more sane points wins (ties keep
/// the first read), never a point merge. Traces without a valid identity all stay.
fn select_whole_traces(traces: Vec<AircraftTrace>) -> Vec<AircraftTrace> {
    let sane = |trace: &AircraftTrace| trace.points.iter().filter(|p| point_is_sane(p)).count();
    let mut index_of: HashMap<(bool, u32), usize> = HashMap::new();
    let mut selected: Vec<AircraftTrace> = Vec::with_capacity(traces.len());
    for trace in traces {
        let identity = address_identity(&trace.address);
        match identity.and_then(|id| index_of.get(&id).copied()) {
            Some(index) => {
                if sane(&trace) > sane(&selected[index]) {
                    selected[index] = trace;
                }
            }
            None => {
                if let Some(id) = identity {
                    index_of.insert(id, selected.len());
                }
                selected.push(trace);
            }
        }
    }
    selected
}

/// Byte-contiguous parts read as one stream.
struct Concatenated<R> {
    readers: Vec<R>,
    current: usize,
}

impl<R: Read> Concatenated<R> {
    fn new(readers: Vec<R>) -> Self {
        Concatenated {
            readers,
            current: 0,
        }
    }
}

impl<R: Read> Read for Concatenated<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.current < self.readers.len() {
            let read = self.readers[self.current].read(buffer)?;
            if read > 0 {
                return Ok(read);
            }
            self.current += 1;
        }
        Ok(0)
    }
}

/// The TAR parts of a directory (every file whose name holds `.tar`), checked.
pub fn directory_archive_parts(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(".tar"))
        {
            paths.push(path);
        }
    }
    checked_archive_parts(paths)
}

#[cfg(test)]
#[path = "archive_tests.rs"]
pub(crate) mod tests;

//! TAR day validation and streaming archive traversal; a structurally broken archive fails loudly, a corrupt trace member is recorded.

use super::selection::trace_identity;
use super::{parse_trace, AircraftTrace};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// One provider-day archive walk: one whole trace per aircraft address plus
/// the trace members that failed to decode.
pub struct DayArchiveRead {
    pub traces: Vec<AircraftTrace>,
    pub corrupt_members: Vec<CorruptMember>,
}

/// A trace member whose gzip or JSON failed to decode.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CorruptMember {
    pub member: String,
    pub error: String,
    /// Another export of the same day carries an intact trace of this address.
    pub recovered: bool,
}

/// Read every aircraft trace from a single day's TAR archive(s).
/// Multipart support handles `.tar.aa` + `.tar.ab` continuation files. A
/// member's bytes are read before decoding, so an I/O or TAR error fails the
/// day while a corrupt member is only recorded.
pub fn read_day_archive(day_dir: &Path) -> Result<DayArchiveRead> {
    let tar_parts = archive_parts(day_dir)?;
    let readers: Vec<File> = tar_parts
        .iter()
        .map(File::open)
        .collect::<io::Result<Vec<_>>>()?;
    let concat = ConcatReader::new(readers);
    let buf = BufReader::with_capacity(1 << 20, concat);
    let mut archive = tar::Archive::new(buf);
    archive.set_ignore_zeros(true);

    let mut traces = Vec::new();
    let mut corrupt: Vec<(String, String)> = Vec::new();
    for entry in archive.entries()? {
        let mut entry =
            entry.with_context(|| format!("read TAR entry in {}", day_dir.display()))?;
        let path = entry.path()?.into_owned();
        let path_str = path.to_string_lossy().into_owned();
        if !path_str.contains("trace_full_")
            || !(path_str.ends_with(".json") || path_str.ends_with(".json.gz"))
        {
            continue;
        }
        let mut gz_bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut gz_bytes)
            .with_context(|| format!("read {path_str} in {}", day_dir.display()))?;
        match parse_trace(gz_bytes.as_slice()) {
            Ok(Some(trace)) => traces.push(trace),
            Ok(None) => {}
            Err(error) => corrupt.push((path_str, format!("{error:#}"))),
        }
    }
    let traces = super::selection::select_whole_traces(traces);
    let intact: HashSet<(bool, u32)> = traces
        .iter()
        .filter_map(|trace| trace_identity(&trace.icao24))
        .collect();
    let corrupt_members = corrupt
        .into_iter()
        .map(|(member, error)| CorruptMember {
            recovered: member_address(&member)
                .and_then(trace_identity)
                .is_some_and(|identity| intact.contains(&identity)),
            member,
            error,
        })
        .collect();
    Ok(DayArchiveRead {
        traces,
        corrupt_members,
    })
}

/// The aircraft address a `…/trace_full_<address>.json[.gz]` member names.
fn member_address(member: &str) -> Option<&str> {
    let name = member.rsplit('/').next()?;
    let name = name.strip_prefix("trace_full_")?;
    name.strip_suffix(".json.gz")
        .or_else(|| name.strip_suffix(".json"))
}

/// Resolve every TAR stream and require contiguous split parts plus its end marker.
pub(crate) fn archive_parts(day_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut groups: BTreeMap<String, Vec<(String, PathBuf)>> = BTreeMap::new();
    for entry in std::fs::read_dir(day_dir)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 ADS-B filename"))?;
        let Some((stem, suffix)) = name.rsplit_once(".tar") else {
            continue;
        };
        if !(suffix.is_empty()
            || (suffix.len() == 3
                && suffix.starts_with('.')
                && suffix[1..].bytes().all(|b| b.is_ascii_lowercase())))
        {
            anyhow::bail!(
                "unfinished or unsupported TAR part: {}",
                entry.path().display()
            );
        }
        anyhow::ensure!(
            entry.file_type()?.is_file(),
            "not an archive file: {}",
            entry.path().display()
        );
        groups
            .entry(stem.to_string())
            .or_default()
            .push((suffix.to_string(), entry.path()));
    }
    anyhow::ensure!(
        !groups.is_empty(),
        "no TAR archive in {}",
        day_dir.display()
    );
    let mut paths = Vec::new();
    for (stem, mut parts) in groups {
        parts.sort_by(|a, b| a.0.cmp(&b.0));
        let complete = if parts.first().is_some_and(|(suffix, _)| suffix.is_empty()) {
            Some(parts.remove(0).1)
        } else {
            None
        };
        for (i, (suffix, _)) in parts.iter().enumerate() {
            let expected = format!(
                ".{}{}",
                (b'a' + (i / 26) as u8) as char,
                (b'a' + (i % 26) as u8) as char
            );
            anyhow::ensure!(*suffix == expected, "missing TAR part {stem}.tar{expected}");
        }
        if !parts.is_empty() {
            let part_paths: Vec<_> = parts.into_iter().map(|(_, p)| p).collect();
            require_tar_end_marker(&part_paths)?;
            paths.extend(part_paths);
        }
        // Prefer the reassembled split export on equal trace quality.
        // Validate every stream before parsing any trace.
        if let Some(path) = complete {
            require_tar_end_marker(std::slice::from_ref(&path))?;
            paths.push(path);
        }
    }
    Ok(paths)
}

fn require_tar_end_marker(parts: &[PathBuf]) -> Result<()> {
    let sizes = parts
        .iter()
        .map(|p| Ok(p.metadata()?.len()))
        .collect::<io::Result<Vec<_>>>()?;
    let total: u64 = sizes.iter().sum();
    anyhow::ensure!(
        sizes.iter().all(|s| *s > 0) && total >= 1024 && total.is_multiple_of(512),
        "incomplete TAR byte length: {}",
        parts[0].display()
    );
    let mut remaining = 1024;
    for (path, size) in parts.iter().zip(sizes).rev() {
        let take = remaining.min(size as usize);
        let mut file = File::open(path)?;
        file.seek(SeekFrom::End(-(take as i64)))?;
        let mut tail = vec![0; take];
        file.read_exact(&mut tail)?;
        anyhow::ensure!(
            tail.iter().all(|b| *b == 0),
            "missing TAR end marker: {}",
            path.display()
        );
        remaining -= take;
        if remaining == 0 {
            break;
        }
    }
    Ok(())
}

/// Sequentially reads a list of byte-contiguous parts as one stream.
/// Used to recover the multipart TAR continuation files.
pub(super) struct ConcatReader<R> {
    readers: Vec<R>,
    current: usize,
}

impl<R: Read> ConcatReader<R> {
    pub(super) fn new(readers: Vec<R>) -> Self {
        Self {
            readers,
            current: 0,
        }
    }
}

impl<R: Read> Read for ConcatReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.current < self.readers.len() {
            let n = self.readers[self.current].read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            self.current += 1;
        }
        Ok(0)
    }
}

//! TAR day validation and streaming archive traversal; corrupt or incomplete inputs fail loudly.

use super::typecode_probe::probe_typecode_prefix;
use super::{AircraftTrace, parse_trace};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Read every aircraft trace from a single day's TAR archive(s).
/// Multipart support handles `.tar.aa` + `.tar.ab` continuation files.
pub fn read_day_traces(day_dir: &Path) -> Result<Vec<AircraftTrace>> {
    Ok(read_day_traces_filtered(day_dir, None)?.0)
}

/// Outcome counters for the gzip typecode prefix probe in
/// [`read_day_traces_filtered`]. The probe is an optimization ONLY —
/// a miss falls back to the full inflate+parse and the post-parse
/// filter, never to classification by absence.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TypecodeProbeStats {
    /// `"t":"…"` recovered from the inflated prefix → the probe alone
    /// decided keep / skip.
    pub probe_hits: u64,
    /// Probe hit whose typecode the prefilter rejected — full
    /// inflate+parse avoided (the GA pass's cost lever: airliner
    /// traces are the longest files).
    pub skipped_pre_parse: u64,
    /// No typecode in the prefix (absent `"t"` key — e.g. `noRegData`
    /// TIS-B targets — non-string value, value crossing the probe
    /// window, undecodable gzip) → full parse fallback.
    pub probe_misses: u64,
}

/// Like [`read_day_traces`], with an optional typecode prefilter that
/// drives a gzip prefix probe: entries whose probed typecode the
/// filter rejects skip the full inflate+parse entirely; probe misses
/// are fully parsed and then filtered on the authoritative parsed
/// typecode. With `None` the walk is identical to `read_day_traces`.
pub fn read_day_traces_filtered(
    day_dir: &Path,
    typecode_prefilter: Option<&dyn Fn(&str) -> bool>,
) -> Result<(Vec<AircraftTrace>, TypecodeProbeStats)> {
    let tar_parts = archive_parts(day_dir)?;
    let mut stats = TypecodeProbeStats::default();

    let readers: Vec<File> = tar_parts
        .iter()
        .map(File::open)
        .collect::<io::Result<Vec<_>>>()?;
    let concat = ConcatReader::new(readers);
    let buf = BufReader::with_capacity(1 << 20, concat);
    let mut archive = tar::Archive::new(buf);
    archive.set_ignore_zeros(true);

    let mut traces = Vec::new();
    for entry in archive.entries()? {
        let mut entry =
            entry.with_context(|| format!("read TAR entry in {}", day_dir.display()))?;
        let path = entry.path()?.into_owned();
        let path_str = path.to_string_lossy();
        if !path_str.contains("trace_full_")
            || !(path_str.ends_with(".json") || path_str.ends_with(".json.gz"))
        {
            continue;
        }
        let Some(filter) = typecode_prefilter else {
            if let Some(trace) =
                parse_trace(entry).with_context(|| format!("parse {}", path.display()))?
            {
                traces.push(trace);
            }
            continue;
        };
        // Sequential tar reading consumes the entry either way; buffer
        // the compressed bytes once so the prefix probe and the
        // (conditional) full parse share a single read.
        let mut gz_bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut gz_bytes)?;
        match probe_typecode_prefix(&gz_bytes) {
            Some(typecode) => {
                stats.probe_hits += 1;
                if !filter(&typecode) {
                    stats.skipped_pre_parse += 1;
                    continue;
                }
                if let Some(trace) = parse_trace(gz_bytes.as_slice())
                    .with_context(|| format!("parse {}", path.display()))?
                {
                    traces.push(trace);
                }
            }
            None => {
                stats.probe_misses += 1;
                // Never classify by absence: parse fully, then filter on
                // the parsed typecode.
                if let Some(trace) = parse_trace(gz_bytes.as_slice())
                    .with_context(|| format!("parse {}", path.display()))?
                {
                    if filter(&trace.aircraft_type) {
                        traces.push(trace);
                    }
                }
            }
        }
    }
    Ok((super::selection::select_whole_traces(traces), stats))
}

/// Resolve every TAR stream and require contiguous split parts plus its end marker.
fn archive_parts(day_dir: &Path) -> Result<Vec<PathBuf>> {
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

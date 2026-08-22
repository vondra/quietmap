//! adsb.lol JSON trace parser → `TracePoint` stream.
//!
//! TAR layout: one `subset.tar` (or split `.tar.aa` + `.tar.ab`) per day,
//! containing `traces/{xx}/trace_full_{icao24}.json.gz`. Each gzipped
//! JSON has shape `{ "icao", "t", "timestamp", "trace": [[ts_offset, lat,
//! lon, alt_ft|"ground", speed_kt, track_deg, on_ground_bit, baro_rate,
//! ...], ...] }`.
//!
//! Parsing here is intentionally permissive — only structural failures
//! (truncated gzip, malformed JSON) drop a record. Per-point sanity
//! checks live in `filters::point_is_sane` so the filter set has a
//! single source of truth.

use anyhow::Result;
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

/// Trace-point bit 0 — `on_ground` set by the adsb.lol bitfield.
pub const FLAG_ON_GROUND_RAW: u8 = 1 << 0;
/// Trace-point bit 1 — altitude column was the literal string `"ground"`.
pub const FLAG_ALT_IS_GROUND: u8 = 1 << 1;

/// One ADS-B point. `flags` packs the two ground-related raw signals so
/// the v6 Arrow schema can carry them as a single byte without losing
/// the distinction the composite ground inference relies on.
#[derive(Clone, Debug)]
pub struct TracePoint {
    pub timestamp: f64,
    pub lat: f32,
    pub lon: f32,
    /// Barometric altitude in feet. `NaN` when [`FLAG_ALT_IS_GROUND`] is
    /// set; read via [`TracePoint::airborne_alt_ft`] to keep ground
    /// sentinels out of arithmetic.
    pub alt_ft: f32,
    pub speed_kt: f32,
    pub track_deg: f32,
    pub baro_rate_fpm: f32,
    pub flags: u8,
}

/// Inline callsign transition: at trace `point_idx`, the callsign became
/// `value`. Most traces have 1–4 transitions per day (single flight or
/// rotation through 2–3 schedules). Stored on the trace, not per-point,
/// to avoid the 24-byte Option<String> on the hot 1.6M-point Vec.
#[derive(Clone, Debug)]
pub struct CallsignChange {
    pub point_idx: usize,
    pub value: String,
}

impl TracePoint {
    pub fn alt_is_ground(&self) -> bool {
        self.flags & FLAG_ALT_IS_GROUND != 0
    }
    pub fn on_ground_raw(&self) -> bool {
        self.flags & FLAG_ON_GROUND_RAW != 0
    }
    /// `Some(alt_ft)` for airborne points, `None` for `alt_is_ground`
    /// sentinel rows whose `alt_ft` is `NaN`. Funnels every alt-arithmetic
    /// site through one flag-aware accessor so a missed branch can't
    /// silently propagate NaN into AGL / ROCD / teleport arithmetic.
    pub fn airborne_alt_ft(&self) -> Option<f32> {
        if self.alt_is_ground() {
            None
        } else {
            Some(self.alt_ft)
        }
    }
}

/// All trace points for one aircraft on one day.
pub struct AircraftTrace {
    pub icao24: String,
    pub aircraft_type: String,
    pub points: Vec<TracePoint>,
    /// Callsign transitions in raw-trace `point_idx` order. The Stage 0
    /// driver (`source_adsb_tar::trace_to_flight`) rebases these onto
    /// post-`point_is_sane` indices and reduces them to one scalar
    /// callsign per emitted [`Flight`].
    pub callsigns: Vec<CallsignChange>,
}

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
    let mut tar_parts: Vec<_> = std::fs::read_dir(day_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => return false,
            };
            name.ends_with(".tar") || name.ends_with(".tar.aa") || name.ends_with(".tar.ab")
        })
        .collect();
    tar_parts.sort();
    let mut stats = TypecodeProbeStats::default();
    if tar_parts.is_empty() {
        return Ok((Vec::new(), stats));
    }

    let readers: Vec<File> = tar_parts
        .iter()
        .map(File::open)
        .collect::<io::Result<Vec<_>>>()?;
    let concat = ConcatReader::new(readers);
    let buf = BufReader::with_capacity(1 << 20, concat);
    let mut archive = tar::Archive::new(buf);

    let mut traces = Vec::new();
    for entry in archive.entries()? {
        let Ok(mut entry) = entry else {
            continue;
        };
        let path = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(_) => continue,
        };
        let path_str = path.to_str().unwrap_or("");
        if !path_str.contains("trace_full_") || !path_str.ends_with(".json") {
            continue;
        }
        let Some(filter) = typecode_prefilter else {
            if let Ok(Some(trace)) = parse_trace(entry) {
                traces.push(trace);
            }
            continue;
        };
        // Sequential tar reading consumes the entry either way; buffer
        // the compressed bytes once so the prefix probe and the
        // (conditional) full parse share a single read.
        let mut gz_bytes = Vec::with_capacity(entry.size() as usize);
        if entry.read_to_end(&mut gz_bytes).is_err() {
            continue;
        }
        match probe_typecode_prefix(&gz_bytes) {
            Some(typecode) => {
                stats.probe_hits += 1;
                if !filter(&typecode) {
                    stats.skipped_pre_parse += 1;
                    continue;
                }
                if let Ok(Some(trace)) = parse_trace(gz_bytes.as_slice()) {
                    traces.push(trace);
                }
            }
            None => {
                stats.probe_misses += 1;
                // Never classify by absence: parse fully, then filter on
                // the parsed typecode.
                if let Ok(Some(trace)) = parse_trace(gz_bytes.as_slice()) {
                    if filter(&trace.aircraft_type) {
                        traces.push(trace);
                    }
                }
            }
        }
    }
    Ok((traces, stats))
}

/// Decompressed-byte budget for the typecode prefix probe. The readsb
/// `trace_full` header carries `"t":"<typecode>"` at byte ~32 when
/// present (verified on the real 2025 release tree); 512 leaves slack for
/// long `desc` / `ownOp` fields ahead of it while staying orders of
/// magnitude below a full trace's decompressed size.
const TYPECODE_PROBE_DECOMPRESSED_BYTES: usize = 512;

/// Inflate at most [`TYPECODE_PROBE_DECOMPRESSED_BYTES`] of a gzipped
/// trace and scan for the `"t":"<typecode>"` header field. `None` is a
/// probe MISS (no `"t"` key, non-string value, value crossing the
/// probe window, undecodable gzip) — callers MUST fall back to the
/// full parse on miss, so
/// a miss can never misclassify a trace.
fn probe_typecode_prefix(gz_bytes: &[u8]) -> Option<String> {
    let mut head = [0u8; TYPECODE_PROBE_DECOMPRESSED_BYTES];
    let mut filled = 0usize;
    let mut gz = GzDecoder::new(gz_bytes);
    while filled < head.len() {
        match gz.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            // Truncated/odd gzip → let the full parse path decide.
            Err(_) => break,
        }
    }
    scan_json_typecode(&head[..filled])
}

/// Byte-scan a JSON prefix for `"t"` ws* `:` ws* `"<value>"` (handles
/// readsb's compact form and pretty-printed variants). Escaped quotes
/// inside string values can't false-match — `\"t\"` never contains
/// the raw byte sequence `"t"` — and a value running past the prefix
/// (or a non-string value like `null`) returns `None` = probe miss.
fn scan_json_typecode(head: &[u8]) -> Option<String> {
    let mut i = 0usize;
    while i + 3 <= head.len() {
        if head[i] == b'"' && head[i + 1] == b't' && head[i + 2] == b'"' {
            let mut j = i + 3;
            while j < head.len() && head[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < head.len() && head[j] == b':' {
                j += 1;
                while j < head.len() && head[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < head.len() && head[j] == b'"' {
                    j += 1;
                    let start = j;
                    // Typecodes are plain ASCII; a backslash inside the
                    // value is out-of-contract → miss, full parse decides.
                    while j < head.len() && head[j] != b'"' && head[j] != b'\\' {
                        j += 1;
                    }
                    if j < head.len() && head[j] == b'"' {
                        return String::from_utf8(head[start..j].to_vec()).ok();
                    }
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

/// Parse one gzipped `trace_full_*.json` from a TAR entry. Returns
/// `None` when the JSON is empty, missing the trace array, or has
/// fewer than two valid points.
pub fn parse_trace<R: Read>(reader: R) -> Result<Option<AircraftTrace>> {
    let json_bytes = {
        let mut gz = GzDecoder::new(reader);
        let mut buf = Vec::new();
        match gz.read_to_end(&mut buf) {
            Ok(_) if !buf.is_empty() => buf,
            _ => return Ok(None),
        }
    };
    let val: serde_json::Value = match serde_json::from_slice(&json_bytes) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    let icao24 = val
        .get("icao")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let aircraft_type = val
        .get("t")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let base_timestamp = val.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let trace_arr = match val.get("trace").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Ok(None),
    };

    let mut points = Vec::with_capacity(trace_arr.len());
    let mut callsigns: Vec<CallsignChange> = Vec::new();
    for entry in trace_arr {
        let arr = match entry.as_array() {
            Some(a) if a.len() >= 7 => a,
            _ => continue,
        };
        let ts_offset = arr[0].as_f64().unwrap_or(0.0);
        let lat = arr[1].as_f64().unwrap_or(0.0) as f32;
        let lon = arr[2].as_f64().unwrap_or(0.0) as f32;
        let (alt_ft, alt_is_ground) = parse_altitude_ft(&arr[3]);
        let speed_kt = arr[4].as_f64().unwrap_or(0.0) as f32;
        let track_deg = arr[5].as_f64().unwrap_or(0.0) as f32;
        let on_ground_bit = arr[6].as_i64().unwrap_or(0);
        let baro_rate_fpm = arr.get(7).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

        // Compare on `&str` before allocating — adsb.lol re-emits the
        // meta block on every position, so most points produce a
        // duplicate that would otherwise allocate a String just to be
        // dropped.
        if let Some(raw) = arr
            .get(8)
            .and_then(|v| v.get("flight"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if callsigns.last().map(|c| c.value.as_str()) != Some(raw) {
                callsigns.push(CallsignChange {
                    point_idx: points.len(),
                    value: raw.to_string(),
                });
            }
        }

        let mut flags = 0u8;
        if on_ground_bit & 1 != 0 {
            flags |= FLAG_ON_GROUND_RAW;
        }
        if alt_is_ground {
            flags |= FLAG_ALT_IS_GROUND;
            // adsb.lol semantics: "alt is ground" implies on_ground.
            flags |= FLAG_ON_GROUND_RAW;
        }
        points.push(TracePoint {
            timestamp: base_timestamp + ts_offset,
            lat,
            lon,
            alt_ft,
            speed_kt,
            track_deg,
            baro_rate_fpm,
            flags,
        });
    }
    if points.len() < 2 {
        return Ok(None);
    }
    Ok(Some(AircraftTrace {
        icao24,
        aircraft_type,
        points,
        callsigns,
    }))
}

/// `"ground"` string maps to `(NaN, true)` — not `(0.0, true)` — so a
/// sub-sea-level aerodrome (Schiphol −3 m, Atyrau −22 m) can't collide
/// with the on-surface marker, and a missed flag check downstream
/// surfaces as NaN rather than as a silent underground truncation.
fn parse_altitude_ft(value: &serde_json::Value) -> (f32, bool) {
    if let Some(alt_ft) = value.as_f64() {
        return (alt_ft as f32, false);
    }
    if value
        .as_str()
        .map(|s| s.eq_ignore_ascii_case("ground"))
        .unwrap_or(false)
    {
        return (f32::NAN, true);
    }
    (f32::NAN, false)
}

/// Sequentially reads a list of byte-contiguous parts as one stream.
/// Used to recover the multipart TAR continuation files.
struct ConcatReader<R> {
    readers: Vec<R>,
    current: usize,
}

impl<R: Read> ConcatReader<R> {
    fn new(readers: Vec<R>) -> Self {
        Self {
            readers,
            current: 0,
        }
    }
}

impl<R: Read> Read for ConcatReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    fn gz(json: &str) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(json.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn parses_ground_altitude_marker() {
        let raw = gz(r#"{"icao":"49d261","t":"PC12","timestamp":1000,"trace":[
                [10,50.0,14.0,"ground",12.0,90.0,0,0],
                [20,50.001,14.001,600.0,80.0,120.0,0,0]
            ]}"#);
        let t = parse_trace(raw.as_slice()).unwrap().unwrap();
        assert_eq!(t.points.len(), 2);
        assert!(t.points[0].alt_is_ground());
        assert!(t.points[0].on_ground_raw()); // implied by alt_is_ground
        assert!(t.points[0].alt_ft.is_nan());
        assert!(t.points[0].airborne_alt_ft().is_none());
        assert!(!t.points[1].alt_is_ground());
        assert_eq!(t.points[1].alt_ft, 600.0);
    }

    #[test]
    fn parses_bitfield_on_ground() {
        let raw = gz(r#"{"icao":"49c083","t":"WT9","timestamp":2000,"trace":[
                [10,50.0,14.0,300.0,40.0,90.0,1,0],
                [20,50.001,14.001,400.0,60.0,120.0,0,0]
            ]}"#);
        let t = parse_trace(raw.as_slice()).unwrap().unwrap();
        assert!(t.points[0].on_ground_raw());
        assert!(!t.points[0].alt_is_ground());
        assert!(!t.points[1].on_ground_raw());
    }

    #[test]
    fn drops_traces_with_fewer_than_two_points() {
        let raw = gz(
            r#"{"icao":"abc123","t":"B738","timestamp":1,"trace":[[1,50.0,14.0,1000.0,250.0,90.0,0,0]]}"#,
        );
        assert!(parse_trace(raw.as_slice()).unwrap().is_none());
    }

    #[test]
    fn extracts_callsign_from_point_metadata() {
        // adsb.lol col 8 is sometimes a meta object carrying `flight`,
        // sometimes null. The parser records each value transition;
        // duplicates from re-emitted meta blocks must coalesce.
        let raw = gz(r#"{"icao":"49d328","t":"A320","timestamp":1000,"trace":[
                [10,50.0,14.0,1000.0,250.0,90.0,0,0,null],
                [20,50.001,14.001,1100.0,250.0,90.0,0,0,{"flight":"TVS100P  "}],
                [30,50.002,14.002,1200.0,250.0,90.0,0,0,{"flight":"TVS100P  "}],
                [40,50.003,14.003,1300.0,250.0,90.0,0,0,{"flight":"TVS200X  "}]
            ]}"#);
        let t = parse_trace(raw.as_slice()).unwrap().unwrap();
        assert_eq!(t.callsigns.len(), 2);
        assert_eq!(t.callsigns[0].point_idx, 1);
        assert_eq!(t.callsigns[0].value, "TVS100P");
        assert_eq!(t.callsigns[1].point_idx, 3);
        assert_eq!(t.callsigns[1].value, "TVS200X");
    }

    #[test]
    fn callsign_metadata_optional() {
        let raw = gz(r#"{"icao":"49d262","t":"PC12","timestamp":1000,"trace":[
                [10,50.0,14.0,1000.0,250.0,90.0,0,0],
                [20,50.001,14.001,1100.0,250.0,90.0,0,0]
            ]}"#);
        let t = parse_trace(raw.as_slice()).unwrap().unwrap();
        assert!(t.callsigns.is_empty());
    }

    #[test]
    fn missing_baro_rate_defaults_to_zero() {
        // 7-element row (no baro_rate column) should still parse.
        let raw = gz(r#"{"icao":"49d262","t":"PC12","timestamp":1000,"trace":[
                [10,50.0,14.0,500.0,80.0,90.0,0],
                [20,50.001,14.001,600.0,80.0,90.0,0]
            ]}"#);
        let t = parse_trace(raw.as_slice()).unwrap().unwrap();
        assert_eq!(t.points[0].baro_rate_fpm, 0.0);
    }

    #[test]
    fn concat_reader_glues_two_streams() {
        let a = b"first half".to_vec();
        let b = b" second half".to_vec();
        let concat = ConcatReader::new(vec![a.as_slice(), b.as_slice()]);
        let mut out = Vec::new();
        BufReader::new(concat).read_to_end(&mut out).unwrap();
        assert_eq!(&out, b"first half second half");
    }

    /// Two-point trace JSON with the `"t"` field in readsb's normal
    /// early-header position.
    fn trace_json(icao: &str, typecode_field: &str) -> String {
        format!(
            r#"{{"icao":"{icao}",{typecode_field}"timestamp":1000,"trace":[
                [10,50.0,14.0,1000.0,250.0,90.0,0,0],
                [20,50.001,14.001,1100.0,250.0,90.0,0,0]
            ]}}"#
        )
    }

    #[test]
    fn scan_json_typecode_fixtures() {
        // Normal compact readsb form.
        assert_eq!(
            scan_json_typecode(br#"{"icao":"a","r":"OK-ABC","t":"B738","trace":[]}"#),
            Some("B738".to_string())
        );
        // Pretty-printed: whitespace around the colon + newlines.
        assert_eq!(
            scan_json_typecode(b"{\n  \"icao\": \"b\",\n  \"t\" : \"C172\",\n}"),
            Some("C172".to_string())
        );
        // Empty string value is a valid HIT (blank typecode = FALLBACK).
        assert_eq!(scan_json_typecode(br#"{"t":""}"#), Some(String::new()));
        // No "t" key at all (noRegData TIS-B shape) → miss.
        assert_eq!(
            scan_json_typecode(br#"{"icao":"c","noRegData":true}"#),
            None
        );
        // Non-string value → miss (full parse decides).
        assert_eq!(scan_json_typecode(br#"{"t":null}"#), None);
        // Value cut off by the probe window → miss, NOT a partial hit.
        assert_eq!(scan_json_typecode(br#"{"icao":"d","t":"C17"#), None);
        // `"t"` as a string VALUE (not a key) must not match; the real
        // key later in the buffer still hits.
        assert_eq!(
            scan_json_typecode(br#"{"r":"t","t":"R44"}"#),
            Some("R44".to_string())
        );
        // Escaped quotes inside an earlier value can't false-match.
        assert_eq!(
            scan_json_typecode(br#"{"desc":"say \"t\": hi","t":"EC35"}"#),
            Some("EC35".to_string())
        );
    }

    #[test]
    fn probe_typecode_prefix_inflates_only_the_window() {
        // "t" within the first 512 decompressed bytes → hit.
        let early = gz(&trace_json("aaa111", r#""t":"B738","#));
        assert_eq!(probe_typecode_prefix(&early), Some("B738".to_string()));
        // "t" pushed past the probe window by a long desc → miss.
        let pad = "x".repeat(TYPECODE_PROBE_DECOMPRESSED_BYTES + 64);
        let late = gz(&trace_json(
            "bbb222",
            &format!(r#""desc":"{pad}","t":"C172","#),
        ));
        assert_eq!(probe_typecode_prefix(&late), None);
        // Not gzip at all → miss (full parse path decides).
        assert_eq!(probe_typecode_prefix(b"plain bytes, not gzip"), None);
    }

    fn day_dir_with_tar(entries: &[(&str, &[u8])]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(tmp.path().join("subset.tar")).unwrap();
        let mut builder = tar::Builder::new(file);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.finish().unwrap();
        tmp
    }

    /// End-to-end probe semantics:
    /// probe hits skip rejected traces pre-parse; probe misses (late
    /// `"t"`, absent `"t"`) ALWAYS full-parse and are filtered on the
    /// parsed typecode — a trace is never classified by absence.
    #[test]
    fn prefilter_skips_on_hit_and_full_parses_on_miss() {
        let b738 = gz(&trace_json("aaa111", r#""t":"B738","#));
        let c172_pretty = gz(
            "{\n  \"icao\": \"bbb222\",\n  \"t\": \"C172\",\n  \"timestamp\": 1000,\n  \"trace\": [\n    [10,50.0,14.0,1000.0,250.0,90.0,0,0],\n    [20,50.001,14.001,1100.0,250.0,90.0,0,0]\n  ]\n}",
        );
        let pad = "x".repeat(TYPECODE_PROBE_DECOMPRESSED_BYTES + 64);
        let c172_late = gz(&trace_json(
            "ccc333",
            &format!(r#""desc":"{pad}","t":"C172","#),
        ));
        let no_typecode = gz(&trace_json("ddd444", ""));
        let dir = day_dir_with_tar(&[
            ("traces/11/trace_full_aaa111.json", b738.as_slice()),
            ("traces/22/trace_full_bbb222.json", c172_pretty.as_slice()),
            ("traces/33/trace_full_ccc333.json", c172_late.as_slice()),
            ("traces/44/trace_full_ddd444.json", no_typecode.as_slice()),
        ]);
        let keep_ga = |tc: &str| tc == "C172";
        let (traces, stats) = read_day_traces_filtered(dir.path(), Some(&keep_ga)).unwrap();
        let mut kept: Vec<&str> = traces.iter().map(|t| t.icao24.as_str()).collect();
        kept.sort_unstable();
        assert_eq!(
            kept,
            ["bbb222", "ccc333"],
            "early + late C172 kept; B738 skipped; blank filtered post-parse"
        );
        assert!(traces.iter().all(|t| t.aircraft_type == "C172"));
        assert_eq!(stats.probe_hits, 2, "B738 + pretty C172");
        assert_eq!(
            stats.skipped_pre_parse, 1,
            "B738 dropped without full parse"
        );
        assert_eq!(
            stats.probe_misses, 2,
            "late-t + no-t fell back to full parse"
        );

        // Without a prefilter the same tar yields every trace and zero
        // probe activity — the default path is untouched.
        let (all, no_stats) = read_day_traces_filtered(dir.path(), None).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(no_stats, TypecodeProbeStats::default());
    }

    /// Smoke test against real cached data when available — proves the
    /// parser handles the actual adsb.lol layout, not just synthetic
    /// fixtures. Skips unless QM_FLIGHTS_CACHE points at a radius cache root
    /// with the year-nested 2025/2025-01-21 day dir.
    #[test]
    fn smoke_real_praha_cache() {
        let Ok(root) = std::env::var("QM_FLIGHTS_CACHE") else {
            return;
        };
        let day = Path::new(&root).join("2025/2025-01-21");
        if !day.exists() {
            return;
        }
        let traces = read_day_traces(&day).unwrap();
        assert!(traces.len() > 100, "got only {} traces", traces.len());
        let total_pts: usize = traces.iter().map(|t| t.points.len()).sum();
        assert!(total_pts > 50_000, "got only {total_pts} pts");
        // At least some have a 4-character ICAO typecode (most common case).
        let typed = traces.iter().filter(|t| t.aircraft_type.len() >= 3).count();
        assert!(typed > traces.len() / 2);
        // ≥30 % of traces must carry a callsign. Robust to GA-heavy
        // days where many Mode-S aircraft never broadcast `flight`.
        let with_callsign = traces.iter().filter(|t| !t.callsigns.is_empty()).count();
        assert!(
            with_callsign * 10 > traces.len() * 3,
            "expected ≥30% traces to carry a callsign, got {with_callsign}/{}",
            traces.len()
        );
    }
}

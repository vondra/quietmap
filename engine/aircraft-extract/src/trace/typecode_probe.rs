//! Conservative gzip prefix optimization; an undecidable header always takes the full parser.

use flate2::read::GzDecoder;
use std::io::Read;

/// Decompressed-byte budget for the typecode prefix probe. The readsb
/// `trace_full` header carries `"t":"<typecode>"` at byte ~32 when
/// present (verified on the real 2025 release tree); 512 leaves slack for
/// long `desc` / `ownOp` fields ahead of it while staying orders of
/// magnitude below a full trace's decompressed size.
pub(super) const TYPECODE_PROBE_DECOMPRESSED_BYTES: usize = 512;

/// Inflate at most [`TYPECODE_PROBE_DECOMPRESSED_BYTES`] of a gzipped
/// trace and scan for the `"t":"<typecode>"` header field. `None` is a
/// probe MISS (no `"t"` key, non-string value, value crossing the
/// probe window, undecodable gzip) — callers MUST fall back to the
/// full parse on miss, so
/// a miss can never misclassify a trace.
pub(super) fn probe_typecode_prefix(gz_bytes: &[u8]) -> Option<String> {
    let mut head = [0u8; TYPECODE_PROBE_DECOMPRESSED_BYTES];
    let mut filled = 0usize;
    let mut gz = GzDecoder::new(gz_bytes);
    while filled < head.len() {
        match gz.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            // Truncated/odd gzip → let the full parse path decide.
            Err(_) => return None,
        }
    }
    scan_json_typecode(&head[..filled])
}

/// Only a top-level JSON header key can decide the class-window pass.
pub(super) fn scan_json_typecode(head: &[u8]) -> Option<String> {
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < head.len() {
        match head[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.checked_sub(1)?,
            b'"' => {
                let end = string_end(head, i)?;
                let key = &head[i..end];
                i = end;
                while head.get(i).is_some_and(u8::is_ascii_whitespace) {
                    i += 1;
                }
                if depth == 1 && key == b"\"t\"" && head.get(i) == Some(&b':') {
                    i += 1;
                    while head.get(i).is_some_and(u8::is_ascii_whitespace) {
                        i += 1;
                    }
                    if head.get(i) != Some(&b'"') {
                        return None;
                    }
                    let end = string_end(head, i)?;
                    return serde_json::from_slice(&head[i..end]).ok();
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some(i + 1),
            b'\\' => i += 1,
            _ => {}
        }
        i += 1;
    }
    None
}

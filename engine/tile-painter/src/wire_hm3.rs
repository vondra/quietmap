//! `HM3` binary tile wire format (v3 — the 512@z12 world; v2/256² died with
//! the 2026-07 shift's Phase B).
//!
//! One period (Lden) per cell as a `u8 × 0.5 dB` value (sentinel `255` = no data;
//! the 0.5 dB step is well below the END Annex II ±1 dB field tolerance). The WHOLE
//! file is a single **Brotli** stream of a 6-byte header + the dense 512×512 cell
//! grid, served with HTTP `Content-Encoding: br` so the browser decompresses it
//! natively in its network stack — the client never decodes in JS. Direct file
//! readers (this crate's [`read_tile`], Node tooling) Brotli-decode first, then read
//! the header. Mirror: `frontend/src/lib/hm3-decoder.ts`.
//!
//! Decompressed layout (little-endian, header 6 B):
//! ```text
//! 0:  4  magic "HM3 " (ASCII, trailing space pads to four bytes)
//! 4:  1  version    = 3
//! 5:  1  source_id  (aircraft = 3, road = 1, rail = 2, … ; total = 0) — frontend metadata
//! 6:  …  262144 raw u8 cells (row-major, py*512 + px)
//! ```
//! Brotli already collapses the empty/uniform regions that v1's hand-rolled
//! sparse-row encoding targeted (an empty tile → ~13 B), so the body is plain dense.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{bail, Context, Result};
use brotli::enc::BrotliEncoderParams;

use crate::accumulator::{TileAccumulator, NUM_PERIODS};
use crate::grid::TILE_PX;
use noise_compute::emission::aircraft;
use noise_compute::periods;

pub const MAGIC: &[u8; 4] = b"HM3 ";
/// v3 = 512² cells (the 512@z12 world). v2 (256²) is gone — the last v2
/// producers and stores died with the shift's Phase B (2026-07-09).
pub const VERSION: u8 = 3;
/// Cells per tile, tied to the grid's tile side — one source of truth.
const CELLS: usize = TILE_PX * TILE_PX;
/// Decompressed header length: magic(4) + version(1) + source_id(1).
const HEADER_BYTES: usize = 6;
/// Brotli quality for tile bodies. 9 (not 10): q9→q10 is the optimal-parsing cost cliff — measured
/// on real z13 tiles, q10 is ~2.2× slower to compress than q9 for only ~9 % smaller tiles (and q11
/// ~3× slower than q10 for ~4 % smaller). The combine/pyramid re-encode is Brotli-bound, so q9 roughly
/// halves it. Decode is quality-agnostic, so a map of mixed q9/q10 tiles is fine — old q10 tiles
/// converge to q9 as they are rewritten, no forced re-encode needed.
const BROTLI_QUALITY: i32 = 9;

// Per-layer header discriminator (frontend palette/legend metadata). One
// HM3 tile tree per id under the staging root’s `{layer}/`. `total` is the
// energy-summed combine of every source layer (the default all-on view).
pub const SOURCE_ID_TOTAL: u8 = 0;
pub const SOURCE_ID_ROAD: u8 = 1;
pub const SOURCE_ID_RAIL: u8 = 2;
pub const SOURCE_ID_AIRCRAFT: u8 = 3;
pub const SOURCE_ID_INDUSTRIAL: u8 = 4;
pub const SOURCE_ID_BUILDING: u8 = 5;

/// Sentinel value meaning "no data" — frontend renders transparent.
pub const NO_DATA: u8 = 255;

/// One validated HM3 tile decoded in a single Brotli pass.
#[derive(Debug)]
pub struct DecodedTile {
    pub source_id: u8,
    pub cells: Vec<u8>,
}

/// Quantise a single Lden dB value to `u8 × 0.5 dB`. `NEG_INFINITY`,
/// `NaN`, or values below 0 dB map to the [`NO_DATA`] sentinel.
#[inline]
pub fn quantise_lden(db: f64) -> u8 {
    if !db.is_finite() || db < 0.0 {
        return NO_DATA;
    }
    let q = (db * 2.0).round();
    if q >= 254.0 {
        254
    } else {
        q as u8
    }
}

/// Inverse of [`quantise_lden`]. `NO_DATA` returns `f64::NEG_INFINITY`.
#[inline]
pub fn dequantise_lden(byte: u8) -> f64 {
    if byte == NO_DATA {
        f64::NEG_INFINITY
    } else {
        byte as f64 / 2.0
    }
}

/// Combine the internal 3-period linear energy into one Lden value per
/// cell and quantise. Returns `TILE_PX * TILE_PX` bytes in row-major
/// `(py, px)` order. Cells with zero energy in all periods get
/// [`NO_DATA`].
///
/// Lden combination MUST execute in the linear-energy domain (END
/// 2002/49/EC Annex I) — converting per-period dB then averaging is
/// wrong on cells where one period dominates. We call
/// [`periods::compute_lden`] which already does the linear blend with
/// the +5 dB evening and +10 dB night penalties.
pub fn collapse_lden_u8(acc: &TileAccumulator, n_days_f: f64) -> Vec<u8> {
    // Aircraft store event energy summed over n_days; period_leq divides by
    // `n_days × period_seconds` to recover the period's average power.
    collapse_lden_with(acc, |e, p| {
        aircraft::period_leq(e, n_days_f, aircraft::PERIOD_SECONDS[p])
    })
}

/// Surface-source variant: road / rail / industrial / building emit a
/// **steady continuous level**, so the accumulator already holds each
/// period's A-weighted mean-square pressure (power), NOT an event-energy
/// sum over days. So the period level is `10·log10(power)` directly — with
/// NO `n_days × period_seconds` division (that would suppress a steady
/// source by ~10·log10(period_seconds) ≈ 46 dB). Lden combine is identical.
pub fn collapse_lden_surface_u8(acc: &TileAccumulator) -> Vec<u8> {
    collapse_lden_with(acc, |power, _p| 10.0 * power.log10())
}

/// Shared collapse: per cell, map each period's accumulated linear value to
/// a dB level via `period_db`, then blend to one quantised Lden byte
/// ([`periods::compute_lden`] — the linear-domain 12/4/8-hour + 5/10-penalty
/// END formula). Cells with no energy in any period get [`NO_DATA`].
fn collapse_lden_with(acc: &TileAccumulator, period_db: impl Fn(f64, usize) -> f64) -> Vec<u8> {
    let n_cells = TILE_PX * TILE_PX;
    let mut out = vec![NO_DATA; n_cells];
    // Index loops kept verbatim: `idx` maps a cell to both `out[idx]` and the
    // flat `acc.energy[base + p]` stripe (`base = idx * NUM_PERIODS`), so an
    // enumerate() rewrite would still have to index `acc.energy` by hand. The
    // f64 period sum/order is part of HM3 byte parity — left untouched.
    #[allow(clippy::needless_range_loop)]
    for idx in 0..n_cells {
        let base = idx * NUM_PERIODS;
        let mut periods_db = [f64::NEG_INFINITY; NUM_PERIODS];
        let mut any = false;
        #[allow(clippy::needless_range_loop)]
        for p in 0..NUM_PERIODS {
            let e = acc.energy[base + p] as f64;
            if e > 0.0 {
                periods_db[p] = period_db(e, p);
                any = true;
            }
        }
        if !any {
            continue;
        }
        let lden = periods::compute_lden(periods_db[0], periods_db[1], periods_db[2]);
        out[idx] = quantise_lden(lden);
    }
    out
}

/// Default median-fill window radius (pixels) for AREA layers. The base level
/// is ≈ 19 m/px (equator), so R=3 (a 7×7 ≈ 130 m window) spans both the 30 m
/// building and 75 m industrial
/// discretisation grids — wide enough to capture the inter-point ripple it must
/// smooth. Tuned by eye on Praha.
pub const AREA_FILL_RADIUS_PX: usize = 3;

/// Solidify a discretised AREA source (building / industrial / leisure) on its
/// collapsed Lden tile. The area-grid leaves the footprint as overlapping
/// point disks — bright at each grid point, dimmer in the valleys between —
/// which reads as grain, not one shape. For every cell this RAISES it to the
/// MEDIAN Lden of the data cells in a `(2R+1)²` window when it sits below that
/// median, so:
/// * a ripple valley between two points rises to the typical interior level;
/// * a real LOUDER pixel is never lowered (we only raise);
/// * MEDIAN, not max — a shielded courtyard between two loud points fills to the
///   interior norm, not a false hot plate (this raster also feeds the popup /
///   quiet-zones, so max would lie);
/// * a NO_DATA hole is filled only when ≥75 % of its window is data (deep
///   interior), so the footprint EDGE never grows into the background.
///
/// Display-only: the per-pixel propagation the cells came from is unchanged.
pub fn fill_area_median(cells: &mut [u8], radius: usize) {
    debug_assert_eq!(cells.len(), TILE_PX * TILE_PX);
    // Window every cell against a pre-fill snapshot: we only ever RAISE, and the
    // median reads the original field, so the result is scan-order independent.
    let src = cells.to_vec();
    let side = 2 * radius + 1;
    let mut window: Vec<u8> = Vec::with_capacity(side * side);
    for py in 0..TILE_PX {
        let y0 = py.saturating_sub(radius);
        let y1 = (py + radius).min(TILE_PX - 1);
        for px in 0..TILE_PX {
            let x0 = px.saturating_sub(radius);
            let x1 = (px + radius).min(TILE_PX - 1);
            window.clear();
            let mut window_cells = 0usize;
            for wy in y0..=y1 {
                let row = wy * TILE_PX;
                for wx in x0..=x1 {
                    window_cells += 1;
                    let b = src[row + wx];
                    if b != NO_DATA {
                        window.push(b);
                    }
                }
            }
            if window.len() < 2 {
                continue; // an isolated point — nothing to smooth it into
            }
            window.sort_unstable();
            let med = window[window.len() / 2];
            let idx = py * TILE_PX + px;
            if src[idx] != NO_DATA && med > src[idx] {
                cells[idx] = med; // raise a ripple valley to the interior median
            } else if src[idx] == NO_DATA && window.len() * 4 >= window_cells * 3 {
                cells[idx] = med; // fill a deep-interior NO_DATA hole
            }
        }
    }
}

/// True iff every cell is the [`NO_DATA`] sentinel — THE definition of an
/// empty/silent tile ("present ⟺ audible"), shared by [`write_tile`] and the
/// tile store so the two can never drift.
pub fn is_silent(cells: &[u8]) -> bool {
    cells.iter().all(|&b| b == NO_DATA)
}

/// Maximum encoded tile size, used to reserve output disk before painting.
pub fn maximum_encoded_tile_bytes() -> usize {
    brotli::enc::BrotliEncoderMaxCompressedSize(HEADER_BYTES + CELLS)
}

/// Compose + compress a complete HM3 file image in memory: ONE Brotli
/// stream of `MAGIC + VERSION + source_id + cells` — exactly the bytes a loose
/// `.bin` holds and the byte layout served with `Content-Encoding: br`.
/// Callers that ship tiles without touching the filesystem (the tile store's
/// ship-out path, the pmtiles packer) use this; [`write_tile`] is this +
/// `fs::write`.
pub fn encode_tile_bytes(cells: &[u8], source_id: u8) -> Result<Vec<u8>> {
    assert_eq!(
        cells.len(),
        CELLS,
        "encode_tile_bytes: cell count must be TILE_PX² (programmer error)"
    );
    let mut raw = Vec::with_capacity(HEADER_BYTES + cells.len());
    raw.extend_from_slice(MAGIC);
    raw.push(VERSION);
    raw.push(source_id);
    raw.extend_from_slice(cells);

    let params = BrotliEncoderParams {
        quality: BROTLI_QUALITY,
        ..Default::default()
    };
    let mut payload = Vec::new();
    brotli::BrotliCompress(&mut Cursor::new(&raw), &mut payload, &params)
        .context("brotli encode")?;
    if payload.len() > maximum_encoded_tile_bytes() {
        bail!("encoded tile exceeds the declared output reservation");
    }
    Ok(payload)
}

/// Write a complete HM3 tile to disk ([`encode_tile_bytes`] + `fs::write`).
/// Returns bytes written, or `Ok(0)` if `skip_if_empty` is set and no cell has
/// data.
pub fn write_tile(path: &Path, cells: &[u8], source_id: u8, skip_if_empty: bool) -> Result<usize> {
    if skip_if_empty && is_silent(cells) {
        return Ok(0);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let payload = encode_tile_bytes(cells, source_id)
        .with_context(|| format!("encode {}", path.display()))?;
    fs::write(path, &payload).with_context(|| format!("write {}", path.display()))?;
    Ok(payload.len())
}

/// Decode an HM3 tile file (whole-file Brotli) back to its dense
/// `TILE_PX × TILE_PX` cell array. Brotli-decode FIRST, then check the header.
pub fn read_tile(path: &Path) -> Result<Vec<u8>> {
    Ok(read_tile_decoded(path)?.cells)
}

/// Decode and validate an HM3 tile file, retaining its source discriminator.
pub fn read_tile_decoded(path: &Path) -> Result<DecodedTile> {
    let compressed = fs::read(path).with_context(|| format!("open {}", path.display()))?;
    read_tile_bytes_decoded(&compressed).with_context(|| path.display().to_string())
}

/// [`read_tile`] over an in-memory blob — the same decode+validate for tiles
/// stored outside the filesystem (the tile store, pmtiles entries).
pub fn read_tile_bytes(compressed: &[u8]) -> Result<Vec<u8>> {
    Ok(read_tile_bytes_decoded(compressed)?.cells)
}

/// [`read_tile_bytes`] with the validated HM3 source discriminator.
pub fn read_tile_bytes_decoded(compressed: &[u8]) -> Result<DecodedTile> {
    let mut raw = decode_validated(compressed)?;
    let source_id = raw[5];
    raw.drain(..HEADER_BYTES); // strip the header in place — no second 256 KB alloc
    Ok(DecodedTile {
        source_id,
        cells: raw,
    })
}

/// The `source_id` byte of an HM3 blob — lets a tool derive the layer
/// discriminator from existing tiles instead of taking a CLI flag for it.
pub fn read_tile_bytes_source_id(compressed: &[u8]) -> Result<u8> {
    Ok(decode_validated(compressed)?[5])
}

/// Brotli-decode + validate magic/version/length; returns header + cells.
fn decode_validated(compressed: &[u8]) -> Result<Vec<u8>> {
    let mut raw = Vec::with_capacity(HEADER_BYTES + CELLS);
    let mut cursor = Cursor::new(compressed);
    brotli::BrotliDecompress(&mut cursor, &mut raw).context("brotli decode")?;
    // The decoder returns success the instant it finds ONE complete valid
    // stream — it never checks for leftover input. A corrupt entry whose blob
    // is [valid stream for a DIFFERENT, shorter tile][trailing garbage] would
    // otherwise decode "successfully" and silently return the wrong tile.
    // A well-formed single-stream blob must consume every input byte.
    let unconsumed = compressed.len() - cursor.position() as usize;
    if unconsumed > 0 {
        bail!(
            "HM3 blob: {unconsumed} unconsumed trailing byte(s) after a valid Brotli stream \
             ({} of {} bytes consumed) — likely a length/offset alias onto another tile's blob",
            cursor.position(),
            compressed.len()
        );
    }
    // Length gate BEFORE any header index — a truncated blob must be a clean
    // Err from this Result API, not an index panic.
    if raw.len() < HEADER_BYTES {
        bail!("HM3 blob too short: {} bytes", raw.len());
    }
    if &raw[0..4] != MAGIC {
        bail!("not HM3 (magic = {:?})", &raw[0..4]);
    }
    if raw[4] != VERSION {
        bail!("HM3 version {} ≠ {VERSION} (pre-512-shift blob?)", raw[4]);
    }
    if raw.len() != HEADER_BYTES + CELLS {
        bail!("HM3 size {} ≠ {}", raw.len(), HEADER_BYTES + CELLS);
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompressible_tile_fits_the_declared_output_reservation() {
        let mut state = 1_u32;
        let cells: Vec<u8> = (0..CELLS)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let encoded = encode_tile_bytes(&cells, SOURCE_ID_ROAD).unwrap();
        assert!(encoded.len() <= maximum_encoded_tile_bytes());
        assert_eq!(read_tile_bytes(&encoded).unwrap(), cells);
    }

    #[test]
    fn quantise_round_trip_inside_range() {
        for byte in [0u8, 1, 50, 120, 200, 254] {
            let db = dequantise_lden(byte);
            let back = quantise_lden(db);
            assert_eq!(back, byte, "byte {byte} → {db} dB → {back}");
        }
    }

    #[test]
    fn no_data_sentinel_round_trips() {
        assert_eq!(dequantise_lden(NO_DATA), f64::NEG_INFINITY);
        assert_eq!(quantise_lden(f64::NEG_INFINITY), NO_DATA);
        assert_eq!(quantise_lden(f64::NAN), NO_DATA);
        assert_eq!(quantise_lden(-1.0), NO_DATA);
    }

    #[test]
    fn quantise_saturates_above_range() {
        assert_eq!(quantise_lden(128.0), 254);
        assert_eq!(quantise_lden(200.0), 254);
    }

    #[test]
    fn tile_round_trips_through_brotli() {
        let dir = tempfile::tempdir().unwrap();
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        cells[0] = 100;
        cells[TILE_PX + 5] = 110;
        cells[TILE_PX * 200 + 128] = 120;

        let path = dir.path().join("0.bin");
        let written = write_tile(&path, &cells, SOURCE_ID_AIRCRAFT, false).unwrap();
        assert!(written > 0);
        // The raw file is a Brotli stream — the HM3 magic lives INSIDE it, not at byte 0.
        assert_ne!(&fs::read(&path).unwrap()[..4], MAGIC.as_slice());

        let decoded = read_tile_decoded(&path).unwrap();
        assert_eq!(decoded.source_id, SOURCE_ID_AIRCRAFT);
        assert_eq!(decoded.cells, cells);
        assert_eq!(read_tile(&path).unwrap(), cells);
    }

    #[test]
    fn skip_if_empty_drops_silent_tile() {
        let dir = tempfile::tempdir().unwrap();
        let cells = vec![NO_DATA; TILE_PX * TILE_PX];
        let path = dir.path().join("0.bin");
        let written = write_tile(&path, &cells, SOURCE_ID_AIRCRAFT, true).unwrap();
        assert_eq!(written, 0);
        assert!(!path.exists());
    }

    #[test]
    fn collapse_lden_silent_cell_is_sentinel() {
        let acc = TileAccumulator::new();
        let out = collapse_lden_u8(&acc, 14.0);
        assert_eq!(out.len(), TILE_PX * TILE_PX);
        assert!(out.iter().all(|&b| b == NO_DATA));
    }

    #[test]
    fn collapse_lden_linear_combination() {
        // Stuff equal day/evening/night energy into one cell. The
        // END Lden formula then ≈ Lday + 6.4 dB (the +5 / +10 penalty
        // averaged with the 12 / 4 / 8 hour weights). Roundtrip must
        // not lose more than 0.25 dB (half the quant step). `e` is
        // chosen so the per-period LAeq lands well inside the
        // displayable 0..=127 dB range (= ~55 dB), avoiding the
        // sub-zero clamp.
        let mut acc = TileAccumulator::new();
        let e = 1.0e8_f32;
        for p in 0..NUM_PERIODS {
            acc.add_energy_at(10, 20, p as u8, e);
        }
        let out = collapse_lden_u8(&acc, 14.0);
        let cell = out[10 * TILE_PX + 20];
        assert_ne!(cell, NO_DATA, "non-zero energy must produce a value");
        let lden_decoded = dequantise_lden(cell);
        // Direct reference: compute the per-period dB the same way, then
        // run the canonical formula.
        let ld = aircraft::period_leq(e as f64, 14.0, aircraft::PERIOD_SECONDS[0]);
        let le = aircraft::period_leq(e as f64, 14.0, aircraft::PERIOD_SECONDS[1]);
        let ln = aircraft::period_leq(e as f64, 14.0, aircraft::PERIOD_SECONDS[2]);
        let expected = periods::compute_lden(ld, le, ln);
        assert!(
            (lden_decoded - expected).abs() <= 0.25,
            "decoded {lden_decoded:.3} vs expected {expected:.3}"
        );
    }

    #[test]
    fn surface_collapse_treats_energy_as_steady_power_no_time_division() {
        // A steady 60 dB daytime level: store power = 10^(60/10) in the day
        // period. The surface collapse must read it as a level directly
        // (Lday = 60 dB) → Lden = 10·log10(12·10^6/24) ≈ 57.0 dB. The
        // aircraft collapse (n_days=1) would instead divide by 43 200 s,
        // giving Lday ≈ 13.6 → Lden ≈ 10.6 dB — the ~46 dB suppression bug
        // this layer must NOT have.
        let mut acc = TileAccumulator::new();
        let power = 10f32.powf(60.0 / 10.0); // 1e6
        acc.add_energy_at(10, 20, 0, power); // day period only
        let cell = collapse_lden_surface_u8(&acc)[10 * TILE_PX + 20];
        assert_ne!(cell, NO_DATA);
        let lden = dequantise_lden(cell);
        let expected = periods::compute_lden(60.0, f64::NEG_INFINITY, f64::NEG_INFINITY);
        assert!(
            (lden - expected).abs() <= 0.25,
            "surface Lden {lden:.2} vs {expected:.2}"
        );
        assert!(
            lden > 55.0,
            "steady 60 dB day must stay ~57 dB Lden, got {lden:.2} (time-division bug)"
        );
    }

    #[test]
    fn fill_area_median_smooths_ripple_without_growing_footprint() {
        let mut cells = vec![NO_DATA; TILE_PX * TILE_PX];
        // A 9×9 building patch held at 100, with two ripple valleys (80) and one
        // NO_DATA hole deep inside.
        for py in 10..19 {
            for px in 10..19 {
                cells[py * TILE_PX + px] = 100;
            }
        }
        cells[14 * TILE_PX + 14] = 80; // ripple valley
        cells[13 * TILE_PX + 15] = 80; // ripple valley
        cells[15 * TILE_PX + 13] = NO_DATA; // deep-interior hole
        cells[100 * TILE_PX + 100] = 90; // an isolated lone point far away

        fill_area_median(&mut cells, 3);

        assert_eq!(
            cells[14 * TILE_PX + 14],
            100,
            "ripple valley raised to median"
        );
        assert_eq!(
            cells[13 * TILE_PX + 15],
            100,
            "ripple valley raised to median"
        );
        assert_eq!(cells[15 * TILE_PX + 13], 100, "deep-interior hole filled");
        assert_eq!(
            cells[10 * TILE_PX + 10],
            100,
            "a real value is never lowered"
        );
        // Background one cell off the patch edge: its window is mostly NO_DATA, so
        // the footprint must NOT grow into it.
        assert_eq!(
            cells[9 * TILE_PX + 14],
            NO_DATA,
            "edge background not grown"
        );
        assert_eq!(cells[14 * TILE_PX + 4], NO_DATA, "far background untouched");
        // The lone isolated point has <2 data cells in its window — left as is.
        assert_eq!(cells[100 * TILE_PX + 100], 90, "isolated point preserved");
    }
}

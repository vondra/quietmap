// Small display helpers shared by DetailPopup. Kept pure — no React, no IO.

/** Formats a signed dB value; always prefixes + for positive numbers. */
export function fmt(v: number): string {
  return v > 0 ? `+${v.toFixed(1)}` : v.toFixed(1)
}

/**
 * Lden-style dB value that may be null. Engine emits `f64::NEG_INFINITY`
 * for silent periods (no energy) and serde_json maps non-finite floats to
 * JSON `null`, so any `NoisePeriodsData` field may arrive null at runtime
 * even though the TS type says `number`. Renders "—" for null/undefined,
 * otherwise `12.3 dB`.
 */
export function fmtDb(v: number | null | undefined): string {
  return v == null ? '—' : `${v.toFixed(1)} dB`
}

/** Same null handling as fmtDb but returns just the number — for composing
 *  multi-value strings like "12.3/—/8.7 dB" without unit round-tripping. */
export function fmtDbValue(v: number | null | undefined): string {
  return v == null ? '—' : v.toFixed(1)
}

/** Generic float formatter that renders "—" for null/undefined. The Rust
 *  side serializes non-finite f64 (e.g. NaN, ±Infinity from divisions over
 *  empty datasets) to JSON null, so any per-flight or per-period count
 *  field can arrive null even when typed `number`. */
export function fmtFloat(v: number | null | undefined, digits = 1): string {
  return v == null ? '—' : v.toFixed(digits)
}

/** Rounds to integer and formats with thousands separators. */
export function fmtInt(v: number): string {
  return Math.round(v).toLocaleString('en-US')
}

/** Compact number formatting: 12 345 → "12k", 1 234 567 → "1.2M". */
export function fmtCompact(v: number): string {
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`
  if (v >= 1000) return `${(v / 1000).toFixed(v >= 10000 ? 0 : 1)}k`
  return Math.round(v).toString()
}

/** Meters → kilometers, fixed digits, no unit suffix. Use when the unit
 * lives in the column header to keep cell width minimal. */
export function metersToKm(m: number, digits = 2): string {
  return (m / 1000).toFixed(digits)
}

/** Unix seconds → "YYYY-MM-DD" (UTC). Used for globe.adsbexchange.com
 *  trace deep-links whose `showTrace=` query parameter expects an ISO
 *  date. */
export function unixToIsoDate(unix: number): string {
  return new Date(unix * 1000).toISOString().slice(0, 10)
}

/** Unix seconds → "YYYY-MM-DD HH:MM:SS UTC". Display format for
 *  tooltips that quote the exact takeoff timestamp (drop ms, suffix
 *  "UTC" so users don't read it as local time). */
export function unixToIsoDateTimeUtc(unix: number): string {
  return new Date(unix * 1000).toISOString().replace('T', ' ').replace(/\..+/, ' UTC')
}

import { PROFILE_CLASS } from './profile-class.generated'

/** Flight-tracker trace deep-link for an ICAO 24-bit hex, routed to the
 *  network that actually SOURCED the flight (feeder communities differ, so
 *  the sourcing network's globe is the one guaranteed to hold the trace):
 *  GA + helicopter classes come from the adsb.lol full available-year archive →
 *  adsb.lol globe; every other class comes from the ADSBexchange
 *  first-of-month TAR samples (12 d/yr) → globe.adsbexchange.com. The GA
 *  set mirrors the engine's `is_ga_sampled_class` (npd/mod.rs); class is
 *  taken from the wire `class_name` when present, else derived from the
 *  typecode via the generated PROFILE_CLASS table (same table the extract
 *  uses). Both globes verified to replay per-day traces via
 *  `?icao=…&showTrace=YYYY-MM-DD` (owner + live check, 2026-07-03).
 *  Callers pass raw wire hex (lowercase) or display hex (uppercase);
 *  normalize here so every href is identical. */
export function adsbTraceHref(
  icaoHex: string,
  date?: string | null,
  opts?: { noiseClass?: string | null; typecode?: string | null },
): string {
  const cls = opts?.noiseClass || (opts?.typecode ? PROFILE_CLASS[opts.typecode.toUpperCase()] : undefined)
  const host = cls === 'PROP_C172' || cls === 'HELICOPTER'
    ? 'https://adsb.lol/'
    : 'https://globe.adsbexchange.com/'
  const hex = icaoHex.toUpperCase()
  return date ? `${host}?icao=${hex}&showTrace=${date}` : `${host}?icao=${hex}`
}

/**
 * Build a 2-column table-like text block for native title= tooltips.
 * Renders with monospace columns: label padded, value right-aligned.
 * Use \n joins for multi-line. Uses U+2500 box-drawing character for separators.
 */
export type TableRow = readonly [string, string] | { sep: true } | string

export function txtTable(rows: TableRow[], labelWidth = 22, valueWidth = 11): string {
  return rows
    .map(r => {
      if (typeof r === 'string') return r
      if ('sep' in r) return '─'.repeat(labelWidth) + '  ' + '─'.repeat(valueWidth)
      const [label, value] = r
      return label.padEnd(labelWidth) + '  ' + value.padStart(valueWidth)
    })
    .join('\n')
}

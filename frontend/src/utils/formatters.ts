// Small display helpers of the popup. Kept pure — no React, no IO.

/** Formats a signed dB value; always prefixes + for positive numbers. */
export function fmt(v: number): string {
  return v > 0 ? `+${v.toFixed(1)}` : v.toFixed(1)
}

/** A level: `12.3 dB`, or "—" for silence and for any level at or under the popup's 0 dB display
 *  floor (a night level of -20.1 dB tells nothing a dash does not). */
export function fmtDb(v: number | null | undefined): string {
  return v == null || v <= 0 ? '—' : `${v.toFixed(1)} dB`
}

/** Same as fmtDb but just the number, for strings like "12.3/—/8.7 dB". */
export function fmtDbValue(v: number | null | undefined): string {
  return v == null || v <= 0 ? '—' : v.toFixed(1)
}

/** A loudness in sone to two significant digits: 0.43, 4.3, 43. */
export function fmtSone(sone: number): string {
  if (sone >= 10) return Math.round(sone).toString()
  if (sone >= 1) return sone.toFixed(1)
  return sone.toFixed(2)
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

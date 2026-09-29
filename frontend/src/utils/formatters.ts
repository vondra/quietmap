// Small display helpers of the popup. Kept pure — no React, no IO.

/** Formats a signed dB value; always prefixes + for positive numbers. */
export function fmt(v: number): string {
  return v > 0 ? `+${v.toFixed(1)}` : v.toFixed(1)
}

/** A level that may be null (silence): "—" for null, otherwise `12.3 dB`. */
export function fmtDb(v: number | null | undefined): string {
  return v == null ? '—' : `${v.toFixed(1)} dB`
}

/** Same null handling as fmtDb but returns just the number — for composing
 *  multi-value strings like "12.3/—/8.7 dB" without unit round-tripping. */
export function fmtDbValue(v: number | null | undefined): string {
  return v == null ? '—' : v.toFixed(1)
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

// Small display helpers of the popup. Kept pure — no React, no IO.

/** Formats a signed dB value; always prefixes + for positive numbers. */
export function fmt(v: number): string {
  return v > 0 ? `+${v.toFixed(1)}` : v.toFixed(1)
}

/** A level in dB to a tenth, or "—" for silence and for any level at or under the popup's 0 dB
 *  display floor (a night level of -20.1 dB tells nothing a dash does not). */
export function fmtDbValue(v: number | null | undefined): string {
  return v == null || v <= 0 ? '—' : v.toFixed(1)
}

/** A loudness in sone to two significant digits: 0.43, 4.3, 43. */
export function fmtSone(sone: number): string {
  if (sone >= 10) return Math.round(sone).toString()
  if (sone >= 1) return sone.toFixed(1)
  return sone.toFixed(2)
}

/** Shares of a whole as whole percents adding up to 100: each rounded down, the largest
 *  remainders up (rounded one by one, thirty rows of 1.6 % and one of 52 % would read 112 %). */
export function wholePercents(shares: number[]): number[] {
  const total = shares.reduce((sum, share) => sum + share, 0)
  if (!(total > 0)) return shares.map(() => 0)
  const exact = shares.map(share => (100 * share) / total)
  const percents = exact.map(Math.floor)
  const left = 100 - percents.reduce((sum, percent) => sum + percent, 0)
  exact
    .map((value, row) => ({ remainder: value - percents[row], row }))
    .sort((a, b) => b.remainder - a.remainder || a.row - b.row)
    .slice(0, left)
    .forEach(({ row }) => { percents[row] += 1 })
  return percents
}

/** A whole percent of the whole; under one "<1 %". */
export function fmtPercent(percent: number): string {
  return percent < 1 ? '<1 %' : `${percent} %`
}

/** Rounds to integer and formats with thousands separators. */
export function fmtInt(v: number): string {
  return Math.round(v).toLocaleString('en-US')
}

/** A rate or a mean count as the popup writes every one (flights a day, vessels at a time): whole
 *  from ten, one decimal from one, below one its first significant digit, so one flight a year
 *  reads 0.003 a day; under a thousandth "<0.001". */
export function fmtCount(count: number): string {
  if (count >= 9.95) return fmtInt(count)
  if (count >= 0.995) return count.toFixed(1).replace(/\.0$/, '')
  return count >= 0.001 ? String(Number(count.toPrecision(1))) : '<0.001'
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

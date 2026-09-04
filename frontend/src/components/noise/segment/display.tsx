import type { ReactNode } from 'react'

// Display primitives shared by the §1…§6 sections of the segment detail view
// (SegmentExpanded). Kept here so each section file stays focused on its own
// computation rather than re-deriving layout/formatting.

const BAND_FREQS = [63, 125, 250, 500, 1000, 2000, 4000, 8000] as const
export const BAND_LABELS = ['63 Hz', '125 Hz', '250 Hz', '500 Hz', '1 kHz', '2 kHz', '4 kHz', '8 kHz'] as const

/** Multi-line monospace tooltip body listing per-octave-band values. */
export function bandsTooltip(
  bands: readonly (number | null)[],
  {
    title,
    signed = true,
    note,
  }: { title: string; signed?: boolean; note?: string } = { title: '' },
) {
  // Period-keyed bands (lw_bands.day, etc.) are null when the period had no
  // emission — runway segments often have day/evening null with night-only
  // data. Render "—" instead of crashing on null.toFixed.
  const lines = bands.map((v, i) => {
    const label = BAND_LABELS[i] ?? `${BAND_FREQS[i]} Hz`
    if (v == null) return `  ${label.padEnd(8)} ${'—'.padStart(7)}`
    const sign = signed && v > 0 ? '+' : ''
    return `  ${label.padEnd(8)} ${sign}${v.toFixed(2).padStart(7)} dB`
  })
  const header = title ? `${title}\n\n` : ''
  const footer = note ? `\n\n${note}` : ''
  return `${header}${lines.join('\n')}${footer}`
}

/**
 * Signed dB formatter with configurable digits; "—" for non-finite values.
 * Distinct from utils/formatters.fmtDb (unsigned, nullable, for absolute Lden):
 * this one prefixes "+" for positive deltas — path-effect / surface corrections
 * read as "+2.3 dB" / "−1.5 dB".
 */
export function fmtDbSigned(v: number, { signed = true, digits = 1 } = {}): string {
  if (!Number.isFinite(v)) return '—'
  const sign = signed && v > 0 ? '+' : ''
  return `${sign}${v.toFixed(digits)} dB`
}

/** Bare visual block — no header. Spacing alone separates the §1…§6 sections. */
export function Section({ children }: { children: ReactNode }) {
  return <div className="mt-3">{children}</div>
}

/** Two-column key/value grid used by the source + path-effect sections. */
export function InlineTable({ rows }: { rows: [string | ReactNode, ReactNode][] }) {
  return (
    <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
      {rows.map(([k, v], i) => (
        <div key={i} className="contents">
          <span className="text-muted-foreground/70 whitespace-nowrap">{k}</span>
          {/* `min-w-0` makes the 1fr cell honour its grid track size when the
             content includes long unbreakable runs ("Average NPD 42% · B738
             28% · A320 13%"). Without it the cell expands and overflows the
             320 px side panel. */}
          <span className="min-w-0 text-foreground text-right tabular-nums break-words">{v}</span>
        </div>
      ))}
    </div>
  )
}

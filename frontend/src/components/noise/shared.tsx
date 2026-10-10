// The popup's one look, shared by its rows, their details and the detailed calculation: one font
// with tabular figures, a value in the foreground colour and its label muted, a table's column names
// fainter, a dotted underline for a hover tip and for nothing else, a chevron on whatever opens.
// Display fields as text, distances, label-value lines and tables.
import type { ReactNode } from 'react'
import { ChevronDown } from 'lucide-react'

/** The text of an opened row, piece or section. */
export const DETAIL_TEXT = 'text-[11px] leading-snug text-muted-foreground tabular-nums'
/** A table's column names. */
export const COLUMN_NAME = 'text-[10px] font-normal text-muted-foreground/70'
/** What a table or a section is, above it. */
export const CAPTION = 'text-[10px] font-medium uppercase tracking-[0.06em] text-muted-foreground'

/** Any display field as text, whatever its shape. */
export function fieldText(value: unknown): string {
  if (Array.isArray(value)) return value.map(fieldText).join(', ')
  if (value !== null && typeof value === 'object') return JSON.stringify(value)
  return String(value)
}

export function formatDist(m: number): string {
  if (m === 0) return 'overhead'
  if (m < 1000) return `${m} m`
  return `${(m / 1000).toFixed(1)} km`
}

/** A label and its value on one line, the value right-aligned. */
export function lineRow(label: ReactNode, value: ReactNode) {
  return (
    <div className="flex justify-between gap-3">
      <span className="shrink-0">{label}</span>
      <span className="min-w-0 text-right text-foreground">{value}</span>
    </div>
  )
}

/** A table: its caption, its column names (a unit after the name), then its rows, each row's first
 *  cell its name and every other cell a value, right-aligned so the values read down. */
export function DetailTable({ caption, head, rows }: {
  caption?: ReactNode
  head?: ReactNode[]
  rows: ReactNode[][]
}) {
  return (
    <table className="w-full">
      {caption && <caption className={`${CAPTION} pb-0.5 text-left`}>{caption}</caption>}
      {head && (
        <thead>
          <tr className={COLUMN_NAME}>
            {head.map((cell, k) => (
              <th key={k} scope="col" className={`pb-px font-normal ${k ? 'pl-2 text-right' : 'text-left'}`}>{cell}</th>
            ))}
          </tr>
        </thead>
      )}
      <tbody>
        {rows.map((row, r) => (
          <tr key={r}>
            {row.map((cell, k) => <td key={k} className={k ? 'pl-2 text-right text-foreground' : 'whitespace-nowrap text-left'}>{cell}</td>)}
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** The mark of whatever opens in place: a chevron, turned up while open. */
export function Chevron({ open }: { open: boolean }) {
  return (
    <ChevronDown
      aria-hidden="true"
      className={`size-3 shrink-0 self-center text-muted-foreground/50 ${open ? 'rotate-180' : ''}`}
    />
  )
}

/** Period labels with their hours, for tables and tooltips. */
export const PERIOD_LABELS_DETAIL = [
  'Day (07–19)',
  'Evening (19–23)',
  'Night (23–07)',
] as const

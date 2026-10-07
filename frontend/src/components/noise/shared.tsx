// Shared by the popup's contributor rows: display fields as text, distances, label-value rows.
import type { ReactNode } from 'react'

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

export function lineRow(label: ReactNode, value: ReactNode, muted?: boolean) {
  return (
    <div className={`flex justify-between gap-3 ${muted ? 'text-muted-foreground/40' : ''}`}>
      <span className="shrink-0">{label}</span>
      <span className={`text-right ${muted ? '' : 'text-foreground'}`}>{value}</span>
    </div>
  )
}

/** Period labels with their hours, for tables and tooltips. */
export const PERIOD_LABELS_DETAIL = [
  'Day (07–19)',
  'Evening (19–23)',
  'Night (23–07)',
] as const

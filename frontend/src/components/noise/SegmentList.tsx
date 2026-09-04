import { useMemo, useState } from 'react'
import {
  AIRCRAFT_SUBTYPE,
  type SegmentKind,
  type SegmentTrace,
  type SegmentTracesSummary,
} from '../../types/noise'
import { SegmentRow } from './SegmentRow'
import { HoverText } from '../ui/info-tip'

// Labels are abbreviated to fit a single row in a ~360 px popup; the
// long form lives in the title tooltip so users can still identify the
// kind without guessing. Wrapping to two rows looked broken.
const KIND_FILTERS: { key: SegmentKind; label: string; longName: string }[] = [
  { key: 'road', label: 'Road', longName: 'Road traffic' },
  { key: 'railway', label: 'Rail', longName: 'Railway' },
  { key: 'aircraft_ground', label: 'Ground', longName: 'Aircraft ground ops' },
  { key: 'aircraft_airborne', label: 'Airborne', longName: 'Aircraft airborne' },
  { key: 'aircraft_cruise', label: 'Cruise', longName: 'Aircraft cruise' },
  { key: 'building', label: 'Building', longName: 'Buildings' },
  { key: 'industrial', label: 'Industry', longName: 'Industrial sites' },
]

const kindMap = (fn: (k: SegmentKind) => boolean): Record<SegmentKind, boolean> =>
  Object.fromEntries(KIND_FILTERS.map(f => [f.key, fn(f.key)])) as Record<SegmentKind, boolean>

type UnifiedEntry = { trace: SegmentTrace; sortKey: number }

function traceKind(t: SegmentTrace): SegmentKind {
  if (t.kind === 'aircraft') {
    switch (t.aircraft_subtype) {
      case AIRCRAFT_SUBTYPE.GROUND:
        return 'aircraft_ground'
      case AIRCRAFT_SUBTYPE.AIRBORNE:
        return 'aircraft_airborne'
      case AIRCRAFT_SUBTYPE.CRUISE:
        return 'aircraft_cruise'
      default:
        // Unknown subtype — backend-side schema drift. Fall back to
        // ground so the row still renders and console-warn so
        // operators see the regression in DevTools.
        if (typeof console !== 'undefined') {
          console.warn(`Unknown aircraft_subtype ${t.aircraft_subtype}; defaulting to ground.`)
        }
        return 'aircraft_ground'
    }
  }
  return t.kind as SegmentKind
}

// Unique React key. Point sources (buildings, industrial sites) are
// discretized into multiple grid points sharing one parent osm_id and
// segment_idx=0; aircraft ground ops emit the same physical segment
// per movement-kind so even (osm_id, segment_idx, cp_lat, cp_lon) can
// repeat with different `received_lden`. Including the received Lden
// keeps each row's React identity unique across all known engine
// duplicates.
function segmentRowKey(t: SegmentTrace): string {
  const rl = t.received_lden.full
  const rlKey = Number.isFinite(rl) ? rl.toFixed(2) : 'na'
  return `s-${t.osm_id ?? 'pt'}-${t.segment_idx}-${t.cp_lat.toFixed(6)},${t.cp_lon.toFixed(6)}-${rlKey}`
}

const META_FIELD: Record<SegmentKind, { count: keyof SegmentTracesSummary; total: keyof SegmentTracesSummary }> = {
  road: { count: 'road_count', total: 'road_total' },
  railway: { count: 'railway_count', total: 'railway_total' },
  aircraft_ground: { count: 'aircraft_ground_count', total: 'aircraft_ground_total' },
  aircraft_airborne: { count: 'aircraft_airborne_count', total: 'aircraft_airborne_total' },
  aircraft_cruise: { count: 'aircraft_cruise_count', total: 'aircraft_cruise_total' },
  building: { count: 'building_count', total: 'building_total' },
  industrial: { count: 'industrial_count', total: 'industrial_total' },
}

function metaCount(meta: SegmentTracesSummary | null | undefined, kind: SegmentKind, field: 'count' | 'total'): number {
  return (meta?.[META_FIELD[kind][field]] as number | undefined) ?? 0
}

export function SegmentList({
  segments,
  meta,
  onHighlight,
  onShowAll,
  loadingFull = false,
}: {
  segments: SegmentTrace[]
  meta: SegmentTracesSummary | null
  onHighlight?: (geometry: unknown | null) => void
  onShowAll?: () => void | Promise<void>
  loadingFull?: boolean
}) {
  const [enabled, setEnabled] = useState<Record<SegmentKind, boolean>>(() => kindMap(() => true))

  // Sort once when `segments` arrive — `enabled` toggles only filter
  // the pre-sorted list and don't touch the sort cost.
  const sortedSegments = useMemo<UnifiedEntry[]>(
    () =>
      segments
        .map(s => ({ trace: s, sortKey: s.received_lden.full }))
        .sort((x, y) => y.sortKey - x.sortKey),
    [segments],
  )

  const entries = useMemo<UnifiedEntry[]>(
    () => sortedSegments.filter(e => enabled[traceKind(e.trace)]),
    [sortedSegments, enabled],
  )

  // Strip should reflect the active toggle selection, not the global total.
  let shownCount = 0
  let totalCount = 0
  for (const { key } of KIND_FILTERS) {
    if (!enabled[key]) continue
    shownCount += metaCount(meta, key, 'count')
    totalCount += metaCount(meta, key, 'total')
  }
  const truncated = totalCount > shownCount

  return (
    <div>
      <div className="flex mt-1 mb-1.5 whitespace-nowrap text-[11px] bg-muted/30 rounded py-1 -mx-1 divide-x divide-foreground/25 overflow-x-auto">
        {KIND_FILTERS.map(({ key, label, longName }) => {
          const kindCount = metaCount(meta, key, 'count')
          const kindTotal = metaCount(meta, key, 'total')
          // Empty kinds stay visible (greyed) so the user always sees
          // the full filter set — disappearing tabs on a sparse popup
          // looked unpredictable. Greyed-out + count=0 makes the
          // empty state explicit instead of hiding it.
          const isEmpty = kindCount === 0
          const on = enabled[key] && !isEmpty
          // "Isolated" = this kind is the only one enabled (everything
          // else is off). A second click in that state restores all.
          const isolated = on && KIND_FILTERS.every(({ key: k }) => k === key ? enabled[k] : !enabled[k])
          const countLabel = kindTotal > kindCount ? `${kindCount} of ${kindTotal}` : `${kindCount}`
          const handleClick = (e: React.MouseEvent) => {
            if (e.shiftKey) {
              // Shift-click toggles just this kind, keeping other
              // selections — useful when narrowing down 2-3 kinds.
              setEnabled(prev => ({ ...prev, [key]: !prev[key] }))
              return
            }
            // Plain click isolates this kind; re-clicking the isolated
            // kind restores the full set.
            setEnabled(isolated ? kindMap(() => true) : kindMap(k => k === key))
          }
          const titleAction =
            isEmpty  ? ' (no data at this point)'
          : isolated ? ' (click to show all kinds)'
          : on       ? ' (click to show only this; shift-click to hide)'
          :            ' (click to show only this; shift-click to add)'
          return (
            <button
              key={key}
              type="button"
              disabled={isEmpty}
              aria-pressed={on}
              onClick={handleClick}
              title={`${longName} — ${countLabel} segment${kindTotal === 1 ? '' : 's'}${titleAction}`}
              className={`shrink-0 px-1 transition-colors ${
                isEmpty
                  ? 'text-muted-foreground/30 cursor-not-allowed'
                  : on
                    ? 'text-foreground hover:text-foreground/80'
                    : 'text-muted-foreground/40 line-through hover:text-foreground'
              }`}
            >
              {label}
            </button>
          )
        })}
      </div>
      <div>
        {entries.map(e => (
          <SegmentRow
            key={segmentRowKey(e.trace)}
            trace={e.trace}
            onHighlight={onHighlight}
          />
        ))}
      </div>
      {truncated && (
        <div className="flex items-center justify-between border-t border-border/40 py-1 text-[10px] text-muted-foreground">
          <HoverText title={'Segments ranked by their energy contribution at this point. "Total" counts segments with a measurable contribution — for aircraft, sub-segments whose peak is below the ~25 dB audibility cutoff are omitted (their energy is negligible, far below the displayed ranks), so this is the loudest set, not every Doc 29 evaluation. Energy-summing the returned rows approximates the source Lden (modulo the display cap); "Show all" raises the cap.'}>
            <span>Showing {shownCount.toLocaleString()} of {totalCount.toLocaleString()}</span>
          </HoverText>
          <button
            disabled={loadingFull || !onShowAll}
            onClick={() => void onShowAll?.()}
            className="px-1.5 py-0.5 border border-border/60 rounded hover:bg-foreground/[0.03] disabled:text-muted-foreground/50 disabled:cursor-not-allowed"
            title="Fetches the full segment list — can be several MB at airport points."
          >
            {loadingFull ? 'Loading…' : 'Show all'}
          </button>
        </div>
      )}
    </div>
  )
}


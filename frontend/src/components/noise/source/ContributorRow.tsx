// One row of the popup's source list: label, distance and level; expands to its details. A
// contributor has one, and so has the aircraft layer as a whole, which lists no contributors.
import { useState, type ReactNode } from 'react'
import type { Contributor, LayerLevels, PeriodLevels, TopFlight } from '../../../types/noise'
import { ldenToColor } from '../../../utils/noise-colors'
import { fmtDb, txtTable } from '../../../utils/formatters'
import { DataPoint } from '../noise-tooltips'
import { contributorLabel, formatDist, PERIOD_LABELS_DETAIL, SOURCE_LABELS } from '../shared'
import { AircraftLayerDetail, ContributorDetail } from './ContributorDetail'

function SourceRow({ label, distance, received, onToggle, children }: {
  label: string
  distance: string
  received: PeriodLevels
  /** Told when the row opens or closes (a tap on a phone as well as a click). */
  onToggle?: (expanded: boolean) => void
  /** The expanded body. */
  children: ReactNode
}) {
  // Keyed by a stable id, so the row stays open across streamed updates.
  const [expanded, setExpanded] = useState(false)
  const lden = received.lden ?? 0
  const periodsText = txtTable([
    [PERIOD_LABELS_DETAIL[0], fmtDb(received.ld)],
    [PERIOD_LABELS_DETAIL[1], fmtDb(received.le)],
    [PERIOD_LABELS_DETAIL[2], fmtDb(received.ln)],
    { sep: true },
    ['→ Lden', fmtDb(received.lden)],
  ], 16, 9)

  return (
    <div className="border-b border-border/50 last:border-b-0">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={(e) => {
          e.stopPropagation()
          setExpanded(!expanded)
          onToggle?.(!expanded)
        }}
        className="w-full py-1.5 text-left cursor-pointer hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <div className="flex items-baseline gap-1.5 text-xs px-0">
          <span className="font-medium truncate flex-1">{label}</span>
          <span className="text-muted-foreground/60 shrink-0 w-14 text-right tabular-nums">
            {distance}
          </span>
          <span
            className="font-bold shrink-0 w-14 text-right tabular-nums"
            style={{ color: ldenToColor(lden) }}
          >
            <DataPoint title="Received level per period" text={periodsText}>
              {lden.toFixed(1)} dB
            </DataPoint>
          </span>
          <span className="text-[10px] text-muted-foreground/40 shrink-0">
            {expanded ? '▲' : '▼'}
          </span>
        </div>
      </button>

      {expanded && children}
    </div>
  )
}

/** A source's row; opening it shows the source on the map. */
export function ContributorRow({ c, onHighlight }: { c: Contributor, onHighlight?: (id: string | null) => void }) {
  return (
    <SourceRow
      label={contributorLabel(c)}
      distance={formatDist(c.distance_m)}
      received={c.received}
      onToggle={open => onHighlight?.(open ? c.id : null)}
    >
      <ContributorDetail c={c} />
    </SourceRow>
  )
}

/** Flights pass at every distance: the layer's row has none. */
export function AircraftLayerRow({ layer, flights, onHighlightFlight }: {
  layer: LayerLevels
  flights: TopFlight[]
  onHighlightFlight: (key: string | null) => void
}) {
  return (
    <SourceRow label={SOURCE_LABELS.aircraft} distance="" received={layer}>
      <AircraftLayerDetail received={layer} flights={flights} onHighlightFlight={onHighlightFlight} />
    </SourceRow>
  )
}

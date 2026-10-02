// One row of the popup's "what you hear" list: the source, how far, how it is heard and its share
// of the noise; it expands to its details. A contributor has one, and so has the aircraft layer as a
// whole, which lists no contributors.
import { useState, type ReactNode } from 'react'
import type { Contributor, LayerLevels, PeriodLevels, TopFlight } from '../../../types/noise'
import { ldenToColor } from '../../../utils/noise-colors'
import { fmtDb, txtTable } from '../../../utils/formatters'
import { DataPoint } from '../noise-tooltips'
import { heardText } from '../heard'
import { contributorLabel, formatDist, PERIOD_LABELS_DETAIL, SOURCE_LABELS } from '../shared'
import { AircraftLayerDetail, ContributorDetail } from './ContributorDetail'

/** A share of the noise in whole percent, or "<1 %". */
function percentText(share: number): string {
  const percent = 100 * share
  return percent < 0.5 ? '<1 %' : `${Math.round(percent)} %`
}

function SourceRow({ label, distance, heard, received, totalLden, onToggle, children }: {
  label: string
  distance: string
  /** How it is heard, in words; none for a steady source. */
  heard: string | null
  received: PeriodLevels
  /** The click's Lden: the row's share of the noise is its Lden energy's. */
  totalLden: number
  /** Told when the row opens or closes (a tap on a phone as well as a click). */
  onToggle?: (expanded: boolean) => void
  /** The expanded body. */
  children: ReactNode
}) {
  // Keyed by a stable id, so the row stays open across streamed updates.
  const [expanded, setExpanded] = useState(false)
  const lden = received.lden ?? 0
  const share = Math.min(1, 10 ** ((lden - totalLden) / 10))
  const shareText = txtTable([
    ['Share of the noise', percentText(share)],
    '',
    [PERIOD_LABELS_DETAIL[0], fmtDb(received.ld)],
    [PERIOD_LABELS_DETAIL[1], fmtDb(received.le)],
    [PERIOD_LABELS_DETAIL[2], fmtDb(received.ln)],
    { sep: true },
    ['→ Lden', fmtDb(received.lden)],
    '',
    'Its share of the sound energy of the',
    'day, evening and night here (Lden).',
  ], 18, 9)

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
        <div className="grid grid-cols-[minmax(0,1fr)_3.5rem_2.3rem_0.6rem] gap-x-1.5 items-center text-xs">
          <span className="truncate">
            <span className="font-medium">{label}</span>
            {distance && <span className="text-muted-foreground/60"> · {distance}</span>}
          </span>
          <DataPoint title={label} text={shareText}>
            <span className="block h-1.5 rounded-full bg-muted overflow-hidden" aria-hidden="true">
              <span className="block h-full rounded-full" style={{ width: `${Math.max(3, 100 * share)}%`, background: ldenToColor(lden) }} />
            </span>
          </DataPoint>
          <span className="text-right tabular-nums text-muted-foreground">{percentText(share)}</span>
          <span className="text-[10px] text-muted-foreground/40">{expanded ? '▲' : '▼'}</span>
          {heard && <span className="col-span-4 truncate text-[11px] text-muted-foreground">{heard}</span>}
        </div>
      </button>

      {expanded && children}
    </div>
  )
}

/** A source's row; opening it shows the source on the map. */
export function ContributorRow({ c, totalLden, onHighlight }: {
  c: Contributor
  totalLden: number
  onHighlight?: (id: string | null) => void
}) {
  return (
    <SourceRow
      label={contributorLabel(c)}
      distance={formatDist(c.distance_m)}
      heard={heardText(c.source_type, c.heard)}
      received={c.received}
      totalLden={totalLden}
      onToggle={open => onHighlight?.(open ? c.id : null)}
    >
      <ContributorDetail c={c} />
    </SourceRow>
  )
}

/** Flights pass at every distance: the layer's row has none. */
export function AircraftLayerRow({ layer, totalLden, flights, onHighlightFlight }: {
  layer: LayerLevels
  totalLden: number
  flights: TopFlight[]
  onHighlightFlight: (key: string | null) => void
}) {
  return (
    <SourceRow label={SOURCE_LABELS.aircraft} distance="" heard={null} received={layer} totalLden={totalLden}>
      <AircraftLayerDetail received={layer} flights={flights} onHighlightFlight={onHighlightFlight} />
    </SourceRow>
  )
}

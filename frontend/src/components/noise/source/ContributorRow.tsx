// One row of the popup's "what you hear" list, in columns: the source, why it is loud (how often it
// passes, or steady), how far and its share of the loud moments; it expands to its details. A
// contributor has one, the aircraft layer as a whole has one (it lists no contributors), and so has
// everything the list does not name.
import { useState, type ReactNode } from 'react'
import type { Contributor, LayerLevels, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtDb, txtTable } from '../../../utils/formatters'
import { DataPoint } from '../noise-tooltips'
import { flightsText, heardText } from '../heard'
import { contributorLabel, SOURCE_LABELS } from '../labels'
import { formatDist, PERIOD_LABELS_DETAIL } from '../shared'
import { AircraftLayerDetail, ContributorDetail } from './ContributorDetail'

/** A row's level of the loud moments: the popup's (`null`: none, a rare event), or its Lden before
 *  the final update brings it. */
export function loudLevel(loud: number | null | undefined, lden: number | null | undefined): number {
  return loud === undefined ? (lden ?? -Infinity) : (loud ?? -Infinity)
}

/** A share in whole percent, or "<1 %". */
function percentText(share: number): string {
  const percent = 100 * share
  return percent < 0.5 ? '<1 %' : `${Math.round(percent)} %`
}

function SourceRow({ label, distance, heard, received, loud, loudTotal, onToggle, children }: {
  label: string
  distance: string
  /** Why it is loud, short: how often it passes, or steady; none for a source without passes. */
  heard: string | null
  received: PeriodLevels
  /** The row's level exceeded 5 % of the time by itself (Lden-weighted), and the energy sum of
   *  the list's: its share of the loud moments. */
  loud: number
  loudTotal: number
  /** Told when the row opens or closes (a tap on a phone as well as a click). */
  onToggle?: (expanded: boolean) => void
  /** The expanded body; none for a row that does not open. */
  children?: ReactNode
}) {
  // Keyed by a stable id, so the row stays open across streamed updates.
  const [expanded, setExpanded] = useState(false)
  const share = Math.min(1, 10 ** ((loud - loudTotal) / 10))
  const shareText = txtTable([
    ['Share of the loud moments', percentText(share)],
    '',
    [PERIOD_LABELS_DETAIL[0], fmtDb(received.ld)],
    [PERIOD_LABELS_DETAIL[1], fmtDb(received.le)],
    [PERIOD_LABELS_DETAIL[2], fmtDb(received.ln)],
    { sep: true },
    ['→ Lden', fmtDb(received.lden)],
    '',
    'How much of the loudest 5 % of the',
    'time comes from it: a car every few',
    'hours counts little, steady traffic',
    'and frequent flights a lot.',
  ], 18, 9)

  return (
    <div className="border-b border-border/50 last:border-b-0">
      <button
        type="button"
        aria-expanded={children ? expanded : undefined}
        disabled={!children}
        onClick={(e) => {
          e.stopPropagation()
          setExpanded(!expanded)
          onToggle?.(!expanded)
        }}
        className="w-full py-1.5 text-left enabled:cursor-pointer enabled:hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        {/* What it is, how much of it (why it is loud), how far (context, grey) and its share (the
            result, bold); every number right-aligned in its own column, so they read down. */}
        <div className="grid grid-cols-[minmax(0,1fr)_5.8rem_3rem_2.5rem_0.6rem] gap-x-2 items-baseline text-xs">
          <span className="truncate font-medium">{label}</span>
          <span className="truncate text-right tabular-nums">{heard}</span>
          <span className="text-right tabular-nums text-muted-foreground/60">{distance}</span>
          <DataPoint title={label} text={shareText}>
            <span className="block text-right tabular-nums font-semibold">{percentText(share)}</span>
          </DataPoint>
          <span className="text-[10px] text-muted-foreground/40">{children ? (expanded ? '▲' : '▼') : ''}</span>
        </div>
      </button>

      {expanded && children}
    </div>
  )
}

/** A source's row; opening it shows the source on the map. */
export function ContributorRow({ c, loudTotal, onHighlight }: {
  c: Contributor
  loudTotal: number
  onHighlight?: (id: string | null) => void
}) {
  return (
    <SourceRow
      label={contributorLabel(c)}
      distance={formatDist(c.distance_m)}
      heard={heardText(c.source_type, c.heard)}
      received={c.received}
      loud={loudLevel(c.loud_lden, c.received_lden)}
      loudTotal={loudTotal}
      onToggle={open => onHighlight?.(open ? c.id : null)}
    >
      <ContributorDetail c={c} />
    </SourceRow>
  )
}

/** Flights pass at every distance: the layer's row has none. */
export function AircraftLayerRow({ layer, loudTotal, flights, onHighlightFlight }: {
  layer: LayerLevels
  loudTotal: number
  flights: TopFlight[]
  onHighlightFlight: (key: string | null) => void
}) {
  return (
    <SourceRow label={SOURCE_LABELS.aircraft} distance="" heard={flightsText(layer.events)} received={layer} loud={loudLevel(layer.loud_lden, layer.lden)} loudTotal={loudTotal}>
      <AircraftLayerDetail received={layer} kinds={layer.kinds} events={layer.events} flights={flights} onHighlightFlight={onHighlightFlight} />
    </SourceRow>
  )
}

/** Everything the list does not name: the sources cut from it or under 0 dB. Its share is their
 *  loud moments. */
export function RestRow({ levels, loud, loudTotal }: {
  levels: PeriodLevels
  /** Its loud moments: its share of the list's is its row's. */
  loud: number
  loudTotal: number
}) {
  return (
    <SourceRow
      label="Everything else"
      distance=""
      heard={null}
      received={levels}
      loud={loud}
      loudTotal={loudTotal}
    />
  )
}

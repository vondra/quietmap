// One row of the popup's "what you hear" list, in columns: the source, why it is loud (how often it
// passes, or steady), how far and how loud it is alone over the day; it expands to its details. A
// contributor has one, the aircraft layer as a whole has one (it lists no contributors), and so has
// everything the list does not name.
import { useState, type ReactNode } from 'react'
import type { Contributor, LayerLevels, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtDb, fmtSone, txtTable } from '../../../utils/formatters'
import { DataPoint } from '../noise-tooltips'
import { flightsText, heardText } from '../heard'
import { contributorLabel, SOURCE_LABELS } from '../labels'
import { formatDist, PERIOD_LABELS_DETAIL } from '../shared'
import { AircraftLayerDetail, ContributorDetail } from './ContributorDetail'

/** A row's order in the list: its own Nden, or before the final update its Lden. */
export function rowRank(nden: number | undefined, lden: number | null | undefined): number {
  return nden ?? (lden == null ? -Infinity : 10 ** (lden / 10))
}

function SourceRow({ label, distance, heard, received, nden, onToggle, children }: {
  label: string
  distance: string
  /** Why it is loud, short: how often it passes, or steady; none for a source without passes. */
  heard: string | null
  received: PeriodLevels
  /** The row's own Nden in sone, alone: how loud it is over the day (undefined before the final
   *  update, null when it cannot be told). */
  nden: number | null | undefined
  /** Told when the row opens or closes (a tap on a phone as well as a click). */
  onToggle?: (expanded: boolean) => void
  /** The expanded body; none for a row that does not open. */
  children?: ReactNode
}) {
  // Keyed by a stable id, so the row stays open across streamed updates.
  const [expanded, setExpanded] = useState(false)
  const ndenText = txtTable([
    ['Alone over the day', nden == null ? '—' : `${fmtSone(nden)} sone`],
    '',
    [PERIOD_LABELS_DETAIL[0], fmtDb(received.ld)],
    [PERIOD_LABELS_DETAIL[1], fmtDb(received.le)],
    [PERIOD_LABELS_DETAIL[2], fmtDb(received.ln)],
    { sep: true },
    ['→ Lden', fmtDb(received.lden)],
    '',
    'How loud it sounds by itself, on',
    'average over the day (Nden): a car',
    'every few hours counts little, a',
    'steady hum all of its loudness.',
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
        {/* What it is, how much of it (why it is loud), how far (context, grey) and its loudness (the
            result, bold); every number right-aligned in its own column, so they read down. */}
        <div className="grid grid-cols-[minmax(0,1fr)_5.8rem_3rem_2.5rem_0.6rem] gap-x-2 items-baseline text-xs">
          <span className="truncate font-medium">{label}</span>
          <span className="truncate text-right tabular-nums">{heard}</span>
          <span className="text-right tabular-nums text-muted-foreground/60">{distance}</span>
          <DataPoint title={label} text={ndenText}>
            <span className="block text-right tabular-nums font-semibold">{nden === undefined ? '…' : nden === null ? '—' : fmtSone(nden)}</span>
          </DataPoint>
          <span className="text-[10px] text-muted-foreground/40">{children ? (expanded ? '▲' : '▼') : ''}</span>
        </div>
      </button>

      {expanded && children}
    </div>
  )
}

/** A source's row; opening it shows the source on the map. */
export function ContributorRow({ c, onHighlight }: {
  c: Contributor
  onHighlight?: (id: string | null) => void
}) {
  return (
    <SourceRow
      label={contributorLabel(c)}
      distance={formatDist(c.distance_m)}
      heard={heardText(c.source_type, c.heard)}
      received={c.received}
      nden={c.nden_sone}
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
    <SourceRow label={SOURCE_LABELS.aircraft} distance="" heard={flightsText(layer.events)} received={layer} nden={layer.nden_sone}>
      <AircraftLayerDetail received={layer} kinds={layer.kinds} events={layer.events} flights={flights} onHighlightFlight={onHighlightFlight} />
    </SourceRow>
  )
}

/** Everything the list does not name: the sources cut from it or under 0 dB, together. */
export function RestRow({ levels, nden }: {
  levels: PeriodLevels
  /** Its Nden, all of it together and steady (the final update's). */
  nden: number | null | undefined
}) {
  return (
    <SourceRow
      label="Everything else"
      distance=""
      heard={null}
      received={levels}
      nden={nden}
    />
  )
}

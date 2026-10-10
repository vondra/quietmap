// One row of the popup's "what you hear" list, in columns: the source, why it is loud (how often it
// passes, or steady), how far, and how loud it is alone over the day (its own Nden, sone); it opens to
// its details. A contributor has one, the aircraft layer as a whole has one (it lists no
// contributors), and so has the rest of the sources, together.
import { useState, type ReactNode } from 'react'
import type { Contributor, LayerLevels, TopFlight } from '../../../types/noise'
import { fmtInt, fmtSone } from '../../../utils/formatters'
import { FadingText } from '../../ui/fading-text'
import { flightsText, heardText } from '../heard'
import { contributorLabel, SOURCE_LABELS } from '../labels'
import { Chevron, formatDist } from '../shared'
import { AircraftLayerDetail, ContributorDetail } from './ContributorDetail'

/** A row's order in the list: its own Nden, or before the final update its Lden. */
export function rowRank(nden: number | undefined, lden: number | null | undefined): number {
  return nden ?? (lden == null ? -Infinity : 10 ** (lden / 10))
}

function SourceRow({ label, distance, heard, nden, onToggle, children }: {
  label: string
  distance: string
  /** Why it is loud, short: how often it passes, or steady; none for a source without passes. */
  heard: string | null
  /** The row's own Nden in sone, alone: how loud it is over the day (undefined before the final
   *  update, null when it cannot be told). */
  nden: number | null | undefined
  /** Told when the row opens or closes (a tap on a phone as well as a click). */
  onToggle?: (expanded: boolean) => void
  /** The opened body; none for a row that does not open. */
  children?: ReactNode
}) {
  // Keyed by a stable id, so the row stays open across streamed updates.
  const [expanded, setExpanded] = useState(false)
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
        {/* The name takes the room the passes leave it and fades out where they would meet; opened, it
            shows whole. How far (context, grey) and the loudness (the result, bold) in their own
            columns, right-aligned, so they read down. */}
        <div className="grid grid-cols-[minmax(0,1fr)_3rem_2.5rem_0.75rem] gap-x-2 items-baseline text-xs">
          <span className="flex min-w-0 items-baseline gap-2">
            <FadingText whole={expanded} className="flex-1 font-medium">{label}</FadingText>
            {heard && <span className="shrink-0 tabular-nums">{heard}</span>}
          </span>
          <span className="text-right tabular-nums text-muted-foreground/60">{distance}</span>
          <span className="text-right tabular-nums font-semibold">{nden === undefined ? '…' : nden === null ? '—' : fmtSone(nden)}</span>
          {children ? <Chevron open={expanded} /> : <span />}
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
    <SourceRow label={SOURCE_LABELS.aircraft} distance="" heard={flightsText(layer.events)} nden={layer.nden_sone}>
      <AircraftLayerDetail received={layer} kinds={layer.kinds} events={layer.events} flights={flights} onHighlightFlight={onHighlightFlight} />
    </SourceRow>
  )
}

/** The sources the list does not name, together: how many, and their Nden. */
export function RestRow({ sources, nden }: {
  sources: number
  /** Its Nden, all of it together and steady (the final update's). */
  nden: number | null | undefined
}) {
  return <SourceRow label={`Rest (${fmtInt(sources)})`} distance="" heard={null} nden={nden} />
}

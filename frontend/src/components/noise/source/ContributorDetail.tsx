// The expanded body of a source row: a contributor's class, its layer's display fields and its
// level by day, evening and night; the aircraft layer's levels and its loudest flights.
import { Fragment } from 'react'
import type { AircraftKind, Contributor, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtDbValue } from '../../../utils/formatters'
import { aircraftKindShares, contributorClass, labelNamesClass, subtypeLabel } from '../labels'
import { PERIOD_LABELS_DETAIL } from '../shared'
import { HoverText } from '../../ui/info-tip'
import { MetadataRows } from './MetadataRows'
import { TopFlightsTable } from './TopFlightsTable'

const PERIODS_TOOLTIP =
  'Level received here in each period (local time):\n' +
  PERIOD_LABELS_DETAIL.join('\n') +
  '\n\nLden adds +5 dB to the evening and +10 dB to the night.'

const DETAIL_CLASS = 'mt-1 ml-2 mr-4 mb-1 text-[11px] leading-relaxed font-mono text-muted-foreground'

/** The level by day, evening and night, each period's value under its name, the columns right-aligned. */
function PeriodLevelsLine({ received }: { received: PeriodLevels }) {
  return (
    <div className="grid grid-cols-[1fr_repeat(3,auto)] gap-x-3 text-right">
      <span />
      {['Day', 'Evening', 'Night'].map(name => <span key={name} className="text-muted-foreground/60">{name}</span>)}
      <HoverText title={PERIODS_TOOLTIP} className="text-left">Level dB</HoverText>
      {[received.ld, received.le, received.ln].map((level, period) => (
        <span key={period} className="text-foreground">{fmtDbValue(level)}</span>
      ))}
    </div>
  )
}

export function ContributorDetail({ c }: { c: Contributor }) {
  const cls = contributorClass(c)
  // The row shows the name; the class goes here unless the row's label already says it.
  const showClass = !labelNamesClass(c)
  return (
    <div className={DETAIL_CLASS}>
      {showClass && (
        <div className="text-muted-foreground/60 mb-0.5">{subtypeLabel(c.source_type, cls)}</div>
      )}
      <MetadataRows c={c} />
      <PeriodLevelsLine received={c.received} />
    </div>
  )
}

const KINDS_TOOLTIP =
  'Share of the aircraft noise here (Lden) by kind.\n' +
  'Airliners: jets with their engines under the wings (A320, B737, E-Jets, wide-bodies).\n' +
  'Regional and business jets: engines at the tail (CRJ, ERJ, business jets).\n' +
  'Propeller aircraft: turboprops and light aircraft.\n' +
  'Airport ground operations: taxiing and take-off rolls.'

export function AircraftLayerDetail({ received, kinds, flights, onHighlightFlight }: {
  received: PeriodLevels
  kinds?: Partial<Record<AircraftKind, number>>
  flights: TopFlight[]
  onHighlightFlight: (key: string | null) => void
}) {
  const shares = aircraftKindShares(kinds ?? {})
  return (
    <div className={DETAIL_CLASS}>
      {shares.length > 0 && (
        <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3">
          <HoverText title={KINDS_TOOLTIP} className="col-span-2">Made of</HoverText>
          {shares.map(([label, share]) => (
            <Fragment key={label}>
              <span className="truncate pl-2" title={label}>{label}</span>
              <span className="text-right text-foreground">{`${Math.round(100 * share)}\u00a0%`}</span>
            </Fragment>
          ))}
        </div>
      )}
      <PeriodLevelsLine received={received} />
      <TopFlightsTable flights={flights} onHighlightFlight={onHighlightFlight} />
    </div>
  )
}

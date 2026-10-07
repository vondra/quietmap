// The expanded body of a source row: a contributor's class, its layer's display fields and its
// level by day, evening and night; the aircraft layer's levels and its loudest flights.
import type { AircraftKind, Contributor, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtDbValue } from '../../../utils/formatters'
import { aircraftKindShares, contributorClass, contributorLabel, subtypeLabel } from '../labels'
import { lineRow, PERIOD_LABELS_DETAIL } from '../shared'
import { HoverText } from '../../ui/info-tip'
import { MetadataRows } from './MetadataRows'
import { TopFlightsTable } from './TopFlightsTable'

const PERIODS_TOOLTIP =
  'Level received here in each period (local time):\n' +
  PERIOD_LABELS_DETAIL.join('\n') +
  '\n\nLden adds +5 dB to the evening and +10 dB to the night.'

const DETAIL_CLASS = 'mt-1 ml-2 mr-4 mb-1 text-[11px] leading-relaxed font-mono text-muted-foreground'

function PeriodLevelsLine({ received }: { received: PeriodLevels }) {
  return lineRow(
    <HoverText title={PERIODS_TOOLTIP}>Day/Evening/Night</HoverText>,
    `${fmtDbValue(received.ld)}/${fmtDbValue(received.le)}/${fmtDbValue(received.ln)} dB`,
  )
}

export function ContributorDetail({ c }: { c: Contributor }) {
  const cls = contributorClass(c)
  // The row shows the name; the class goes here unless the row's label already starts with it.
  const showClass = cls !== '' && !contributorLabel(c).startsWith(subtypeLabel(c.source_type, cls))
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
  'Airliners: jets with their engines under the wings (A320, B737, wide-bodies).\n' +
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
      {shares.length > 0 && lineRow(
        <HoverText title={KINDS_TOOLTIP}>Made of</HoverText>,
        <span className="flex flex-col items-end">
          {shares.map(([label, share]) => <span key={label}>{`${label} ${Math.round(100 * share)}\u00a0%`}</span>)}
        </span>,
      )}
      <PeriodLevelsLine received={received} />
      <TopFlightsTable flights={flights} onHighlightFlight={onHighlightFlight} />
    </div>
  )
}

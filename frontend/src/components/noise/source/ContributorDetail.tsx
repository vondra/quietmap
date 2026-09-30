// The expanded body of a source row: a contributor's class, its layer's display fields and its
// level by day, evening and night; the aircraft layer's levels and its loudest flights.
import type { Contributor, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtDbValue } from '../../../utils/formatters'
import { contributorClass, contributorLabel, lineRow, PERIOD_LABELS_DETAIL, subtypeLabel } from '../shared'
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
  // The row shows the name; the class goes here (a row without a name already shows the class).
  const showClass = cls !== '' && contributorLabel(c) !== subtypeLabel(c.source_type, cls)
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

export function AircraftLayerDetail({ received, flights, onHighlightFlight }: {
  received: PeriodLevels
  flights: TopFlight[]
  onHighlightFlight: (key: string | null) => void
}) {
  return (
    <div className={DETAIL_CLASS}>
      <PeriodLevelsLine received={received} />
      <TopFlightsTable flights={flights} onHighlightFlight={onHighlightFlight} />
    </div>
  )
}

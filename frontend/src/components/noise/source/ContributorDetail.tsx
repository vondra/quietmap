// The expanded body of a source row: a contributor's class, its layer's display fields and its
// level by day, evening and night; the aircraft layer's levels, what flies over the point and its
// loudest flights.
import type { AircraftEvents, AircraftKind, Contributor, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtDbValue } from '../../../utils/formatters'
import { aircraftEventRows, eventCount } from '../aircraft-events'
import { aircraftKindShares, contributorClass, labelNamesClass, subtypeLabel } from '../labels'
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

const EVENTS_TOOLTIP =
  'Flights an average day whose peak level here (outdoors, in the open) reaches 50, 60 or 70 dB.\n' +
  'Every flight of the year (ADS-B) counts once, at its loudest moment here; under one a day,\n' +
  'a year. Night: 23\u201307. Height: above the ground here at that moment, the mean. Type: the one\n' +
  'flying most of them. Flights in the air; take-off rolls are the airport ground operations.'

/** What flies over the point: the bands any flight reaches, and the helicopters. */
function AircraftEventsTable({ events }: { events: AircraftEvents }) {
  const rows = aircraftEventRows(events)
  if (!rows.length) {
    return lineRow(<HoverText title={EVENTS_TOOLTIP}>Flights above 50 dB</HoverText>, 'none')
  }
  return (
    <div className="mt-1 overflow-x-auto">
      <table className="w-full text-[10px] whitespace-nowrap [&_tr>*+*]:pl-2 [&_td]:align-baseline">
        <caption className="text-left font-medium text-foreground/70 mb-0.5">
          <HoverText title={EVENTS_TOOLTIP}>Flights louder than</HoverText>
        </caption>
        <tbody>
          {rows.map(row => (
            <tr key={row.above}>
              <td className="text-right">{row.above}</td>
              <td className="text-right font-medium text-foreground">{row.count}</td>
              <td className="text-right text-indigo-600">{row.night && `night ${row.night}`}</td>
              <td className="text-right">{row.height}</td>
              <td className="w-full max-w-0 truncate text-left" title={row.type}>{row.type}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {events.helicopters_per_day > 0 && lineRow('Helicopters above 50 dB', eventCount(events.helicopters_per_day))}
    </div>
  )
}

export function AircraftLayerDetail({ received, kinds, events, flights, onHighlightFlight }: {
  received: PeriodLevels
  kinds?: Partial<Record<AircraftKind, number>>
  events?: AircraftEvents
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
      {events && <AircraftEventsTable events={events} />}
      <TopFlightsTable flights={flights} onHighlightFlight={onHighlightFlight} />
    </div>
  )
}

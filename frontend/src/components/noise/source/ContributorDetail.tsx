// The expanded body of a source row: a contributor's class, its layer's display fields and its
// level by day, evening and night; the aircraft layer's levels, what flies over the point and its
// loudest flights.
import { Fragment } from 'react'
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
      {events && <AircraftEventsTable events={events} />}
      <TopFlightsTable flights={flights} onHighlightFlight={onHighlightFlight} />
    </div>
  )
}

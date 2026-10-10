// The opened body of a source row, a block apart from the next: a contributor's class and the facts
// of its layer, then its levels by period; the aircraft layer's makeup, its levels, the flights of a
// day by their peak level here and its loudest flights.
import type { AircraftEvents, AircraftKind, Contributor, PeriodLevels, TopFlight } from '../../../types/noise'
import { fmtCount, fmtDbValue } from '../../../utils/formatters'
import { FadingText } from '../../ui/fading-text'
import { HoverText } from '../../ui/info-tip'
import { aircraftEventRows } from '../aircraft-events'
import { aircraftKindShares, contributorClass, labelNamesClass, subtypeLabel } from '../labels'
import { CAPTION, COLUMN_NAME, DETAIL_TEXT, DetailTable, lineRow } from '../shared'
import { MetadataRows } from './MetadataRows'
import { TopFlightsTable } from './TopFlightsTable'

const DETAIL = `mb-2 ml-2 mr-5 space-y-2 ${DETAIL_TEXT}`

const LEVELS_TOOLTIP = 'The level here by period (local time): day 07–19, evening 19–23, night 23–07.\n'
  + 'Lden counts the evening 5 dB and the night 10 dB louder.'

/** The level by day, evening and night, and Lden. */
function LevelsTable({ received }: { received: PeriodLevels }) {
  return (
    <DetailTable
      head={['', 'Day', 'Evening', 'Night', 'Lden']}
      rows={[[
        <HoverText title={LEVELS_TOOLTIP}>dB</HoverText>,
        ...[received.ld, received.le, received.ln, received.lden].map(fmtDbValue),
      ]]}
    />
  )
}

export function ContributorDetail({ c }: { c: Contributor }) {
  return (
    <div className={DETAIL}>
      <div>
        {/* The row shows the name; the class goes here unless the row's label already says it. */}
        {!labelNamesClass(c) && <div>{subtypeLabel(c.source_type, contributorClass(c))}</div>}
        <MetadataRows c={c} />
      </div>
      <LevelsTable received={c.received} />
    </div>
  )
}

const KINDS_TOOLTIP = 'Each kind\'s share of the aircraft noise here (Lden).\n'
  + 'Airliners: engines under the wings (A320, B737, E-Jets, wide-bodies).\n'
  + 'Regional and business jets: engines at the tail.\n'
  + 'Propeller aircraft: turboprops and light aircraft.\n'
  + 'Airport ground operations: taxiing and take-off rolls.'

const FLIGHTS_TOOLTIP = 'Flights of an average day of the year (ADS-B), each counted once,\n'
  + 'at its loudest moment here, outdoors in the open. In the air only:\n'
  + 'take-off rolls are the airport ground operations.'

const PEAK_TOOLTIP = 'The flight\'s loudest moment here (LAmax)'

/** What flies over the point: the flights of a day by the peak level they reach here, and the
 *  helicopters among them. */
function FlightsByPeakTable({ events }: { events: AircraftEvents }) {
  const rows = aircraftEventRows(events)
  if (!rows.length) return lineRow(<HoverText title={FLIGHTS_TOOLTIP}>Flights ≥ 50 dB</HoverText>, 'none')
  return (
    <div>
      <table className="w-full">
        <caption className={`${CAPTION} pb-0.5 text-left`}>
          <HoverText title={FLIGHTS_TOOLTIP}>Flights a day</HoverText>
        </caption>
        <thead>
          <tr className={`${COLUMN_NAME} whitespace-nowrap [&_th]:pb-px [&_th]:align-bottom [&_th]:font-normal`}>
            <th scope="col" className="text-left"><HoverText title={PEAK_TOOLTIP}>Peak</HoverText></th>
            <th scope="col" className="pl-2 text-right">All</th>
            <th scope="col" className="pl-2 text-right"><HoverText title="Of them at night (23–07)">At night</HoverText></th>
            <th scope="col" className="pl-2 text-right"><HoverText title="Their mean height above the ground here">Height</HoverText><br />km</th>
            <th scope="col" className="w-full pl-2 text-left"><HoverText title="The type flying most of them">Type</HoverText></th>
          </tr>
        </thead>
        <tbody>
          {rows.map(row => (
            <tr key={row.peak} className="[&_td]:align-baseline">
              <td className="whitespace-nowrap">{row.peak}</td>
              <td className="pl-2 text-right text-foreground">{row.perDay}</td>
              <td className="pl-2 text-right text-foreground">{row.night}</td>
              <td className="pl-2 text-right text-foreground">{row.heightKm}</td>
              <td className="max-w-0 pl-2 text-foreground">
                {row.type && <FadingText><HoverText title={row.typeTitle}>{row.type}</HoverText></FadingText>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {events.helicopters_per_day > 0 && lineRow('Helicopters ≥ 50 dB', `${fmtCount(events.helicopters_per_day)}/day`)}
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
    <div className={DETAIL}>
      {shares.length > 0 && (
        <DetailTable
          caption={<HoverText title={KINDS_TOOLTIP}>Made of</HoverText>}
          rows={shares.map(([label, share]) => [label, `${Math.round(100 * share)} %`])}
        />
      )}
      <LevelsTable received={received} />
      {events && <FlightsByPeakTable events={events} />}
      <TopFlightsTable flights={flights} onHighlightFlight={onHighlightFlight} />
    </div>
  )
}

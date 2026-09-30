// The aircraft layer's loudest flights at the point, loudest first: peak level, where the flight
// passed, when, and what flew, linked to the flight's trace on the adsb.lol globe.
import type { TopFlight } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { PERIOD_LABELS_DETAIL } from '../shared'
import { topFlightCells } from '../top-flights'

const TITLE_TOOLTIP =
  'The ADS-B flights with the highest peak level (Lmax) at this point, loudest first.'
const AIRCRAFT_TOOLTIP =
  'The aircraft type, with its callsign and ICAO address in its tooltip.\n' +
  'It opens the flight\'s trace of that day on adsb.lol, in a new tab.'

/** The numeric columns, each with its unit under its name. */
const NUMBER_COLUMNS = [
  {
    name: 'Lmax',
    unit: 'dB',
    tooltip: 'The flight\'s peak A-weighted level here (LAmax): the loudest part of its track at its ' +
      'closest point, from the Doc 29 noise-power-distance tables. The list is ordered by it.',
  },
  { name: 'Dist', unit: 'km', tooltip: 'Horizontal distance to where the flight\'s peak level is reached.' },
  { name: 'Alt', unit: 'km', tooltip: 'Height above this point where the flight\'s peak level is reached.' },
  {
    name: 'Date',
    unit: 'UTC',
    tooltip: 'The day the flight started (UTC, month-day) and the period it passed here in (local time):\n' +
      PERIOD_LABELS_DETAIL.join('\n'),
  },
]

/** The date's colour by period: day as the rest of the row, evening amber, night indigo. */
const PERIOD_COLOURS = ['', 'text-amber-700', 'text-indigo-600']

export function TopFlightsTable({ flights }: { flights: TopFlight[] }) {
  if (!flights.length) return null
  // Only the type name wraps; should a row still not fit, the table scrolls sideways, never the popup.
  return (
    <div className="mt-2 overflow-x-auto">
      <table className="w-full text-[10px] whitespace-nowrap [&_tr>*+*]:pl-2 [&_td]:align-baseline">
        <caption className="text-left font-medium text-foreground/70 mb-0.5">
          <HoverText title={TITLE_TOOLTIP}>Loudest flights</HoverText>
        </caption>
        <thead>
          <tr className="text-muted-foreground/60 [&_th]:font-normal [&_th]:pb-0.5 [&_th]:align-top">
            {NUMBER_COLUMNS.map(column => (
              <th key={column.name} scope="col" className="text-right">
                <HoverText title={column.tooltip}>{column.name}</HoverText>
                <span className="block"> {column.unit}</span>
              </th>
            ))}
            <th scope="col" className="w-full text-left">
              <HoverText title={AIRCRAFT_TOOLTIP}>Aircraft</HoverText>
            </th>
          </tr>
        </thead>
        <tbody>
          {flights.map(flight => {
            const cells = topFlightCells(flight)
            return (
              <tr key={`${flight.icao}-${flight.start_unix}`}>
                <td className="text-right font-medium text-foreground">{cells.lmax}</td>
                <td className="text-right">{cells.closestKm}</td>
                <td className="text-right">{cells.altitudeKm}</td>
                <td className={`text-right ${PERIOD_COLOURS[cells.period] ?? ''}`}>
                  <HoverText title={`${cells.startUtc} (flight start)\n${PERIOD_LABELS_DETAIL[cells.period] ?? 'Unknown period'}`}>
                    {cells.date}
                  </HoverText>
                </td>
                <td className="whitespace-normal">
                  <HoverText title={cells.aircraftTitle}>
                    <a href={cells.href} target="_blank" rel="noopener noreferrer" className="text-sky-700 hover:underline">
                      {cells.aircraft}
                      <span className="sr-only"> (its trace on adsb.lol, in a new tab)</span>
                    </a>
                  </HoverText>
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

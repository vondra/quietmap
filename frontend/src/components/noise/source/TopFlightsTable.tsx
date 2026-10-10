// The aircraft layer's loudest flights at the point, loudest first: peak level, where the flight
// passed, when, and what flew, linked to the flight's trace on the adsb.lol globe. A row hovered
// (or tapped on a phone) highlights the flight's track on the map.
import type { TopFlight } from '../../../types/noise'
import { FadingText } from '../../ui/fading-text'
import { HoverText } from '../../ui/info-tip'
import { CAPTION, COLUMN_NAME, PERIOD_LABELS_DETAIL } from '../shared'
import { topFlightCells, topFlightKey } from '../top-flights'

const TITLE_TOOLTIP = 'The flights of the year with the highest peak level here, loudest first.\n'
  + 'Hover a row (tap it on a phone) for its track within 20 km on the map.'

/** The numeric columns, each with its unit under its name. */
const NUMBER_COLUMNS = [
  { name: 'Peak', unit: 'dB', tooltip: 'The flight\'s loudest moment here (LAmax)' },
  { name: 'Dist', unit: 'km', tooltip: 'Horizontal distance to where the flight was loudest' },
  { name: 'Height', unit: 'km', tooltip: 'Its height above this point there' },
  {
    name: 'Date',
    unit: 'UTC',
    tooltip: 'The day the flight started (UTC, month-day) and when it passed here (local time):\n'
      + 'D day 07–19, E evening 19–23, N night 23–07',
  },
]

export function TopFlightsTable({ flights, onHighlightFlight }: {
  flights: TopFlight[]
  /** Shows a flight's track on the map, by `topFlightKey`; null shows none. */
  onHighlightFlight: (key: string | null) => void
}) {
  if (!flights.length) return null
  // A row is one line: a long type name fades out where it is cut (whole in its tooltip).
  return (
    <table className="w-full">
      <caption className={`${CAPTION} pb-0.5 text-left`}>
        <HoverText title={TITLE_TOOLTIP}>Loudest flights</HoverText>
      </caption>
      <thead>
        <tr className={`${COLUMN_NAME} whitespace-nowrap [&_th]:pb-px [&_th]:align-bottom [&_th]:font-normal`}>
          {NUMBER_COLUMNS.map((column, k) => (
            <th key={column.name} scope="col" className={k ? 'pl-2 text-right' : 'text-right'}>
              <HoverText title={column.tooltip}>{column.name}</HoverText><br />{column.unit}
            </th>
          ))}
          <th scope="col" className="w-full pl-2 text-left">
            <HoverText title="The type, its callsign and ICAO address on hover; a click opens the flight's trace on adsb.lol">Aircraft</HoverText>
          </th>
        </tr>
      </thead>
      <tbody>
        {flights.map(flight => {
          const cells = topFlightCells(flight)
          const key = topFlightKey(flight)
          // A phone has no hover, but browsers send a tapped row mouseenter and, on the next tap
          // elsewhere, mouseleave: the tapped flight's track stays until then.
          return (
            <tr
              key={key}
              className="hover:bg-muted/30 [&_td]:align-baseline"
              onMouseEnter={() => onHighlightFlight(key)}
              onMouseLeave={() => onHighlightFlight(null)}
            >
              <td className="text-right text-foreground">{cells.lmax}</td>
              <td className="pl-2 text-right text-foreground">{cells.closestKm}</td>
              <td className="pl-2 text-right text-foreground">{cells.altitudeKm}</td>
              <td className="pl-2 text-right whitespace-nowrap text-foreground">
                <HoverText title={`${cells.startUtc} (flight start)\n${PERIOD_LABELS_DETAIL[cells.period] ?? 'Unknown period'}`}>
                  {cells.date}
                </HoverText>
              </td>
              <td className="max-w-0 pl-2">
                <FadingText>
                  <HoverText title={cells.aircraftTitle}>
                    <a href={cells.href} target="_blank" rel="noopener noreferrer" className="text-sky-700 hover:underline">
                      {cells.aircraft}
                      <span className="sr-only"> (its trace on adsb.lol, in a new tab)</span>
                    </a>
                  </HoverText>
                </FadingText>
              </td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

// A loudest flight's cells in words: its peak level, where it passed, when (UTC) and in which
// period, what flew, and the link to its trace on the adsb.lol globe. Pure TypeScript, so the
// wording has dependency-free unit tests.
import type { TopFlight } from '../../types/noise.ts'
import { aircraftTypeName } from '../../utils/aircraft-types.ts'

/** In the order of the popup's period labels. */
const PERIODS = ['day', 'evening', 'night'] as const

export interface TopFlightCells {
  /** Whole decibels; the units are in the column headers. */
  lmax: string
  /** Kilometres to two decimals. */
  closestKm: string
  altitudeKm: string
  /** The start's UTC month and day and the period letter: "09-02 N". */
  date: string
  /** Day 0, evening 1, night 2; -1 for a period the popup has no label for. */
  period: number
  /** "2025-09-02 14:26:40 UTC". */
  startUtc: string
  /** The type in words, else its designator. */
  aircraft: string
  /** The type with its designator, the callsign and the ICAO address. */
  aircraftTitle: string
  /** The globe's trace of the flight's day: a flight is cut from one UTC day of ADS-B, the day the
   *  globe files its traces by, so its start names that day. */
  href: string
}

// Rounded before formatting, so a few metres below the receiver read 0.00, not -0.00.
const kilometres = (metres: number) => (Math.round(metres / 10) / 100).toFixed(2)

export function topFlightCells(flight: TopFlight): TopFlightCells {
  const start = new Date(flight.start_unix * 1000).toISOString()
  const day = start.slice(0, 10)
  const period = PERIODS.indexOf(flight.period)
  const name = aircraftTypeName(flight.type)
  const type = !flight.type ? 'unknown' : name === flight.type ? name : `${name} (${flight.type})`
  return {
    lmax: flight.lmax_db.toFixed(0),
    closestKm: kilometres(flight.closest_m),
    altitudeKm: kilometres(flight.altitude_m),
    date: `${day.slice(5)} ${'DEN'.charAt(period) || '?'}`,
    period,
    startUtc: `${day} ${start.slice(11, 19)} UTC`,
    aircraft: flight.type ? name : 'unknown type',
    aircraftTitle: [
      `Type: ${type}`,
      `Callsign: ${flight.callsign || '—'}`,
      `ICAO address: ${flight.icao.toUpperCase()}`,
      '',
      `Opens the flight's trace of ${day} on adsb.lol.`,
    ].join('\n'),
    href: `https://adsb.lol/?icao=${flight.icao}&showTrace=${day}`,
  }
}

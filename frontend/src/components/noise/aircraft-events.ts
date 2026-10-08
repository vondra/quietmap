// What flies over the point in words: per band of maximum level the flights of an average day
// (under one a day, of a year), those at night, their height and type. Pure TypeScript, so it has a
// dependency-free unit test.
import type { AircraftEvents } from '../../types/noise.ts'
import { aircraftTypeName } from '../../utils/aircraft-types.ts'

export interface AircraftEventRow {
  /** "50 dB" */
  above: string
  /** "412 a day", "3 a year", "<1 a year" */
  count: string
  /** The night's count ("21 a day"), or empty without night flights */
  night: string
  /** "0.6 km", or empty */
  height: string
  /** The type in words, or empty */
  type: string
}

/** A rate as the visitor counts it: a day from one a day up, else a year. */
export function eventCount(perDay: number): string {
  if (perDay >= 9.95) return `${Math.round(perDay)} a day`
  if (perDay >= 0.995) return `${perDay.toFixed(1).replace(/\.0$/, '')} a day`
  const perYear = perDay * 365.25
  return perYear >= 0.5 ? `${Math.round(perYear)} a year` : '<1 a year'
}

/** The bands any flight reaches, loudest last. */
export function aircraftEventRows(events: AircraftEvents): AircraftEventRow[] {
  return events.above_db.flatMap((above, band) => {
    const perDay = events.per_day[band] ?? 0
    if (perDay <= 0) return []
    const night = events.night[band] ?? 0
    const height = events.height_m[band]
    const type = events.type[band]
    return [{
      above: `${above} dB`,
      count: eventCount(perDay),
      night: night > 0 ? eventCount(night) : '',
      height: height == null ? '' : `${(height / 1000).toFixed(1)} km`,
      type: type ? aircraftTypeName(type) : '',
    }]
  })
}

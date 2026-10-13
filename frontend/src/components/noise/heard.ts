// How a source is heard, one figure short enough for a column of the popup's row (END periods: day
// 12 h, evening 4 h, night 8 h): a road's vehicles an hour by day, both directions, a quiet one's
// too ("732 veh/h", "0.6 veh/h": one unit down the column), a railway's trains a day ("120
// trains/day"); the building layer's events how often they sound
// ("3× a day": church bells, calls to prayer); the aircraft layer its flights a day above 50 dB.
// Steady sources (buildings, industry, ships) carry no passes and get no words.
import type { AircraftEvents, Contributor } from '../../types/noise'
import { fmtCount } from '../../utils/formatters.ts'

const PERIOD_HOURS = { day: 12, evening: 4, night: 8 } as const

/** Passes a day from passes per hour by period. */
function perDay(perHour: { day: number, evening: number, night: number }): number {
  return perHour.day * PERIOD_HOURS.day + perHour.evening * PERIOD_HOURS.evening + perHour.night * PERIOD_HOURS.night
}

export function heardText(sourceType: string, heard: Contributor['heard'], metadata?: Contributor['metadata']): string | null {
  if (sourceType === 'road') {
    const perHour = roadPerHourByDay(metadata) ?? heard?.per_hour.day
    return perHour == null ? null : `${fmtCount(perHour)} veh/h`
  }
  if (!heard) return null
  const daily = fmtCount(perDay(heard.per_hour))
  if (sourceType === 'railway') return `${daily} trains/day`
  if (sourceType === 'building') return `${daily}× a day`
  return null
}

/** A road's vehicles an hour by day, both directions: its cross-section's daily traffic, the day's
 *  share of it over the day's 12 hours (a carriageway's own passes are one direction's). */
function roadPerHourByDay(metadata: Contributor['metadata'] | undefined): number | undefined {
  const daily = metadata?.cross_section_aadt
  const shares = metadata?.period_shares
  return typeof daily === 'number' && Array.isArray(shares) && typeof shares[0] === 'number'
    ? (daily * shares[0]) / PERIOD_HOURS.day
    : undefined
}

/** The aircraft layer's flights a day whose peak level here reaches the flights table's first band
 *  (50 dB); none without such flights. */
export function flightsText(events: AircraftEvents | undefined): string | null {
  const perDay = events?.per_day[0] ?? 0
  return perDay > 0 ? `${fmtCount(perDay)} flights/day` : null
}

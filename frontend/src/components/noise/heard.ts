// How a source is heard, short enough for a column of the popup's row, from its passes per hour (END
// periods: day 12 h, evening 4 h, night 8 h): steady when its passes run together at its distance,
// else how often it passes, a road's vehicles an hour by day ("732 veh/h") and every rarer pass a
// day ("120 trains/day", "0.3 veh/day"); the building layer's events how often they sound
// ("3× a day": church bells, calls to prayer); the aircraft layer its flights a day above 50 dB.
// Steady sources (buildings, industry, ships) carry no passes and get no words.
import type { AircraftEvents, Contributor } from '../../types/noise'
import { fmtCount } from '../../utils/formatters.ts'

const PERIOD_HOURS = { day: 12, evening: 4, night: 8 } as const

/** Passes a day from passes per hour by period. */
function perDay(perHour: { day: number, evening: number, night: number }): number {
  return perHour.day * PERIOD_HOURS.day + perHour.evening * PERIOD_HOURS.evening + perHour.night * PERIOD_HOURS.night
}

export function heardText(sourceType: string, heard: Contributor['heard']): string | null {
  if (!heard) return null
  if (heard.steady) return 'steady'
  const daily = fmtCount(perDay(heard.per_hour))
  if (sourceType === 'railway') return `${daily} trains/day`
  if (sourceType === 'building') return `${daily}× a day`
  return heard.per_hour.day >= 1 ? `${Math.round(heard.per_hour.day)} veh/h` : `${daily} veh/day`
}

/** The aircraft layer's flights a day whose peak level here reaches the flights table's first band
 *  (50 dB); none without such flights. */
export function flightsText(events: AircraftEvents | undefined): string | null {
  const perDay = events?.per_day[0] ?? 0
  return perDay > 0 ? `${fmtCount(perDay)} flights/day` : null
}

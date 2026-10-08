// How a source is heard, short enough for a column of the popup's row, from its passes per hour (END
// periods: day 12 h, evening 4 h, night 8 h): steady when its passes run together at its distance,
// else how often it passes ("732 veh/h", "120 trains/day"); the building layer's events how often
// they sound ("3× a day": church bells, calls to prayer); the aircraft layer its flights a day above
// 50 dB. Steady sources (buildings, industry, ships) carry no passes and get no words.
import type { AircraftEvents, Contributor } from '../../types/noise'

const PERIOD_HOURS = { day: 12, evening: 4, night: 8 } as const

/** Passes a day from passes per hour by period. */
function perDay(perHour: { day: number, evening: number, night: number }): number {
  return perHour.day * PERIOD_HOURS.day + perHour.evening * PERIOD_HOURS.evening + perHour.night * PERIOD_HOURS.night
}

/** `count` a day, by the week or month when fewer: "120 trains/day", "2 trains/week", "3× a day". */
function daily(count: number, unit: string | null): string {
  const [n, per] = count >= 1.5 ? [count, 'day'] : count * 7 >= 0.75 ? [count * 7, 'week'] : [count * 30, 'month']
  const whole = Math.max(1, Math.round(n))
  return unit ? `${whole} ${unit}/${per}` : `${whole}× a ${per}`
}

export function heardText(sourceType: string, heard: Contributor['heard']): string | null {
  if (!heard) return null
  if (heard.steady) return 'steady'
  const day = heard.per_hour.day
  if (sourceType === 'railway') return daily(perDay(heard.per_hour), 'trains')
  if (sourceType === 'building') return daily(perDay(heard.per_hour), null)
  if (day >= 1) return `${Math.round(day)} veh/h`
  return daily(perDay(heard.per_hour), 'veh')
}

/** The aircraft layer's flights a day whose peak level here reaches the events table's first band
 *  (50 dB); none without such flights. */
export function flightsText(events: AircraftEvents | undefined): string | null {
  const perDay = events?.per_day[0] ?? 0
  return perDay > 0 ? daily(perDay, 'flights') : null
}

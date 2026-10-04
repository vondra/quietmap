// How a source is heard, in words, from its passes per hour (END periods: day 12 h, evening 4 h,
// night 8 h): a steady sound when its passes run together at its distance, else how often it
// passes; the building layer's events how often they sound (church bells ring, a mosque calls).
// Steady sources (buildings, industry, ships) carry no passes and get no words.
import type { Contributor } from '../../types/noise'

const PERIOD_HOURS = { day: 12, evening: 4, night: 8 } as const

/** Passes a day from passes per hour by period. */
function perDay(perHour: { day: number, evening: number, night: number }): number {
  return perHour.day * PERIOD_HOURS.day + perHour.evening * PERIOD_HOURS.evening + perHour.night * PERIOD_HOURS.night
}

/** `count` things a day; fewer by the week, or one every so many days. */
function daily(count: number, one: string, many: string): string {
  if (count >= 1.5) return `${Math.round(count)} ${many} a day`
  if (count >= 0.75) return `1 ${one} a day`
  if (count * 7 >= 1.5) return `${Math.round(count * 7)} ${many} a week`
  if (count * 7 >= 0.75) return `1 ${one} a week`
  return `a ${one} every ${Math.round(1 / count)} days`
}

export function heardText(sourceType: string, heard: Contributor['heard'], name = ''): string | null {
  if (!heard) return null
  if (heard.steady) return sourceType === 'road' ? 'steady hum' : 'steady'
  const day = heard.per_hour.day
  if (sourceType === 'railway') return daily(perDay(heard.per_hour), 'train', 'trains')
  if (sourceType === 'aircraft') return daily(perDay(heard.per_hour), 'movement', 'movements')
  if (sourceType === 'building') {
    const verb = name === 'call_to_prayer' ? 'calls' : 'rings'
    return `${verb} ${daily(perDay(heard.per_hour), 'time', 'times')}`
  }
  if (day >= 60) return `${Math.round(day)} vehicles an hour`
  if (day >= 1) return `a vehicle every ${Math.round(60 / day)} min`
  return daily(perDay(heard.per_hour), 'vehicle', 'vehicles')
}

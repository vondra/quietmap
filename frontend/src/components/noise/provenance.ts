// Where a contributor's traffic comes from, in words: counted or estimated per road vehicle class,
// known, estimated or unknown per train category. Pure TypeScript, so the wording has
// dependency-free unit tests.
import { fmtCount } from '../../utils/formatters.ts'

/** Prepared road traffic: effective vehicles/day per class plus the builder's estimated bitmask
 *  (light 1, medium 2, heavy 4, moto 8; bit set = estimate or prior, clear = observed count). */
export interface RoadTrafficCounts {
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
  traffic_estimated: number
}

/** One train category at a railway: trains per day, evening and night, and what they rest on
 *  (0 unknown, 1 known count, 2 estimated). */
export interface RailCategoryTraffic {
  periods: [number, number, number]
  status: number
}

export interface RailTraffic {
  passenger: RailCategoryTraffic
  freight: RailCategoryTraffic
}

export const ROAD_ESTIMATED_LIGHT = 1
export const ROAD_ESTIMATED_MEDIUM = 2
export const ROAD_ESTIMATED_HEAVY = 4
export const ROAD_ESTIMATED_MOTO = 8

const roadCount = (value: number): string =>
  value.toLocaleString('en', { maximumFractionDigits: 0 })

/** True when the category's prepared value is an estimate or prior. */
export function roadCategoryEstimated(traffic: Pick<RoadTrafficCounts, 'traffic_estimated'>, bit: number): boolean {
  return (traffic.traffic_estimated & bit) !== 0
}

/** One category line: value (vehicles/day) plus its counted/estimated status. */
export function roadCategoryLine(label: string, value: number, estimated: boolean): string {
  const status = estimated ? 'estimated' : value > 0 ? 'counted' : 'counted zero'
  return `${label}: ${roadCount(value)}/day — ${status}`
}

/** Category status is explicit; a numeric zero never proves absence of trains. */
export function railTrainSourceLine(category: RailCategoryTraffic): string {
  if (category.status === 0) return 'Unknown traffic; no count available'
  return category.status === 1 ? 'Known count' : 'Estimated traffic'
}

const railCount = (value: number): string => value.toLocaleString('en', { maximumSignificantDigits: 3 })

/** The trains a day by category, the row beside it having their sum: a category without trains
 *  left out, an unknown one named ("86 passenger · 16 freight/day", "60 passenger/day · freight
 *  unknown"); a level crossing's horn its soundings. */
export function railTrafficLabel(traffic: RailTraffic, soundings = false): string {
  const categories: Array<[string, RailCategoryTraffic]> = soundings
    ? [['soundings', traffic.passenger]]
    : [['passenger', traffic.passenger], ['freight', traffic.freight]]
  const daily = (category: RailCategoryTraffic) => category.periods.reduce((sum, value) => sum + value, 0)
  const running = categories
    .filter(([, category]) => category.status !== 0 && daily(category) > 0)
    .map(([name, category]) => `${fmtCount(daily(category))} ${name}`)
  const unknown = categories.filter(([, category]) => category.status === 0).map(([name]) => `${name} unknown`)
  return [...(running.length ? [`${running.join(' · ')}/day`] : []), ...unknown].join(' · ') || '0/day'
}

export function railTrafficDescription(
  traffic: RailTraffic,
  // Horn approaches carry soundings (not trains) in the passenger slots.
  soundings = false,
): string {
  const rows: Array<[string, RailCategoryTraffic]> = soundings
    ? [['Soundings', traffic.passenger]]
    : [['Passenger', traffic.passenger], ['Freight', traffic.freight]]
  return rows.map(([label, category]) => {
    const periods = category.status === 0 ? '' : '\nDay / evening / night: ' + category.periods.map(railCount).join(' / ')
    return `${label}: ${railTrainSourceLine(category)}${periods}`
  }).join('\n\n')
}

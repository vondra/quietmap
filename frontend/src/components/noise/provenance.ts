// Where a contributor's traffic comes from, in words: counted or estimated per road vehicle class,
// known, estimated or unknown per train category. Pure TypeScript, so the wording has
// dependency-free unit tests.

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

export function railTrafficLabel(traffic: RailTraffic): string {
  const categories = [traffic.passenger, traffic.freight]
  const count = categories.flatMap(category => category.periods).reduce((sum, value) => sum + value, 0)
  return `${railCount(count)}/day${categories.some(category => category.status === 0) ? ' + unknown' : ''}`
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

import type {
  DatasetProvenance,
  RailCategoryTraffic,
  RailTraffic,
  RoadTimingAttribution,
  RoadTrafficCounts,
} from '../../types/noise.ts'

export type { RoadTrafficCounts } from '../../types/noise.ts'

// Pure provenance wording shared by the contributor and segment views. Keep
// this module free of React/DOM imports so the strings can be tested with
// Node's built-in test runner without adding a frontend test framework.

export function formatProv(p: DatasetProvenance | null | undefined): string {
  if (!p) return ''
  const parts: string[] = [p.name]
  if (p.year != null) parts.push(`(${p.year})`)
  if (p.license) parts.push(`· ${p.license}`)
  return parts.join(' ')
}

/** Prepared road traffic: per-category value with counted/estimated status.
 *
 * Counts are FINAL at read time — the build resolved observations, priors and
 * allocation — so the popup reports them verbatim with the producer's
 * per-category estimated bit. There is no runtime default, oneway share or
 * access factor to undo or explain. */
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

/** Dataset attribution for the row; a model prior carries no dataset. */
export function roadTrafficSourceLine(provenance: DatasetProvenance | null | undefined): string {
  if (!provenance) return 'Source: class prior (no observation dataset)'
  const url = provenance.url ? `\n  ${provenance.url}` : ''
  return `Source: ${formatProv(provenance)}${url}`
}

/** Hostname of a stored timing-source URL; the raw string when unparseable. */
export function sourceHost(source: string): string {
  try {
    return new URL(source).hostname.replace(/^www\./, '')
  } catch {
    return source
  }
}

/** One-line attribution of the observed day/evening/night traffic timing:
 *  counting-station dataset · observation window · transfer caveat. */
export function roadTimingLine(attr: RoadTimingAttribution): string {
  const caveat = attr.total_transfer ? ' · vehicle-class timing estimated' : ''
  return `Timing: ${sourceHost(attr.source)} · ${attr.window.replace('..', '–')}${caveat}`
}

export function roadTrafficLabel(traffic: RoadTrafficCounts): string {
  const total =
    traffic.aadt_light + traffic.aadt_medium + traffic.aadt_heavy + traffic.aadt_moto
  return `${roadCount(total)}/day`
}

export function roadTrafficDescription(
  traffic: RoadTrafficCounts,
  provenance: DatasetProvenance | null | undefined,
): string {
  const categories: ReadonlyArray<readonly [string, number, number]> = [
    ['Light', traffic.aadt_light, ROAD_ESTIMATED_LIGHT],
    ['Medium', traffic.aadt_medium, ROAD_ESTIMATED_MEDIUM],
    ['Heavy', traffic.aadt_heavy, ROAD_ESTIMATED_HEAVY],
    ['Moto', traffic.aadt_moto, ROAD_ESTIMATED_MOTO],
  ]
  return [
    roadTrafficSourceLine(provenance),
    '',
    'Prepared daily traffic, this road:',
    ...categories.map(([label, value, bit]) =>
      `  ${roadCategoryLine(label, value, roadCategoryEstimated(traffic, bit))}`,
    ),
  ].join('\n')
}

/** Category status is explicit; a numeric zero never proves absence of trains. */
export function railTrainSourceLine(
  category: RailCategoryTraffic,
  provenance: DatasetProvenance | null | undefined,
): string {
  if (category.status === 0) return 'Unknown traffic; no count available'
  const status = category.status === 1 ? 'Known count' : 'Estimated traffic'
  const source = provenance ? `\n${formatProv(provenance)}${provenance.url ? `\n${provenance.url}` : ''}` : ''
  const matching = [
    ...(category.matching & 1 ? ['estimated relation alignment'] : []),
    ...(category.matching & 2 ? ['estimated graph alignment'] : []),
  ]
  return `${status}${source}${matching.length ? `\n${matching.join('; ')}` : ''}`
}

const railCount = (value: number): string => value.toLocaleString('en', { maximumSignificantDigits: 3 })

export function railTrafficLabel(traffic: RailTraffic): string {
  const categories = [traffic.passenger, traffic.freight]
  const count = categories.flatMap(category => category.periods).reduce((sum, value) => sum + value, 0)
  return `${railCount(count)}/day${categories.some(category => category.status === 0) ? ' + unknown' : ''}`
}

export function railTrafficDescription(
  traffic: RailTraffic,
  passengerProvenance: DatasetProvenance | null,
  freightProvenance: DatasetProvenance | null,
): string {
  return ([['Passenger', traffic.passenger, passengerProvenance],
    ['Freight', traffic.freight, freightProvenance]] as const).map(([label, category, provenance]) => {
    const periods = category.status === 0 ? '' : '\nDay / evening / night: ' + category.periods.map(railCount).join(' / ')
    return `${label}: ${railTrainSourceLine(category, provenance)}${periods}`
  }).join('\n\n')
}

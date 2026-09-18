/** Source count scope and stable observation identity, independent of OSM travel direction. */

import { createHash } from 'node:crypto'

/**
 * `both-directions` is the two-way total of a road (national censuses): roads-finalize shares it
 * between the carriageways it finds and gives a lone one-way main-road row one half, because its
 * sibling was missed. `street-cross-section` is a city profile counter's total of that one street:
 * shared the same way, but a lone one-way street keeps it whole (Praha Legerova 35,800 + Sokolská
 * 32,300 = Nuselský most 68,100). A separate basis, not `unknown`, because `unknown` also labels
 * every class estimated and sources mix both kinds (Amsterdam inside NL, Auckland inside NZ), so
 * source provenance cannot carry the distinction. Order is the stored `traffic_count_basis` value.
 */
export const ROAD_COUNT_BASES = ['unknown', 'directional', 'both-directions', 'allocated', 'street-cross-section'] as const
export type RoadCountBasis = typeof ROAD_COUNT_BASES[number]
export interface RoadObservation {
  countBasis: RoadCountBasis
  /** Identity within sourceId; empty only for a derived, already allocated flow. */
  observationId: string
}

function canonicalRecord(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value) ?? 'null'
  if (Array.isArray(value)) return `[${value.map(canonicalRecord).join(',')}]`
  return `{${Object.keys(value).sort().map(key =>
    `${JSON.stringify(key)}:${canonicalRecord((value as Record<string, unknown>)[key])}`).join(',')}}`
}

/** Use the publisher's ID where available, otherwise the preserved original record. */
export function roadObservation(
  originalRecord: string | object,
  countBasis: Exclude<RoadCountBasis, 'allocated'>,
): RoadObservation {
  const observationId = typeof originalRecord === 'string' ? originalRecord
    : createHash('sha256').update(canonicalRecord(originalRecord)).digest('hex')
  if (!observationId) throw new Error('road observation identity cannot be empty')
  return { countBasis, observationId }
}

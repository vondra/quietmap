/** Source count scope and stable observation identity, independent of OSM travel direction. */

import { createHash } from 'node:crypto'

export const ROAD_COUNT_BASES = ['unknown', 'directional', 'both-directions', 'allocated'] as const
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

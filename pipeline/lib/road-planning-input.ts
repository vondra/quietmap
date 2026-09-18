/** Strict native road facts shared by continuity and final transition planning. */

import { DataType, Precision, type Table, type Vector } from 'apache-arrow'
import { segmentGeometryReader } from './prepared-grid.js'
import { ROAD_COUNT_BASES, type RoadCountBasis } from './road-observation.js'

export type Aadt = readonly [number, number, number, number]

export interface PlanningRoad {
  i: number; osmId: number; segIdx: number; cls: number; src: number
  speedTag: number; builtUp: number; access: number; roundabout: boolean
  len: number; a: string; b: string; ref: string; name: string; aadt: Aadt
}

export function readPlanningRoads(table: Table): Array<PlanningRoad & { estimatedClasses: number; countBasis: RoadCountBasis; observationId: string; observationSourceId: number }> {
  const finalized = table.schema.metadata.get('road_traffic_contract') === '1'
  const geometry = segmentGeometryReader(table), columns = new Map<string, Vector>()
  for (const [name, bits, signed] of [
    ['osm_id', 64, true], ['segment_idx', 16, true], ['road_class', 8, false],
    ['source_id', 16, false], ['speed_limit', 8, false], ['built_up', 8, false],
    ['access', 8, false], ['junction', 8, false],
  ] as const) {
    const vector = table.getChild(name)
    if (!vector || !DataType.isInt(vector.type) || vector.type.bitWidth !== bits ||
        vector.type.isSigned !== signed || vector.nullCount) throw new Error(`invalid road planning column ${name}`)
    columns.set(name, vector)
  }
  const trafficNames = ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto']
  const traffic = trafficNames.map(name => table.getChild(name))
  if (finalized || traffic.some(Boolean)) for (const [index, vector] of traffic.entries()) {
    if (!vector || !DataType.isFloat(vector.type) || vector.type.precision !== Precision.DOUBLE || vector.nullCount) {
      throw new Error(`invalid road planning column ${trafficNames[index]}`)
    }
    columns.set(trafficNames[index], vector)
  }
  const estimated = table.getChild('traffic_estimated'), basis = table.getChild('traffic_count_basis')
  const observation = table.getChild('traffic_observation_id'), origin = table.getChild('traffic_observation_source')
  const unsigned = (vector: Vector | null, bits: number) => vector && DataType.isInt(vector.type) &&
    vector.type.bitWidth === bits && !vector.type.isSigned && !vector.nullCount
  if ((estimated || finalized) && !unsigned(estimated, 8)) throw new Error('invalid road planning traffic_estimated')
  if (basis || observation || origin) {
    if (finalized || !unsigned(basis, 8) || !unsigned(origin, 16) || !observation ||
        !DataType.isUtf8(observation.type) || observation.nullCount) throw new Error('invalid road planning observation columns')
  }
  const length = table.getChild('length_m'), ref = table.getChild('ref'), name = table.getChild('name')
  if (!length || !DataType.isFloat(length.type) || length.nullCount || !ref || !DataType.isUtf8(ref.type) ||
      !name || !DataType.isUtf8(name.type)) {
    throw new Error('invalid road planning length_m/ref/name columns')
  }
  const number = (name: string, index: number) => Number(columns.get(name)?.get(index) ?? 0)
  return Array.from({ length: table.numRows }, (_, i) => {
    const endpoints = geometry.endpointKeys(i), cls = number('road_class', i), builtUp = number('built_up', i)
    const len = length.get(i) as number, osmId = number('osm_id', i)
    const aadt: Aadt = [number('aadt_light', i), number('aadt_medium', i), number('aadt_heavy', i), number('aadt_moto', i)]
    const estimatedClasses = Number(estimated?.get(i) ?? 15)
    const countBasis = ROAD_COUNT_BASES[Number(basis?.get(i) ?? (finalized ? 3 : 0))]
    const observationId = String(observation?.get(i) ?? '')
    const observationSourceId = Number(origin?.get(i) ?? 0)
    if (!countBasis || (!finalized && number('source_id', i) !== 0 &&
        ((countBasis !== 'allocated' && !observationId) || !observationSourceId || !traffic.every(Boolean)))) throw new Error(`missing road observation at row ${i}`)
    if (cls > 12 || builtUp > 2 || !Number.isFinite(len) || len < 0 || !Number.isSafeInteger(osmId) || estimatedClasses > 15 || aadt.some(value => !Number.isFinite(value) || value < 0)) {
      throw new Error(`invalid road planning values at row ${i}`)
    }
    return { i, osmId, segIdx: number('segment_idx', i), cls, src: number('source_id', i),
      speedTag: number('speed_limit', i), builtUp, access: number('access', i), roundabout: number('junction', i) !== 0,
      estimatedClasses, countBasis, observationId, observationSourceId, len, a: endpoints.startKey, b: endpoints.endKey,
      ref: ((ref.get(i) as string | null) ?? '').trim().toUpperCase().replace(/\s+/g, ''),
      name: ((name.get(i) as string | null) ?? '').trim().toLowerCase().replace(/\s+/g, ' '), aadt }
  })
}

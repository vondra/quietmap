/** Strict native road facts shared by continuity and final transition planning. */

import { DataType, type Table, type Vector } from 'apache-arrow'
import { segmentGeometryReader } from './prepared-grid.js'
import { SOURCE_ID_OSM_TRANSITION_TAPER } from './source-ids.generated.js'
import { WORLD_DEFAULT } from './road-planning-defaults.generated.js'

export type Aadt = readonly [number, number, number, number]
export const classDefault = (roadClass: number): Aadt => WORLD_DEFAULT[Math.min(Math.max(roadClass, 0), WORLD_DEFAULT.length - 1)]
export const classDefaultTotal = (roadClass: number): number => classDefault(roadClass).reduce((sum, value) => sum + value, 0)

export interface PlanningRoad {
  i: number; osmId: number; segIdx: number; cls: number; src: number
  speedTag: number; builtUp: number; access: number; roundabout: boolean
  len: number; a: string; b: string; ref: string; aadt: Aadt
}

export function readPlanningRoads(table: Table): PlanningRoad[] {
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
  if (traffic.some(Boolean)) for (const [index, vector] of traffic.entries()) {
    if (!vector || !DataType.isInt(vector.type) || vector.type.bitWidth !== 32 || !vector.type.isSigned || vector.nullCount) {
      throw new Error(`invalid road planning column ${trafficNames[index]}`)
    }
    columns.set(trafficNames[index], vector)
  }
  const length = table.getChild('length_m'), ref = table.getChild('ref')
  if (!length || !DataType.isFloat(length.type) || length.nullCount || !ref || !DataType.isUtf8(ref.type)) {
    throw new Error('invalid road planning length_m/ref columns')
  }
  const number = (name: string, index: number) => Number(columns.get(name)?.get(index) ?? 0)
  return Array.from({ length: table.numRows }, (_, i) => {
    const endpoints = geometry.endpointKeys(i), cls = number('road_class', i), builtUp = number('built_up', i)
    const len = length.get(i) as number, osmId = number('osm_id', i)
    const aadt: Aadt = [number('aadt_light', i), number('aadt_medium', i), number('aadt_heavy', i), number('aadt_moto', i)]
    if (!traffic.some(Boolean) && number('source_id', i) !== 0) throw new Error(`missing stamped traffic at row ${i}`)
    if (cls > 12 || builtUp > 2 || !Number.isFinite(len) || len < 0 || !Number.isSafeInteger(osmId) || aadt.some(value => value < 0)) {
      throw new Error(`invalid road planning values at row ${i}`)
    }
    return { i, osmId, segIdx: number('segment_idx', i), cls, src: number('source_id', i),
      speedTag: number('speed_limit', i), builtUp, access: number('access', i), roundabout: number('junction', i) !== 0,
      len, a: endpoints.startKey, b: endpoints.endKey,
      ref: ((ref.get(i) as string | null) ?? '').trim().toUpperCase().replace(/\s+/g, ''), aadt }
  })
}

/** A taper stamp owns only derived traffic; its pre-taper authored traffic is zero. */
export function restorePreTaperFacts(roads: PlanningRoad[]): void {
  for (const road of roads) if (road.src === SOURCE_ID_OSM_TRANSITION_TAPER) {
    road.src = 0
    road.aadt = [0, 0, 0, 0]
  }
}

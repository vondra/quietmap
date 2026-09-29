/** Assign each original OSM building once to its nearest eligible road within 50 m. */

import { DataType, type Table } from 'apache-arrow'
import { gridToLonLat, normalizeLongitude } from './prepared-grid.js'
import { estimateBuildingLoad, type BuildingLoad } from './trip-rates.js'
import type { ServiceRoad } from './service-tree-flow.js'

// Inherited service-tree frontage heuristic: one block's plot depth, not a standard.
export const MAX_BUFFER_M = 50
export interface ServiceBuilding { lat: number; lon: number; type: number; storeys: number; area: number | null }

const STRUCTURE_KIND_BUILDING = 0

/**
 * The OSM buildings of a structures_v5 table (its emission rows: building kind with an
 * osm_id, at the OSM centroid) with the demand storeys the structures builder derived from
 * its height ladder. Sorted by osm_id and position: the finalize step may permute rows, and
 * float load sums must not depend on file order.
 */
export function readServiceBuildings(table: Table): ServiceBuilding[] {
  if (table.schema.metadata.get('structures_contract') !== 'structures_v5' || table.schema.metadata.get('grid') !== 'z30') {
    throw new Error('service-tree requires structures_v5/z30')
  }
  for (const [name, bits, signed] of [['kind', 8, false], ['osm_id', 64, true], ['building_type', 8, false],
    ['storeys', 8, false], ['centroid_gx', 32, true], ['centroid_gy', 32, true],
    ['emission_centroid_gx', 32, true], ['emission_centroid_gy', 32, true]] as const) {
    const type = table.schema.fields.find(field => field.name === name)?.type
    if (!type || !DataType.isInt(type) || type.bitWidth !== bits || type.isSigned !== signed) {
      throw new Error(`invalid structures column ${name}`)
    }
  }
  const areaType = table.schema.fields.find(field => field.name === 'area_m2')?.type
  if (!areaType || !DataType.isFloat(areaType)) throw new Error('invalid structures area_m2')
  const rows: { osmId: bigint; gx: number; gy: number; building: ServiceBuilding }[] = []
  let row = 0
  // One flat array per batch column: per-row `get` on a chunked table costs more than the tree.
  for (const batch of table.batches) {
    const child = (name: string) => batch.getChild(name)!
    const kinds = child('kind').toArray() as Uint8Array, osmIds = child('osm_id')
    const types = child('building_type'), storeys = child('storeys'), areas = child('area_m2')
    const centroidGx = child('centroid_gx').toArray() as Int32Array, centroidGy = child('centroid_gy').toArray() as Int32Array
    const emissionGx = child('emission_centroid_gx'), emissionGy = child('emission_centroid_gy')
    const osmIdValues = osmIds.toArray() as BigInt64Array, typeValues = types.toArray() as Uint8Array
    const storeyValues = storeys.toArray() as Uint8Array, areaValues = areas.toArray() as Float32Array | Float64Array
    const emissionGxValues = emissionGx.toArray() as Int32Array, emissionGyValues = emissionGy.toArray() as Int32Array
    for (let index = 0; index < batch.numRows; index++, row++) {
      if (kinds[index] !== STRUCTURE_KIND_BUILDING || !osmIds.isValid(index)) continue
      const type = typeValues[index], storeyCount = storeyValues[index]
      const footprint = areas.isValid(index) ? areaValues[index] : null
      if (!types.isValid(index) || type > 13 || !storeys.isValid(index) || storeyCount < 1 ||
          (footprint !== null && (!Number.isFinite(footprint) || footprint < 0))) {
        throw new Error(`invalid building load at row ${row}`)
      }
      const emission = emissionGx.isValid(index) && emissionGy.isValid(index)
      const gx = emission ? emissionGxValues[index] : centroidGx[index]
      const gy = emission ? emissionGyValues[index] : centroidGy[index]
      rows.push({ osmId: osmIdValues[index], gx, gy,
        building: { ...gridToLonLat(gx, gy), type, storeys: storeyCount, area: footprint } })
    }
  }
  rows.sort((a, b) => (a.osmId < b.osmId ? -1 : a.osmId > b.osmId ? 1 : a.gx - b.gx || a.gy - b.gy))
  return rows.map(entry => entry.building)
}

export function assignBuildingsGlobally(
  roads: readonly ServiceRoad[], eligibleSegments: readonly number[], buildings: readonly ServiceBuilding[],
): Map<number, BuildingLoad> {
  const loads = new Map<number, BuildingLoad>()
  if (!buildings.length || !eligibleSegments.length) return loads
  // Preserve the historical owner-average projection and sqrt tie decisions;
  // unwrap longitude around this owner's first building for dateline owners.
  const longitudeOrigin = buildings[0].lon
  const metresPerLongitude = 111320 * Math.cos(buildings.reduce((sum, b) => sum + b.lat, 0) / buildings.length * Math.PI / 180)
  const project = (lat: number, lon: number) => [
    (Math.abs(lon - longitudeOrigin) > 180 ? longitudeOrigin + normalizeLongitude(lon - longitudeOrigin) : lon) * metresPerLongitude,
    lat * 110540,
  ]
  // |x| < 2^21 cells (540 degrees of unwrapped longitude) and |y| < 2^19 (85 degrees of latitude) make this exact and unique.
  const gridCell = (x: number, y: number) => x * 2 ** 20 + (y + 2 ** 19)
  const grid = new Map<number, number[]>()
  const segments = eligibleSegments.map(index => {
    const road = roads[index]
    const [ax, ay] = project(road.startLat, road.startLon)
    const [bx, by] = project(road.endLat, road.endLon)
    return { index, ax, ay, bx, by }
  })
  segments.forEach(({ ax, ay, bx, by }, index) => {
    for (let y = Math.floor((Math.min(ay, by) - MAX_BUFFER_M) / MAX_BUFFER_M);
      y <= Math.floor((Math.max(ay, by) + MAX_BUFFER_M) / MAX_BUFFER_M); y++) {
      for (let x = Math.floor((Math.min(ax, bx) - MAX_BUFFER_M) / MAX_BUFFER_M);
        x <= Math.floor((Math.max(ax, bx) + MAX_BUFFER_M) / MAX_BUFFER_M); x++) {
        const key = gridCell(x, y), list = grid.get(key)
        if (list) list.push(index)
        else grid.set(key, [index])
      }
    }
  })
  for (const building of buildings) {
    const [px, py] = project(building.lat, building.lon)
    const candidates = grid.get(gridCell(Math.floor(px / MAX_BUFFER_M), Math.floor(py / MAX_BUFFER_M)))
    let best = -1, distance = Infinity
    for (const candidate of candidates ?? []) {
      const { index, ax, ay, bx, by } = segments[candidate]
      const dx = bx - ax, dy = by - ay, lengthSquared = dx * dx + dy * dy
      const t = lengthSquared < 1e-6 ? 0 : Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / lengthSquared))
      const ex = px - (ax + t * dx), ey = py - (ay + t * dy)
      const nextDistance = Math.sqrt(ex * ex + ey * ey)
      if (nextDistance <= MAX_BUFFER_M && nextDistance < distance) { distance = nextDistance; best = index }
    }
    if (best < 0) continue
    const load = estimateBuildingLoad(building.type, building.storeys, building.area)
    const existing = loads.get(best)
    if (existing) { existing.dwellings += load.dwellings; existing.trips += load.trips }
    else loads.set(best, load)
  }
  return loads
}

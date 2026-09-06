/** Assign each original OSM building once to its nearest eligible road within 50 m. */

import { DataType, type Table } from 'apache-arrow'
import { gridToLonLat, normalizeLongitude } from './prepared-grid.js'
import { estimateBuildingLoad, type BuildingLoad } from './trip-rates.js'
import type { ServiceRoad } from './service-tree-flow.js'

// Inherited service-tree frontage heuristic: one block's plot depth, not a standard.
export const MAX_BUFFER_M = 50
export interface ServiceBuilding { lat: number; lon: number; type: number; floors: number; area: number | null }

export function readServiceBuildings(table: Table): ServiceBuilding[] {
  if (table.schema.metadata.get('buildings_contract') !== 'buildings_v3' || table.schema.metadata.get('grid') !== 'z30') {
    throw new Error('service-tree requires original buildings_v3/z30')
  }
  for (const [name, bits, signed] of [['centroid_gx', 32, true], ['centroid_gy', 32, true],
    ['building_type', 8, false], ['floors', 8, false]] as const) {
    const column = table.getChild(name)
    if (!column || !DataType.isInt(column.type) || column.type.bitWidth !== bits ||
        column.type.isSigned !== signed || column.nullCount) throw new Error(`invalid building column ${name}`)
  }
  const area = table.getChild('area_m2')
  if (!area || !DataType.isFloat(area.type)) throw new Error('invalid building area_m2')
  return Array.from({ length: table.numRows }, (_, index) => {
    const type = table.getChild('building_type')!.get(index) as number
    const footprint = area.get(index) as number | null
    if (type > 13 || (footprint !== null && (!Number.isFinite(footprint) || footprint < 0))) {
      throw new Error(`invalid building load at row ${index}`)
    }
    return { ...gridToLonLat(table.getChild('centroid_gx')!.get(index) as number,
      table.getChild('centroid_gy')!.get(index) as number), type,
      floors: table.getChild('floors')!.get(index) as number, area: footprint }
  })
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
  const grid = new Map<string, number[]>()
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
        const key = `${x}_${y}`, list = grid.get(key)
        if (list) list.push(index)
        else grid.set(key, [index])
      }
    }
  })
  for (const building of buildings) {
    const [px, py] = project(building.lat, building.lon)
    const candidates = grid.get(`${Math.floor(px / MAX_BUFFER_M)}_${Math.floor(py / MAX_BUFFER_M)}`)
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
    const load = estimateBuildingLoad(building.type, building.floors, building.area)
    const existing = loads.get(best)
    if (existing) { existing.dwellings += load.dwellings; existing.trips += load.trips }
    else loads.set(best, load)
  }
  return loads
}

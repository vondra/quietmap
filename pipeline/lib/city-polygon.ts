/** Strict municipal ADM2 geometry; a rectangle only bounds iteration, never ownership. */

import type { PreparedBbox } from './prepared-grid.js'
import { inBbox } from './spatial.js'

export type CityCoordinate = readonly [longitude: number, latitude: number]
type Ring = readonly CityCoordinate[]

function pointInRing(lon: number, lat: number, ring: Ring): boolean {
  let inside = false
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i], [xj, yj] = ring[j]
    if (yi > lat !== yj > lat && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi) inside = !inside
  }
  return inside
}

export function municipalityFromGeoJson(text: string, name: string) {
  const json = JSON.parse(text) as { type?: unknown; features?: Array<{
    properties?: { shapeName?: unknown }; geometry?: { type?: unknown; coordinates?: unknown }
  }> }
  if (json?.type !== 'FeatureCollection' || !Array.isArray(json.features)) throw new Error('invalid municipal FeatureCollection')
  const normalize = (value: string) => value.normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase().trim()
  const matches = json.features.filter(f => typeof f?.properties?.shapeName === 'string' && normalize(f.properties.shapeName) === normalize(name))
  if (matches.length !== 1) throw new Error(`municipality '${name}' must have exactly one ADM2 feature`)
  const geometry = matches[0].geometry
  const polygons = geometry?.type === 'Polygon' ? [geometry.coordinates] : geometry?.type === 'MultiPolygon' ? geometry.coordinates : null
  if (!Array.isArray(polygons) || polygons.length === 0) throw new Error(`invalid municipal polygon '${name}'`)
  const bbox: [number, number, number, number] = [90, 180, -90, -180]
  for (const polygon of polygons) {
    if (!Array.isArray(polygon) || !polygon.length) throw new Error(`invalid municipal rings '${name}'`)
    for (const ring of polygon) {
      if (!Array.isArray(ring) || ring.length < 4) throw new Error(`invalid municipal ring '${name}'`)
      for (const point of ring) {
        if (!Array.isArray(point) || point.length !== 2 || point.some(v => typeof v !== 'number' || !Number.isFinite(v)) ||
            Math.abs(point[0]) > 180 || Math.abs(point[1]) > 90) throw new Error(`invalid municipal coordinate '${name}'`)
        bbox[0] = Math.min(bbox[0], point[1]); bbox[1] = Math.min(bbox[1], point[0])
        bbox[2] = Math.max(bbox[2], point[1]); bbox[3] = Math.max(bbox[3], point[0])
      }
      if (ring[0][0] !== ring.at(-1)[0] || ring[0][1] !== ring.at(-1)[1]) throw new Error(`open municipal ring '${name}'`)
    }
  }
  const rings = polygons as Ring[][]
  return { bbox: bbox as PreparedBbox, contains(lat: number, lon: number): boolean {
    if (!inBbox(lat, lon, bbox)) return false
    return rings.some(polygon => pointInRing(lon, lat, polygon[0]) && !polygon.slice(1).some(hole => pointInRing(lon, lat, hole)))
  } }
}

/** Strict GeoJSON line admission and proximity index for national road sources. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'
import { pointToPolylineDist } from './spatial.js'

export interface PinnedRoadLineFile {
  relativePath: string
  sha256: string
}

export interface PinnedRoadLine {
  coordinates: readonly (readonly [number, number])[]
  properties: Readonly<Record<string, unknown>>
  relativePath: string
}

interface GeoJsonFeature {
  geometry?: { type?: unknown; coordinates?: unknown }
  properties?: unknown
}

function coordinatePair(value: unknown): readonly [number, number] | null {
  if (!Array.isArray(value) || value.length < 2) return null
  const longitude = Number(value[0]), latitude = Number(value[1])
  return Number.isFinite(longitude) && Number.isFinite(latitude) &&
    longitude >= -180 && longitude <= 180 && latitude >= -90 && latitude <= 90
    ? [longitude, latitude] : null
}

function lineParts(feature: GeoJsonFeature): readonly unknown[][] {
  const geometry = feature.geometry
  if (!geometry || !Array.isArray(geometry.coordinates)) return []
  if (geometry.type === 'LineString') return [geometry.coordinates]
  return geometry.type === 'MultiLineString' ? geometry.coordinates.filter(Array.isArray) : []
}

export function loadPinnedRoadLines(
  options: RoadLoaderArguments,
  files: readonly PinnedRoadLineFile[],
): { lines: PinnedRoadLine[]; sourceRows: number; invalidGeometrySkipped: number } {
  const lines: PinnedRoadLine[] = []
  let sourceRows = 0, invalidGeometrySkipped = 0
  for (const file of files) {
    let parsed: unknown
    try {
      parsed = JSON.parse(readPinnedRoadSource(options, file.relativePath, file.sha256).toString('utf8'))
    } catch (error) {
      if (error instanceof SyntaxError) throw new Error(`${file.relativePath}: invalid GeoJSON: ${error.message}`)
      throw error
    }
    const features = (parsed as { features?: unknown }).features
    if (!Array.isArray(features)) throw new Error(`${file.relativePath}: GeoJSON FeatureCollection has no features`)
    sourceRows += features.length
    for (const value of features) {
      if (!value || typeof value !== 'object') { invalidGeometrySkipped++; continue }
      const feature = value as GeoJsonFeature
      const properties = feature.properties && typeof feature.properties === 'object' && !Array.isArray(feature.properties)
        ? feature.properties as Record<string, unknown> : {}
      const parts = lineParts(feature)
      if (parts.length === 0) { invalidGeometrySkipped++; continue }
      let acceptedPart = false
      for (const rawPart of parts) {
        const coordinates = rawPart.map(coordinatePair)
        if (coordinates.length < 2 || coordinates.some(point => point === null)) continue
        lines.push({ coordinates: coordinates as readonly (readonly [number, number])[], properties,
          relativePath: file.relativePath })
        acceptedPart = true
      }
      if (!acceptedPart) invalidGeometrySkipped++
    }
  }
  if (sourceRows === 0 || lines.length === 0) throw new Error('national road line sources have no usable geometry')
  return { lines, sourceRows, invalidGeometrySkipped }
}

const GRID_SCALE = 100
const gridKey = (latitudeCell: number, longitudeCell: number): string => `${latitudeCell}_${longitudeCell}`

/** Dev1-compatible vertex index; separate multipart lines prevent phantom connectors. */
export function buildRoadLineVertexGrid(
  lines: readonly PinnedRoadLine[],
): ReadonlyMap<string, readonly PinnedRoadLine[]> {
  const grid = new Map<string, PinnedRoadLine[]>()
  for (const line of lines) {
    const seen = new Set<string>()
    for (const [longitude, latitude] of line.coordinates) {
      const key = gridKey(Math.floor(latitude * GRID_SCALE), Math.floor(longitude * GRID_SCALE))
      if (seen.has(key)) continue
      seen.add(key)
      const bucket = grid.get(key)
      if (bucket) bucket.push(line)
      else grid.set(key, [line])
    }
  }
  return grid
}

export function nearestRoadLine(
  latitude: number,
  longitude: number,
  grid: ReadonlyMap<string, readonly PinnedRoadLine[]>,
  radiusMetres: number,
  accept: (line: PinnedRoadLine) => boolean = () => true,
): PinnedRoadLine | null {
  const reach = Math.max(1, Math.ceil(radiusMetres / 1000))
  const latitudeCell = Math.floor(latitude * GRID_SCALE)
  const longitudeCell = Math.floor(longitude * GRID_SCALE)
  let closest: PinnedRoadLine | null = null
  let closestDistance = radiusMetres
  const visited = new Set<PinnedRoadLine>()
  for (let y = latitudeCell - reach; y <= latitudeCell + reach; y++) {
    for (let x = longitudeCell - reach; x <= longitudeCell + reach; x++) {
      for (const line of grid.get(gridKey(y, x)) ?? []) {
        if (visited.has(line) || !accept(line)) continue
        visited.add(line)
        const distance = pointToPolylineDist(latitude, longitude, line.coordinates)
        if (distance < closestDistance) {
          closest = line
          closestDistance = distance
        }
      }
    }
  }
  return closest
}

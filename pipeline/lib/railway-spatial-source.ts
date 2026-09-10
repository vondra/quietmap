/** Spatial national rail source parsing, indexing and traffic classification for CN and IN. */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { inBbox, pointToSegmentDist } from './spatial.js'
import type { RailwayRow, RailwayTraffic } from './railways-arrow.js'

export type SpatialRailCountry = 'CN' | 'IN'
type FeatureKind = 'national' | 'metro'
type Coordinate = readonly [lon: number, lat: number]

export interface SpatialRailFeature {
  coordinates: Coordinate[]
  kind: FeatureKind
  properties: Readonly<Record<string, unknown>>
}

interface SourceGeometry { type?: string; coordinates?: unknown }
interface FeatureCollection {
  features?: Array<{
    geometry?: SourceGeometry
    properties?: Record<string, unknown>
  }>
}

function geometryLines(geometry: SourceGeometry | undefined): Coordinate[][] {
  if (geometry?.type === 'LineString') return [geometry.coordinates as Coordinate[]]
  if (geometry?.type === 'MultiLineString') return geometry.coordinates as Coordinate[][]
  return []
}

function textProperty(properties: Readonly<Record<string, unknown>>, name: string): string {
  const value = properties[name]
  return typeof value === 'string' ? value.trim() : ''
}

function numberProperty(properties: Readonly<Record<string, unknown>>, name: string): number {
  const value = Number(properties[name] ?? 0)
  return Number.isFinite(value) ? value : 0
}

function acceptsFeature(country: SpatialRailCountry, kind: FeatureKind, properties: Readonly<Record<string, unknown>>): boolean {
  if (country !== 'CN') return true
  const status = textProperty(properties, 'Status')
  if (status && status !== '运营中' && status !== 'operating') return false
  return kind !== 'metro' || textProperty(properties, 'ServiceType') !== 'BRT'
}

function readFeatures(path: string, country: SpatialRailCountry, kind: FeatureKind): SpatialRailFeature[] {
  const collection = JSON.parse(readFileSync(path, 'utf8')) as FeatureCollection
  const features: SpatialRailFeature[] = []
  for (const source of collection.features ?? []) {
    const properties = source.properties ?? {}
    if (!acceptsFeature(country, kind, properties)) continue
    for (const coordinates of geometryLines(source.geometry)) {
      if (coordinates.length >= 2) features.push({ coordinates, kind, properties })
    }
  }
  return features
}

export function loadSpatialRailSource(
  sourceDirectory: string,
  country: SpatialRailCountry,
): { national: SpatialRailFeature[]; metro: SpatialRailFeature[] } {
  const names = country === 'CN'
    ? ['railway-national.geojson', 'metro-lines.geojson']
    : ['railway-network.geojson', 'metro-lines.geojson']
  const national = readFeatures(resolve(sourceDirectory, country.toLowerCase(), names[0]), country, 'national')
  const metro = readFeatures(resolve(sourceDirectory, country.toLowerCase(), names[1]), country, 'metro')
  if (national.length === 0 || metro.length === 0) {
    throw new Error(`${country} spatial rail source must contain both national and metro line features`)
  }
  return { national, metro }
}

const gridKey = (lat: number, lon: number): string => `${Math.floor(lat * 100)}_${Math.floor(lon * 100)}`

/** Index every polyline at <=0.005° steps so long source segments cannot vanish between vertex cells. */
export function buildSpatialRailGrid(features: readonly SpatialRailFeature[]): Map<string, SpatialRailFeature[]> {
  const grid = new Map<string, SpatialRailFeature[]>()
  for (const feature of features) {
    const keys = new Set<string>()
    for (let index = 0; index < feature.coordinates.length - 1; index++) {
      const [startLon, startLat] = feature.coordinates[index]
      const [endLon, endLat] = feature.coordinates[index + 1]
      const steps = Math.max(1, Math.ceil(Math.max(Math.abs(endLat - startLat), Math.abs(endLon - startLon)) / 0.005))
      for (let step = 0; step <= steps; step++) {
        const fraction = step / steps
        keys.add(gridKey(startLat + (endLat - startLat) * fraction, startLon + (endLon - startLon) * fraction))
      }
    }
    for (const key of keys) {
      const cell = grid.get(key)
      if (cell) cell.push(feature)
      else grid.set(key, [feature])
    }
  }
  return grid
}

export function nearestSpatialRailFeature(
  row: Pick<RailwayRow, 'midLat' | 'midLon'>,
  grids: readonly ReadonlyMap<string, SpatialRailFeature[]>[],
  radiusM = 500,
): SpatialRailFeature | null {
  const latitudeReach = Math.max(1, Math.ceil(radiusM / 1111.949))
  const longitudeCellM = 1111.949 * Math.max(0.05, Math.cos(row.midLat * Math.PI / 180))
  const longitudeReach = Math.max(1, Math.ceil(radiusM / longitudeCellM))
  const baseLatitude = Math.floor(row.midLat * 100)
  const baseLongitude = Math.floor(row.midLon * 100)
  const candidates = new Set<SpatialRailFeature>()
  for (const grid of grids) {
    for (let dy = -latitudeReach; dy <= latitudeReach; dy++) {
      for (let dx = -longitudeReach; dx <= longitudeReach; dx++) {
        for (const feature of grid.get(`${baseLatitude + dy}_${baseLongitude + dx}`) ?? []) candidates.add(feature)
      }
    }
  }
  let nearest: SpatialRailFeature | null = null
  let nearestDistance = radiusM
  for (const feature of candidates) {
    for (let index = 0; index < feature.coordinates.length - 1; index++) {
      const [startLon, startLat] = feature.coordinates[index]
      const [endLon, endLat] = feature.coordinates[index + 1]
      const distance = pointToSegmentDist(row.midLat, row.midLon, startLat, startLon, endLat, endLon)
      if (distance < nearestDistance) {
        nearestDistance = distance
        nearest = feature
      }
    }
  }
  return nearest
}

export function spatialRailFeatureKinds(country: SpatialRailCountry, railType: number): readonly FeatureKind[] {
  if (railType === 1 || railType === 2) return ['metro']
  if (railType !== 0) return []
  return country === 'CN' ? ['national', 'metro'] : ['national']
}

function cnTraffic(feature: SpatialRailFeature): Pick<RailwayTraffic, 'passenger' | 'freight'> {
  if (feature.kind === 'metro') {
    const service = textProperty(feature.properties, 'ServiceType')
    if (service === 'Metro Heavy Rail') return { passenger: 500, freight: 0 }
    if (service === 'Metro Express Heavy Rail') return { passenger: 400, freight: 0 }
    if (service === 'LRT' || service === 'Metro Light Rail') return { passenger: 300, freight: 0 }
    if (service === 'APM') return { passenger: 500, freight: 0 }
    if (service === 'Streetcar') return { passenger: 200, freight: 0 }
    if (service.includes('旅游')) return { passenger: 30, freight: 0 }
    return { passenger: 400, freight: 0 }
  }
  const speed = numberProperty(feature.properties, 'TopSpeed')
  if (speed >= 350) return { passenger: 180, freight: 0 }
  if (speed >= 300) return { passenger: 150, freight: 0 }
  if (speed >= 250) return { passenger: 120, freight: 0 }
  if (speed >= 200) return { passenger: 80, freight: 10 }
  if (speed >= 150) return { passenger: 50, freight: 20 }
  if (speed >= 100) return { passenger: 30, freight: 20 }
  return speed > 0 ? { passenger: 15, freight: 10 } : { passenger: 25, freight: 10 }
}

const INDIA_SUBURBAN: ReadonlyArray<{
  bbox: readonly [number, number, number, number]
  zone?: string
  passenger: number
  freight: number
}> = [
  { bbox: [18.9, 72.7, 19.4, 73.3], zone: 'central|western', passenger: 1300, freight: 30 },
  { bbox: [28.3, 76.8, 29.2, 77.6], zone: 'northern', passenger: 350, freight: 20 },
  { bbox: [22.3, 88.2, 22.8, 88.6], zone: 'eastern', passenger: 500, freight: 25 },
  { bbox: [12.8, 80.1, 13.3, 80.4], zone: 'southern', passenger: 350, freight: 20 },
  { bbox: [12.8, 77.4, 13.2, 77.8], passenger: 60, freight: 20 },
  { bbox: [17.2, 78.2, 17.6, 78.7], passenger: 60, freight: 20 },
  { bbox: [22.9, 72.4, 23.2, 72.8], passenger: 60, freight: 20 },
  { bbox: [18.4, 73.7, 18.7, 74], passenger: 60, freight: 20 },
]

function inTrafficBox(latitude: number, longitude: number, bbox: readonly [number, number, number, number]): boolean {
  return inBbox(latitude, longitude, bbox)
}

function inTraffic(feature: SpatialRailFeature, latitude: number, longitude: number): Pick<RailwayTraffic, 'passenger' | 'freight'> {
  if (feature.kind === 'metro') return { passenger: 400, freight: 0 }
  const gauge = textProperty(feature.properties, 'type').toLowerCase()
  if (gauge.includes('narrow') || gauge.includes('metre')) return { passenger: 5, freight: 0 }
  const zone = textProperty(feature.properties, 'railwayzone').toLowerCase()
  for (const tier of INDIA_SUBURBAN) {
    if (!inTrafficBox(latitude, longitude, tier.bbox)) continue
    if (tier.zone && !tier.zone.split('|').some(token => zone.includes(token))) continue
    return { passenger: tier.passenger, freight: tier.freight }
  }
  const speed = numberProperty(feature.properties, 'speed')
  if (speed >= 120) return { passenger: 30, freight: 15 }
  if (speed >= 100) return { passenger: 25, freight: 15 }
  if (speed >= 60) return { passenger: 15, freight: 10 }
  return { passenger: 8, freight: 5 }
}

export function trafficForSpatialRailFeature(
  country: SpatialRailCountry,
  feature: SpatialRailFeature,
  latitude: number,
  longitude: number,
): Pick<RailwayTraffic, 'passenger' | 'freight'> {
  return country === 'CN' ? cnTraffic(feature) : inTraffic(feature, latitude, longitude)
}

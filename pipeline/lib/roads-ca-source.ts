/** Parse the admitted Quebec MTQ DJMA line census. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SOURCE_PATH = 'ca/qc-djma.geojson'
const SOURCE_SHA256 = '702cf202250235c7d5f84729d857eb47d851a58947620bfe897853d4d58fcf4e'

export interface QuebecDjmaSection {
  sourceRow: number
  route: number
  rank: number
  latitude: number
  longitude: number
  total: number
  truckPercent: number
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface QuebecDjmaSource {
  sections: QuebecDjmaSection[]
  sourceRows: number
  unavailableTrafficSkipped: number
  invalidRouteSkipped: number
  invalidGeometrySkipped: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export function quebecRouteRank(route: number): number {
  if (route <= 99 || route >= 400) return 0
  if (route <= 199) return 1
  if (route <= 299) return 2
  return 3
}

export function normalizeQuebecOsmRef(ref: string): number | null {
  for (const token of ref.split(';')) {
    const match = /^(?:A-|R-)?(\d{1,3})(?:\s+(?:Est|Ouest|Nord|Sud))?$/i.exec(token.trim())
    if (match) return Number(match[1])
  }
  return null
}

function currentTraffic(properties: UnknownRecord): { total: number; truckPercent: number } | null {
  for (const key of ['annee_en_cours', 'annee2', 'annee3']) {
    const value = properties[key]
    if (typeof value !== 'string') continue
    const totalMatch = /DJMA\s*:\s*(\d+)/.exec(value)
    if (!totalMatch || Number(totalMatch[1]) <= 0) continue
    const truckMatch = /%cam\s*:\s*([\d.]+)/.exec(value)
    const truckPercent = truckMatch ? Number(truckMatch[1]) : 7
    if (!Number.isFinite(truckPercent) || truckPercent < 0 || truckPercent > 100) continue
    return { total: Number(totalMatch[1]), truckPercent }
  }
  return null
}

function geometryCentroid(geometry: unknown): readonly [number, number] | null {
  if (!isRecord(geometry) || !Array.isArray(geometry.coordinates) ||
      (geometry.type !== 'LineString' && geometry.type !== 'MultiLineString')) return null
  const rawLines = geometry.type === 'LineString' ? [geometry.coordinates] : geometry.coordinates
  let latitude = 0
  let longitude = 0
  let points = 0
  for (const rawLine of rawLines) {
    if (!Array.isArray(rawLine)) return null
    for (const point of rawLine) {
      if (!Array.isArray(point) || typeof point[0] !== 'number' || typeof point[1] !== 'number' ||
          !Number.isFinite(point[0]) || !Number.isFinite(point[1])) return null
      longitude += point[0]
      latitude += point[1]
      points++
    }
  }
  return points ? [latitude / points, longitude / points] : null
}

function splitQuebecDjma(total: number, truckPercent: number) {
  const moto = Math.round(total * 0.01)
  const heavyTotal = Math.min(total - moto, Math.round(total * truckPercent / 100))
  const medium = Math.round(heavyTotal * 0.20)
  return { light: total - moto - heavyTotal, medium, heavy: heavyTotal - medium, moto }
}

export function parseQuebecDjmaSource(raw: string): QuebecDjmaSource {
  const parsed = JSON.parse(raw) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error('Quebec DJMA source must be a GeoJSON FeatureCollection')
  }
  const result: QuebecDjmaSource = { sections: [], sourceRows: parsed.features.length,
    unavailableTrafficSkipped: 0, invalidRouteSkipped: 0, invalidGeometrySkipped: 0 }
  for (let sourceRow = 0; sourceRow < parsed.features.length; sourceRow++) {
    const feature = parsed.features[sourceRow]
    const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
    const traffic = properties ? currentTraffic(properties) : null
    if (!traffic) {
      result.unavailableTrafficSkipped++
      continue
    }
    const rtss = String(properties!.rtss_debut ?? '')
    const route = /^\d{5}/.test(rtss) ? Number(rtss.slice(0, 5)) : 0
    if (route <= 0 || route >= 1000) {
      result.invalidRouteSkipped++
      continue
    }
    const centroid = geometryCentroid(isRecord(feature) ? feature.geometry : null)
    if (!centroid) {
      result.invalidGeometrySkipped++
      continue
    }
    result.sections.push({ sourceRow, route, rank: quebecRouteRank(route),
      latitude: centroid[0], longitude: centroid[1], ...traffic,
      ...splitQuebecDjma(traffic.total, traffic.truckPercent) })
  }
  if (result.sections.length === 0) throw new Error('Quebec DJMA source has no usable measurements')
  return result
}

export function loadQuebecDjmaSource(options: RoadLoaderArguments): QuebecDjmaSource {
  return parseQuebecDjmaSource(readPinnedRoadSource(options, SOURCE_PATH, SOURCE_SHA256).toString('utf8'))
}

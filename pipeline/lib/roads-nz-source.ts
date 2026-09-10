/** Parse admitted NZTA carriageway and Auckland Transport AADT sources. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const NZTA_PAGES = [
  ['nzta-page-0.json', 'eeaec4f8bdebb3ebcea119d1a84b8759de7cd5081c60257a51db1387da5e35ea', 2000],
  ['nzta-page-2000.json', 'c990b9c629af46165b9a7e5e302c34614aa29aafbf93880958cd5fe9ea8b52a3', 2000],
  ['nzta-page-4000.json', '696e69daf4c1ba2bdcce1d1de894b34f418ba7b265a4d7eda3424eae9e0b9699', 2000],
  ['nzta-page-6000.json', '1b5d92ed1624fa923d28beeb8b3b4e2363b9e487b98ac3aa4397df1987d79f11', 2000],
  ['nzta-page-8000.json', '8c5f836357015458776a45fe22e61d2eb20ccf8c7f59848facaca79d725281d4', 2000],
  ['nzta-page-10000.json', 'd45e66cde63fce72bbd71a8b88f729ca5f196e1c11b8807f520a3c96d21e9dd4', 835],
] as const
const AT_FILE = 'at-aadt.geojson'
const AT_SHA256 = '5885d1c383bd2339a896812b802a9c4e4da7aa0795ba910d8b3db23b012226d9'

export interface NewZealandRoadObservation {
  source: 'nzta' | 'at'
  sourceRow: number
  latitude: number
  longitude: number
  rank: number | null
  total: number
  heavyPercent: number
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface NewZealandRoadSource {
  observations: NewZealandRoadObservation[]
  nztaRows: number
  atRows: number
  unavailableTrafficSkipped: number
  unsupportedClassSkipped: number
  invalidGeometrySkipped: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export function newZealandOnrcRank(value: unknown): number | null {
  switch (value) {
    case 'High Volume': case 'National': return 0
    case 'Regional': return 1
    case 'Arterial': return 2
    case 'Primary Collector': return 3
    case 'Secondary Collector': return 4
    default: return null
  }
}

function featureCollection(raw: string, label: string): unknown[] {
  const parsed = JSON.parse(raw) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error(`${label} must be a GeoJSON FeatureCollection`)
  }
  return parsed.features
}

function centroid(geometry: unknown): readonly [number, number] | null {
  if (!isRecord(geometry) || !Array.isArray(geometry.coordinates)) return null
  if (geometry.type === 'Point') {
    const point = geometry.coordinates
    return typeof point[0] === 'number' && typeof point[1] === 'number' &&
      Number.isFinite(point[0]) && Number.isFinite(point[1]) ? [point[1], point[0]] : null
  }
  if (geometry.type !== 'LineString' && geometry.type !== 'MultiLineString') return null
  const lines = geometry.type === 'LineString' ? [geometry.coordinates] : geometry.coordinates
  let latitude = 0
  let longitude = 0
  let points = 0
  for (const line of lines) {
    if (!Array.isArray(line)) return null
    for (const point of line) {
      if (!Array.isArray(point) || typeof point[0] !== 'number' || typeof point[1] !== 'number' ||
          !Number.isFinite(point[0]) || !Number.isFinite(point[1])) return null
      longitude += point[0]
      latitude += point[1]
      points++
    }
  }
  return points ? [latitude / points, longitude / points] : null
}

function positiveInteger(...values: unknown[]): number | null {
  for (const value of values) {
    if (typeof value === 'number' && Number.isFinite(value) &&
        Number.isSafeInteger(Math.round(value)) && Math.round(value) > 0) return Math.round(value)
  }
  return null
}

function split(total: number, heavyPercent: number) {
  const moto = Math.round(total * 0.01)
  const heavyTotal = Math.min(total - moto, Math.round(total * heavyPercent / 100))
  const medium = Math.round(heavyTotal * 0.20)
  return { light: total - moto - heavyTotal, medium, heavy: heavyTotal - medium, moto }
}

export function parseNewZealandRoadSources(
  nztaSources: readonly string[],
  atSource: string,
): NewZealandRoadSource {
  const result: NewZealandRoadSource = { observations: [], nztaRows: 0, atRows: 0,
    unavailableTrafficSkipped: 0, unsupportedClassSkipped: 0, invalidGeometrySkipped: 0 }
  for (const [page, raw] of nztaSources.entries()) {
    const features = featureCollection(raw, `NZTA page ${page}`)
    result.nztaRows += features.length
    for (let sourceRow = 0; sourceRow < features.length; sourceRow++) {
      const feature = features[sourceRow]
      const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
      const total = properties ? positiveInteger(properties.trafficADTEst, properties.trafficADTCount) : null
      if (total === null) {
        result.unavailableTrafficSkipped++
        continue
      }
      const rank = newZealandOnrcRank(properties!.ONRC)
      if (rank === null) {
        result.unsupportedClassSkipped++
        continue
      }
      const point = centroid(isRecord(feature) ? feature.geometry : null)
      if (!point) {
        result.invalidGeometrySkipped++
        continue
      }
      const rawHeavy = properties!.loadingPcHeavy
      const heavyPercent = typeof rawHeavy === 'number' && Number.isFinite(rawHeavy) &&
        rawHeavy >= 0 && rawHeavy <= 100 ? rawHeavy : 8
      result.observations.push({ source: 'nzta', sourceRow: page * 2000 + sourceRow,
        latitude: point[0], longitude: point[1], rank, total, heavyPercent,
        ...split(total, heavyPercent) })
    }
  }
  const atFeatures = featureCollection(atSource, 'Auckland Transport AADT')
  result.atRows = atFeatures.length
  for (let sourceRow = 0; sourceRow < atFeatures.length; sourceRow++) {
    const feature = atFeatures[sourceRow]
    const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
    const total = properties ? positiveInteger(properties.adt) : null
    if (total === null) {
      result.unavailableTrafficSkipped++
      continue
    }
    const point = centroid(isRecord(feature) ? feature.geometry : null)
    if (!point) {
      result.invalidGeometrySkipped++
      continue
    }
    const rawHeavy = properties!.pcheavy
    const heavyPercent = typeof rawHeavy === 'number' && Number.isFinite(rawHeavy) &&
      rawHeavy >= 0 && rawHeavy <= 100 ? rawHeavy : 0
    result.observations.push({ source: 'at', sourceRow, latitude: point[0], longitude: point[1],
      rank: null, total, heavyPercent, ...split(total, heavyPercent) })
  }
  if (result.observations.length === 0) throw new Error('New Zealand road sources have no usable measurements')
  return result
}

export function loadNewZealandRoadSources(options: RoadLoaderArguments): NewZealandRoadSource {
  const pages = NZTA_PAGES.map(([name, digest, rows]) => {
    const raw = readPinnedRoadSource(options, `nz/${name}`, digest).toString('utf8')
    const actual = featureCollection(raw, name).length
    if (actual !== rows) throw new Error(`${name}: expected ${rows} rows, found ${actual}`)
    return raw
  })
  const at = readPinnedRoadSource(options, `nz/${AT_FILE}`, AT_SHA256).toString('utf8')
  return parseNewZealandRoadSources(pages, at)
}

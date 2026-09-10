/** Parse the admitted Anas TGM point census for Italian state roads. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SOURCE_PATH = 'it/tgm-roads.geojson'
const SOURCE_SHA256 = '156bfe8936a4b595e453656e956080b2447be80e1b55bdd57e8f4af3f434bfea'
const ITALY_BBOX = [35.5, 6.6, 47.1, 18.6] as const

export interface ItalianTgmStation {
  sourceRow: number
  ref: string
  latitude: number
  longitude: number
  total: number
}

export interface ItalianTgmSource {
  stations: ItalianTgmStation[]
  sourceRows: number
  invalidGeometrySkipped: number
  invalidTrafficSkipped: number
  invalidMetadataSkipped: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export function normalizeAnasRef(value: string): string {
  const base = value.trim().replace(/(dir(?:-[a-z])?|bis|ter|quater|radd)$/i, '').trim()
  for (const [pattern, prefix] of [
    [/^A\s*0*(\d+)$/i, 'A'],
    [/^RA\s*0*(\d+)$/i, 'RA '],
    [/^SS\s*0*(\d+)$/i, 'SS '],
    [/^NSA\s*0*(\d+)$/i, 'NSA '],
  ] as const) {
    const match = pattern.exec(base)
    if (match) return `${prefix}${match[1]}`
  }
  return ''
}

export function normalizeItalianOsmRef(value: string): string {
  for (const part of value.split(';').map(ref => ref.trim())) {
    if (/^E\s*\d/i.test(part)) continue
    const regional = /^SR\s*0*(\d+)$/i.exec(part)
    if (regional) return `SS ${regional[1]}`
    const state = /^(?:SS|S\.S\.)\s*0*(\d+)$/i.exec(part)
    if (state) return `SS ${state[1]}`
    const normalized = normalizeAnasRef(part)
    if (normalized) return normalized
  }
  return ''
}

export function parseItalianTgmSource(raw: string): ItalianTgmSource {
  const parsed = JSON.parse(raw) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error('Italian TGM source must be a GeoJSON FeatureCollection')
  }
  const result: ItalianTgmSource = {
    stations: [], sourceRows: parsed.features.length,
    invalidGeometrySkipped: 0, invalidTrafficSkipped: 0, invalidMetadataSkipped: 0,
  }
  for (let sourceRow = 0; sourceRow < parsed.features.length; sourceRow++) {
    const feature = parsed.features[sourceRow]
    const properties = isRecord(feature) && feature.type === 'Feature' &&
      isRecord(feature.properties) ? feature.properties : null
    const ref = properties && typeof properties.Strada === 'string'
      ? normalizeAnasRef(properties.Strada) : ''
    if (!properties || !ref) {
      result.invalidMetadataSkipped++
      continue
    }
    const total = properties.TGM
    if (typeof total !== 'number' || !Number.isFinite(total) ||
        !Number.isSafeInteger(Math.round(total)) || Math.round(total) <= 0) {
      result.invalidTrafficSkipped++
      continue
    }
    const geometry = isRecord(feature.geometry) ? feature.geometry : null
    const coordinates = geometry?.type === 'Point' && Array.isArray(geometry.coordinates)
      ? geometry.coordinates : null
    const longitude = coordinates?.[0]
    const latitude = coordinates?.[1]
    if (typeof longitude !== 'number' || typeof latitude !== 'number' ||
        !Number.isFinite(longitude) || !Number.isFinite(latitude) ||
        latitude < ITALY_BBOX[0] || latitude > ITALY_BBOX[2] ||
        longitude < ITALY_BBOX[1] || longitude > ITALY_BBOX[3]) {
      result.invalidGeometrySkipped++
      continue
    }
    result.stations.push({ sourceRow, ref, latitude, longitude, total: Math.round(total) })
  }
  if (result.stations.length === 0) throw new Error('Italian TGM source has no usable measurements')
  return result
}

export function loadItalianTgmSource(options: RoadLoaderArguments): ItalianTgmSource {
  return parseItalianTgmSource(readPinnedRoadSource(options, SOURCE_PATH, SOURCE_SHA256).toString('utf8'))
}

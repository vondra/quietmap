/** Parse the admitted Norwegian NVDB Trafikkmengde cache. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SOURCE_PATH = 'no/nvdb-trafikkmengde.json'
const SOURCE_SHA256 = 'a505e2e66cdfb853d1df10907edfa1d0c3a0b109dd342ddf15367f1380576c10'
const NORWAY_BBOX = [57.9, 4.5, 71.2, 31.2] as const

export interface NorwegianNvdbSegment {
  sourceId: number
  vegref: string
  rank: number
  latitude: number
  longitude: number
  year: number
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface NorwegianNvdbSource {
  segments: NorwegianNvdbSegment[]
  sourceRows: number
  unsupportedRoadCategorySkipped: number
  invalidGeometrySkipped: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export function norwegianVegrefRank(vegref: string): number | null {
  const prefix = vegref.slice(0, 2).toUpperCase()
  if (prefix === 'EV' || prefix === 'RV') return 1
  if (prefix === 'FV') return 2
  return null
}

export function splitNorwegianAadt(total: number, heavyPercent: number) {
  const moto = Math.round(total * 0.01)
  const heavyTotal = Math.min(total - moto, Math.round(total * heavyPercent / 100))
  const medium = Math.round(heavyTotal * 0.25)
  const heavy = heavyTotal - medium
  return { light: total - moto - heavyTotal, medium, heavy, moto }
}

export function parseNorwegianNvdbSource(raw: string): NorwegianNvdbSource {
  const parsed = JSON.parse(raw) as unknown
  if (!Array.isArray(parsed) || parsed.length === 0) throw new Error('Norwegian NVDB source must be a non-empty array')
  const result: NorwegianNvdbSource = {
    segments: [], sourceRows: parsed.length,
    unsupportedRoadCategorySkipped: 0, invalidGeometrySkipped: 0,
  }
  for (let index = 0; index < parsed.length; index++) {
    const row = parsed[index]
    if (!isRecord(row) || !Number.isSafeInteger(row.id) || (row.id as number) <= 0 ||
        typeof row.vegref !== 'string' || !Number.isSafeInteger(row.aadt) || (row.aadt as number) <= 0 ||
        !Number.isSafeInteger(row.heavyPct) || (row.heavyPct as number) < 0 ||
        (row.heavyPct as number) > 100 || !Number.isSafeInteger(row.year) || (row.year as number) <= 0) {
      throw new Error(`Norwegian NVDB row ${index} has invalid metadata or traffic values`)
    }
    const sourceId = row.id as number
    const total = row.aadt as number
    const heavyPercent = row.heavyPct as number
    const year = row.year as number
    const rank = norwegianVegrefRank(row.vegref)
    if (rank === null) {
      result.unsupportedRoadCategorySkipped++
      continue
    }
    if (typeof row.midLat !== 'number' || typeof row.midLon !== 'number' ||
        !Number.isFinite(row.midLat) || !Number.isFinite(row.midLon) ||
        row.midLat < NORWAY_BBOX[0] || row.midLat > NORWAY_BBOX[2] ||
        row.midLon < NORWAY_BBOX[1] || row.midLon > NORWAY_BBOX[3]) {
      result.invalidGeometrySkipped++
      continue
    }
    result.segments.push({
      sourceId,
      vegref: row.vegref,
      rank,
      latitude: row.midLat,
      longitude: row.midLon,
      year,
      ...splitNorwegianAadt(total, heavyPercent),
    })
  }
  if (result.segments.length === 0) throw new Error('Norwegian NVDB source has no usable measurements')
  return result
}

export function loadNorwegianNvdbSource(options: RoadLoaderArguments): NorwegianNvdbSource {
  return parseNorwegianNvdbSource(readPinnedRoadSource(options, SOURCE_PATH, SOURCE_SHA256).toString('utf8'))
}

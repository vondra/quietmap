/** Parse the admitted GDDKiA GPR 2020/2021 segment cache. */

import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SOURCE_PATH = 'pl/gpr-2020-parsed-v3.json'
const SOURCE_SHA256 = '9e1ca606167c1800ee23096640f5ba1be99b30069581cd1edcbd368d1fab9480'

export interface PolishGprSegment {
  sourceId: string
  ref: string
  isProvincial: boolean
  coordinates: ReadonlyArray<readonly [number, number]> | null
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface PolishGprSource {
  segments: PolishGprSegment[]
  sourceRows: number
  zeroTrafficSkipped: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)
const isCount = (value: unknown): value is number =>
  Number.isSafeInteger(value) && (value as number) >= 0

function lineCoordinates(value: unknown): ReadonlyArray<readonly [number, number]> | null {
  if (!Array.isArray(value) || value.length < 2) return null
  const result: Array<readonly [number, number]> = []
  for (const coordinate of value) {
    if (!Array.isArray(coordinate) || coordinate.length < 2 ||
        typeof coordinate[0] !== 'number' || typeof coordinate[1] !== 'number' ||
        !Number.isFinite(coordinate[0]) || !Number.isFinite(coordinate[1]) ||
        coordinate[0] < 14 || coordinate[0] > 24.5 ||
        coordinate[1] < 49 || coordinate[1] > 55) return null
    result.push([coordinate[0], coordinate[1]])
  }
  return result
}

export function parsePolishGprSource(raw: string): PolishGprSource {
  const parsed = JSON.parse(raw) as unknown
  if (!Array.isArray(parsed) || parsed.length === 0) throw new Error('Polish GPR source must be a non-empty array')
  const result: PolishGprSource = { segments: [], sourceRows: parsed.length, zeroTrafficSkipped: 0 }
  for (let index = 0; index < parsed.length; index++) {
    const row = parsed[index]
    if (!isRecord(row) || typeof row.nr2020 !== 'string' || !row.nr2020 ||
        typeof row.ref !== 'string' || !row.ref ||
        typeof row.isProvincial !== 'boolean' ||
        !isCount(row.aadt_light) || !isCount(row.aadt_medium) ||
        !isCount(row.aadt_heavy) || !isCount(row.aadt_moto)) {
      throw new Error(`Polish GPR row ${index} has invalid metadata or traffic classes`)
    }
    const coordinates = row.isProvincial ? null : lineCoordinates(row.coords)
    if (!row.isProvincial && coordinates === null) {
      throw new Error(`Polish GPR national row ${index} has invalid line geometry`)
    }
    const total = row.aadt_light + row.aadt_medium + row.aadt_heavy + row.aadt_moto
    if (total === 0) {
      result.zeroTrafficSkipped++
      continue
    }
    result.segments.push({
      sourceId: row.nr2020,
      ref: row.ref,
      isProvincial: row.isProvincial,
      coordinates,
      light: row.aadt_light,
      medium: row.aadt_medium,
      heavy: row.aadt_heavy,
      moto: row.aadt_moto,
    })
  }
  if (result.segments.length === 0) throw new Error('Polish GPR source has no usable measurements')
  return result
}

export function loadPolishGprSource(options: RoadLoaderArguments): PolishGprSource {
  return parsePolishGprSource(readPinnedRoadSource(options, SOURCE_PATH, SOURCE_SHA256).toString('utf8'))
}

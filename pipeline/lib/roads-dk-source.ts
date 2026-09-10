/** Parse admitted Vejdirektoratet Mastra traffic-count pages. */

import proj4 from 'proj4'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const MASTRA_PAGES = [
  ['mastra-page-0.json', '195f201c44e59fb07c5b60cf8d96d072e8ef8bb3a70a39cb4174e406f62000ed', 3000],
  ['mastra-page-3000.json', 'ca821c5c8c4a9a93b56bdb0d2cccdd1970ca87319eb52b9bc9f48a5b26c2404c', 3000],
  ['mastra-page-6000.json', '85e87b6d2b366602c9bbcfb39623dbfeb6b986e88126496f5f1cdd1089c97480', 3000],
  ['mastra-page-9000.json', '3ad64683ddf57b2a09e1ca100cbdd2f34c4021f0e5f0d4c37f4f62fa277d9761', 3000],
  ['mastra-page-12000.json', 'ed2dd484bb56d417a5bec1987ff69ba09b5078ff2d61e04d1e24b9e6b057774e', 3000],
  ['mastra-page-15000.json', '0adebcdc71803589e2647855fb92d0bdd15c27dc4187fd31781bd779e5e8f7b6', 3000],
  ['mastra-page-18000.json', 'babe9731422ca4015635c045dd7d3d1415705c47d70d05748111632875c4dfa7', 3000],
  ['mastra-page-21000.json', '38b7bd82ca5e4e1fbab40f0a15f59f5f056038dc9d2ffb282b2edbcc30c11c4d', 1089],
] as const
const DENMARK_BBOX = [54.5, 8, 57.8, 13] as const

proj4.defs('EPSG:25832', '+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

export interface DanishMastraObservation {
  sourceRow: number
  roadNumber: number
  kilometre: number
  year: number
  latitude: number
  longitude: number
  rank: number
  total: number
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface DanishMastraSource {
  observations: DanishMastraObservation[]
  sourceRows: number
  admittedRecords: number
  nonMotorSkipped: number
  invalidRowsSkipped: number
  outOfBoundsSkipped: number
  supersededRecords: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

function finiteNumber(value: unknown): number | null {
  const number = typeof value === 'number' ? value :
    typeof value === 'string' && value.trim() ? Number(value) : NaN
  return Number.isFinite(number) ? number : null
}

function featureCollection(raw: string, label: string): unknown[] {
  const parsed = JSON.parse(raw) as unknown
  if (!isRecord(parsed) || parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features)) {
    throw new Error(`${label} must be a GeoJSON FeatureCollection`)
  }
  return parsed.features
}

function splitDanishTraffic(total: number, publishedHeavy: number) {
  const moto = Math.round(total * 0.01)
  const heavyTotal = Math.min(total - moto, Math.max(0, Math.round(publishedHeavy)))
  const medium = Math.round(heavyTotal * 0.05)
  return { light: total - moto - heavyTotal, medium, heavy: heavyTotal - medium, moto }
}

export function parseDanishMastraPages(rawPages: readonly string[]): DanishMastraSource {
  const result: DanishMastraSource = { observations: [], sourceRows: 0, admittedRecords: 0,
    nonMotorSkipped: 0, invalidRowsSkipped: 0, outOfBoundsSkipped: 0, supersededRecords: 0 }
  const latest = new Map<string, DanishMastraObservation>()
  let sourceRow = 0
  for (const [page, raw] of rawPages.entries()) {
    const features = featureCollection(raw, `Mastra page ${page}`)
    result.sourceRows += features.length
    for (const feature of features) {
      const properties = isRecord(feature) && isRecord(feature.properties) ? feature.properties : null
      if (properties?.KOERETOEJSART !== undefined && properties.KOERETOEJSART !== 'MOTORKTJ') {
        result.nonMotorSkipped++
        sourceRow++
        continue
      }
      const coordinates = isRecord(feature) && isRecord(feature.geometry) &&
        feature.geometry.type === 'Point' && Array.isArray(feature.geometry.coordinates) ?
        feature.geometry.coordinates : null
      const x = coordinates ? finiteNumber(coordinates[0]) : null
      const y = coordinates ? finiteNumber(coordinates[1]) : null
      const roadNumber = finiteNumber(properties?.VEJNR)
      const kilometre = finiteNumber(properties?.KILOMETER)
      const year = finiteNumber(properties?.AAR)
      const totalValue = finiteNumber(properties?.AADT)
      const publishedHeavy = finiteNumber(properties?.LBIL_AADT) ?? 0
      if (x === null || y === null || roadNumber === null || !Number.isSafeInteger(roadNumber) ||
          roadNumber <= 0 || kilometre === null || year === null || !Number.isInteger(year) ||
          totalValue === null || !Number.isSafeInteger(Math.round(totalValue)) || totalValue <= 0) {
        result.invalidRowsSkipped++
        sourceRow++
        continue
      }
      const [longitude, latitude] = proj4('EPSG:25832', 'WGS84', [x, y])
      if (latitude < DENMARK_BBOX[0] || latitude > DENMARK_BBOX[2] ||
          longitude < DENMARK_BBOX[1] || longitude > DENMARK_BBOX[3]) {
        result.outOfBoundsSkipped++
        sourceRow++
        continue
      }
      const total = Math.round(totalValue)
      const observation: DanishMastraObservation = { sourceRow, roadNumber, kilometre, year,
        latitude, longitude, rank: String(properties?.VEJBESTYRER ?? '').trim() === '0' ? 1 : 4,
        total, ...splitDanishTraffic(total, publishedHeavy) }
      result.admittedRecords++
      const key = `${roadNumber}_${kilometre}`
      const previous = latest.get(key)
      if (!previous || observation.year > previous.year) latest.set(key, observation)
      sourceRow++
    }
  }
  result.observations = [...latest.values()]
  result.supersededRecords = result.admittedRecords - result.observations.length
  if (result.observations.length === 0) throw new Error('Mastra source has no usable traffic observations')
  return result
}

export function loadDanishMastraSource(options: RoadLoaderArguments): DanishMastraSource {
  const pages = MASTRA_PAGES.map(([name, digest, expectedRows]) => {
    const raw = readPinnedRoadSource(options, `dk/${name}`, digest).toString('utf8')
    const actualRows = featureCollection(raw, name).length
    if (actualRows !== expectedRows) {
      throw new Error(`${name}: expected ${expectedRows} rows, found ${actualRows}`)
    }
    return raw
  })
  return parseDanishMastraPages(pages)
}

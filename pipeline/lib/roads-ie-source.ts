/** Parse admitted TII counter sites and one pre-COVID weekday class aggregate. */

import { parse } from 'csv-parse/sync'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SITES_PATH = 'ie/tii-tmu-sites.json'
const SITES_SHA256 = '442c59ae78b5b9cf2d9f4043695463bccdd89c7f8c8e16e2daaca90e542b09d9'
const COUNTS_PATH = 'ie/tii-aggr-2019-06-19.csv'
const COUNTS_SHA256 = 'fb41b362dd62c05c405b4210a5b3feef4e064c665dd486fe9e0f914d93801efc'
const IRELAND_BBOX = [51.4, -10.5, 55.4, -5.4] as const

export interface IrishTiiObservation {
  cosit: string
  ref: string
  latitude: number
  longitude: number
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface IrishTiiSource {
  observations: IrishTiiObservation[]
  siteRows: number
  countRows: number
  usableSites: number
  countedSites: number
  unsupportedClassRows: number
}

type UnknownRecord = Record<string, unknown>
const isRecord = (value: unknown): value is UnknownRecord =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export function normalizeIrishRoadRef(value: string): string {
  const match = /(?:^|\s)([MNRL])[- ]*0*(\d+)\b/i.exec(value)
  return match ? `${match[1].toUpperCase()}${match[2]}` : ''
}

export function parseIrishTiiSource(sitesRaw: string, countsRaw: string): IrishTiiSource {
  const parsedSites = JSON.parse(sitesRaw) as unknown
  if (!Array.isArray(parsedSites) || parsedSites.length === 0) {
    throw new Error('TII sites source must be a non-empty array')
  }
  const sites = new Map<string, { ref: string; latitude: number; longitude: number }>()
  for (const value of parsedSites) {
    if (!isRecord(value) || typeof value.cosit !== 'string' || !isRecord(value.location)) continue
    const latitude = value.location.lat
    const longitude = value.location.lng
    const ref = normalizeIrishRoadRef(String(value.name ?? ''))
    if (typeof latitude !== 'number' || typeof longitude !== 'number' || !ref ||
        latitude < IRELAND_BBOX[0] || latitude > IRELAND_BBOX[2] ||
        longitude < IRELAND_BBOX[1] || longitude > IRELAND_BBOX[3]) continue
    sites.set(value.cosit, { ref, latitude, longitude })
  }
  const rows = parse(countsRaw, { columns: true, skip_empty_lines: true, bom: true }) as UnknownRecord[]
  const counts = new Map<string, number[]>()
  let unsupportedClassRows = 0
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index]
    const cosit = row.cosit
    const vehicleClass = Number(row.class)
    const count = Number(row.VehicleCount)
    if (typeof cosit !== 'string' || !Number.isInteger(vehicleClass) ||
        !Number.isSafeInteger(count) || count < 0) {
      throw new Error(`TII count row ${index} has invalid class or count`)
    }
    if (vehicleClass < 1 || vehicleClass > 7) {
      unsupportedClassRows++
      continue
    }
    const classes = counts.get(cosit) ?? [0, 0, 0, 0, 0, 0, 0]
    classes[vehicleClass - 1] += count
    counts.set(cosit, classes)
  }
  const observations: IrishTiiObservation[] = []
  for (const [cosit, classes] of counts) {
    const site = sites.get(cosit)
    const total = classes.reduce((sum, count) => sum + count, 0)
    if (!site || total < 100) continue
    observations.push({ cosit, ...site,
      moto: classes[0], light: classes[1] + classes[2], medium: classes[3],
      heavy: classes[4] + classes[5] + classes[6] })
  }
  if (observations.length === 0) throw new Error('TII source has no usable traffic observations')
  return { observations, siteRows: parsedSites.length, countRows: rows.length,
    usableSites: sites.size, countedSites: counts.size, unsupportedClassRows }
}

export function loadIrishTiiSource(options: RoadLoaderArguments): IrishTiiSource {
  const sites = readPinnedRoadSource(options, SITES_PATH, SITES_SHA256).toString('utf8')
  const counts = readPinnedRoadSource(options, COUNTS_PATH, COUNTS_SHA256).toString('utf8')
  return parseIrishTiiSource(sites, counts)
}

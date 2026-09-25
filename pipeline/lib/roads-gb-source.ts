/** Read the retained DfT AADF release: latest major-road count per point with its slip-road flag, latest manual minor-road count. */

import { withholdsCountPoint } from './count-holdout.js'
import { spawn } from 'node:child_process'
import { resolve } from 'node:path'
import { parse } from 'csv-parse'
import { readPinnedRoadSource } from './pinned-road-source.js'
import { roadObservation, type RoadObservation } from './road-observation.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { disjointVehicleClassCountsFitPublishedTotal } from './roads-arrow.js'
import { flatDist, type RankedPoint } from './spatial.js'

// DfT road traffic statistics, AADF by count point, all years; server Last-Modified 2026-06-03 (OGL v3.0).
const RELEASE_ZIP = 'gb/dft_traffic_counts_aadf.zip'
const RELEASE_ZIP_SHA256 = '5273cf63da4ca8b317bfe972d8a345e7932db0fff7a7789998a890f0081232b6'
const RELEASE_CSV = 'dft_traffic_counts_aadf.csv'

/** DfT's own road class as an OSM rank: motorway, trunk A, principal A, B road, then C or unclassified. */
const DFT_CATEGORY_RANK: Readonly<Record<string, number>> = { TM: 0, PM: 0, TA: 1, PA: 2, MB: 3, MCU: 5 }
export const DFT_MINOR_ROAD_RANK = DFT_CATEGORY_RANK.MCU

export interface DftCountPoint extends RoadObservation, RankedPoint {
  ref: string
  roadCategory: string
  light: number
  medium: number
  heavy: number
  moto: number
  total: number
  year: number
  rank: number
  isRamp: boolean
  startJunction: string
  endJunction: string
  /** Null where DfT publishes none (every minor-road point). */
  linkLengthKm: number | null
}

type CsvRow = Record<string, string>

function nonNegativeInteger(row: CsvRow, name: string): number {
  const raw = row[name]?.trim() ?? ''
  if (raw === '') return 0
  const value = Number(raw)
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`invalid DfT integer '${name}': ${JSON.stringify(raw)}`)
  }
  return value
}

function countPoint(row: CsvRow): DftCountPoint | null {
  const id = row.count_point_id?.trim()
  const year = Number(row.year)
  const latitude = Number(row.latitude)
  const longitude = Number(row.longitude)
  const rank = DFT_CATEGORY_RANK[row.road_category ?? '']
  if (!id || rank === undefined || !Number.isSafeInteger(year) || !Number.isFinite(latitude) ||
      !Number.isFinite(longitude) || latitude < 49 || latitude > 61 ||
      longitude < -8.5 || longitude > 2.5) return null
  const link = Number(row.link_length_km)
  const point: DftCountPoint = { ...roadObservation(id, 'both-directions'),
    ref: (row.road_name ?? '').replace(/\s+/g, ''),
    latitude,
    longitude,
    roadCategory: row.road_category,
    light: nonNegativeInteger(row, 'cars_and_taxis') + nonNegativeInteger(row, 'LGVs'),
    medium: nonNegativeInteger(row, 'buses_and_coaches'),
    heavy: nonNegativeInteger(row, 'all_HGVs'),
    moto: nonNegativeInteger(row, 'two_wheeled_motor_vehicles'),
    total: nonNegativeInteger(row, 'all_motor_vehicles'),
    year,
    rank,
    isRamp: false,
    startJunction: row.start_junction_road_name ?? '',
    endJunction: row.end_junction_road_name ?? '',
    linkLengthKm: row.link_length_km?.trim() && Number.isFinite(link) ? link : null,
  }
  // DfT independently rounds the four class AADFs and published total, so a
  // four-class sum may exceed total by at most two; larger excess is invalid.
  return point.total > 0 && point.light + point.medium + point.heavy + point.moto > 0 &&
    disjointVehicleClassCountsFitPublishedTotal(
      point.total, [point.light, point.medium, point.heavy, point.moto], 'independently-rounded',
    ) ? point : null
}

// DfT publishes no ramp field; its slip-road points show in junction names and short links, or as a
// motorway or trunk point far below its own road (M1 J13: 3,121-6,219 beside 55,655-59,759). 490 of the
// June 2025 release's points qualify; 648 km of class 0-2 mainline had taken one of them (r260919).
const SLIP_LINK_MAXIMUM_KM = 1
const SLIP_JUNCTION_NAME = /ramp|slip/i
const SLIP_SHARE_OF_OWN_ROAD = 0.25
const SLIP_NEIGHBOURHOOD_METRES = 10_000

function flagSlipRoadPoints(points: DftCountPoint[]): void {
  const byRef = new Map<string, DftCountPoint[]>()
  for (const point of points) if (point.rank <= 3) byRef.set(point.ref, [...byRef.get(point.ref) ?? [], point])
  for (const sameRef of byRef.values()) for (const point of sameRef) {
    if (point.linkLengthKm === null || point.linkLengthKm > SLIP_LINK_MAXIMUM_KM) continue
    const namedSlip = SLIP_JUNCTION_NAME.test(point.startJunction) !== SLIP_JUNCTION_NAME.test(point.endJunction)
    const mainline = point.rank <= 1 ? sameRef.filter(other => other !== point && (other.linkLengthKm ?? 0) > SLIP_LINK_MAXIMUM_KM &&
      flatDist(point.latitude, point.longitude, other.latitude, other.longitude) < SLIP_NEIGHBOURHOOD_METRES)
      .map(other => other.total).sort((a, b) => a - b) : []
    const median = mainline.length ? (mainline[(mainline.length - 1) >> 1] + mainline[mainline.length >> 1]) / 2 : 0
    point.isRamp = namedSlip || (mainline.length > 0 && point.total < SLIP_SHARE_OF_OWN_ROAD * median)
  }
}

// 2020 manual counts measured lockdown traffic, not a representative year (DfT methodology note).
const UNREPRESENTATIVE_COUNT_YEARS = new Set([2020])

/** Major roads: each point's latest row, estimates included. Minor roads: the latest manual count. Ten years at most. */
export async function selectDftCountPoints(rows: Iterable<CsvRow> | AsyncIterable<CsvRow>): Promise<DftCountPoint[]> {
  const latest = new Map<string, DftCountPoint>()
  for await (const row of rows) {
    const point = countPoint(row)
    if (!point || (point.rank === DFT_MINOR_ROAD_RANK &&
        (row.estimation_method !== 'Counted' || UNREPRESENTATIVE_COUNT_YEARS.has(point.year)))) continue
    const existing = latest.get(point.observationId)
    if (!existing || point.year > existing.year) latest.set(point.observationId, point)
  }
  if (latest.size === 0) return []
  const newestYear = [...latest.values()].reduce((newest, point) => Math.max(newest, point.year), 0)
  const points = [...latest.values()].filter(point => point.year > newestYear - 10)
  const admitted = points.filter(point => !withholdsCountPoint(point.latitude, point.longitude))
  flagSlipRoadPoints(admitted)
  return admitted
}

/** Stream the pinned release's CSV out of its zip; only the selected points stay in memory. */
export async function loadDftCountPoints(options: RoadLoaderArguments): Promise<DftCountPoint[]> {
  readPinnedRoadSource(options, RELEASE_ZIP, RELEASE_ZIP_SHA256)
  const unzip = spawn('unzip', ['-p', resolve(options.enrichmentDirectory, RELEASE_ZIP), RELEASE_CSV],
    { stdio: ['ignore', 'pipe', 'inherit'] })
  const exited = new Promise<number | null>(done => unzip.on('close', done))
  const points = await selectDftCountPoints(unzip.stdout.pipe(parse({ bom: true, columns: true, skip_empty_lines: true })))
  if (await exited !== 0) throw new Error(`unzip of ${RELEASE_ZIP} failed`)
  return points
}

/** Parse admitted ASTRA SASVZ 2024 annual results joined to the station list. */

import { withholdsCountPoint } from './count-holdout.js'
import proj4 from 'proj4'
import { readSheet } from 'read-excel-file/node'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const RESULTS_PATH = 'ch/sasvz-2024-jahresergebnisse.xlsx'
const RESULTS_SHA256 = '8495d5ad25bf9751ed8b7478cdd9ba05cb85346ed3ddb98aca00f25d42f3e0c1'
const STATIONS_PATH = 'ch/sasvz-messstellen-2026-04.xlsx'
const STATIONS_SHA256 = 'bd1c97ec7ed84b3b65b3141aa539c6a32159324f0441ae5f473ced7662afd2ff'
const SWITZERLAND_BBOX = [45.7, 5.8, 47.9, 10.6] as const

proj4.defs(
  'EPSG:2056',
  '+proj=somerc +lat_0=46.95240555555556 +lon_0=7.439583333333333 +k_0=1 +x_0=2600000 +y_0=1200000 +ellps=bessel +towgs84=674.374,15.056,405.346,0,0,0,0 +units=m +no_defs',
)

export interface SwissSasvzObservation {
  station: string
  latitude: number
  longitude: number
  rank: number
  light: number
  medium: number
  heavy: number
  moto: number
  estimatedClasses: number
}

export interface SwissSasvzSource {
  observations: SwissSasvzObservation[]
  stationRows: number
  unusableSkipped: number
  unlocatedSkipped: number
  unrankedSkipped: number
  inconsistentClassesSkipped: number
}

// Sheet 'DTV mit Klassen': seven header rows, then six-row station blocks
// (DTV, DTV SV, DTV SGF, DWV, DWV SV, DWV SGF) with the annual mean in column S.
const RESULTS_DATA_START = 7
const ANNUAL_COLUMN = 18
const STATION_LABELS = ['DTV', 'DTV SV', 'DTV SGF', 'DWV', 'DWV SV', 'DWV SGF'] as const

type ExcelRow = unknown[]

const stationKey = (value: unknown): string | null => {
  if (typeof value !== 'string' && typeof value !== 'number') return null
  const key = String(value).trim().replace(/^0+/, '')
  return /^\d+$/.test(key) ? key : null
}

const annualMean = (value: unknown): number | null =>
  typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null

export interface SwissSasvzResults {
  stations: Map<string, { road: string; dtv: number; sv: number | null; sgf: number | null }>
  unusableSkipped: number
}

export function parseSwissSasvzResults(rows: readonly ExcelRow[]): SwissSasvzResults {
  const stations = new Map<string, { road: string; dtv: number; sv: number | null; sgf: number | null }>()
  let unusableSkipped = 0
  let index = RESULTS_DATA_START
  while (index < rows.length) {
    const head = rows[index] ?? []
    const key = stationKey(head[0])
    if (key === null) {
      if (head.every(cell => cell === null || cell === undefined || cell === '')) {
        index++
        continue
      }
      throw new Error(`Swiss SASVZ results row ${index + 1} has no station number`)
    }
    if (stations.has(key)) throw new Error(`Swiss SASVZ station ${key} repeats`)
    const block = rows.slice(index, index + STATION_LABELS.length)
    if (
      block.length < STATION_LABELS.length ||
      !block.every((row, position) => (row ?? [])[5] === STATION_LABELS[position])
    ) {
      throw new Error(`Swiss SASVZ station ${key} breaks the six-row block layout`)
    }
    const annual = block.map(row => annualMean((row ?? [])[ANNUAL_COLUMN]))
    const road = typeof head[4] === 'string' ? head[4].trim().toUpperCase().replace(/\s+/g, '') : ''
    const dtv = annual[0]
    if (dtv === null) {
      // No published annual total: Bemerkungen confirms these stations have no usable 2024 data.
      unusableSkipped++
    } else {
      // A partial violation (SV above the total, SGF above SV) keeps the total: the class
      // split is marked estimated below instead of silently clamping a published value.
      const valid = annual[1] !== null && annual[1] <= dtv && annual[2] !== null && annual[2] <= annual[1]
      stations.set(key, {
        road,
        dtv,
        sv: valid ? annual[1] : null,
        sgf: valid ? annual[2] : null,
      })
    }
    index += STATION_LABELS.length
  }
  return { stations, unusableSkipped }
}

export function parseSwissSasvzStations(
  rows: readonly ExcelRow[],
): Map<string, { latitude: number; longitude: number }> {
  const stations = new Map<string, { latitude: number; longitude: number }>()
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index] ?? []
    if (row[0] === null || row[0] === undefined || row[0] === '') continue
    // The list carries a ten-row trilingual header; data rows hold a station number with LV95 metres.
    if (typeof row[0] === 'string' && !/^\d+$/.test(row[0].trim())) continue
    const key = stationKey(row[0])
    const east = row[6],
      north = row[7]
    if (key === null || typeof east !== 'number' || typeof north !== 'number') continue
    const [longitude, latitude] = proj4('EPSG:2056', 'WGS84', [east, north])
    if (
      latitude < SWITZERLAND_BBOX[0] ||
      latitude > SWITZERLAND_BBOX[2] ||
      longitude < SWITZERLAND_BBOX[1] ||
      longitude > SWITZERLAND_BBOX[3]
    )
      continue
    stations.set(key, { latitude, longitude })
  }
  return stations
}

function stationRank(road: string): number | null {
  if (road.startsWith('A')) return 0
  if (road.startsWith('H')) return 1
  return null
}

export function joinSwissSasvzSource(
  results: SwissSasvzResults,
  stations: Map<string, { latitude: number; longitude: number }>,
): SwissSasvzSource {
  const source: SwissSasvzSource = {
    observations: [],
    stationRows: results.stations.size,
    unusableSkipped: results.unusableSkipped,
    unlocatedSkipped: 0,
    unrankedSkipped: 0,
    inconsistentClassesSkipped: 0,
  }
  for (const [station, counts] of results.stations) {
    const location = stations.get(station)
    if (!location) {
      source.unlocatedSkipped++
      continue
    }
    // Hauptstrassen span trunk to secondary in OSM; rank 1 with the ±1 class gate admits
    // motorway, trunk and primary rows while keeping counts off secondary streets.
    const rank = stationRank(counts.road)
    if (rank === null) {
      source.unrankedSkipped++
      continue
    }
    if (withholdsCountPoint(location.latitude, location.longitude)) continue
    const total = Math.round(counts.dtv)
    if (counts.sv === null || counts.sgf === null) {
      source.observations.push({
        station,
        ...location,
        rank,
        light: total,
        medium: 0,
        heavy: 0,
        moto: 0,
        estimatedClasses: 15,
      })
      continue
    }
    // SWISS10: SGF (8, 9, 10) is heavy goods, SV minus SGF the coaches; the rest is cars,
    // vans and motorcycles with 1 % imputed as moto. Van-trailer combos (6, 7) stay in
    // light: splitting them would invent a share the sheet does not publish.
    const heavy = Math.round(counts.sgf)
    const medium = Math.round(counts.sv - counts.sgf)
    const moto = Math.round(counts.dtv * 0.01)
    const light = total - heavy - medium - moto
    if (light < 0) {
      source.inconsistentClassesSkipped++
      continue
    }
    source.observations.push({
      station,
      ...location,
      rank,
      light,
      medium,
      heavy,
      moto,
      estimatedClasses: 1 | 8,
    })
  }
  if (source.observations.length === 0) throw new Error('Swiss SASVZ source has no usable traffic observations')
  return source
}

export async function loadSwissSasvzSource(options: RoadLoaderArguments): Promise<SwissSasvzSource> {
  const results = readPinnedRoadSource(options, RESULTS_PATH, RESULTS_SHA256)
  const stations = readPinnedRoadSource(options, STATIONS_PATH, STATIONS_SHA256)
  return joinSwissSasvzSource(
    parseSwissSasvzResults(await readSheet(results, 'DTV mit Klassen')),
    parseSwissSasvzStations(await readSheet(stations, 'SASVZ_CSACR')),
  )
}

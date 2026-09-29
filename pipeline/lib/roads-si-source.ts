/** Parse admitted DRSI PLDP 2024 sections joined to their counting sites. */

import { withholdsCountPoint } from './count-holdout.js'
import { parse } from 'csv-parse/sync'
import proj4 from 'proj4'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const COUNTS_PATH = 'si/pldp2024noo.csv'
const COUNTS_SHA256 = 'dfe71c84fd5b6f46b754792fc72258fc7d88984adcb816f72e399e7af4e2457d'
const SITES_PATH = 'si/stm2024.csv'
const SITES_SHA256 = 'b0fe40d68795a70c3b1c517fada191147a0580c24c3f7a88aa6721147479fa86'
const SLOVENIA_BBOX = [45.4, 13.3, 46.9, 16.7] as const

// D96 Slovenia TM: D96 and WGS 84 agree to a metre, far below the 200 m match radius.
proj4.defs(
  'EPSG:3794',
  '+proj=tmerc +lat_0=0 +lon_0=15 +k=0.9999 +x_0=500000 +y_0=-5000000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs',
)

/** Count types from the file's own footer legend: P is assumed traffic (skipped); MWTC1
 *  occasional counts see cars versus the rest and QLTC44 no classes (total kept, classes
 *  estimated); every QLTC classifier and manual counts publish all classes. */
const ESTIMATED_TOTAL_TYPES = new Set(['P', 'Prazno', ''])
const ESTIMATED_CLASS_TYPES = new Set(['MWTC1', 'QLTC44', 'QLT44'])
const COUNTED_CLASS_TYPES = new Set(['QLTC10', 'QLTC8', 'QLTC', 'QLTC*', 'ROČNO', 'CIMATIC D2'])

const CATEGORY_RANK: Readonly<Record<string, number>> = {
  AC: 0,
  HC: 1,
  G1: 1,
  G2: 2,
  R1: 3,
  R2: 3,
  R3: 4,
  RT: 4,
}

export interface SlovenianPldpObservation {
  site: string
  latitude: number
  longitude: number
  rank: number
  light: number
  medium: number
  heavy: number
  moto: number
  estimatedClasses: number
}

export interface SlovenianPldpSource {
  observations: SlovenianPldpObservation[]
  countRows: number
  siteRows: number
  estimatedSkipped: number
  uncategorizedSkipped: number
  unlocatedSkipped: number
  zeroTrafficSkipped: number
}

type UnknownRecord = Record<string, unknown>

/** Slovenian thousands separator: '25.724' is 25,724 vehicles. */
function slovenianInteger(value: unknown): number | null {
  if (typeof value !== 'string') return null
  const text = value.trim().replace(/\./g, '').replace(/ /g, '')
  if (!/^\d+$/.test(text)) return null
  const parsed = Number(text)
  return Number.isSafeInteger(parsed) ? parsed : null
}

function coordinate(value: unknown): number | null {
  if (typeof value !== 'string' || !/^-?\d+(\.\d+)?$/.test(value.trim())) return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

export function parseSlovenianPldpSource(countsRaw: string, sitesRaw: string): SlovenianPldpSource {
  const sites = new Map<string, { latitude: number; longitude: number }>()
  const siteRows = parse(sitesRaw, {
    columns: true,
    skip_empty_lines: true,
    bom: true,
    delimiter: ';',
  }) as UnknownRecord[]
  for (const row of siteRows) {
    const site = typeof row['Števno mesto'] === 'string' ? row['Števno mesto'].trim() : ''
    if (!/^\d+$/.test(site)) continue
    const east = coordinate(row['Koordinata E'])
    const north = coordinate(row['Koordinata N'])
    if (east === null || north === null) continue
    const [longitude, latitude] = proj4('EPSG:3794', 'WGS84', [east, north])
    if (
      latitude < SLOVENIA_BBOX[0] ||
      latitude > SLOVENIA_BBOX[2] ||
      longitude < SLOVENIA_BBOX[1] ||
      longitude > SLOVENIA_BBOX[3]
    )
      continue
    sites.set(site, { latitude, longitude })
  }
  const result: SlovenianPldpSource = {
    observations: [],
    countRows: 0,
    siteRows: siteRows.length,
    estimatedSkipped: 0,
    uncategorizedSkipped: 0,
    unlocatedSkipped: 0,
    zeroTrafficSkipped: 0,
  }
  const countRows = parse(countsRaw, {
    columns: true,
    skip_empty_lines: true,
    bom: true,
    delimiter: ';',
  }) as UnknownRecord[]
  for (const row of countRows) {
    // The file ends with its own legend, which carries no traffic total.
    const total = slovenianInteger(row['Vsa vozila (PLDP)'])
    if (total === null) continue
    result.countRows++
    const site = typeof row['Števno mesto'] === 'string' ? row['Števno mesto'].trim() : ''
    const countType = typeof row['Tip   štetja'] === 'string' ? row['Tip   štetja'].trim() : ''
    const category = typeof row['Kat. ceste'] === 'string' ? row['Kat. ceste'].trim() : ''
    if (ESTIMATED_TOTAL_TYPES.has(countType)) {
      result.estimatedSkipped++
      continue
    }
    const rank = CATEGORY_RANK[category] ?? null
    if (rank === null || !/^\d+$/.test(site)) {
      result.uncategorizedSkipped++
      continue
    }
    const classes = [
      'Motorji',
      'Osebna vozila',
      'Avtobusi',
      'Lah. tov.  < 3,5t',
      'Sr. tov.  3,5-7t',
      'Tež. tov. nad 7t',
      'Tov. s prik.',
      'Vlačilci',
    ].map(name => slovenianInteger(row[name]))
    if (classes.some(value => value === null)) {
      throw new Error(`Slovenian PLDP site ${site} has invalid vehicle classes`)
    }
    const [moto, cars, buses, vans, mediumTrucks, heavyTrucks, trailers, tractors] = classes as number[]
    if (moto + cars + buses + vans + mediumTrucks + heavyTrucks + trailers + tractors !== total) {
      throw new Error(`Slovenian PLDP site ${site} classes do not sum to its total`)
    }
    if (total === 0) {
      result.zeroTrafficSkipped++
      continue
    }
    const location = sites.get(site)
    if (!location) {
      result.unlocatedSkipped++
      continue
    }
    if (withholdsCountPoint(location.latitude, location.longitude)) continue
    // Buses join the medium class with 3.5-7 t trucks, vans with cars in light, exactly as
    // the DfT bins do; one-direction sites already publish doubled two-way PLDP (A5 0806
    // reads 29,322 where the road carries about 15,000 per direction), so every row stays
    // a both-directions total.
    const estimatedClasses = COUNTED_CLASS_TYPES.has(countType) ? 0 : ESTIMATED_CLASS_TYPES.has(countType) ? 15 : null
    if (estimatedClasses === null) throw new Error(`Slovenian PLDP site ${site} has unknown count type '${countType}'`)
    result.observations.push({
      site,
      ...location,
      rank,
      light: cars + vans,
      medium: buses + mediumTrucks,
      heavy: heavyTrucks + trailers + tractors,
      moto,
      estimatedClasses,
    })
  }
  if (result.observations.length === 0) throw new Error('Slovenian PLDP source has no usable traffic observations')
  return result
}

export function loadSlovenianPldpSource(options: RoadLoaderArguments): SlovenianPldpSource {
  const counts = readPinnedRoadSource(options, COUNTS_PATH, COUNTS_SHA256).toString('utf8')
  const sites = readPinnedRoadSource(options, SITES_PATH, SITES_SHA256).toString('utf8')
  return parseSlovenianPldpSource(counts, sites)
}

/** Cerema RRN TMJA download and source-faithful CSV parsing. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { parse } from 'csv-parse/sync'
import proj4 from 'proj4'
import { writeCacheAtomically } from './atomic-cache.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'

const CACHE_DIRECTORY = 'fr'
const CSV_2024 = 'tmja-2024.csv'
const CSV_2019 = 'tmja-2019.csv'
const URL_2024 = 'https://static.data.gouv.fr/resources/trafic-moyen-journalier-annuel-sur-le-reseau-routier-national/20250818-100154/tmja-rrnc-2024.csv'
const URL_2019 = 'https://static.data.gouv.fr/resources/trafic-moyen-journalier-annuel-sur-le-reseau-routier-national/20211222-165040/tmja-2019.csv'
const SOURCE_BOUNDS = [41, -5.5, 51.5, 10] as const
const REQUIRED_COLUMNS = ['route', 'TMJA', 'ratio_PL', 'xD', 'yD', 'xF', 'yF'] as const

proj4.defs('EPSG:2154', '+proj=lcc +lat_0=46.5 +lon_0=3 +lat_1=49 +lat_2=44 +x_0=700000 +y_0=6600000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

export interface CeremaCensusSection {
  route: string
  ref: string
  lat: number
  lon: number
  coords: Array<readonly [number, number]>
  tmja: number
  ratio_pl: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

export interface CeremaFileStats {
  year: 2019 | 2024
  sourceRows: number
  accepted: number
  noTrafficSkipped: number
  missingHeavyRatioSkipped: number
  invalidHeavyRatioSkipped: number
  invalidCoordinatesSkipped: number
  outsideMetropolitanFranceSkipped: number
  duplicateSkipped: number
}

export interface CeremaCensus {
  sections: CeremaCensusSection[]
  files: readonly CeremaFileStats[]
}

type CsvRow = Record<string, string>

function localizedNumber(value: string | undefined): number | null {
  const text = value?.trim() ?? ''
  if (!text) return null
  const parsed = Number(text.replace(',', '.'))
  return Number.isFinite(parsed) ? parsed : null
}

/**
 * The 2019 producer correction is data, not a guessed default: discard zero
 * and >500, divide integer percentages by ten, retain decimal percentages.
 * https://ame.gitpages.univ-eiffel.fr/tmja-2019-analysis/
 */
function heavyRatio(raw: string | undefined, year: 2019 | 2024): 'missing' | 'invalid' | number {
  const text = raw?.trim() ?? ''
  const supplied = localizedNumber(text)
  if (supplied === null || supplied === 0) return 'missing'
  if (supplied < 0) return 'invalid'
  let percentage = supplied
  if (year === 2019) {
    if (supplied > 500) return 'invalid'
    if (!text.includes(',') && !text.includes('.')) percentage /= 10
  }
  if (percentage > 100) return 'invalid'
  return percentage / 100
}

function csvRows(csv: string, year: number): CsvRow[] {
  return parse(csv, {
    bom: true,
    columns: (headers: string[]) => {
      const canonical = headers.map(header => header.trim())
      for (const required of REQUIRED_COLUMNS) {
        if (!canonical.includes(required)) throw new Error(`Cerema ${year} CSV missing '${required}' column`)
      }
      return canonical
    },
    delimiter: ';',
    skip_empty_lines: true,
  }) as CsvRow[]
}

function parseFile(
  csv: string,
  year: 2019 | 2024,
  shadowedKeys: ReadonlySet<string>,
): { sections: CeremaCensusSection[]; stats: CeremaFileStats } {
  const rows = csvRows(csv, year)
  if (rows.length === 0) throw new Error(`Cerema ${year} census is empty`)
  const sections: CeremaCensusSection[] = []
  const stats: CeremaFileStats = {
    year,
    sourceRows: rows.length,
    accepted: 0,
    noTrafficSkipped: 0,
    missingHeavyRatioSkipped: 0,
    invalidHeavyRatioSkipped: 0,
    invalidCoordinatesSkipped: 0,
    outsideMetropolitanFranceSkipped: 0,
    duplicateSkipped: 0,
  }
  for (const row of rows) {
    const route = row.route?.trim() ?? ''
    const rawTotal = localizedNumber(row.TMJA)
    const tmja = rawTotal === null ? 0 : Math.round(rawTotal)
    if (!route || tmja <= 0) {
      stats.noTrafficSkipped++
      continue
    }
    const ratio = heavyRatio(row.ratio_PL, year)
    if (ratio === 'missing') {
      stats.missingHeavyRatioSkipped++
      continue
    }
    if (ratio === 'invalid') {
      stats.invalidHeavyRatioSkipped++
      continue
    }
    const xD = localizedNumber(row.xD)
    const yD = localizedNumber(row.yD)
    const xF = localizedNumber(row.xF)
    const yF = localizedNumber(row.yF)
    if (xD === null || yD === null || xD < 100_000) {
      stats.invalidCoordinatesSkipped++
      continue
    }
    const hasEnd = xF !== null && yF !== null && xF !== 0 && yF !== 0
    const midpointX = hasEnd ? (xD + xF) / 2 : xD
    const midpointY = hasEnd ? (yD + yF) / 2 : yD
    const [lon, lat] = proj4('EPSG:2154', 'WGS84', [midpointX, midpointY])
    if (!Number.isFinite(lat) || !Number.isFinite(lon)) {
      stats.invalidCoordinatesSkipped++
      continue
    }
    if (lat < SOURCE_BOUNDS[0] || lat > SOURCE_BOUNDS[2] ||
        lon < SOURCE_BOUNDS[1] || lon > SOURCE_BOUNDS[3]) {
      stats.outsideMetropolitanFranceSkipped++
      continue
    }
    const ref = route.replace(/^([A-Z])0*/, '$1')
    if (shadowedKeys.has(coverageKey(ref, lat, lon))) {
      stats.duplicateSkipped++
      continue
    }

    const [startLon, startLat] = proj4('EPSG:2154', 'WGS84', [xD, yD])
    const coords: Array<readonly [number, number]> = [[startLon, startLat]]
    if (hasEnd) {
      const [endLon, endLat] = proj4('EPSG:2154', 'WGS84', [xF, yF])
      coords.push([endLon, endLat])
    }
    const aadt_moto = Math.round(tmja * 0.01)
    const totalHeavy = Math.round(tmja * ratio)
    const aadt_medium = Math.round(totalHeavy * 0.02)
    const aadt_heavy = totalHeavy - aadt_medium
    const aadt_light = tmja - totalHeavy - aadt_moto
    if (aadt_light < 0) {
      stats.invalidHeavyRatioSkipped++
      continue
    }
    sections.push({
      route, ref, lat, lon, coords, tmja, ratio_pl: ratio,
      aadt_light, aadt_medium, aadt_heavy, aadt_moto,
    })
    stats.accepted++
  }
  if (stats.accepted + stats.duplicateSkipped === 0) {
    throw new Error(`Cerema ${year} census has no usable traffic measurements`)
  }
  return { sections, stats }
}

// Preserve cross-year coverage without merging same-year observations: nearby
// sections can have different measured traffic.
const coverageKey = (ref: string, lat: number, lon: number): string =>
  `${ref}:${lat.toFixed(2)}:${lon.toFixed(2)}`

/** Parse newest first so only a complete, valid 2024 measurement shadows 2019. */
export function parseCeremaCsvFiles(csv2024: string, csv2019: string): CeremaCensus {
  const newest = parseFile(csv2024, 2024, new Set())
  const newerCoverage = new Set(newest.sections.map(section => coverageKey(section.ref, section.lat, section.lon)))
  const older = parseFile(csv2019, 2019, newerCoverage)
  const sections = [...newest.sections, ...older.sections]
  return { sections, files: [newest.stats, older.stats] }
}

async function download(url: string, year: number): Promise<Buffer> {
  const response = await fetch(url, { signal: AbortSignal.timeout(120_000) })
  if (!response.ok) throw new Error(`Cerema ${year} download returned HTTP ${response.status}`)
  return Buffer.from(await response.arrayBuffer())
}

/** The two small official CSVs are the SSOT; the old derived JSON erased ratio_PL. */
export async function loadCeremaCensus(options: RoadLoaderArguments): Promise<CeremaCensus> {
  const directory = resolve(options.enrichmentDirectory, CACHE_DIRECTORY)
  const inputs = [
    { year: 2024 as const, path: resolve(directory, CSV_2024), url: URL_2024 },
    { year: 2019 as const, path: resolve(directory, CSV_2019), url: URL_2019 },
  ]
  const loaded = []
  for (const input of inputs) {
    const downloading = options.forceDownload || !existsSync(input.path)
    if (downloading && options.enrichOnly) throw new Error(`Cerema ${input.year} CSV missing: ${input.path}`)
    const bytes = downloading ? await download(input.url, input.year) : readFileSync(input.path)
    loaded.push({ ...input, bytes, downloading })
  }
  const census = parseCeremaCsvFiles(loaded[0].bytes.toString('utf8'), loaded[1].bytes.toString('utf8'))
  for (const input of loaded) if (input.downloading) writeCacheAtomically(input.path, input.bytes)
  return census
}

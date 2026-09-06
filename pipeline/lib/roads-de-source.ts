/** BASt SVZ 2021 download, workbook parsing and validated reusable cache. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import proj4 from 'proj4'
import { readSheet } from 'read-excel-file/node'
import { writeCacheAtomically } from './atomic-cache.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'

const CACHE_DIRECTORY = 'de'
const CACHE_JSON = 'svz-census.json'
const AUTOBAHN_XLSX = 'svz-autobahnen-2021.xlsx'
const BUNDESSTRASSE_XLSX = 'svz-bundesstrassen-2021.xlsx'
const AUTOBAHN_URL = 'https://www.bast.de/DE/Publikationen/Statistik/Verkehrsdaten/2021/Autobahnen-2021.xlsx?__blob=publicationFile&v=1'
const BUNDESSTRASSE_URL = 'https://www.bast.de/DE/Publikationen/Statistik/Verkehrsdaten/2021/Bundesstrassen-2021.xlsx?__blob=publicationFile&v=1'
const SOURCE_BOUNDS = [47, 5.5, 55.5, 15.5] as const

proj4.defs('EPSG:25832', '+proj=utm +zone=32 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs')

export interface BastCensusSection {
  road: string
  ref: string
  tkzst: string
  lat: number
  lon: number
  dtv: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

export interface BastCensus {
  sections: BastCensusSection[]
  sourceRows: number
  invalidRowsSkipped: number
  zeroClassSplitsSkipped: number
  inconsistentClassTotalsSkipped: number
}

interface ParsedWorkbook {
  sections: BastCensusSection[]
  sourceRows: number
  invalidRowsSkipped: number
}

type WorkbookKind = 'A' | 'B'
type Cell = unknown

function finiteNumber(value: Cell): number | null {
  if (value === null || value === undefined || value === '' || value === '-') return null
  if (typeof value !== 'number' && typeof value !== 'string') return null
  const parsed = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

function classCount(value: Cell, field: string, row: number): number {
  const parsed = finiteNumber(value) ?? 0
  const rounded = Math.round(parsed)
  if (rounded < 0 || !Number.isSafeInteger(rounded)) {
    throw new Error(`invalid BASt ${field} at workbook row ${row}: ${JSON.stringify(value)}`)
  }
  return rounded
}

function requiredColumns(headers: readonly Cell[], label: string): Record<string, number> {
  const names = [
    'Str', 'TKZST', 'DTV', 'DTVLVm', 'DTVBus', 'DTVLoA', 'DTVLZ', 'DTVKrad',
    'X_Koordinate', 'Y_Koordinate',
  ]
  return Object.fromEntries(names.map(name => {
    const index = headers.indexOf(name)
    if (index < 0) throw new Error(`BASt ${label} workbook missing '${name}' column`)
    return [name, index]
  }))
}

/** Parse exact `Zeilenformat` values, retaining rows without a class split for audit. */
export function parseBastRows(rows: readonly (readonly Cell[])[], kind: WorkbookKind): ParsedWorkbook {
  if (rows.length === 0) throw new Error(`BASt ${kind} workbook is empty`)
  const columns = requiredColumns(rows[0], kind)
  const sections: BastCensusSection[] = []
  let invalidRowsSkipped = 0

  for (let index = 1; index < rows.length; index++) {
    const row = rows[index]
    const road = String(row[columns.Str] ?? '').trim()
    const tkzst = String(row[columns.TKZST] ?? '').trim()
    const x = finiteNumber(row[columns.X_Koordinate])
    const y = finiteNumber(row[columns.Y_Koordinate])
    const rawTotal = finiteNumber(row[columns.DTV])
    const dtv = rawTotal === null ? 0 : Math.round(rawTotal)
    if (!road || !road.startsWith(kind) || !tkzst || x === null || y === null ||
        !Number.isSafeInteger(dtv) || dtv <= 0 || x < 100_000 || y < 5_000_000) {
      invalidRowsSkipped++
      continue
    }
    const [lon, lat] = proj4('EPSG:25832', 'WGS84', [x, y])
    if (!Number.isFinite(lat) || !Number.isFinite(lon) ||
        lat < SOURCE_BOUNDS[0] || lat > SOURCE_BOUNDS[2] ||
        lon < SOURCE_BOUNDS[1] || lon > SOURCE_BOUNDS[3]) {
      invalidRowsSkipped++
      continue
    }
    const ref = road.replace(/\s+/g, '')
    if (!new RegExp(`^${kind}\\d+[A-Za-z]*$`).test(ref)) {
      throw new Error(`invalid BASt ${kind} road ref at workbook row ${index + 1}: ${JSON.stringify(road)}`)
    }
    sections.push({
      road,
      ref,
      tkzst,
      lat,
      lon,
      dtv,
      aadt_light: classCount(row[columns.DTVLVm], 'DTVLVm', index + 1),
      aadt_medium: classCount(row[columns.DTVBus], 'DTVBus', index + 1) +
        classCount(row[columns.DTVLoA], 'DTVLoA', index + 1),
      aadt_heavy: classCount(row[columns.DTVLZ], 'DTVLZ', index + 1),
      aadt_moto: classCount(row[columns.DTVKrad], 'DTVKrad', index + 1),
    })
  }
  return { sections, sourceRows: rows.length - 1, invalidRowsSkipped }
}

/** Read one official BASt workbook without executing formulas or macros. */
export async function parseBastWorkbook(bytes: Buffer, kind: WorkbookKind): Promise<ParsedWorkbook> {
  const rows = await readSheet(bytes, 'Zeilenformat')
  return parseBastRows(rows, kind)
}

function validatedCacheSection(value: unknown, index: number, path: string): BastCensusSection {
  if (!value || typeof value !== 'object') throw new Error(`invalid BASt cache row ${index}: ${path}`)
  const row = value as Record<string, unknown>
  const section: BastCensusSection = {
    road: String(row.road ?? ''),
    ref: String(row.ref ?? ''),
    tkzst: String(row.tkzst ?? ''),
    lat: Number(row.lat),
    lon: Number(row.lon),
    dtv: Number(row.dtv),
    aadt_light: Number(row.aadt_light),
    aadt_medium: Number(row.aadt_medium),
    aadt_heavy: Number(row.aadt_heavy),
    aadt_moto: Number(row.aadt_moto),
  }
  const integers = [section.dtv, section.aadt_light, section.aadt_medium, section.aadt_heavy, section.aadt_moto]
  if (!section.road || !/^[AB]\d+[A-Za-z]*$/.test(section.ref) || !section.tkzst ||
      !Number.isFinite(section.lat) || !Number.isFinite(section.lon) ||
      section.lat < SOURCE_BOUNDS[0] || section.lat > SOURCE_BOUNDS[2] ||
      section.lon < SOURCE_BOUNDS[1] || section.lon > SOURCE_BOUNDS[3] ||
      integers.some(number => !Number.isSafeInteger(number) || number < 0) || section.dtv === 0) {
    throw new Error(`invalid BASt cache row ${index}: ${path}`)
  }
  return section
}

function stampableCensus(
  sections: BastCensusSection[], sourceRows: number, invalidRowsSkipped: number,
): BastCensus {
  const accepted: BastCensusSection[] = []
  let zeroClassSplitsSkipped = 0
  let inconsistentClassTotalsSkipped = 0
  for (const section of sections) {
    const classTotal = section.aadt_light + section.aadt_medium + section.aadt_heavy + section.aadt_moto
    if (classTotal === 0) {
      zeroClassSplitsSkipped++
    } else if (Math.abs(classTotal - section.dtv) > 2) {
      inconsistentClassTotalsSkipped++
    } else {
      accepted.push(section)
    }
  }
  if (accepted.length === 0) throw new Error('BASt snapshot has no usable traffic measurements')
  return {
    sections: accepted, sourceRows, invalidRowsSkipped,
    zeroClassSplitsSkipped, inconsistentClassTotalsSkipped,
  }
}

function parseBastCache(bytes: string, path: string): BastCensus {
  const value = JSON.parse(bytes) as unknown
  if (!Array.isArray(value)) throw new Error(`BASt cache is not an array: ${path}`)
  const sections = value.map((row, index) => validatedCacheSection(row, index, path))
  return stampableCensus(sections, sections.length, 0)
}

async function download(url: string, label: string): Promise<Buffer> {
  const response = await fetch(url, {
    signal: AbortSignal.timeout(120_000),
    headers: { 'User-Agent': 'Mozilla/5.0 (QuietMap noise enrichment)' },
  })
  if (!response.ok) throw new Error(`BASt ${label} download returned HTTP ${response.status}`)
  return Buffer.from(await response.arrayBuffer())
}

/** Load the preserved parsed census, or rebuild it losslessly from both official workbooks. */
export async function loadBastCensus(options: RoadLoaderArguments): Promise<BastCensus> {
  const directory = resolve(options.enrichmentDirectory, CACHE_DIRECTORY)
  const jsonPath = resolve(directory, CACHE_JSON)
  if (!options.forceDownload && existsSync(jsonPath)) {
    return parseBastCache(readFileSync(jsonPath, 'utf8'), jsonPath)
  }
  const inputs = [
    { kind: 'A' as const, path: resolve(directory, AUTOBAHN_XLSX), url: AUTOBAHN_URL, label: 'Autobahn' },
    { kind: 'B' as const, path: resolve(directory, BUNDESSTRASSE_XLSX), url: BUNDESSTRASSE_URL, label: 'Bundesstraße' },
  ]
  const loaded = []
  for (const input of inputs) {
    const downloading = options.forceDownload || !existsSync(input.path)
    if (downloading && options.enrichOnly) throw new Error(`BASt workbook missing: ${input.path}`)
    const bytes = downloading ? await download(input.url, input.label) : readFileSync(input.path)
    loaded.push({ ...input, bytes, downloading })
  }
  const parsed = await Promise.all(loaded.map(async input => {
    const result = await parseBastWorkbook(input.bytes, input.kind)
    stampableCensus(result.sections, result.sourceRows, result.invalidRowsSkipped)
    return result
  }))
  const allSections = parsed.flatMap(result => result.sections)
  const census = stampableCensus(
    allSections,
    parsed.reduce((sum, result) => sum + result.sourceRows, 0),
    parsed.reduce((sum, result) => sum + result.invalidRowsSkipped, 0),
  )
  for (const input of loaded) if (input.downloading) writeCacheAtomically(input.path, input.bytes)
  if (!options.enrichOnly) writeCacheAtomically(jsonPath, JSON.stringify(allSections))
  return census
}

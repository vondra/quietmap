/** Enrich z9 road vectors with Czech ŘSD traffic census measurements. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { writeCacheAtomically } from './lib/atomic-cache.js'
import { SOURCE_ID_CZ_RSD_SCITANI } from './lib/source-ids.generated.js'
import { listPreparedSquares } from './lib/prepared-grid.js'
import { shouldOverwrite } from './lib/provenance.js'
import {
  ROAD_CLASS_RANK_TOLERANCE, osmRoadClassRank, writeRoadAadt, type RoadRow,
} from './lib/roads-arrow.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { parseRoadLoaderArguments, type RoadLoaderArguments } from './lib/road-loader-cli.js'

const SOURCE_ID = SOURCE_ID_CZ_RSD_SCITANI
const COVERED_ROAD_CLASSES: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])
const CZECHIA_BBOX = [48.2, 11.7, 51.4, 19.2] as const
const CACHE_RELATIVE_PATH = 'cz/rsd-scitani.json'
const RSD_QUERY_URL = 'https://geoportal.rsd.cz/arcgis/rest/services/ScitaniDopravy/MapServer/3/query'
const RSD_BBOX_SJTSK = { xmin: -900000, ymin: -1300000, xmax: -400000, ymax: -900000 }
const PAGE_SIZE = 2000

export interface CensusSection {
  ref: string
  rank: number
  light: number
  medium: number
  heavy: number
  moto: number
  paths: ReadonlyArray<ReadonlyArray<readonly [number, number]>>
}

export interface ParsedCensus {
  byRef: ReadonlyMap<string, readonly CensusSection[]>
  zeroSectionsSkipped: number
}

export interface CzEnrichmentResult {
  rows: number
  matched: number
  retracted: number
  skippedForeign: number
  squares: number
  squaresUpdated: number
}

function count(attributes: Record<string, unknown>, name: string): number {
  const value = attributes[name] ?? 0
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new Error(`invalid ŘSD count '${name}': ${JSON.stringify(value)}`)
  }
  return value
}

function geometryPaths(value: unknown): Array<Array<readonly [number, number]>> {
  if (!Array.isArray(value)) return []
  const parsed: Array<Array<readonly [number, number]>> = []
  for (const path of value) {
    if (!Array.isArray(path) || path.length === 0) continue
    const coordinates: Array<readonly [number, number]> = []
    for (const point of path) {
      if (!Array.isArray(point) || point.length < 2 ||
          typeof point[0] !== 'number' || typeof point[1] !== 'number' ||
          !Number.isFinite(point[0]) || !Number.isFinite(point[1])) {
        throw new Error(`invalid ŘSD geometry point: ${JSON.stringify(point)}`)
      }
      coordinates.push([point[0], point[1]])
    }
    parsed.push(coordinates)
  }
  return parsed
}

/** Parse the cached ArcGIS response without joining disconnected path parts. */
export function parseCensus(features: readonly unknown[]): ParsedCensus {
  const byRef = new Map<string, CensusSection[]>()
  let zeroSectionsSkipped = 0
  for (const feature of features) {
    if (!feature || typeof feature !== 'object') continue
    const attributes = (feature as { attributes?: unknown }).attributes
    const geometry = (feature as { geometry?: { paths?: unknown } }).geometry
    if (!attributes || typeof attributes !== 'object') continue
    const values = attributes as Record<string, unknown>
    const psilnice = String(values.PSILNICE ?? '')
    const pkodR = String(values.PKOD_R ?? '')
    const ref = normalizeRsdRef(psilnice, pkodR)
    const paths = geometryPaths(geometry?.paths)
    if (!ref || paths.length === 0) continue
    const section: CensusSection = {
      ref,
      rank: rsdRank(psilnice, pkodR),
      light: count(values, 'O') + count(values, 'LN'),
      medium: count(values, 'SN') + count(values, 'A') + count(values, 'TR') + count(values, 'TRP'),
      heavy: count(values, 'TN') + count(values, 'TNP') + count(values, 'SNP') + count(values, 'NSN') + count(values, 'AK'),
      moto: count(values, 'M'),
      paths,
    }
    if (section.light + section.medium + section.heavy + section.moto === 0) {
      zeroSectionsSkipped++
      continue
    }
    const sections = byRef.get(ref)
    if (sections) sections.push(section)
    else byRef.set(ref, [section])
  }
  return { byRef, zeroSectionsSkipped }
}

/** The one claim oracle shared by stamping and self-healing retraction. */
export function matchCensusSection(
  row: RoadRow,
  censusByRef: ReadonlyMap<string, readonly CensusSection[]>,
): CensusSection | null {
  if (!COVERED_ROAD_CLASSES.has(row.roadClass) || !row.ref) return null
  const normalizedRef = normalizeOsmRef(row.ref)
  if (!normalizedRef) return null
  const candidates = censusByRef.get(normalizedRef)
  if (!candidates) return null

  const rowRank = osmRoadClassRank(row.roadClass)
  let closest: CensusSection | null = null
  let closestDistance = 10_000
  for (const candidate of candidates) {
    if (Math.abs(rowRank - candidate.rank) > ROAD_CLASS_RANK_TOLERANCE) continue
    for (const path of candidate.paths) {
      const distance = pointToPolylineDist(row.midLat, row.midLon, path)
      if (distance < closestDistance) {
        closest = candidate
        closestDistance = distance
      }
    }
  }
  return closest
}

export async function enrichCzechRoads(
  preparedDirectory: string,
  censusByRef: ReadonlyMap<string, readonly CensusSection[]>,
): Promise<CzEnrichmentResult> {
  const squares = listPreparedSquares(preparedDirectory, CZECHIA_BBOX)
  if (squares.length === 0) throw new Error(`no Czech roads.arrow squares found under ${preparedDirectory}`)
  const result: CzEnrichmentResult = {
    rows: 0, matched: 0, retracted: 0, skippedForeign: 0,
    squares: squares.length, squaresUpdated: 0,
  }
  for (const square of squares) {
    const write = await writeRoadAadt(
      resolve(preparedDirectory, square, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const section = matchCensusSection(row, censusByRef)
        return section ? {
          light: section.light,
          medium: section.medium,
          heavy: section.heavy,
          moto: section.moto,
          sourceId: SOURCE_ID,
        } : null
      },
      undefined,
      COVERED_ROAD_CLASSES,
      { sourceIds: [SOURCE_ID], when: row => matchCensusSection(row, censusByRef) === null },
    )
    result.rows += write.rows
    result.matched += write.matched
    result.retracted += write.retracted
    result.skippedForeign += write.skippedForeign
    if (write.updated) result.squaresUpdated++
  }
  return result
}

async function downloadCensus(): Promise<unknown[]> {
  const allFeatures: unknown[] = []
  for (let offset = 0; ; offset += PAGE_SIZE) {
    const query = new URLSearchParams({
      geometry: JSON.stringify(RSD_BBOX_SJTSK), geometryType: 'esriGeometryEnvelope',
      inSR: '5514', outFields: '*', resultRecordCount: String(PAGE_SIZE),
      resultOffset: String(offset), f: 'json', outSR: '4326',
    })
    const response = await fetch(`${RSD_QUERY_URL}?${query}`, { signal: AbortSignal.timeout(60_000) })
    if (!response.ok) throw new Error(`ŘSD API returned HTTP ${response.status}`)
    const payload = await response.json() as { features?: unknown[]; error?: unknown }
    if (payload.error) throw new Error(`ŘSD API error: ${JSON.stringify(payload.error)}`)
    if (!Array.isArray(payload.features)) throw new Error('ŘSD API response has no features array')
    allFeatures.push(...payload.features)
    if (payload.features.length < PAGE_SIZE) return allFeatures
  }
}

async function loadCensusFeatures(options: RoadLoaderArguments): Promise<unknown[]> {
  const cachePath = resolve(options.enrichmentDirectory, CACHE_RELATIVE_PATH)
  if (!options.forceDownload && existsSync(cachePath)) {
    const parsed = JSON.parse(readFileSync(cachePath, 'utf8')) as unknown
    if (!Array.isArray(parsed)) throw new Error(`ŘSD cache is not an array: ${cachePath}`)
    return parsed
  }
  if (options.enrichOnly) throw new Error(`ŘSD cache missing: ${cachePath}`)
  const features = await downloadCensus()
  writeCacheAtomically(cachePath, JSON.stringify(features))
  return features
}

function normalizeRsdRef(psilnice: string, pkodR: string): string {
  const value = psilnice.trim()
  if (!value) return ''
  if (/^D\d+/.test(value)) return value
  const number = value.replace(/\D/g, '')
  if (!number) return value
  return pkodR === '1' || pkodR === '5' ? `D${number}` : number
}

export function rsdRank(psilnice: string, pkodR: string): number {
  if (pkodR === '1' || pkodR === '5' || /^D\d/.test(psilnice.trim())) return 0
  if (pkodR === '3') return 3
  if (pkodR === '4') return 4
  return 1
}

export function normalizeOsmRef(ref: string): string {
  for (const token of ref.split(';')) {
    const value = token.trim()
    if (/^D\d+$/.test(value)) return value
    if (/^E\d+$/.test(value)) continue
    const roadNumber = value.match(/^(?:[IV]+\/)?(\d+)[a-zA-Z]?$/)
    if (roadNumber) return roadNumber[1]
  }
  return ''
}

async function main(): Promise<void> {
  const options = parseRoadLoaderArguments(process.argv.slice(2), 'enrich-roads-cz.ts')
  const features = await loadCensusFeatures(options)
  const census = parseCensus(features)
  const result = await enrichCzechRoads(options.preparedDirectory, census.byRef)
  console.log(JSON.stringify({
    cache: resolve(options.enrichmentDirectory, CACHE_RELATIVE_PATH),
    features: features.length,
    refs: census.byRef.size,
    zeroSectionsSkipped: census.zeroSectionsSkipped,
    ...result,
  }))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}

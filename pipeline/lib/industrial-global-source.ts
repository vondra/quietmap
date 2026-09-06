/** Admit retained GPPD, E-PRTR and GEM observations before any industrial Arrow write. */

import { createHash } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { parse } from 'csv-parse/sync'
import { PROVENANCE_RANK, SOURCES_BY_ID, SOURCE_ID_GLOBAL_GPPD, SOURCE_ID_EUROPE_EPRTR,
  SOURCE_ID_GLOBAL_GEM_STEEL, SOURCE_ID_GLOBAL_GEM_CEMENT, SOURCE_ID_GLOBAL_GEM_COALMINE } from './sources.js'
import type { MatchFacility } from './facility-match.js'

export const GLOBAL_INDUSTRIAL_SOURCES = [
  { file: 'gppd.csv', id: SOURCE_ID_GLOBAL_GPPD, minimum: 1000 },
  { file: 'eprtr-facilities.json', id: SOURCE_ID_EUROPE_EPRTR, minimum: 10_000 },
  { file: 'gem-steel.geojson', id: SOURCE_ID_GLOBAL_GEM_STEEL, minimum: 100, nace4: 2410 },
  { file: 'gem-cement.geojson', id: SOURCE_ID_GLOBAL_GEM_CEMENT, minimum: 100, nace4: 2351 },
  { file: 'gem-coalmine.geojson', id: SOURCE_ID_GLOBAL_GEM_COALMINE, minimum: 100, nace4: 510 },
] as const
export type GlobalIndustrialSource = typeof GLOBAL_INDUSTRIAL_SOURCES[number]

interface ParsedFacility { lat: number; lon: number; nace4: number }
export interface IndustrialSourceCensus {
  raw: number
  invalidCoordinates: number
  inactive: number
  unclassified: number
  classified: number
}

// Original global GPPD fuel policy: geothermal shares hydro; the national GEM
// integrated-power feed has a separate observed-type policy and is not this feed.
const GPPD_FUEL_NACE: Record<string, number> = {
  Nuclear: 3511, Hydro: 3512, Solar: 3599, Gas: 3511, Oil: 3511,
  Coal: 3511, Petcoke: 3511, Biomass: 3511, Waste: 3511, Geothermal: 3512,
}
// E-PRTR Annex I activity sectors, as mapped by the actual dev1 global producer.
const ANNEX_SECTOR_NACE: Record<number, number> = {
  1: 3511, 2: 2410, 3: 2351, 4: 2011, 5: 3821, 6: 1711, 7: 146, 8: 1011, 9: 1310,
}
const ACTIVE_GEM_STATUS = new Set(['operating', 'operating-pre-retirement'])
const validCoordinates = (lat: unknown, lon: unknown): lat is number =>
  typeof lat === 'number' && typeof lon === 'number' && Number.isFinite(lat) && Number.isFinite(lon) &&
  Math.abs(lat) <= 90 && Math.abs(lon) <= 180

export function parseGlobalIndustrialSource(text: string, source: GlobalIndustrialSource) {
  const census: IndustrialSourceCensus = { raw: 0, invalidCoordinates: 0, inactive: 0, unclassified: 0, classified: 0 }
  const facilities: ParsedFacility[] = []
  const add = (lat: unknown, lon: unknown, nace4: number) => {
    if (!validCoordinates(lat, lon)) { census.invalidCoordinates++; return }
    if (!nace4) { census.unclassified++; return }
    facilities.push({ lat, lon: lon as number, nace4 })
    census.classified++
  }
  if (source.id === SOURCE_ID_GLOBAL_GPPD) {
    const rows = parse(text, { columns: true, skip_empty_lines: true, bom: true }) as Record<string, string>[]
    if (!rows.length || !['latitude', 'longitude', 'primary_fuel'].every(key => key in rows[0])) {
      throw new Error(`${source.file}: missing GPPD observations/columns`)
    }
    census.raw = rows.length
    for (const row of rows) add(parseFloat(row.latitude), parseFloat(row.longitude), GPPD_FUEL_NACE[row.primary_fuel.trim()] ?? 0)
  } else if (source.id === SOURCE_ID_EUROPE_EPRTR) {
    const parsed = JSON.parse(text) as { results?: Array<Record<string, unknown>> }
    if (!Array.isArray(parsed.results) || !parsed.results.length || parsed.results.length >= 100_000) {
      throw new Error(`${source.file}: invalid/empty DISCODATA results or reached original single-page cap`)
    }
    census.raw = parsed.results.length
    for (const row of parsed.results) {
      const activity = typeof row.EPRTRAnnexIMainActivity === 'string' ? row.EPRTRAnnexIMainActivity : ''
      const sector = activity.match(/^\s*(\d{1,2})\s*[.(]/)
      add(row.y_4326, row.x_4326, sector ? ANNEX_SECTOR_NACE[Number(sector[1])] ?? 0 : 0)
    }
  } else {
    const parsed = JSON.parse(text) as { type?: string; features?: Array<{
      geometry?: { type?: string; coordinates?: unknown[] }; properties?: Record<string, unknown>
    }> }
    if (parsed.type !== 'FeatureCollection' || !Array.isArray(parsed.features) || !parsed.features.length) {
      throw new Error(`${source.file}: expected non-empty GEM FeatureCollection`)
    }
    census.raw = parsed.features.length
    for (const feature of parsed.features) {
      const p = feature.properties ?? {}
      const status = String(p.status ?? '').trim().toLowerCase()
      if (!ACTIVE_GEM_STATUS.has(status)) { census.inactive++; continue }
      const geometry = feature.geometry?.type === 'Point' ? feature.geometry.coordinates : undefined
      const latitude = p.Latitude ?? p.latitude ?? geometry?.[1]
      const longitude = p.Longitude ?? p.longitude ?? geometry?.[0]
      add(parseFloat(String(latitude ?? '')), parseFloat(String(longitude ?? '')), source.nace4)
    }
  }
  return { facilities, census }
}

export function loadGlobalIndustrialSources(directory: string) {
  const facilities: MatchFacility[] = []
  const receipts = []
  for (const source of GLOBAL_INDUSTRIAL_SOURCES) {
    const path = resolve(directory, source.file)
    const before = statSync(path, { bigint: true })
    const bytes = readFileSync(path)
    const parsed = parseGlobalIndustrialSource(bytes.toString('utf8'), source)
    const after = statSync(path, { bigint: true })
    if (before.dev !== after.dev || before.ino !== after.ino || before.size !== after.size ||
        before.mtimeNs !== after.mtimeNs || before.ctimeNs !== after.ctimeNs) throw new Error(`${path}: changed during source admission`)
    // Preserve actual dev1 participation floors; a source below its floor fails
    // the requested five-source run before any output, rather than silently skipping.
    const admitted = source.id === SOURCE_ID_GLOBAL_GPPD
      ? parsed.census.classified + parsed.census.unclassified : parsed.census.classified
    if (admitted < source.minimum) throw new Error(`${path}: ${admitted} admitted observations below ${source.minimum} source floor`)
    const authority = SOURCES_BY_ID.get(source.id)!
    facilities.push(...parsed.facilities.map(facility => ({ ...facility, id: source.id,
      rank: PROVENANCE_RANK[authority.provenance], year: authority.year ?? 0 })))
    receipts.push({ path, sourceId: source.id, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex'),
      ...parsed.census })
  }
  return { facilities, resetSourceIds: GLOBAL_INDUSTRIAL_SOURCES.map(source => source.id), receipts }
}

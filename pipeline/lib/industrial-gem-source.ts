/** Admit the common national GEM family once, retaining observation order and country ownership. */

import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { bakedIndustrialCountryReader, iso2Code } from './prepared-grid.js'
import { GEM_COUNTRIES, gemAreaContains, gemCountryOwns, type GemCountry } from './industrial-gem-countries.js'
import { PROVENANCE_RANK, SOURCES_BY_ID, SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX, countryOwnershipIsos } from './sources.js'
import type { MatchFacility } from './facility-match.js'
import type { IndustrialOwnership } from './industrial-arrow.js'

const authority = SOURCES_BY_ID.get(SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX)!
const NATIONAL_MIX = { id: authority.id, rank: PROVENANCE_RANK[authority.provenance], year: authority.year ?? 0 }
interface GemPoint { lat: number; lon: number; status: string; fuel: string }

export function parseGemPoints(text: string, country: GemCountry): GemPoint[] {
  const json = JSON.parse(text) as { type?: unknown; features?: unknown[] }
  if (json?.type !== 'FeatureCollection' || !Array.isArray(json.features)) {
    throw new Error(`${country.country}: expected GEM FeatureCollection`)
  }
  if (!json.features.length && !country.knownEmpty) throw new Error(`${country.country}: undocumented empty GEM source`)
  return json.features.map((feature, index) => {
    const row = feature as { geometry?: { type?: string; coordinates?: unknown[] }; properties?: Record<string, unknown> }
    const geometry = row?.geometry, properties = row?.properties
    const [lon, lat] = geometry?.coordinates ?? []
    if (geometry?.type !== 'Point' || typeof lat !== 'number' || typeof lon !== 'number' ||
        !Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180 ||
        !properties || typeof properties.Status !== 'string' || typeof properties.Type !== 'string') {
      throw new Error(`${country.country}: malformed GEM observation ${index}`)
    }
    return { lat, lon, status: properties.Status.toLowerCase(), fuel: properties.Type.toLowerCase() }
  })
}

export function gemFuelNace(fuel: string): number | null {
  if (!fuel || fuel === 'unknown' || fuel.includes('wind')) return null
  if (fuel.includes('solar')) return 3599
  if (fuel.includes('hydro')) return 3512
  return 3511 // Original national GEM policy includes geothermal with thermal generation.
}

export function strictGemCountries(points: readonly GemPoint[], boundaries: string): number[] {
  const root = fileURLToPath(new URL('../..', import.meta.url))
  const run = spawnSync(resolve(root, '.venv/bin/python'), [resolve(root, 'scripts/admin/industrial_countries.py'), boundaries], {
    input: JSON.stringify(points.map(({ lat, lon }) => [lat, lon])), encoding: 'utf8', maxBuffer: 16 * 1024 * 1024,
  })
  if (run.error || run.status !== 0) throw new Error(`GEM strict CGAZ lookup failed: ${run.error?.message ?? run.stderr}`)
  const codes: unknown = JSON.parse(run.stdout)
  if (!Array.isArray(codes) || codes.length !== points.length || codes.some(code => !Number.isInteger(code) || code < 0 || code > 65535)) {
    throw new Error('GEM strict CGAZ lookup returned invalid country codes')
  }
  return codes as number[]
}

export function gemIndustrialOwnership(countries: readonly GemCountry[], facilityCountries: readonly string[]): IndustrialOwnership {
  const byCode = new Map<number, GemCountry>()
  const nc = countries.find(country => country.country === 'NC')
  for (const policy of countries) {
    for (const iso of countryOwnershipIsos(policy.country)) byCode.set(iso2Code(iso), policy)
  }
  return { facilityCountries, countryAt(table) {
    const baked = bakedIndustrialCountryReader(table)
    return (row, polygon) => {
      const code = baked.codeAt(row)
      const policy = nc && gemCountryOwns(nc, polygon.lat, polygon.lon, code) ? nc : byCode.get(code)
      return policy && gemAreaContains(policy, polygon.lat, polygon.lon, code) ? policy.country : null
    }
  } }
}

export function loadGemIndustrialSources(directory: string, boundaries: string) {
  // Every source is admitted before the first prepared row can change.
  const sources = GEM_COUNTRIES.map(country => {
    const path = resolve(directory, country.country.toLowerCase(), 'power-plants-gem.geojson')
    const before = statSync(path, { bigint: true }), bytes = readFileSync(path)
    const points = parseGemPoints(bytes.toString('utf8'), country)
    const after = statSync(path, { bigint: true })
    if (['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'].some(key => before[key as keyof typeof before] !== after[key as keyof typeof after])) {
      throw new Error(`${path}: changed during GEM source admission`)
    }
    return { country, points, path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') }
  })
  const codes = strictGemCountries(sources.flatMap(source => source.points), boundaries)
  const facilities: MatchFacility[] = [], facilityCountries: string[] = [], activeCountries: GemCountry[] = [], receipts = []
  let offset = 0
  for (const source of sources) {
    let outside = 0, inactive = 0, unclassified = 0, classified = 0
    for (const point of source.points) {
      const code = codes[offset++]
      if (!gemAreaContains(source.country, point.lat, point.lon, code)) { outside++; continue }
      if (!point.status.includes('operating')) { inactive++; continue }
      const nace4 = gemFuelNace(point.fuel)
      if (nace4 === null) { unclassified++; continue }
      facilities.push({ lat: point.lat, lon: point.lon, nace4, ...NATIONAL_MIX })
      facilityCountries.push(source.country.country); classified++
    }
    // Like the original driver, an admitted in-area retired feed owns a sweep;
    // a documented empty feed has no destructive authority.
    if (source.points.length > outside) activeCountries.push(source.country)
    receipts.push({ country: source.country.country, path: source.path, bytes: source.bytes, sha256: source.sha256,
      raw: source.points.length, outside, inactive, unclassified, classified,
      state: source.points.length ? 'admitted' : 'documented-empty-no-write' })
  }
  return { facilities, resetSourceIds: [NATIONAL_MIX.id], receipts,
    ownership: gemIndustrialOwnership(activeCountries, facilityCountries) }
}

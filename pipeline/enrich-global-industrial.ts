/**
 * Global industrial enrichment: GPPD (power plants) + E-PRTR (EU facilities)
 * + GEM heavy industry (steel / cement / coal mines).
 *
 * Downloads the registries and stamps nace_4digit + source_id directly into
 * industrial.arrow — ONE polygon per facility (a registry row describes a single
 * site; selection rules live in lib/facility-match.ts). Old stamps from these
 * sources are reset first, so a re-run fully replaces this enricher's output.
 *
 * WHY: OSM only gives generic "landuse=industrial". GPPD provides ~35K power plants
 * worldwide (→ NACE 35, electricity generation). E-PRTR provides ~30K EU regulated
 * facilities with actual NACE codes. Together they dramatically improve sector-specific
 * industrial noise emission profiles globally.
 *
 * Dataset provenance is written per-row; priority resolution keeps higher-priority
 * national entries intact (cz-irz > europe-eprtr > global-gppd).
 *
 * Usage:
 *   cd pipeline && npx tsx enrich-global-industrial.ts
 *   cd pipeline && npx tsx enrich-global-industrial.ts --force-download
 *   cd pipeline && npx tsx enrich-global-industrial.ts --enrich-only
 *   cd pipeline && npx tsx enrich-global-industrial.ts --enrich-only --cells=8419429ffffffff,841e309ffffffff
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { makeTable, tableFromIPC, makeVector } from 'apache-arrow'
import { latLngToCell, gridDisk } from 'h3-js'
import { SOURCES_BY_ID, PROVENANCE_RANK } from './lib/sources.js'
import { bestCandidate, contestBeats, readPolygons, overlapLosers, type MatchPolygon, type OverlapWinner } from './lib/facility-match.js'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import {
  SOURCE_ID_EUROPE_EPRTR,
  SOURCE_ID_GLOBAL_GPPD,
  SOURCE_ID_GLOBAL_GEM_STEEL,
  SOURCE_ID_GLOBAL_GEM_CEMENT,
  SOURCE_ID_GLOBAL_GEM_COALMINE,
} from './lib/source-ids.generated.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, '../data/enrichment/global')
const GPPD_CACHE = resolve(CACHE_DIR, 'gppd.csv')
const EPRTR_CACHE = resolve(CACHE_DIR, 'eprtr-facilities.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
// --cells=<h3r4,h3r4,…> — verification isolation: touch ONLY these cells (small-cells-first
// rule; used for the York+Dobříš before/after checks, never in a production world run).
// NOTE: neighbour-hex facilities are still admitted, so a cell-local decision can differ
// slightly from the world pass (a facility whose true winner lies outside the cell set).
const cellsArg = process.argv.find(a => a.startsWith('--cells='))
const onlyCells = cellsArg ? new Set(cellsArg.slice('--cells='.length).split(',').filter(Boolean)) : null

const GPPD_URL = 'https://raw.githubusercontent.com/wri/global-power-plant-database/master/output_database/global_power_plant_database.csv'

// GEM heavy-industry trackers — public map GeoJSON from GEM's DigitalOcean CDN
// (the same file the live tracker maps fetch; CC-BY-4.0, no auth gate). The
// gated "Download data" form gives a richer per-unit ZIP, but the map GeoJSON
// carries everything noise needs: point lat/lon, lifecycle `status`, and a
// plant/mine type. URLs are pinned to the release referenced by GEM's
// `maps` repo (trackers/<t>/config.js, branch gitpages-production). Bump when
// GEM publishes a newer release.
interface GemTracker {
  key: 'steel' | 'cement' | 'coalmine'
  url: string
  cache: string
  nace: string   // 6-digit NACE → engine emission profile (steel 24, cement 23, coal 05)
}

const GEM_TRACKERS: GemTracker[] = [
  {
    key: 'steel',
    url: 'https://publicgemdata.nyc3.cdn.digitaloceanspaces.com/gist/2025-10/gist_map_2025-10-07.geojson',
    cache: resolve(CACHE_DIR, 'gem-steel.geojson'),
    nace: '241000', // basic iron & steel
  },
  {
    key: 'cement',
    url: 'https://publicgemdata.nyc3.cdn.digitaloceanspaces.com/gcct/2025-07/gcct_map_2025-07-15.geojson',
    cache: resolve(CACHE_DIR, 'gem-cement.geojson'),
    nace: '235100', // cement
  },
  {
    key: 'coalmine',
    url: 'https://publicgemdata.nyc3.cdn.digitaloceanspaces.com/GCMT/2025-09/gcmt_map_2025-09-22-sectionfix.geojson',
    cache: resolve(CACHE_DIR, 'gem-coalmine.geojson'),
    nace: '051000', // hard coal mining
  },
]

// Only active sites emit noise — a retired/cancelled/mothballed/proposed plant
// must not stamp a loud NACE onto an OSM polygon. GEM status values are
// lowercase free text; this is the active allow-list.
const GEM_ACTIVE_STATUS = new Set(['operating', 'operating-pre-retirement'])

const GEM_DATASET_ID: Record<GemTracker['key'], number> = {
  steel: SOURCE_ID_GLOBAL_GEM_STEEL,
  cement: SOURCE_ID_GLOBAL_GEM_CEMENT,
  coalmine: SOURCE_ID_GLOBAL_GEM_COALMINE,
}

// E-PRTR facility data — European Pollutant Release and Transfer Register.
// The European Industrial Emissions Portal (industry.eea.europa.eu) that used to
// serve a facilities CSV export has been down since well before 2026-08 (its
// /download route 500s — a server-side rendering error, not a moved endpoint).
// The EEA's own DISCODATA SQL API (https://discodata.eea.europa.eu/Help.html) is
// the documented, versioned interface behind that portal, so we query it
// directly: [IED].[latest].[ProductionFacility_NoGeo] is the EU Registry on
// Industrial Sites facility table (x_4326/y_4326 = WGS84 centroid,
// EPRTRAnnexIMainActivity = the Annex I sector code mapped to NACE below).
// `reportingYear = MAX(reportingYear)` is a subquery, not a pinned year, so the
// query keeps following the newest published reporting year on its own — the
// CZ RUIAN date bug (pipeline/enrich-buildings-cz.ts) taught us a hardcoded
// "current" value silently rots.
const EPRTR_SQL_QUERY = `
SELECT id, facilityName, countryCode, reportingYear, EPRTRAnnexIMainActivity, x_4326, y_4326
FROM [IED].[latest].[ProductionFacility_NoGeo]
WHERE reportingYear = (SELECT MAX(reportingYear) FROM [IED].[latest].[ProductionFacility_NoGeo])
`.trim()
// nrOfHits caps the single page at 100k rows (current: ~60k) — comfortably above
// today's size with headroom for growth; downloadEprtr() below verifies the
// response stayed under that cap so a future overflow fails loud instead of
// silently truncating.
const EPRTR_PAGE_SIZE = 100_000
const EPRTR_QUERY_URL = `https://discodata.eea.europa.eu/sql?query=${encodeURIComponent(EPRTR_SQL_QUERY)}&p=1&nrOfHits=${EPRTR_PAGE_SIZE}`

const GPPD_DATASET_ID = SOURCE_ID_GLOBAL_GPPD
const EPRTR_DATASET_ID = SOURCE_ID_EUROPE_EPRTR

// E-PRTR/GPPD coordinates are reporting centroids, not the OSM polygon's spot,
// and big sites span >500 m, so match within 2 km. Restored from the old EU-only
// pass after /gg (2026-06-25) found a refactor had silently shrunk this to 500 m,
// dropping ~75% of matches. Sites are spatially sparse → over-reach is rare, and
// the authority/nearest pick below resolves the dense-zone overlaps.
const SEARCH_RADIUS_M = 2000

// Dataset id + authority rank of a facility, used to prefer the higher-authority
// source when several cover one polygon (E-PRTR continental-measured 5 > GPPD/GEM
// global-measured 4) instead of letting a merely-nearer GPPD point win — the
// authority order docs promise (cz-irz > europe-eprtr > {gppd, GEM}).
const facilityDatasetId = (fac: Facility): number =>
  fac.source === 'eprtr' ? EPRTR_DATASET_ID
    : fac.source === 'gppd' ? GPPD_DATASET_ID
    : GEM_DATASET_ID[fac.source.slice('gem-'.length) as GemTracker['key']]
const facilityRank = (fac: Facility): number =>
  PROVENANCE_RANK[SOURCES_BY_ID.get(facilityDatasetId(fac))?.provenance ?? 'none']

// ── Types ──

interface Facility {
  name: string
  lat: number
  lon: number
  nace: string   // 6-digit NACE code string, e.g. "350000"
  source: 'gppd' | 'eprtr' | 'gem-steel' | 'gem-cement' | 'gem-coalmine'
}

// ── Step 1: Download GPPD ──

async function downloadGppd(): Promise<string> {
  if (enrichOnly || (!forceDownload && existsSync(GPPD_CACHE))) {
    if (!existsSync(GPPD_CACHE)) {
      console.error('ERROR: --enrich-only but no GPPD cache found at', GPPD_CACHE)
      process.exit(1)
    }
    console.log(`  Using cached GPPD: ${GPPD_CACHE}`)
    return readFileSync(GPPD_CACHE, 'utf-8')
  }

  console.log(`  Downloading GPPD from ${GPPD_URL}...`)
  const res = await fetch(GPPD_URL, { signal: AbortSignal.timeout(120_000) })
  if (!res.ok) throw new Error(`GPPD download failed: ${res.status} ${res.statusText}`)
  const text = await res.text()

  mkdirSync(CACHE_DIR, { recursive: true })
  writeFileSync(GPPD_CACHE, text)
  console.log(`  Cached GPPD to ${GPPD_CACHE} (${(text.length / 1024 / 1024).toFixed(1)} MB)`)
  return text
}

// ── Step 2: Download E-PRTR ──

// Shared by both return paths in downloadEprtr() below: parseEprtr()'s own
// JSON.parse is unguarded (unlike parseGem()'s), so it relies on every string
// downloadEprtr() hands back — cached or freshly downloaded — already being
// proven-parseable, proven-sized JSON. Skipping this on the cache-hit path
// would let a corrupted cache file (partial write, disk fault, hand edit)
// throw uncaught out of parseEprtr() and abort GPPD's and GEM's results too,
// contradicting downloadEprtr()'s own "one source's failure doesn't abort the
// run" contract below.
function validateEprtrJson(text: string, sourceLabel: string): void {
  const parsed = JSON.parse(text) as { results?: unknown[] }
  if (!Array.isArray(parsed.results) || parsed.results.length < 100) {
    throw new Error(`${sourceLabel} has ${parsed.results?.length ?? 0} rows (expected tens of thousands) — DISCODATA schema or query may have changed, or the cache is corrupt`)
  }
  if (parsed.results.length >= EPRTR_PAGE_SIZE) {
    throw new Error(`${sourceLabel} has ${parsed.results.length} rows, at/above the ${EPRTR_PAGE_SIZE} single-page cap — results may be truncated, add pagination (p=2, ...) instead of raising the cap blindly`)
  }
}

// A failed E-PRTR refresh must never look like success: it's reported with
// console.error (not the routine console.log WARN other optional sources use)
// naming the exact URL and cause, and the caller leaves E-PRTR's existing
// stamps untouched rather than resetting them against an empty result (see the
// participating-source floor in main()). It does not abort the whole run —
// GPPD and GEM are independent sources and still complete (same tolerance the
// per-tracker GEM downloads already have below).
async function downloadEprtr(): Promise<string | null> {
  if (enrichOnly || (!forceDownload && existsSync(EPRTR_CACHE))) {
    if (!existsSync(EPRTR_CACHE)) {
      console.log('  WARN: --enrich-only but no E-PRTR cache — skipping E-PRTR')
      return null
    }
    const cached = readFileSync(EPRTR_CACHE, 'utf-8')
    try {
      validateEprtrJson(cached, EPRTR_CACHE)
    } catch (err: any) {
      console.error(`  ERROR: cached E-PRTR is unusable — ${err.message}`)
      console.error(`  Delete ${EPRTR_CACHE} and re-run without --enrich-only to refresh it. E-PRTR stamps from a previous run (if any) are left untouched.`)
      return null
    }
    console.log(`  Using cached E-PRTR: ${EPRTR_CACHE}`)
    return cached
  }

  console.log(`  Downloading E-PRTR from DISCODATA: ${EPRTR_QUERY_URL}`)
  try {
    const res = await fetch(EPRTR_QUERY_URL, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const text = await res.text()

    // JSON.parse (inside validateEprtrJson) throws its own actionable SyntaxError
    // (e.g. "Unexpected token <" — a giveaway the response was an HTML error page)
    // straight into the catch below, which is where the URL gets attached.
    validateEprtrJson(text, EPRTR_QUERY_URL)

    mkdirSync(CACHE_DIR, { recursive: true })
    writeFileSync(EPRTR_CACHE, text)
    console.log(`  Cached E-PRTR to ${EPRTR_CACHE} (${(text.length / 1024 / 1024).toFixed(1)} MB)`)
    return text
  } catch (err: any) {
    console.error(`  ERROR: E-PRTR refresh failed — GET ${EPRTR_QUERY_URL} — ${err.message}`)
    console.error(`  E-PRTR stamps from a previous run (if any) are left untouched, NOT re-stamped. Fix the query/endpoint above and re-run.`)
    return null
  }
}

// ── Step 3: Parse GPPD CSV ──

async function parseGppd(csvText: string): Promise<Facility[]> {
  const { parse } = await import('csv-parse/sync')
  const records = parse(csvText, {
    columns: true,
    skip_empty_lines: true,
    bom: true,
    relax_column_count: true,
  }) as Record<string, string>[]

  console.log(`  GPPD: ${records.length} raw records, columns: ${Object.keys(records[0] || {}).slice(0, 6).join(', ')}...`)

  const facilities: Facility[] = []
  for (const r of records) {
    const lat = parseFloat(r['latitude'] || '')
    const lon = parseFloat(r['longitude'] || '')
    if (!lat || !lon || isNaN(lat) || isNaN(lon)) continue

    const capacity = parseFloat(r['capacity_mw'] || '0')
    const name = (r['name'] || '').trim()
    const fuel = (r['primary_fuel'] || '').trim()

    // Power plants → NACE 35 (Electricity, gas, steam and air conditioning supply),
    // sub-classified by fuel so the engine picks the right emission profile:
    //   3512 hydro 90dB · 3511 thermal 97dB · 3599 solar 55dB.
    // Wind is modelled separately (source_type=10) — never stamp it onto an OSM
    // industrial polygon, or it inherits a thermal/hydro spectrum. Blank/unknown
    // fuel is left unstamped too: without a fuel we can't pick a profile, so we
    // keep the OSM row untouched rather than guess. nace = '' is the skip sentinel.
    let nace = ''
    if (fuel === 'Wind') nace = ''  // modelled separately — skip
    else if (fuel === 'Nuclear') nace = '351100'
    else if (fuel === 'Hydro') nace = '351200'
    else if (fuel === 'Solar') nace = '359900'
    else if (fuel === 'Gas' || fuel === 'Oil') nace = '351100'
    else if (fuel === 'Coal' || fuel === 'Petcoke') nace = '351100'
    else if (fuel === 'Biomass' || fuel === 'Waste') nace = '351100'
    else if (fuel === 'Geothermal') nace = '351200'

    facilities.push({ name: name || `Power Plant (${fuel}, ${capacity}MW)`, lat, lon, nace, source: 'gppd' })
  }

  console.log(`  GPPD: ${facilities.length} facilities with valid coordinates`)
  return facilities
}

// ── Step 4: Parse E-PRTR (DISCODATA JSON) ──

// E-PRTR Annex I main activity sectors (1-9) → representative NACE 6-digit for noise
// profiling. DISCODATA's ProductionFacility_NoGeo table exposes the Annex I activity
// code, e.g. "5(a)" / "4(a)(viii)", NOT a NACE code. Sector granularity is enough for
// the emission profile — the sector fixes the plant type and thus the loudness class.
// Ref: Regulation (EC) No 166/2006 Annex I.
const EPRTR_ANNEX_SECTOR_TO_NACE: Record<number, string> = {
  1: '351100', // Energy: refineries, coke, thermal power, combustion → electricity/thermal
  2: '241000', // Metals: iron, steel, ferrous/non-ferrous → basic metals
  3: '235100', // Mineral: cement, lime, glass, ceramics → cement
  4: '201100', // Chemical: organic/inorganic, fertilizers, pharma → basic chemicals
  5: '382100', // Waste & waste-water management → waste treatment
  6: '171100', // Paper & wood production → pulp/paper
  7: '014600', // Intensive livestock & aquaculture → animal production
  8: '101100', // Animal/vegetable products (food & beverage) → meat processing
  9: '131000', // Other: textile, leather, surface treatment, shipyards → textiles
}

// Row shape from EPRTR_SQL_QUERY — the columns are fixed by that query, not sniffed,
// since we own the query and DISCODATA's schema is documented (unlike a CSV export of
// unknown provenance, there's nothing to guess here).
interface EprtrRow {
  facilityName: string | null
  EPRTRAnnexIMainActivity: string | null
  x_4326: number | null
  y_4326: number | null
}

async function parseEprtr(jsonText: string): Promise<Facility[]> {
  const parsed = JSON.parse(jsonText) as { results: EprtrRow[] }
  const rows = parsed.results
  console.log(`  E-PRTR: ${rows.length} raw records from DISCODATA`)

  const facilities: Facility[] = []
  let skippedNoCoords = 0, skippedNoActivity = 0
  for (const r of rows) {
    const lat = r.y_4326
    const lon = r.x_4326
    if (lat == null || lon == null || Math.abs(lat) > 90 || Math.abs(lon) > 180) { skippedNoCoords++; continue }

    // Annex I activity code (e.g. "5(a)", "4(a)(viii)") → sector 1-9 → NACE.
    const annexSector = (r.EPRTRAnnexIMainActivity ?? '').match(/^\s*(\d{1,2})\s*[.(]/)
    const nace = annexSector ? (EPRTR_ANNEX_SECTOR_TO_NACE[parseInt(annexSector[1], 10)] ?? '') : ''
    if (!nace) { skippedNoActivity++; continue }

    facilities.push({ name: r.facilityName?.trim() || 'E-PRTR Facility', lat, lon, nace, source: 'eprtr' })
  }

  console.log(`  E-PRTR: ${facilities.length} facilities with valid coordinates + Annex I activity (skipped ${skippedNoCoords} no-coords, ${skippedNoActivity} no-activity-code)`)
  return facilities
}

// ── Step 4b: Download + parse GEM heavy-industry trackers ──

async function downloadGem(t: GemTracker): Promise<string | null> {
  if (enrichOnly || (!forceDownload && existsSync(t.cache))) {
    if (!existsSync(t.cache)) {
      console.log(`  WARN: no GEM ${t.key} cache at ${t.cache} — skipping`)
      return null
    }
    console.log(`  Using cached GEM ${t.key}: ${t.cache}`)
    return readFileSync(t.cache, 'utf-8')
  }

  console.log(`  Downloading GEM ${t.key} from ${t.url}...`)
  try {
    const res = await fetch(t.url, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) {
      console.log(`  WARN: GEM ${t.key} download failed: ${res.status} ${res.statusText} — skipping`)
      return null
    }
    const text = await res.text()
    mkdirSync(CACHE_DIR, { recursive: true })
    writeFileSync(t.cache, text)
    console.log(`  Cached GEM ${t.key} to ${t.cache} (${(text.length / 1024 / 1024).toFixed(1)} MB)`)
    return text
  } catch (err: any) {
    console.log(`  WARN: GEM ${t.key} download error: ${err.message} — skipping`)
    return null
  }
}

// GEM map GeoJSON: FeatureCollection of Point features. Coordinates live both
// as geometry [lon, lat] and as `Latitude`/`Longitude` properties — we prefer
// the explicit properties (some rows carry a display-jittered geometry) and
// fall back to geometry. We stamp one NACE per tracker (the sector is fixed by
// the dataset), gated on an active lifecycle `status`.
function parseGem(t: GemTracker, jsonText: string): Facility[] {
  let fc: any
  try {
    fc = JSON.parse(jsonText)
  } catch (err: any) {
    console.log(`  WARN: GEM ${t.key} — invalid JSON (${err.message}) — skipping`)
    return []
  }
  const features: any[] = Array.isArray(fc?.features) ? fc.features : []
  console.log(`  GEM ${t.key}: ${features.length} raw features`)

  const facilities: Facility[] = []
  let inactive = 0
  for (const f of features) {
    const p = f?.properties ?? {}
    const status = String(p['status'] ?? '').trim().toLowerCase()
    if (!GEM_ACTIVE_STATUS.has(status)) { inactive++; continue }

    const geom: number[] | undefined = f?.geometry?.coordinates
    const lat = parseFloat(p['Latitude'] ?? p['latitude'] ?? (geom ? geom[1] : ''))
    const lon = parseFloat(p['Longitude'] ?? p['longitude'] ?? (geom ? geom[0] : ''))
    if (!isFinite(lat) || !isFinite(lon) || lat === 0 || lon === 0) continue
    if (Math.abs(lat) > 90 || Math.abs(lon) > 180) continue

    const name = String(p['name'] ?? '').trim() || `GEM ${t.key}`
    facilities.push({ name, lat, lon, nace: t.nace, source: `gem-${t.key}` })
  }

  console.log(`  GEM ${t.key}: ${facilities.length} active facilities (skipped ${inactive} inactive/non-operating)`)
  return facilities
}

// ── Step 5: Build spatial index (facilities grouped by H3R4 hex) ──

function groupByHex<T extends Facility>(facilities: T[]): Map<string, T[]> {
  const byHex = new Map<string, T[]>()
  let skipped = 0

  for (const fac of facilities) {
    try {
      const h3r4 = latLngToCell(fac.lat, fac.lon, 4)
      if (!byHex.has(h3r4)) byHex.set(h3r4, [])
      byHex.get(h3r4)!.push(fac)
    } catch {
      skipped++
    }
  }

  if (skipped > 0) console.log(`  Skipped ${skipped} facilities (invalid H3 coordinates)`)
  console.log(`  Facilities spread across ${byHex.size} H3R4 hexes`)
  return byHex
}

// ── Step 6: Facility→polygon match — ONE facility, ONE site ──
//
// A registry row describes a single site, so it stamps a single OSM polygon.
// Selection rules + the old carpet-join backstory live in lib/facility-match.ts.
//
// Two passes so a facility near an R4 border converges to ONE winner globally
// (deciding per hex would let it win once in each neighbouring arrow — /gg Codex):
//   pass 1 (read-only)  every hex → best candidate per facility, reduced globally
//   phase B             polygon contested by several facilities → contestBeats
//   pass 2 (write)      reset our old stamps (only sources that parsed OK) + stamp winners

interface PreparedFacility extends Facility {
  nace4: number
  id: number
  rank: number
  year: number
}

// Drop the empty-nace sentinel (wind / blank-fuel) and precompute the contest
// fields once per facility — the match loops are then pure selection.
function prepareFacilities(all: Facility[]): PreparedFacility[] {
  return all.filter(f => f.nace !== '').map(f => {
    const id = facilityDatasetId(f)
    return {
      ...f,
      nace4: Math.floor((parseInt(f.nace, 10) || 0) / 100),
      id,
      rank: facilityRank(f),
      year: SOURCES_BY_ID.get(id)?.year ?? 0,
    }
  })
}

function hexArrowPath(hexId: string): string {
  return resolve(H3R4_DIR, hexId, 'industrial.arrow')
}

/** Returns matches per source (for the provenance report); all other stats are logged here. */
async function enrichHexes(
  facByHex: Map<string, PreparedFacility[]>,
  resetIds: Set<number>,
  onlyCells: Set<string> | null,
): Promise<Map<Facility['source'], number>> {
  const hexDirs = readdirSync(H3R4_DIR)
    .filter(d => d.length === 15 && d.endsWith('ffffffff'))
    .filter(d => !onlyCells || onlyCells.has(d))

  const startTime = Date.now()
  let lastLog = startTime
  const progress = (phase: string, i: number) => {
    const now = Date.now()
    if (now - lastLog >= 10_000) {
      console.log(`  ${phase}: ${i}/${hexDirs.length} hexes, ${((now - startTime) / 1000).toFixed(0)}s elapsed`)
      lastLog = now
    }
  }

  // ── pass 1: globally best polygon per facility (read-only) ──
  const bestByFac = new Map<PreparedFacility, { hex: string; row: number; edge: number; lat: number; lon: number; areaM2: number }>()
  let totalIndustrial = 0
  for (const [i, hexId] of hexDirs.entries()) {
    progress('match', i + 1)
    // this hex AND its 6 neighbours: a facility just across an R4 border can still
    // be within SEARCH_RADIUS_M of a polygon in this hex
    const hexFacilities = gridDisk(hexId, 1).flatMap(h => facByHex.get(h) ?? [])
    if (hexFacilities.length === 0) continue
    const indPath = hexArrowPath(hexId)
    if (!existsSync(indPath)) continue
    let polygons: MatchPolygon[]
    try {
      polygons = readPolygons(tableFromIPC(readFileSync(indPath)))
    } catch (err: any) {
      console.log(`  WARN: unreadable ${indPath}: ${err.message}`)
      continue
    }
    totalIndustrial += polygons.length
    for (const fac of hexFacilities) {
      const cand = bestCandidate(fac, polygons, SEARCH_RADIUS_M)
      if (!cand) continue
      const prev = bestByFac.get(fac)
      if (!prev || cand.edge < prev.edge) {
        const p = polygons[cand.row]
        bestByFac.set(fac, { hex: hexId, row: cand.row, edge: cand.edge, lat: p.lat, lon: p.lon, areaM2: p.areaM2 })
      }
    }
  }

  // ── phase B: a polygon claimed by several facilities → shouldOverwrite order ──
  type Winner = { fac: PreparedFacility; edge: number; lat: number; lon: number; areaM2: number }
  const winnersByHex = new Map<string, Map<number, Winner>>()
  for (const [fac, w] of bestByFac) {
    const rows = winnersByHex.get(w.hex) ?? new Map<number, Winner>()
    winnersByHex.set(w.hex, rows)
    const cur = rows.get(w.row)
    if (!cur || contestBeats(
      { rank: fac.rank, year: fac.year, id: fac.id, edge: w.edge },
      { rank: cur.fac.rank, year: cur.fac.year, id: cur.fac.id, edge: cur.edge },
    )) rows.set(w.row, { fac, edge: w.edge, lat: w.lat, lon: w.lon, areaM2: w.areaM2 })
  }

  // ── phase B2: I-07 dual-registry overlap dedup — across winning ROWS (not
  // facilities), collapse same-site duplicates (E-PRTR + GPPD/GEM each won a
  // different coincident polygon for ONE physical plant → both emit, +2.7 dB at
  // Temelín). overlapLosers reuses the same contestBeats authority, so E-PRTR
  // (rank 5) keeps its row; the loser gets a `suppressed=1` marker (below) that
  // both loaders skip. Idempotent: recomputed from scratch every run. ──
  const overlapCandidates: OverlapWinner[] = []
  for (const [hex, rows] of winnersByHex) {
    for (const [row, w] of rows) {
      overlapCandidates.push({
        key: `${hex}:${row}`, lat: w.lat, lon: w.lon, areaM2: w.areaM2,
        rank: w.fac.rank, year: w.fac.year, id: w.fac.id, edge: w.edge,
      })
    }
  }
  const suppressedKeys = overlapLosers(overlapCandidates)
  if (suppressedKeys.size > 0) console.log(`  I-07 overlap dedup: ${suppressedKeys.size} duplicate polygon(s) suppressed`)

  // ── pass 2: reset-then-stamp (write) ──
  let totalMatched = 0
  let totalReset = 0
  let hexesWithMatches = 0
  const matchedBySource = new Map<Facility['source'], number>()
  for (const [i, hexId] of hexDirs.entries()) {
    progress('write', i + 1)
    const winners = winnersByHex.get(hexId)
    const indPath = hexArrowPath(hexId)
    if (!existsSync(indPath)) continue
    // Even a hex with no winner today gets the reset sweep: a retired/dropped
    // facility (GEM filters inactive plants; E-PRTR reporting years move) leaves
    // old carpet stamps behind with nothing in reach to replace them — skipping
    // would strand them as phantom heavy industry forever (/gg consensus).
    // withArrowWrite only rewrites the file when something actually changed.
    if (!winners && resetIds.size === 0) continue

    try {
      await withArrowWrite(indPath, table => {
        const n = table.numRows
        if (n === 0) return table

        const existingNaceCol = table.getChild('nace_4digit')
        const existingDatasetIdCol = table.getChild('source_id')
        const existingSuppressedCol = table.getChild('suppressed')
        const newNace = new Uint16Array(n)
        const newDatasetId = new Uint16Array(n)
        const newSuppressed = new Uint8Array(n) // I-07 marker, recomputed fresh every run (0 = emits)
        for (let j = 0; j < n; j++) {
          newNace[j] = (existingNaceCol?.get(j) as number) ?? 0
          newDatasetId[j] = (existingDatasetIdCol?.get(j) as number) ?? 0
        }
        let anyChanged = false

        // reset OUR previous stamps (only sources that parsed OK this run — a failed
        // download must never wipe rows it can't re-stamp, /gg Codex CRITICAL)
        for (let i = 0; i < n; i++) {
          if (resetIds.has(newDatasetId[i])) {
            newNace[i] = 0
            newDatasetId[i] = 0
            totalReset++
            anyChanged = true
          }
        }

        // stamp this hex's winners — ONE polygon per facility, decided globally
        let hexMatched = 0
        if (winners) {
          for (const [row, w] of winners) {
            if (row >= n) continue
            if (!shouldOverwrite(newDatasetId[row], w.fac.id)) continue
            newNace[row] = w.fac.nace4
            newDatasetId[row] = w.fac.id
            hexMatched++
            totalMatched++
            matchedBySource.set(w.fac.source, (matchedBySource.get(w.fac.source) ?? 0) + 1)
            anyChanged = true
            if (onlyCells) console.log(`    ${hexId} row ${row}: ${w.fac.source} nace4=${w.fac.nace4} edge=${w.edge.toFixed(0)}m (${w.fac.name})`)
          }
        }

        if (hexMatched > 0) hexesWithMatches++

        // I-07: mark overlap-dedup losers suppressed. Recomputed fresh each run
        // (default 0), so a row that is no longer a duplicate clears back to 0 —
        // the change is detected against the existing column so the file rewrites.
        for (let i = 0; i < n; i++) {
          if (suppressedKeys.has(`${hexId}:${i}`)) newSuppressed[i] = 1
          if (newSuppressed[i] !== ((existingSuppressedCol?.get(i) as number) ?? 0)) anyChanged = true
        }

        if (!anyChanged) return table

        const columns: Record<string, any> = {}
        for (const field of table.schema.fields) {
          if (field.name === 'nace_4digit' || field.name === 'source_id' || field.name === 'suppressed') continue
          columns[field.name] = table.getChild(field.name)!
        }
        columns['nace_4digit'] = makeVector(newNace)
        columns['source_id'] = makeVector(newDatasetId)
        columns['suppressed'] = makeVector(newSuppressed)
        return makeTable(columns)
      })
    } catch (err: any) {
      console.log(`  WARN: Failed to process ${indPath}: ${err.message}`)
    }
  }

  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)
  console.log(`\n=== Facility→Polygon Match Results ===`)
  console.log(`  Hexes scanned: ${hexDirs.length} (${hexesWithMatches} with matches)`)
  console.log(`  Industrial polygons scanned: ${totalIndustrial}`)
  console.log(`  Old stamps reset: ${totalReset} (sources: ${[...resetIds].join(', ') || 'none'})`)
  console.log(`  Facilities with a winner: ${bestByFac.size} of ${[...facByHex.values()].flat().length}`)
  console.log(`  Polygons stamped: ${totalMatched} (max 1 per facility by construction)`)
  for (const [src, cnt] of [...matchedBySource].sort((a, b) => b[1] - a[1])) {
    console.log(`    ${src}: ${cnt}`)
  }
  console.log(`  Time: ${elapsed}s`)

  return matchedBySource
}

// ── Main ──

async function main() {
  console.log(`=== Global Industrial Enrichment (GPPD + E-PRTR + GEM heavy industry) ===\n`)
  console.log(`  DATA_YEAR: ${YEAR}`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache dir: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    console.error(`  Run OSM extraction first, or set DATA_YEAR=...`)
    process.exit(1)
  }

  // ── Download phase ──
  console.log('--- Step 1: Download GPPD ---')
  const gppdCsv = await downloadGppd()

  console.log('\n--- Step 2: Download E-PRTR ---')
  const eprtrJson = await downloadEprtr()

  // ── Parse phase ──
  console.log('\n--- Step 3: Parse GPPD ---')
  const gppdFacilities = await parseGppd(gppdCsv)

  let eprtrFacilities: Facility[] = []
  if (eprtrJson) {
    console.log('\n--- Step 4: Parse E-PRTR ---')
    eprtrFacilities = await parseEprtr(eprtrJson)
  }

  console.log('\n--- Step 4b: Download + parse GEM heavy-industry trackers ---')
  let gemFacilities: Facility[] = []
  const gemCounts = new Map<GemTracker['key'], number>()
  for (const t of GEM_TRACKERS) {
    const json = await downloadGem(t)
    if (!json) continue
    const parsed = parseGem(t, json)
    gemCounts.set(t.key, parsed.length)
    gemFacilities = gemFacilities.concat(parsed)
  }

  const allFacilities = [...gppdFacilities, ...eprtrFacilities, ...gemFacilities]
  console.log(`\n  Total facilities: ${allFacilities.length} (GPPD: ${gppdFacilities.length}, E-PRTR: ${eprtrFacilities.length}, GEM: ${gemFacilities.length})`)

  if (allFacilities.length === 0) {
    console.error('ERROR: No facilities parsed from any source')
    process.exit(1)
  }

  // A source participates FULLY or not at all: its old stamps are reset AND its
  // facilities stamp only when it parsed a big-enough share of its normal size
  // this run (GPPD ~35 k, E-PRTR ~50 k post-filter from ~60 k raw DISCODATA rows,
  // GEM 900-4 000 — floors catch a corrupt/truncated download, /gg Codex). A
  // sub-floor source stamping WITHOUT its reset would leave a mixed state: fresh
  // stamps on top of its stale carpet (/gg Gemini).
  const resetIds = new Set<number>()
  if (gppdFacilities.length >= 1000) resetIds.add(GPPD_DATASET_ID)
  if (eprtrFacilities.length >= 10_000) resetIds.add(EPRTR_DATASET_ID)
  for (const t of GEM_TRACKERS) {
    if ((gemCounts.get(t.key) ?? 0) >= 100) resetIds.add(GEM_DATASET_ID[t.key])
  }
  const participating = (f: Facility) => resetIds.has(facilityDatasetId(f))
  const excluded = allFacilities.filter(f => !participating(f)).length
  if (excluded > 0) console.log(`  WARN: ${excluded} facilities EXCLUDED (their source parsed below its safety floor — fix the download and re-run)`)
  console.log(`  Participating source ids this run: ${[...resetIds].join(', ') || 'NONE (no source parsed sanely)'}`)

  // ── Spatial index ──
  console.log('\n--- Step 5: Group by H3R4 hex ---')
  const facByHex = groupByHex(prepareFacilities(allFacilities.filter(participating)))

  // ── Facility→polygon match (writes directly into industrial.arrow) ──
  console.log('\n--- Step 6: Facility→polygon match (one facility, one site) ---')
  const matchedBySource = await enrichHexes(facByHex, resetIds, onlyCells)
  const m = (s: Facility['source']) => matchedBySource.get(s) ?? 0

  // ── Provenance ──
  const provPath = resolve(CACHE_DIR, 'provenance.md')
  const provenance = `# Global Industrial Enrichment Provenance

## Sources used
- **GPPD**: Global Power Plant Database (WRI), ${GPPD_URL}, ${gppdFacilities.length} power plants → ${m('gppd')} matched, CC-BY-4.0
- **E-PRTR**: European Pollutant Release and Transfer Register (EEA), ${eprtrFacilities.length} facilities → ${m('eprtr')} matched, CC-BY-4.0
- **GEM Iron & Steel** (NACE 2410): ${GEM_TRACKERS[0].url} → ${m('gem-steel')} matched, CC-BY-4.0
- **GEM Cement & Concrete** (NACE 2351): ${GEM_TRACKERS[1].url} → ${m('gem-cement')} matched, CC-BY-4.0
- **GEM Coal Mine** (NACE 0510): ${GEM_TRACKERS[2].url} → ${m('gem-coalmine')} matched, CC-BY-4.0

## Matching
- ONE polygon per facility: each registry point picks a single OSM industrial polygon
  within 2 km, scored by distance to the polygon EDGE (centroid dist − √(area/π)), so a
  large plant beats a nearer shed; quiet subtypes (farm/warehouse/office) accept only
  their own industry family (always — a registry point ON an office is typically its
  registered address, not the plant)
- A polygon claimed by several facilities goes to the higher authority (rank → year → id),
  edge distance last; decided globally, so a facility near an R4 border wins exactly once
- Previous stamps from these sources are reset first (only sources that parsed this run)
- Written directly to industrial.arrow per-row (nace_4digit + source_id)
- Dataset priority preserves national registries (cz-irz > europe-eprtr > {global-gppd, GEM})
- GEM trackers stamp only active sites (status operating / operating pre-retirement)

## Gaps
- E-PRTR covers EU only; GPPD covers power generation (NACE 35) only
- GEM trackers add three heavy-industry sectors worldwide (steel / cement / coal mining),
  but only sites ≥ tracker capacity threshold (e.g. steel ≥ 500 ktpa) — small sites need
  country-specific registries
- Facilities without a nearby OSM industrial polygon (within 2 km) are not matched
- GEM map GeoJSON is the public subset; the gated form download carries finer per-unit detail

## Run date
${new Date().toISOString()}
`
  writeFileSync(provPath, provenance)
  console.log(`\n  Provenance written to ${provPath}`)

  console.log('\n=== Done ===')
}

main().catch(err => { console.error('FATAL:', err); process.exit(1) })

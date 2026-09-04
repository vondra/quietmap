/**
 * Enrich CZ buildings.arrow with RÚIAN data (floor count + building use type).
 *
 * Downloads VFR zakladni per municipality from ČÚZK, extracts StavebniObjekt
 * fields (PocetPodlazi, ZpusobVyuzitiKod, centroid GPS), spatial-joins to
 * buildings.arrow, fills missing floors and refines building_type.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-buildings-cz.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-buildings-cz.ts --enrich-only
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import proj4 from 'proj4'
import { shouldOverwrite } from './lib/provenance.js'
import { writeBuildingEnrichment } from './lib/buildings-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { SOURCE_ID_CZ_RUIAN_VFR } from './lib/source-ids.generated.js'
import { flatDist } from './lib/spatial.js'
import { DATA_YEAR as YEAR, OSM_EXTRACT_DIR, requireOsmExtractTree } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CZ_RUIAN_VFR

// Define S-JTSK (EPSG:5514) projection
proj4.defs('EPSG:5514', '+proj=krovak +lat_0=49.5 +lon_0=24.83333333333333 +alpha=30.28813975277778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +towgs84=570.8,85.7,462.8,4.998,1.587,5.261,3.56 +units=m +no_defs')

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cz`)
const CACHE_FILE = resolve(CACHE_DIR, 'ruian-buildings.json')

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

// ČÚZK's INSPIRE Atom index for the "současná stavová data - základní datová
// sada" (current state, basic dataset) feed — the feed advertised for RUIAN-S-ZA-U
// by https://atom.cuzk.cz's own getcapabilities.ashx catalog. It lists exactly the
// files currently live under vdp.cuzk.gov.cz/vymenny_format/soucasna/, so its single
// "*_ST_UZSZ.xml.zip" entry names the authoritative current publication date.
const VFR_INDEX_URL = 'https://services.cuzk.cz/atom-index/RUIAN-S-ZA-U/5514/'
const CONCURRENCY = 20

// RÚIAN ZpusobVyuzitiKod → our building_type mapping
const RUIAN_TO_BUILDING_TYPE: Record<number, number> = {
  6: 0,   // bytový dům → residential
  7: 0,   // rodinný dům → residential
  8: 0,   // rekreační → residential
  9: 9,   // shromažďovací → public
  10: 1,  // obchod → commercial
  11: 6,  // ubytovací → hotel
  12: 2,  // výroba/sklad → warehouse/industrial
  13: 8,  // zemědělská → farm
  14: 1,  // administrativa → commercial
  15: 9,  // občanská vybavenost → public (includes schools, hospitals)
  16: 2,  // technické vybavení → warehouse
  17: 7,  // doprava → garage
  18: 7,  // garáž → garage
  19: 0,  // jiná → default residential
  20: 1,  // víceúčelová → commercial
  21: 8,  // skleník → farm
  2: 8,   // zemědělská usedlost → farm
}

/** S-JTSK (EPSG:5514) → WGS84 via proj4. Input: [Y, X] both negative. */
function sjtskToWgs84(y: number, x: number): [number, number] {
  const [lon, lat] = proj4('EPSG:5514', 'EPSG:4326', [y, x])
  return [lat, lon]
}

interface RuianBuilding {
  lat: number
  lon: number
  floors: number
  useCode: number
}

// ČÚZK republishes "současná" (current) VFR roughly once a month and keeps only
// the last ~2 months live — anything older 404s. A pinned date therefore rolls
// off within weeks (it happened: 2026-03-31 died once April/May/June/July
// published over it). The publication date must be discovered fresh every run,
// never hardcoded, and the run must fail loudly — never silently reuse a stale
// date or skip the refresh — if discovery itself fails.
async function discoverVfrDatePrefix(): Promise<string> {
  const res = await fetch(VFR_INDEX_URL, { signal: AbortSignal.timeout(30000) }).catch((err: any) => {
    throw new Error(`RÚIAN VFR publication date discovery failed: GET ${VFR_INDEX_URL} — ${err.message}`)
  })
  if (!res.ok) {
    throw new Error(`RÚIAN VFR publication date discovery failed: GET ${VFR_INDEX_URL} → HTTP ${res.status}`)
  }
  const html = await res.text()
  // The index normally carries exactly one live publication (each href+link-text
  // pair repeats the same date twice), but take the max rather than the first
  // match in HTML order — an undocumented page change adding a second, older
  // entry (e.g. an archive link) must not silently pin last month's date.
  const dates = new Set([...html.matchAll(/(\d{8})_ST_UZSZ\.xml\.zip/g)].map(m => m[1]))
  if (dates.size === 0) {
    throw new Error(`RÚIAN VFR publication date discovery failed: no "*_ST_UZSZ.xml.zip" entry found in ${VFR_INDEX_URL}`)
  }
  return [...dates].sort().at(-1)!
}

// ── Step 1: Download RÚIAN per municipality ──

async function downloadRuian(): Promise<RuianBuilding[]> {
  if (!forceDownload && existsSync(CACHE_FILE)) {
    console.log(`  Using cached RÚIAN data: ${CACHE_FILE}`)
    return JSON.parse(readFileSync(CACHE_FILE, 'utf-8'))
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_FILE)) { console.error('ERROR: --enrich-only but no cache'); process.exit(1) }
    return JSON.parse(readFileSync(CACHE_FILE, 'utf-8'))
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  // Discover today's publication date before touching any dated URL — see
  // discoverVfrDatePrefix() for why this can't be pinned.
  console.log(`  Discovering current VFR publication date from ${VFR_INDEX_URL}...`)
  const DATE_PREFIX = await discoverVfrDatePrefix()
  console.log(`  VFR publication date: ${DATE_PREFIX}`)

  // Get municipality list: download state-level RÚIAN CSV for municipality codes
  console.log('  Getting municipality list...')

  // Use WFS GetFeature to get all municipality codes (lightweight query)
  // Alternative: hardcoded list from RÚIAN state VFR (municipality codes don't change often)
  // For now: use the state-level VFR which has Obec list
  const stateUrl = `https://vdp.cuzk.gov.cz/vymenny_format/soucasna/${DATE_PREFIX}_ST_UZSZ.xml.zip`
  const stateRes = await fetch(stateUrl, { signal: AbortSignal.timeout(120000) })
  if (!stateRes.ok) throw new Error(`State VFR download failed: ${stateRes.status} ${stateUrl}`)

  const stateBuf = Buffer.from(await stateRes.arrayBuffer())
  const stateZip = resolve(CACHE_DIR, 'state-vfr.zip')
  writeFileSync(stateZip, stateBuf)
  console.log(`  State VFR: ${(stateBuf.length / 1e6).toFixed(1)} MB`)

  // Extract municipality codes from state VFR
  const { execSync } = await import('node:child_process')
  const stateXml = execSync(`unzip -p "${stateZip}"`, { maxBuffer: 500 * 1024 * 1024, timeout: 30000 }).toString()
  const obecRegex = /<obi:Kod>(\d{6})<\/obi:Kod>/g
  const municipalityCodes = new Set<string>()
  let mc
  while ((mc = obecRegex.exec(stateXml)) !== null) {
    municipalityCodes.add(mc[1])
  }

  const municipalities = [...municipalityCodes].map(code => ({
    url: `https://vdp.cuzk.gov.cz/vymenny_format/soucasna/${DATE_PREFIX}_OB_${code}_UZSZ.xml.zip`,
    code,
  }))
  console.log(`  ${municipalities.length} municipalities found`)

  // Download and parse in batches
  const { execSync: exec } = await import('node:child_process')
  const { unlinkSync } = await import('node:fs')
  const allBuildings: RuianBuilding[] = []
  let processed = 0
  let errors = 0

  for (let i = 0; i < municipalities.length; i += CONCURRENCY) {
    const batch = municipalities.slice(i, i + CONCURRENCY)

    // Fetch all ZIPs in parallel
    const fetches = await Promise.allSettled(
      batch.map(async ({ url, code }) => {
        const res = await fetch(url, { signal: AbortSignal.timeout(30000) })
        if (!res.ok) return { code, buf: null }
        const buf = Buffer.from(await res.arrayBuffer())
        return { code, buf }
      })
    )

    // Parse sequentially (unzip is CPU-bound)
    for (const r of fetches) {
      if (r.status !== 'fulfilled' || !r.value.buf) { errors++; continue }
      const { code, buf } = r.value
      if (buf.length < 100 || buf[0] !== 0x50 || buf[1] !== 0x4b) { errors++; continue }

      try {
        // Skip very large municipalities (Praha, Brno) — XML > 500 MB causes OOM
        if (buf.length > 20 * 1024 * 1024) {
          if (errors < 5) console.log(`    SKIP ${code}: ZIP ${(buf.length/1e6).toFixed(0)} MB (too large)`)
          continue
        }
        const tmpZip = `/tmp/ruian_${code}.zip`
        writeFileSync(tmpZip, buf)
        exec(`unzip -o -q "${tmpZip}" -d /tmp/ruian_extract_${code}`, { timeout: 30000 })
        const xmlFiles = readdirSync(`/tmp/ruian_extract_${code}`).filter(f => f.endsWith('.xml'))
        for (const xf of xmlFiles) {
          const xml = readFileSync(`/tmp/ruian_extract_${code}/${xf}`, 'utf-8')
          const buildings = parseBuildingsFromVfr(xml)
          allBuildings.push(...buildings)
        }
        exec(`rm -rf /tmp/ruian_extract_${code} "${tmpZip}"`, { timeout: 5000 })
      } catch (e: any) {
        if (errors < 3) console.log(`    ERROR ${code}: ${e.message?.substring(0, 100)}`)
        errors++
        try { exec(`rm -rf /tmp/ruian_extract_${code} /tmp/ruian_${code}.zip`, { timeout: 5000 }) } catch {}
      }
    }

    processed += batch.length
    if (processed % 500 === 0 || processed === municipalities.length) {
      console.log(`    ${processed}/${municipalities.length} municipalities, ${allBuildings.length.toLocaleString()} buildings, ${errors} errors`)
    }
  }

  console.log(`  Total: ${allBuildings.length} buildings from RÚIAN`)
  writeFileSync(CACHE_FILE, JSON.stringify(allBuildings))
  return allBuildings
}

function parseBuildingsFromVfr(xml: string): RuianBuilding[] {
  const buildings: RuianBuilding[] = []
  // Match each StavebniObjekt block
  const soRegex = /<vf:StavebniObjekt[^>]*>([\s\S]*?)<\/vf:StavebniObjekt>/g
  let m
  while ((m = soRegex.exec(xml)) !== null) {
    const block = m[1]

    const floorsMatch = block.match(/<soi:PocetPodlazi>(\d+)</)
    const useMatch = block.match(/<soi:ZpusobVyuzitiKod>(\d+)</)
    const posMatch = block.match(/<gml:pos>([^<]+)</)

    if (!posMatch) continue

    const [yStr, xStr] = posMatch[1].trim().split(/\s+/)
    const [lat, lon] = sjtskToWgs84(parseFloat(yStr), parseFloat(xStr))

    if (lat < 48 || lat > 52 || lon < 11 || lon > 19) continue // outside CZ

    buildings.push({
      lat, lon,
      floors: floorsMatch ? parseInt(floorsMatch[1]) : 0,
      useCode: useMatch ? parseInt(useMatch[1]) : 0,
    })
  }
  return buildings
}

// ── Step 2: Enrich buildings.arrow ──

// Generous box around Czechia (+~0.3 deg halo) so the hex scan skips the rest
// of the planet — same bound as enrich-roads-cz.ts. [minLat,minLon,maxLat,maxLon]
const CZ_HEX_BBOX: [number, number, number, number] = [48.2, 11.7, 51.4, 19.2]

async function enrichHexes(ruianBuildings: RuianBuilding[]): Promise<void> {
  // Build spatial index: 0.01° grid (~1km cells) for fast lookup
  const grid = new Map<string, RuianBuilding[]>()
  for (const b of ruianBuildings) {
    const key = `${Math.floor(b.lat * 100)}_${Math.floor(b.lon * 100)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(b)
  }
  console.log(`  Spatial grid: ${grid.size} cells`)

  const hexDirs = iterateCountryHexes(OSM_EXTRACT_DIR, CZ_HEX_BBOX, 'buildings.arrow')

  let totalBuildings = 0, totalEnriched = 0, floorsAdded = 0, typeRefined = 0, hexesUpdated = 0, typeDowngradesBlocked = 0

  for (const hexId of hexDirs) {
    // The shared writer owns metadata preservation (v2 `buildings_contract`
    // stamp survives), the priority gate and the type-specificity gate.
    const r = await writeBuildingEnrichment(
      resolve(OSM_EXTRACT_DIR, hexId, 'buildings.arrow'),
      (row) => {
        // Fast-exit before the spatial match when a higher-priority dataset
        // owns the row (the writer re-checks the gate — this only saves work).
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null

        // Find nearest RÚIAN building within 30m (3x3 grid cells)
        let bestDist = 30
        let bestRuian: RuianBuilding | null = null
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const k = `${Math.floor(row.lat * 100) + dy}_${Math.floor(row.lon * 100) + dx}`
            const cell = grid.get(k)
            if (!cell) continue
            for (const rb of cell) {
              const d = flatDist(row.lat, row.lon, rb.lat, rb.lon)
              if (d < bestDist) { bestDist = d; bestRuian = rb }
            }
          }
        }
        if (!bestRuian) return null

        const mappedType = bestRuian.useCode > 0 ? RUIAN_TO_BUILDING_TYPE[bestRuian.useCode] : undefined
        return {
          // Fill floors only if missing in OSM
          floors: row.floors === 0 && bestRuian.floors > 0 ? Math.min(bestRuian.floors, 255) : undefined,
          // Refine building type from RÚIAN (coarse 0-9 — the writer keeps
          // the more specific v2 POI-join classes 10-13 untouched)
          buildingType: mappedType,
          sourceId: MY_SOURCE_ID,
        }
      },
      (row, _i, applied) => {
        if (applied.floors !== undefined) floorsAdded++
        if (applied.buildingType !== undefined && applied.buildingType !== row.buildingType) typeRefined++
      },
    )
    totalBuildings += r.rows
    totalEnriched += r.matched
    typeDowngradesBlocked += r.typeDowngradesBlocked
    if (r.updated) hexesUpdated++
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${totalEnriched} / ${totalBuildings} buildings matched to RÚIAN (${(totalEnriched / totalBuildings * 100).toFixed(1)}%)`)
  console.log(`  ${floorsAdded} floors added (were missing in OSM)`)
  console.log(`  ${typeRefined} building types refined from RÚIAN`)
  console.log(`  ${typeDowngradesBlocked} coarse-over-specific type downgrades blocked (v2 POI-join classes kept)`)
  console.log(`  ${hexesUpdated} / ${hexDirs.length} hexes updated`)
}

// ── Main ──

async function main() {
  console.log(`=== CZ Building Enrichment — RÚIAN (${YEAR}) ===\n`)
  console.log(`  OSM extract dir: ${OSM_EXTRACT_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  // Missing OR empty: iterateCountryHexes returns [] for both, so without this
  // the run would print "0 hexes, done" over a world that has buildings.
  requireOsmExtractTree()

  const buildings = await downloadRuian()
  console.log(`\n  RÚIAN buildings: ${buildings.length.toLocaleString()}`)
  console.log(`  Enriching buildings.arrow files...`)
  await enrichHexes(buildings)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

/**
 * Enrich FJ industrial with GEM Global Integrated Power (Fiji filter).
 *
 * All Fiji government portals (FEA, Fiji Electricity Authority) publish
 * corporate HTML only. GEM is the only machine-readable source.
 *
 * Source:
 *   - **GEM Global Integrated Power v1** (Country_area='Fiji'):
 *     2 operating plants, ~90 MW total
 *
 *   Top operating plants:
 *     **Wailoa hydro 80 MW** (hydro — Wailoa River, Viti Levu; -17.74, 178.10)
 *     **Butoni wind farm 10 MW** (wind — Butoni, Viti Levu; -18.11, 177.51)
 *
 * Non-power industrial (OSM only):
 *   - **Sugar**: Fiji Sugar Corporation (FSC) — 4 mills: Lautoka, Ba, Rarawai, Labasa
 *   - **Gold**: Vatukoula/Emperor Gold Mine — one of Pacific's oldest gold mines, intermittent
 *   - **Fiji Water** (Yaqara) — bottled artesian water, major global export brand
 *   - **PAFCO** (Levuka) — tuna processing, Pacific Fishing Company
 *   - **Tourism** — #1 foreign exchange earner (Denarau, Mamanuca, Yasawa Islands)
 *   - **Cement**: Fiji Cement
 *   - **Brewery**: South Pacific Distilleries (Fiji Gold/Fiji Bitter)
 *
 * Antimeridian note:
 *   Fiji straddles ~180° longitude. Main islands (Viti Levu, Vanua Levu) lie
 *   177–179°E; eastern islands (Lau Group, Rotuma area) extend past 180° and
 *   appear as negative longitudes in WGS-84. Two bbox halves are used:
 *     FJ_BBOX_WEST: [-21.0, 176.0, -12.0, 180.0]
 *     FJ_BBOX_EAST: [-21.0, -180.0, -12.0, -177.0]
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-fj.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { DEFAULT_FUEL_TO_NACE, NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/fj`)

// Fiji straddles the antimeridian — two bbox halves:
//   WEST: main islands (Viti Levu, Vanua Levu, Taveuni)
//   EAST: eastern islands (Lau Group) that cross 180° and wrap to negative lon
// Both halves share the same lat range [-21.0, -12.0].
function inFiji(lat: number, lon: number): boolean {
  if (lat < -21.0 || lat > -12.0) return false
  return (lon >= 176.0 && lon <= 180.0) || (lon >= -180.0 && lon <= -177.0)
}

interface IndSite { lat: number; lon: number; name: string; fuel: string }

function loadGemPlants(): { sites: IndSite[]; parsedInArea: number } {
  const path = resolve(CACHE_DIR, 'power-plants-gem.geojson')
  if (!existsSync(path)) return { sites: [], parsedInArea: 0 }
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  let parsedInArea = 0 // pre-status count — feeds datasetNonEmpty (retired-only FJ must still sweep)
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inFiji(lat, lon)) continue
    parsedInArea++
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || 'FJ plant').toString(),
      fuel: (p.Type || 'unknown').toString().toLowerCase(),
    })
  }
  return { sites: out, parsedInArea }
}

async function main() {
  console.log(`=== FJ Industrial Enrichment — GEM Global Integrated Power (${YEAR}) ===\n`)

  const { sites: plants, parsedInArea } = loadGemPlants()
  const fuelCounts: Record<string, number> = {}
  for (const p of plants) fuelCounts[p.fuel] = (fuelCounts[p.fuel] || 0) + 1
  console.log(`  GEM operating plants in FJ: ${plants.length}`)
  for (const [f, c] of Object.entries(fuelCounts).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${f.padEnd(15)} ${c}`)
  }

  const facilities: MatchFacility[] = []
  for (const p of plants) {
    const nace4 = DEFAULT_FUEL_TO_NACE(p.fuel) // wind/blank → null → skip
    if (nace4 == null) continue
    facilities.push({ lat: p.lat, lon: p.lon, nace4, ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inFiji,
    searchRadiusM: 2000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('FJ'),
    datasetNonEmpty: parsedInArea > 0, // pre-status parse count from the loader
    label: 'FJ',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

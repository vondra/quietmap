/**
 * Enrich ID industrial with GEM Global Integrated Power (Indonesia subset).
 *
 * Source: `services.arcgis.com/lqRTrQp2HrfnJt8U/arcgis/rest/services/
 * Global_Integrated_Power_August_2025/FeatureServer/0?where=Country_area='Indonesia'`
 * Records: 974 Indonesian power plants (491 operating)
 *
 * Fuel mix: Coal (Paiton, Suralaya, Tanjung Jati B), Gas (Grati, Priok, Muara
 * Karang, Gilimanuk), Hydro (Cirata, Saguling, Jatiluhur), **Geothermal**
 * (Kamojang, Darajat, Salak, Wayang Windu, Ulubelu — Indonesia has 2nd-largest
 * geothermal installed capacity globally), Solar, Wind (minimal).
 *
 * All map to NACE 35 (Electricity generation).
 *
 * Status filter: only 'operating' (skip cancelled/announced/construction/
 * pre-construction/retired).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-id.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { DEFAULT_FUEL_TO_NACE, NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/id`)

const ID_BBOX: [number, number, number, number] = [-11.5, 94.0, 6.5, 141.5]
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  [1.0, 99.5, 6.5, 104.5],     // Malaysia peninsular
  [0.9, 109.5, 7.5, 119.5],    // Malaysia Sabah/Sarawak
  [1.1, 103.5, 1.6, 104.2],    // Singapore
  [4.0, 114.0, 5.1, 115.4],    // Brunei
  [4.5, 116.9, 20.0, 127.0],   // Philippines
  [-9.5, 124.0, -8.1, 127.3],  // Timor-Leste
  [-11.0, 140.8, -1.0, 155.0], // PNG
]
function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inIndonesia(lat: number, lon: number): boolean {
  return inBbox(lat, lon, ID_BBOX) && !inExcluded(lat, lon)
}

interface IndSite {
  lat: number
  lon: number
  name: string
  fuel: string
}

function loadPowerPlants(): { sites: IndSite[]; parsedInArea: number } {
  const path = resolve(CACHE_DIR, 'power-plants.geojson')
  if (!existsSync(path)) return { sites: [], parsedInArea: 0 }
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  let parsedInArea = 0 // pre-status count — feeds datasetNonEmpty (a retired-only country must still sweep)
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, ID_BBOX) || inExcluded(lat, lon)) continue
    parsedInArea++
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (status && status !== 'operating') continue
    out.push({
      lat, lon,
      name: (p.Name || p.name || 'Indonesia power plant').toString(),
      fuel: (p.Fuel || p.Type || 'unknown').toString(),
    })
  }
  return { sites: out, parsedInArea }
}

async function main() {
  console.log(`=== ID Industrial Enrichment — GEM Global Integrated Power (${YEAR}) ===\n`)

  const { sites: plants, parsedInArea } = loadPowerPlants()
  console.log(`  Operating power plants: ${plants.length}`)

  const facilities: MatchFacility[] = []
  for (const p of plants) {
    const nace4 = DEFAULT_FUEL_TO_NACE(p.fuel) // wind/blank → null → skip
    if (nace4 == null) continue
    facilities.push({ lat: p.lat, lon: p.lon, nace4, ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inIndonesia,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-ID rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, ID_BBOX),
    searchRadiusM: 1500,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('ID'),
    datasetNonEmpty: parsedInArea > 0, // pre-status parse count from the loader
    label: 'ID',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

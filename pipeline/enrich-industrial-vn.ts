/**
 * Enrich VN industrial with GEM Global Integrated Power (Vietnam subset).
 *
 * Source: services.arcgis.com/lqRTrQp2HrfnJt8U/.../Global_Integrated_Power_
 * August_2025/FeatureServer/0?where=Country_area='Vietnam'
 *
 * 1,492 Vietnam plants total, **874 operating** (59% of the catalogue —
 * highest ratio of any country enriched so far, reflecting Vietnam's recent
 * solar/wind boom since 2019):
 *   - Solar: 683 (Ninh Thuan, Dak Lak, Long An, Binh Thuan — 20+ GW installed)
 *   - Wind: 398 (Bac Lieu, Soc Trang, Ca Mau offshore, Quang Tri)
 *   - Coal: 197 (Pha Lai, Quang Ninh, Vinh Tan, Duyen Hai, Mong Duong)
 *   - Gas: 115 (Phu My complex, Nhon Trach, Ca Mau)
 *   - Hydro: 86 (Son La 2.4 GW — SE Asia's largest, Lai Chau 1.2 GW, Hoa Binh 1.92 GW, Tuyen Quang, Yaly, Tri An)
 *   - Nuclear: 8 (never operational, cancelled projects)
 *   - Bioenergy: 5
 *
 * All map to NACE 35.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-vn.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { DEFAULT_FUEL_TO_NACE, NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/vn`)

// bbox stays as the cheap hex-shortlist; inVN (actual-polygon gate) is the real
// filter — the hand-tuned China/Laos/Cambodia EXCLUDE_ZONES bled into neighbours.
const VN_BBOX: [number, number, number, number] = [8.3, 102.1, 23.5, 109.5]

interface IndSite { lat: number; lon: number; name: string; fuel: string }

function loadPlants(inVN: (lat: number, lon: number) => boolean): { sites: IndSite[]; parsedInArea: number } {
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
    if (!inBbox(lat, lon, VN_BBOX) || !inVN(lat, lon)) continue
    parsedInArea++
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (status && status !== 'operating') continue
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || p.Name || 'VN power plant').toString(),
      fuel: (p.Type || p.Fuel || 'unknown').toString(),
    })
  }
  return { sites: out, parsedInArea }
}

async function main() {
  console.log(`=== VN Industrial Enrichment — GEM Global Integrated Power (${YEAR}) ===\n`)
  // Built here, not at module scope: the first call may download/convert CGAZ.
  const inVN = makeCountryGate('VN')
  const isInside = (lat: number, lon: number) => inBbox(lat, lon, VN_BBOX) && inVN(lat, lon)
  const { sites: plants, parsedInArea } = loadPlants(inVN)
  console.log(`  Operating power plants: ${plants.length}`)

  const facilities: MatchFacility[] = []
  for (const p of plants) {
    const nace4 = DEFAULT_FUEL_TO_NACE(p.fuel) // wind/blank → null → skip
    if (nace4 == null) continue
    facilities.push({ lat: p.lat, lon: p.lon, nace4, ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside,
    // hexGate: plain bbox — a border hex centred just outside the CGAZ polygon (coast/estuary) still holds in-VN rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, VN_BBOX),
    searchRadiusM: 1500,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: inVN,
    datasetNonEmpty: parsedInArea > 0, // pre-status parse count from the loader
    label: 'VN',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

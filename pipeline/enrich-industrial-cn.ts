/**
 * Enrich CN industrial with ces_ricegis (Rice University GIS / Global Energy
 * Monitor) datasets — the richest national power/industrial point registries
 * in our pipeline.
 *
 * Source: `services.arcgis.com/lqRTrQp2HrfnJt8U/arcgis/rest/services/`
 *
 * Datasets:
 *   China_coal_power_plants_vJan2024_3    → 6,078 coal unit-phases (GEM)
 *   China_Gas_Power_Plants_EH_v2024        → 252 gas plants
 *   China_Nuclear_Power_Plants_vSep2024    → 163 nuclear plants
 *   China_Wind_Power_Plants_GEM_202406     → 8,281 wind farms/turbines
 *   China_Solar_Power_Plants_GEM_202406    → 13,489 solar plants
 *   ChinaLNGTerminals                       → 82 LNG terminals
 *
 * Total: 28,345 geocoded facilities — the largest industrial point registry
 * of any country in our pipeline. WRI GPPD only has ~4,000 CN plants; this
 * dataset is 7× more complete.
 *
 * Status filter: only 'operating' (skip cancelled/construction/retired).
 *
 * NACE mapping (all map to NACE 35 — Electricity supply):
 *   Coal, Gas, Nuclear, Wind, Solar, LNG → NACE 35
 *
 * This gives tile-painter the correct industrial noise profile:
 *   - Coal plants: ~100 dB base emission (NACE 35 with heavy combustion)
 *   - Gas CCGT: ~95 dB
 *   - Nuclear: ~90 dB (mostly cooling towers)
 *   - Wind: per-turbine specs (hub height, rated power) — use IEC 61400-11 default
 *   - Solar PV: ~70 dB (inverters only — very low)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-cn.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cn`)

const CN_BBOX: [number, number, number, number] = [18.0, 73.0, 54.0, 135.5]
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  [18.0, 73.0, 29.0, 85.0],    // India south
  [24.0, 73.0, 37.5, 74.5],    // Pakistan west
  [26.5, 80.0, 30.5, 88.2],    // Nepal
  [26.7, 88.8, 28.3, 92.1],    // Bhutan
  [18.0, 92.0, 28.5, 100.5],   // Myanmar
  [13.0, 100.0, 22.5, 107.8],  // Laos
  [8.0, 102.0, 23.5, 110.0],   // Vietnam
  [5.0, 97.0, 21.0, 106.0],    // Thailand
  [36.0, 73.0, 50.0, 81.0],    // Central Asia
  [42.0, 88.0, 54.0, 120.0],   // Mongolia
  [37.5, 124.0, 43.0, 131.0],  // North Korea
  [33.0, 126.0, 38.5, 130.0],  // South Korea
  [50.0, 115.0, 54.0, 135.5],  // Russia Far East
  [24.0, 129.0, 54.0, 135.5],  // Japan
  [21.8, 119.5, 25.5, 122.1],  // Taiwan
]
function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inChina(lat: number, lon: number): boolean {
  return inBbox(lat, lon, CN_BBOX) && !inExcluded(lat, lon)
}

interface IndSite {
  lat: number
  lon: number
  name: string
  source: string
  status: string
}

function loadPoints(path: string, source: string): IndSite[] {
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, CN_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || p.status || '').toString().toLowerCase()
    // Filter: only operating
    if (status && !['operating', '运营中', 'in operation'].some(s => status.includes(s))) continue
    out.push({
      lat, lon,
      name: p.Plant_name || p.plant_name || p.Name || p.name || p.Unit_name || 'unknown',
      source,
      status,
    })
  }
  return out
}

async function main() {
  console.log(`=== CN Industrial Enrichment — ces_ricegis / GEM datasets (${YEAR}) ===\n`)

  const coal = loadPoints(resolve(CACHE_DIR, 'coal-plants.geojson'), 'China coal plants (GEM Jan 2024)')
  const gas = loadPoints(resolve(CACHE_DIR, 'gas-plants.geojson'), 'China gas plants (GEM 2024)')
  const nuclear = loadPoints(resolve(CACHE_DIR, 'nuclear-plants.geojson'), 'China nuclear plants (Sep 2024)')
  const solar = loadPoints(resolve(CACHE_DIR, 'solar-plants.geojson'), 'China solar plants (GEM Jun 2024)')
  const lng = loadPoints(resolve(CACHE_DIR, 'lng-terminals.geojson'), 'China LNG terminals')

  console.log(`  Coal plants (operating):   ${coal.length}`)
  console.log(`  Gas plants:                ${gas.length}`)
  console.log(`  Nuclear plants:            ${nuclear.length}`)
  console.log(`  Solar plants:              ${solar.length}`)
  console.log(`  LNG terminals:             ${lng.length}`)
  const total = coal.length + gas.length + nuclear.length + solar.length + lng.length
  console.log(`  Total sites:               ${total}`)

  // The core's empty-guard only covers ALL-empty: a partially-loaded run would
  // reset area stamps it cannot re-stamp (/gg Codex) — any empty source is fatal.
  const sourceFeatureCounts: Array<[string, number]> = [
    ['coal-plants.geojson', coal.length],
    ['gas-plants.geojson', gas.length],
    ['nuclear-plants.geojson', nuclear.length],
    ['solar-plants.geojson', solar.length],
    ['lng-terminals.geojson', lng.length],
  ]
  for (const [file, count] of sourceFeatureCounts) {
    if (count === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }

  // NACE is classified by SOURCE FILE bucket — a deliberate CN exception (these
  // per-fuel registries carry no usable per-row fuel field): solar → 3599,
  // everything else (coal/gas/nuclear/LNG — all thermal, no hydro geojson
  // exists for CN) → 3511. Wind never stamps (modelled as source_type=10), so
  // wind-plants.geojson is skipped entirely at load.
  const facilities: MatchFacility[] = []
  for (const s of [...coal, ...gas, ...nuclear, ...lng]) {
    facilities.push({ lat: s.lat, lon: s.lon, nace4: 3511, ...NATIONAL_MIX })
  }
  for (const s of solar) {
    facilities.push({ lat: s.lat, lon: s.lon, nace4: 3599, ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inChina,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-CN rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, CN_BBOX),
    searchRadiusM: 1500,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('CN'),
    datasetNonEmpty: facilities.length > 0, // = sum of per-fuel registries (loaders keep operating plants only; the file-level fatal guard aborts on any empty registry, so an all-retired CN reads as abort, not sweep — accepted)
    label: 'CN',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

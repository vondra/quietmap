/**
 * Enrich BR industrial with SIGACONTROL ArcGIS Online layers (ANEEL-derived).
 *
 * Source: services5.arcgis.com/qaWxR4XTuVOZEXZ9/arcgis/rest/services/
 *
 * Layers downloaded:
 *   - Aerogeradores_Brasil: **11,182 individual wind turbines** with POT_MW,
 *     ALT_TOTAL, ALT_ROTOR (hub height), DIAM_ROTOR — the richest wind turbine
 *     dataset in our entire pipeline
 *   - UsinaTermoeletrica: 3,226 thermal plants (coal/gas/oil/biomass)
 *   - Aproveitamento_Hidroletrico: 1,138 hydro plants (including Itaipu,
 *     Belo Monte, Tucuruí, Furnas, Itumbiara)
 *   - UsinaFotovoltaica: 322 solar PV plants
 *   - UsinaTermonuclear: 3 nuclear plants (Angra I, II, III)
 *
 * All map to NACE 35 (Electricity generation).
 *
 * Note: status filter — prefer 'OPERACAO=SIM' for operational turbines and
 * 'Operação' for other plant types. GEM-style status field is not in SIGA
 * data; use OPERACAO field instead.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-br.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/br`)

const BR_BBOX: [number, number, number, number] = [-34.0, -74.0, 5.3, -34.8]
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  [-56.0, -74.0, -22.0, -53.7],
  [-35.0, -58.5, -30.1, -53.1],
  [-27.7, -62.7, -19.3, -54.2],
  [-23.0, -69.7, -9.7, -57.5],
  [-18.4, -82.0, -0.0, -68.5],
  [-4.2, -79.1, 13.0, -66.8],
  [0.6, -73.4, 13.0, -59.8],
  [1.2, -61.4, 8.7, -56.5],
  [1.8, -58.1, 6.1, -53.9],
  [2.1, -54.6, 5.8, -51.6],
  [-56.0, -76.0, -17.5, -66.4],
  [-5.0, -81.0, 1.5, -75.2],
]
function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inBrazil(lat: number, lon: number): boolean {
  return inBbox(lat, lon, BR_BBOX) && !inExcluded(lat, lon)
}

interface IndSite { lat: number; lon: number; name: string; fuel: string }

/**
 * Map a plant fuel/type to a NACE 4-digit code for industrial noise stamping.
 *
 * Returns `null` for sources that must NOT be stamped as industrial:
 *  - wind turbines are modelled separately as `source_type=10`, so the OSM row
 *    must keep its existing (empty) NACE/source_id — never inherit a plant code.
 *  - blank/unknown fuel has no defensible emission class.
 *
 * Engine NACE→emission map: 3512 hydro (90 dB), 3511 thermal/nuclear (97 dB),
 * 3599 solar (55 dB).
 */
function fuelToNace4(fuel: string): number | null {
  const f = fuel.toLowerCase()
  if (!f) return null
  if (f.includes('wind') || f.includes('aerogerador') || f.includes('eolic') || f.includes('eólic')) return null
  if (f.includes('hydro') || f.includes('hidro')) return 3512
  if (f.includes('solar') || f.includes('fotovolt')) return 3599
  if (f.includes('thermal') || f.includes('termo') || f.includes('fossil') || f.includes('nuclear') || f.includes('nucle')) return 3511
  return null
}

function loadPoints(path: string, fuel: string, nameField: string, statusField: string, statusOK: string[]): IndSite[] {
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || (g.type !== 'Point' && g.type !== 'MultiPoint')) continue
    const coords = g.type === 'Point' ? g.coordinates : g.coordinates[0]
    if (!coords) continue
    const [lon, lat] = coords
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, BR_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p[statusField] || '').toString().toLowerCase()
    if (status && !statusOK.some(s => status.includes(s))) continue
    out.push({
      lat, lon,
      name: (p[nameField] || p.NOME || 'BR plant').toString(),
      fuel,
    })
  }
  return out
}

async function main() {
  console.log(`=== BR Industrial Enrichment — SIGACONTROL ANEEL layers (${YEAR}) ===\n`)

  const wind = loadPoints(
    resolve(CACHE_DIR, 'wind-turbines.geojson'),
    'wind-turbine',
    'NOME_EOL', 'OPERACAO', ['sim', 'true', 'operacional']
  )
  const thermal = loadPoints(
    resolve(CACHE_DIR, 'thermal-plants.geojson'),
    'thermal', 'NOME', 'ESTAGIO', ['opera', 'sim']
  )
  const hydro = loadPoints(
    resolve(CACHE_DIR, 'hydro-plants.geojson'),
    'hydro', 'NOME', 'ESTAGIO', ['opera', 'sim']
  )
  const solar = loadPoints(
    resolve(CACHE_DIR, 'solar-plants.geojson'),
    'solar', 'NOME', 'ESTAGIO', ['opera', 'sim']
  )
  const nuclear = loadPoints(
    resolve(CACHE_DIR, 'nuclear-plants.geojson'),
    'nuclear', 'NOME', 'ESTAGIO', ['opera', 'sim']
  )

  console.log(`  Wind turbines (operating): ${wind.length}`)
  console.log(`  Thermal plants:            ${thermal.length}`)
  console.log(`  Hydro plants:              ${hydro.length}`)
  console.log(`  Solar plants:              ${solar.length}`)
  console.log(`  Nuclear plants:            ${nuclear.length}`)

  // The core's empty-guard only covers ALL-empty: a partially-loaded run would
  // reset area stamps it cannot re-stamp (/gg Codex) — any empty source is fatal.
  // (wind excluded: turbines never stamp — fuelToNace4 returns null for them.)
  const sourceFeatureCounts: Array<[string, number]> = [
    ['thermal-plants.geojson', thermal.length],
    ['hydro-plants.geojson', hydro.length],
    ['solar-plants.geojson', solar.length],
    ['nuclear-plants.geojson', nuclear.length],
  ]
  for (const [file, count] of sourceFeatureCounts) {
    if (count === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }

  // Priority: thermal > hydro > nuclear > wind > solar (loudest first)
  const allSites: IndSite[] = [...thermal, ...hydro, ...nuclear, ...wind, ...solar]

  const facilities: MatchFacility[] = []
  for (const s of allSites) {
    const nace4 = fuelToNace4(s.fuel) // wind / blank-fuel → null → never stamps
    if (nace4 == null) continue
    facilities.push({ lat: s.lat, lon: s.lon, nace4, ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inBrazil,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-BR rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, BR_BBOX),
    searchRadiusM: 2000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('BR'),
    datasetNonEmpty: allSites.length > 0, // includes wind — parsed pre-filter
    label: 'BR',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

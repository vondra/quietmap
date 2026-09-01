/**
 * Enrich BO industrial with MHE (official) + GEM (backfill).
 *
 * Sources:
 *   - **MHE GeoServer** (Ministerio de Hidrocarburos y Energía):
 *       geoportal.mhe.gob.bo/geoserver/ows
 *     - gen_sin_20260304: 47 SIN grid plants with Tipo/Area/Central/Propietari/Pn
 *     - Gen_Ais_2026: 35 isolated system plants (off-grid)
 *     - Subestaciones_SIN_MAR_20260: 230 substations
 *     - transmision_sin_20260304: 291 HV transmission lines
 *
 *     Tipo codes:
 *       HE = Hidroeléctrica (hydro)
 *       TG = Turbina gas (thermal/gas turbine)
 *       BM = Biomasa
 *       EO = Eólica (wind)
 *       SL = Solar
 *       DO = Diesel
 *
 *     Top operating plants:
 *       Warnes 556 MW TG (Santa Cruz, ENDE ANDINA)
 *       Central del Sur 516 MW TG
 *       Entre Ríos 505 MW TG (Cochabamba, ENDE ANDINA)
 *       Guaracachi 411 MW TG (Santa Cruz, ENDE GUARACACHI)
 *       Carrasco 159 MW TG (Cochabamba, VHE)
 *       Misicuni 126 MW HE (Cochabamba hydro)
 *       Solar Oruro 104 MW SL (Altiplano solar)
 *
 *   - **GEM Global Integrated Power v1** (Country_area='Bolivia'):
 *       66 features, 37 operating — fallback for plants not in MHE grid layer
 *
 *   - Substations ≥69 kV (MHE) also tagged as NACE 35
 *
 * Mining: AJAM (Autoridad Jurisdiccional Administrativa Minera) does not
 * publish a public REST endpoint. Major mines (San Cristóbal, Huanuni,
 * Colquiri, Vinto smelter, San Bartolomé Cerro Rico, Mutún iron) rely on
 * OSM + generic defaults.
 *
 * YPFB hydrocarbons: YPFB Portal GIS metadata is accessible but query
 * endpoints return HTTP 400 (backend DB unavailable). Gas fields (Camiri,
 * Margarita, Incahuasi, Vuelta Grande) and refineries (Gualberto Villarroel
 * Cochabamba, Guillermo Elder Bell Santa Cruz) rely on OSM.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-bo.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/bo`)

// Bolivia bbox
const BO_BBOX: [number, number, number, number] = [-22.9, -69.7, -9.5, -57.5]

const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  // Brazil (N, NE, E)
  [-16.0, -61.0, -9.5, -57.5],
  // Paraguay (SE)
  [-22.9, -62.7, -19.0, -57.5],
  // Argentina (S)
  [-22.9, -67.5, -21.7, -60.0],
  // Chile (SW)
  [-22.9, -69.7, -17.5, -68.0],
  // Peru (W)
  [-18.4, -69.7, -9.5, -68.5],
]

function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inBolivia(lat: number, lon: number): boolean {
  return inBbox(lat, lon, BO_BBOX) && !inExcluded(lat, lon)
}

interface IndSite {
  lat: number; lon: number; name: string; fuel: string; source: string
}

const MHE_TIPO_TO_FUEL: Record<string, string> = {
  HE: 'hydropower',
  TG: 'oil/gas',
  BM: 'bioenergy',
  EO: 'wind',
  SL: 'solar',
  DO: 'diesel',
}

/**
 * Map a plant fuel string to its 4-digit NACE division-35 code for the
 * industrial noise engine, or 0 = do NOT stamp.
 *   3512 hydro (90 dB) · 3511 thermal/combustion (97 dB) · 3599 solar (55 dB)
 * Wind is source_type=10 (modelled separately, never industrial) → skip.
 * Blank/unknown fuel → skip (avoid mis-tagging an OSM site as 97 dB thermal).
 */
function fuelToNace4(fuel: string): number {
  const f = fuel.toLowerCase()
  if (f.includes('wind') || f.includes('eolic') || f.includes('eólic')) return 0
  if (f.includes('hydro')) return 3512
  if (f.includes('solar') || f.includes('photovolt') || f.includes('fotovolt')) return 3599
  if (!f || f.includes('unknown')) return 0
  // everything combustion-based: gas, oil, diesel, coal, biomass/bioenergy,
  // geothermal, plus transmission substations (division-35 infrastructure)
  return 3511
}

function loadMheGen(file: string, source: string): IndSite[] {
  const path = resolve(CACHE_DIR, file)
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, BO_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const tipo = (p.Tipo || '').toString().toUpperCase()
    const fuel = MHE_TIPO_TO_FUEL[tipo] || 'unknown'
    out.push({
      lat, lon,
      name: (p.Central || 'BO plant').toString(),
      fuel,
      source,
    })
  }
  return out
}

function loadMheSubstations(): IndSite[] {
  const path = resolve(CACHE_DIR, 'power-substations.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, BO_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    // MHE substations list includes distribution; we only care about transmission (≥69 kV)
    const tens = p.Tension ?? p.tension ?? 0
    const tv = typeof tens === 'number' ? tens : parseInt(String(tens), 10) || 0
    if (tv > 0 && tv < 69) continue  // skip low-voltage distribution
    out.push({
      lat, lon,
      name: (p.Nombre || p.NOMBRE || p.nombre || 'BO substation').toString(),
      fuel: 'transmission',
      source: 'MHE substation',
    })
  }
  return out
}

function loadGemPlants(): IndSite[] {
  const path = resolve(CACHE_DIR, 'power-plants-gem.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, BO_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    const fuel = (p.Type || 'unknown').toString().toLowerCase()
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || 'BO plant').toString(),
      fuel,
      source: 'GEM BO',
    })
  }
  return out
}

async function main() {
  console.log(`=== BO Industrial Enrichment — MHE + GEM (${YEAR}) ===\n`)

  const mheSin = loadMheGen('power-gen-sin.geojson', 'MHE SIN')
  console.log(`  MHE SIN grid plants:       ${mheSin.length}`)

  const mheAis = loadMheGen('power-gen-ais.geojson', 'MHE AIS')
  console.log(`  MHE isolated system plants: ${mheAis.length}`)

  const mheSub = loadMheSubstations()
  console.log(`  MHE substations (≥69 kV):  ${mheSub.length}`)

  const gem = loadGemPlants()
  console.log(`  GEM operating plants:      ${gem.length}`)

  // The core's empty-guard only covers ALL-empty: a partially-loaded run would
  // reset area stamps it cannot re-stamp (/gg Codex) — any empty source is fatal.
  const sourceFeatureCounts: Array<[string, number]> = [
    ['power-gen-sin.geojson', mheSin.length],
    ['power-gen-ais.geojson', mheAis.length],
    ['power-substations.geojson', mheSub.length],
    ['power-plants-gem.geojson', gem.length],
  ]
  for (const [file, count] of sourceFeatureCounts) {
    if (count === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }

  // Dedup by coordinate (MHE and GEM may overlap)
  const seen = new Set<string>()
  const allSites: IndSite[] = []
  for (const s of [...mheSin, ...mheAis, ...gem, ...mheSub]) {
    const key = `${s.lat.toFixed(3)}_${s.lon.toFixed(3)}`
    if (seen.has(key)) continue
    seen.add(key)
    allSites.push(s)
  }
  console.log(`  Total unique sites:        ${allSites.length}`)

  const facilities: MatchFacility[] = []
  for (const s of allSites) {
    const nace4 = fuelToNace4(s.fuel)
    if (nace4 === 0) continue  // wind (source_type=10) or blank/unknown → don't stamp
    facilities.push({ lat: s.lat, lon: s.lon, nace4, ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inBolivia,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-BO rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, BO_BBOX),
    searchRadiusM: 2000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('BO'),
    datasetNonEmpty: allSites.length > 0, // deduped union of all BO sources, pre-fuel-filter
    label: 'BO',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

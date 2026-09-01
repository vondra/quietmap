/**
 * Enrich ZA industrial with ESKOM (UP mirror) + GEM + GEM Coal Mines 2024.
 *
 * Sources:
 *   - **ESKOM Power Stations** (University of Pretoria mirror,
 *     owner `christel.hansen_uparcgis`):
 *       services8.arcgis.com/ZhTpwEGNVUBxG9VW/.../Power_Stations/FeatureServer/0
 *     33 stations with NAME, CATEGORY, LOAD_MW
 *     Includes: Medupi 4,788 MW, Kusile 4,500 MW, Kendal 4,116 MW,
 *     Majuba 4,110 MW, Matimba 3,990 MW, Lethabo 3,708 MW, Tutuka 3,654 MW,
 *     Duvha 3,600 MW, Matla 3,600 MW, Kriel 3,000 MW, Arnot 2,100 MW,
 *     Hendrina 2,000 MW, **Koeberg nuclear 1,800 MW (Africa's only
 *     operating nuclear plant)**, Camden 1,600 MW, Ingula pumped storage
 *     1,352 MW, plus hydro/wind/peaking gas/CSP
 *     CATEGORY: BASELOAD COAL 11, PEAKING HYDRO 7, STANDBY COAL 3,
 *     PEAKING GAS 2, GAS TURBINE 2, PEAKING PUMP STORAGE 2, WIND 2,
 *     COAL 1, PUMPED STORAGE 1, BASELOAD NUCLEAR 1, CSP 1
 *
 *   - **GEM Global Integrated Power August 2025** (Country_area='South Africa'):
 *     502 total, 314 operating
 *     Fuel (operating): solar 151, coal 90, wind 38, oil 20, hydro 7, gas 6, nuclear 2
 *     Includes all Eskom coal plants + Jeffreys Bay/Cookhouse/Hopefield
 *     wind + Copperton/Garob/Oyster Bay/Nxuba/Nojoli/Gibson Bay wind farms
 *     + Kathu/Ilanga/Karoshoek CSP + many PV plants (REIPPPP program)
 *
 *   - **GEM Global Coal Mines 2024** (Country='South Africa'):
 *     services7.arcgis.com/IyvyFk20mB7Wpc95/.../Global_Coal_Mines_2_view
 *     137 coal mines (79 Operating, 36 Proposed, 13 Mothballed, 6 Cancelled)
 *     Fields: Mine_Name, Status, Mine_Type (Surface/Underground/Both),
 *             State__Province (mostly Mpumalanga Highveld Coalfield +
 *             Limpopo Waterberg Coalfield)
 *     → NACE 05 (Mining of coal and lignite)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-za.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/za`)

// South Africa bbox (excluding Marion Island and the Prince Edward Islands)
const ZA_BBOX: [number, number, number, number] = [-35.0, 16.3, -22.0, 33.0]

// Exclusion zones for neighbouring countries (most are fully enclosed by ZA)
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  // Lesotho (fully enclosed, ~30,355 km²)
  [-30.7, 27.0, -28.5, 29.5],
  // Eswatini / Swaziland (enclosed on 3 sides)
  [-27.3, 30.8, -25.7, 32.2],
  // Namibia (W)
  [-29.0, 16.3, -22.0, 20.0],
  // Botswana (N)
  [-27.0, 20.0, -22.0, 29.4],
  // Zimbabwe (N)
  [-22.4, 25.2, -22.0, 33.0],
  // Mozambique (E)
  [-27.0, 31.9, -22.0, 33.0],
]

function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inSouthAfrica(lat: number, lon: number): boolean {
  return inBbox(lat, lon, ZA_BBOX) && !inExcluded(lat, lon)
}

interface IndSite {
  lat: number; lon: number; name: string; nace: string; source: string
}

/**
 * Map a power-plant fuel to its CNOSSOS industrial NACE, or null to SKIP.
 *
 * Engine NACE→noise: 3599 solar 55 dB, 3512 hydro 90 dB, 3511 thermal 97 dB.
 * Wind is modelled as source_type=10 (separate from industrial NACE) → SKIP.
 * Blank/unknown fuel → SKIP rather than mislabel a plant 97 dB thermal.
 */
function fuelToNace(fuel: string): string | null {
  if (/wind/.test(fuel)) return null            // modelled as source_type=10, not a NACE
  if (/hydro|pump/.test(fuel)) return '351200'  // 90 dB
  if (/solar|csp|photovolt|pv/.test(fuel)) return '359900'  // 55 dB
  if (/coal|nuclear|gas|oil|biomass|bioenergy|thermal|fossil|diesel|peat/.test(fuel)) return '351100'  // 97 dB
  return null  // blank/unknown → skip
}

/** NACE values may be 2-digit ('05') or 6-digit ('351100'); pad to 6-digit
 *  then truncate to the arrow uint16 ('05' → 500, '351100' → 3511) — same
 *  arithmetic the old carpet loop used. */
function naceStringToUint16(nace: string): number {
  const nace6Raw = nace.length < 6 ? (nace + '0000').substring(0, 6) : nace
  return Math.floor((parseInt(nace6Raw, 10) || 0) / 100)
}

function loadEskomPlants(): IndSite[] {
  const path = resolve(CACHE_DIR, 'power-plants-eskom.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, ZA_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const cat = (p.CATEGORY || '').toString().toLowerCase()
    let fuel = 'coal'
    if (/nuclear/.test(cat)) fuel = 'nuclear'
    else if (/hydro|pump/.test(cat)) fuel = 'hydropower'
    else if (/gas/.test(cat)) fuel = 'oil/gas'
    else if (/wind/.test(cat)) fuel = 'wind'
    else if (/csp|solar/.test(cat)) fuel = 'solar'
    const nace = fuelToNace(fuel)
    if (nace === null) continue  // wind (source_type=10) or unknown → skip
    out.push({
      lat, lon,
      name: (p.NAME || 'ZA Eskom plant').toString(),
      nace,
      source: `Eskom UP (${fuel})`,
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
    if (!inBbox(lat, lon, ZA_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    const fuel = (p.Type || p.Fuel || 'unknown').toString().toLowerCase()
    const nace = fuelToNace(fuel)
    if (nace === null) continue  // wind (source_type=10) or unknown → skip
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || 'ZA plant').toString(),
      nace,
      source: `GEM ZA (${fuel})`,
    })
  }
  return out
}

function loadCoalMines(): IndSite[] {
  const path = resolve(CACHE_DIR, 'coal-mines-gem.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, ZA_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString()
    if (status !== 'Operating') continue
    const mtype = (p.Mine_Type || '').toString()
    const prov = (p.State__Province || '').toString()
    out.push({
      lat, lon,
      name: `${p.Mine_Name || 'ZA coal mine'} (${mtype}, ${prov})`,
      nace: '05',  // Mining of coal and lignite
      source: `GEM Coal Mines ZA (${mtype})`,
    })
  }
  return out
}

async function main() {
  console.log(`=== ZA Industrial Enrichment — Eskom UP + GEM + GEM Coal Mines (${YEAR}) ===\n`)

  const eskom = loadEskomPlants()
  console.log(`  Eskom (UP): ${eskom.length} plants`)

  const gem = loadGemPlants()
  console.log(`  GEM operating: ${gem.length} plants`)

  const coalMines = loadCoalMines()
  console.log(`  GEM Coal Mines (Operating): ${coalMines.length} mines`)

  // The core's empty-guard only covers ALL-empty: a partially-loaded run would
  // reset area stamps it cannot re-stamp (/gg Codex) — any empty source is fatal.
  const sourceFeatureCounts: Array<[string, number]> = [
    ['power-plants-eskom.geojson', eskom.length],
    ['power-plants-gem.geojson', gem.length],
    ['coal-mines-gem.geojson', coalMines.length],
  ]
  for (const [file, count] of sourceFeatureCounts) {
    if (count === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }

  // Priority: Eskom UP (verified national data) > GEM (broader renewable coverage) > coal mines
  // Dedup by coordinate
  const seen = new Set<string>()
  const allSites: IndSite[] = []
  for (const s of [...eskom, ...gem, ...coalMines]) {
    const key = `${s.lat.toFixed(3)}_${s.lon.toFixed(3)}`
    if (seen.has(key)) continue
    seen.add(key)
    allSites.push(s)
  }
  console.log(`  Total unique sites: ${allSites.length}`)

  const facilities: MatchFacility[] = []
  for (const s of allSites) {
    facilities.push({ lat: s.lat, lon: s.lon, nace4: naceStringToUint16(s.nace), ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inSouthAfrica,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE (Lesotho/Eswatini enclaves) still holds in-ZA rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, ZA_BBOX),
    searchRadiusM: 2000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('ZA'),
    datasetNonEmpty: allSites.length > 0, // deduped union of all ZA sources, pre-NACE-filter
    label: 'ZA',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

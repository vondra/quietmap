/**
 * Enrich VE industrial with VE360 (SIGOT mirror) + GEM (backfill).
 *
 * Sources:
 *   - **VE360 Parque de Generación Eléctrica** (proyecto.ve360 SIGOT mirror):
 *       services6.arcgis.com/lpJCO3ug8HhNiEOV/.../Parque_de_Generación_Eléctrica_gdb
 *     289 power plant entries with rich fields:
 *       PLANTA, PROPIEDAD (PDVSA/Corpoelec/etc), CAPACIDAD_MW,
 *       OPERACIÓN_ACTUAL_MW (!), ESTADO_DEL_MANTENIMIENTO, FECHA
 *     Critical observation: total nameplate 15,361 MW but ACTUAL operation
 *     only 3,786 MW (~25%). This reflects the real-world collapse of the
 *     Venezuelan electricity grid due to years of underinvestment and brain
 *     drain.
 *     Filter: only include plants with `OPERACIÓN_ACTUAL_MW > 0`
 *
 *   - **VE360 Subestaciones Eléctricas**: 209 substations
 *   - **VE360 Oil wells (Pozos Petroleros)**: 20,714 wells in Faja del Orinoco
 *     + Lake Maracaibo basin (NACE 06)
 *   - **VE360 Oil pipelines (Ductos)**: 2,269 oil/gas line segments
 *   - **VE360 Oil plants**: 28 processing plants
 *   - **VE360 Oil stations**: 110 pumping/compressor stations
 *   - **VE360 Gas flares**: 148 flaring/venting points
 *
 *   - **GEM Global Integrated Power v1** (Country_area='Venezuela'):
 *     102 features for backfill, especially **Guri Dam** (10,200 MW) +
 *     Macagua + Caruachi hydroelectric complex on the Caroní River which
 *     VE360 dataset appears to underrepresent.
 *
 * Mining (not captured — CVG SIDOR, Alcasa, Venalum, Ferrominera rely on OSM).
 * Major refineries: Paraguaná CRP (Amuay, Cardón, Bajo Grande), El Palito,
 * Puerto La Cruz, San Roque — rely on OSM landuse=industrial tags.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-ve.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ve`)

// Venezuela bbox
const VE_BBOX: [number, number, number, number] = [0.6, -73.4, 12.5, -59.0]

const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  // Colombia (W)
  [-4.3, -73.4, 12.5, -67.0],
  // Brazil (S)
  [0.6, -68.0, 5.3, -59.0],
  // Guyana (E, Essequibo disputed — but Venezuela claims it; exclude conservatively)
  [0.6, -61.4, 8.7, -56.5],
]

function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inVenezuela(lat: number, lon: number): boolean {
  return inBbox(lat, lon, VE_BBOX) && !inExcluded(lat, lon)
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

/** NACE values may be 2-digit ('06', '19') or 6-digit ('351100'); pad to
 *  6-digit then truncate to the arrow uint16 ('06' → 600, '19' → 1900,
 *  '351100' → 3511) — same arithmetic the old carpet loop used. */
function naceStringToUint16(nace: string): number {
  const nace6Raw = nace.length < 6 ? (nace + '0000').substring(0, 6) : nace
  return Math.floor((parseInt(nace6Raw, 10) || 0) / 100)
}

function loadVePowerPlants(): IndSite[] {
  const path = resolve(CACHE_DIR, 'power-plants-ve360.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, VE_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    // Filter: only plants with actual operation > 0
    const actual = p['OPERACIÓN_ACTUAL_MW']
    const actualMw = typeof actual === 'number' ? actual : parseFloat(String(actual || 0)) || 0
    if (actualMw <= 0) continue
    out.push({
      lat, lon,
      name: `${p.PLANTA || 'VE plant'} (${p.PROPIEDAD || '?'})`,
      nace: '351100',
      source: `VE360 power (${p.PROPIEDAD || '?'})`,
    })
  }
  return out
}

function loadVeSubstations(): IndSite[] {
  const path = resolve(CACHE_DIR, 'substations-ve360.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, VE_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    out.push({
      lat, lon,
      name: (p.NOMBRE || p.Nombre || p.nombre || 'VE substation').toString(),
      nace: '351100',
      source: 'VE360 substation',
    })
  }
  return out
}

function loadVeOilWells(): IndSite[] {
  const path = resolve(CACHE_DIR, 'oil-wells.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, VE_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    out.push({
      lat, lon,
      name: (p.NOMBRE || p.Nombre || p.nombre || 'VE oil well').toString(),
      nace: '06',  // Extraction of crude petroleum and natural gas
      source: 'VE360 oil well',
    })
  }
  return out
}

function loadVeOilPlants(): IndSite[] {
  const path = resolve(CACHE_DIR, 'oil-plants.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, VE_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    out.push({
      lat, lon,
      name: (p.NOMBRE || p.Nombre || p.nombre || 'VE oil plant').toString(),
      nace: '19',  // Manufacture of coke and refined petroleum products
      source: 'VE360 oil plant',
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
    if (!inBbox(lat, lon, VE_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    const fuel = (p.Type || 'unknown').toString().toLowerCase()
    const nace = fuelToNace(fuel)
    if (nace === null) continue  // wind (source_type=10) or unknown → skip
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || 'VE plant').toString(),
      nace,
      source: `GEM VE (${fuel})`,
    })
  }
  return out
}

async function main() {
  console.log(`=== VE Industrial Enrichment — VE360 SIGOT + GEM (${YEAR}) ===\n`)

  const vePower = loadVePowerPlants()
  console.log(`  VE360 power (actual MW > 0): ${vePower.length}`)

  const veSubs = loadVeSubstations()
  console.log(`  VE360 substations:           ${veSubs.length}`)

  const veOilWells = loadVeOilWells()
  console.log(`  VE360 oil wells:             ${veOilWells.length}`)

  const veOilPlants = loadVeOilPlants()
  console.log(`  VE360 oil plants:            ${veOilPlants.length}`)

  const gem = loadGemPlants()
  console.log(`  GEM operating plants:        ${gem.length}`)

  // The core's empty-guard only covers ALL-empty: a partially-loaded run would
  // reset area stamps it cannot re-stamp (/gg Codex) — any empty source is fatal.
  const sourceFeatureCounts: Array<[string, number]> = [
    ['power-plants-ve360.geojson', vePower.length],
    ['substations-ve360.geojson', veSubs.length],
    ['oil-wells.geojson', veOilWells.length],
    ['oil-plants.geojson', veOilPlants.length],
    ['power-plants-gem.geojson', gem.length],
  ]
  for (const [file, count] of sourceFeatureCounts) {
    if (count === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }

  // Dedup by coordinate
  const seen = new Set<string>()
  const allSites: IndSite[] = []
  // Priority: specific (oil plants/wells) before general (power plants)
  for (const s of [...veOilPlants, ...vePower, ...gem, ...veOilWells, ...veSubs]) {
    const key = `${s.lat.toFixed(3)}_${s.lon.toFixed(3)}`
    if (seen.has(key)) continue
    seen.add(key)
    allSites.push(s)
  }
  console.log(`  Total unique sites:          ${allSites.length}`)

  const facilities: MatchFacility[] = []
  for (const s of allSites) {
    facilities.push({ lat: s.lat, lon: s.lon, nace4: naceStringToUint16(s.nace), ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inVenezuela,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-VE rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, VE_BBOX),
    searchRadiusM: 2000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('VE'),
    datasetNonEmpty: allSites.length > 0, // deduped union of all VE sources, pre-NACE-filter
    label: 'VE',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

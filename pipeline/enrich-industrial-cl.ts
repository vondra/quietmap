/**
 * Enrich CL industrial with multiple Chilean ArcGIS layers + GEM.
 *
 * Sources (Esri Chile mirror — orgId r7t1P5pnkoOLRdhr):
 *   - Centrales_de_Generación_Eléctrica: 203 thermal plants (CNE/ME),
 *     all OPERATIVA. Verified top: Atacama 716 MW, Guacolda 702 MW,
 *     Mejillones 538 MW, Kelar 517 MW, Angamos 502 MW, Cochrane 490 MW
 *   - Catastro_Relaves: 795 mining tailings dams (SERNAGEOMIN),
 *     including 128 ACTIVO + 12 EN_CONSTRUCCION (140 active mines).
 *     Mostly COBRE (copper) and ORO (gold) — unique to Chile, 5th-largest
 *     copper producer in the world. Includes Chuquicamata, Escondida,
 *     Collahuasi, Spence, Radomiro Tomic, El Teniente, Los Pelambres,
 *     Andina, Centinela, Quebrada Blanca, Mantos Blancos, Mantoverde
 *   - Transmisión_Eléctrica layer 2: 1,132 substations (CNE)
 *
 * Plus: GEM Global Integrated Power v1 (Country_area='Chile')
 *   - 722 plants total, 427 operating (highest in pipeline so far)
 *   - 422 solar (Atacama desert, world's best solar resource), 122 wind,
 *     57 coal, 56 oil/gas, 46 hydropower, 14 bioenergy, 5 geothermal
 *
 * NACE mapping:
 *   - Power plants (thermal/hydro/solar/wind/geothermal/bioenergy) → NACE 35
 *   - Mining tailings (copper/gold) → NACE 07 (Mining of metal ores)
 *   - Substations → NACE 35
 *
 * Status filter:
 *   - Centrales: ESTADO='OPERATIVA' (all 203 are operational)
 *   - Catastro Relaves: ESTADO_INS='ACTIVO' or 'EN CONSTRUCCION'
 *   - GEM: Status='operating'
 *   - Substations: ESTADO contains 'oper' or any (most are active)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-cl.ts
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, makeTable, makeVector } from 'apache-arrow'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { cellToLatLng } from 'h3-js'
import { SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { flatDistM, inBbox } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cl`)

// Chile bbox (long thin strip including Easter Island excluded)
const CL_BBOX: [number, number, number, number] = [-56.0, -76.0, -17.5, -66.4]

// Chile is very narrow (~180 km wide). Border with Argentina is along the
// Andes crest, roughly at lon -67.5 in the north (Calama region) and bending
// further west toward -71 in southern Patagonia.
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  // Argentina (north): east of Andes, lat -42 to -17.5
  [-42.0, -67.5, -17.5, -53.0],
  // Argentina (Patagonia): east of -71, lat -56 to -42 (Andes bend west)
  [-56.0, -71.0, -42.0, -53.0],
  // Bolivia (NE corner above Antofagasta region)
  [-22.9, -68.5, -9.5, -57.5],
  // Peru (N of Chile, west of Andes)
  [-17.65, -82.0, -3.0, -68.0],
]

function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}
interface IndSite {
  lat: number
  lon: number
  name: string
  fuel: string
  nace: string
  source: string
}

function loadCnehermal(): IndSite[] {
  const path = resolve(CACHE_DIR, 'thermal-plants.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, CL_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    if ((p.ESTADO || '').toString().toUpperCase() !== 'OPERATIVA') continue
    const fuel = (p.COMBUSTIBL || 'thermal').toString().toLowerCase()
    out.push({
      lat, lon,
      name: (p.NOMBRE || 'CL thermal plant').toString(),
      fuel,
      nace: '351100',
      source: `CNE Centrales (${fuel})`,
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
    if (!inBbox(lat, lon, CL_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    const fuel = (p.Type || 'unknown').toString().toLowerCase()
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || 'CL plant').toString(),
      fuel,
      nace: '351100',
      source: `GEM CL (${fuel})`,
    })
  }
  return out
}

function loadMiningTailings(): IndSite[] {
  const path = resolve(CACHE_DIR, 'mining-tailings.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, CL_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const estado = (p.ESTADO_INS || '').toString().toUpperCase()
    if (!estado.startsWith('ACT') && !estado.startsWith('EN CONST')) continue
    const recurso = (p.RECURSO || '').toString().toLowerCase()
    out.push({
      lat, lon,
      name: `${p.NOMBRE_FAE || 'CL mine'} (${p.NOMBRE_EMP || ''})`,
      fuel: recurso,
      nace: '07',  // Mining of metal ores
      source: `SERNAGEOMIN Catastro Relaves (${recurso})`,
    })
  }
  return out
}

function loadSubstations(): IndSite[] {
  const path = resolve(CACHE_DIR, 'substations.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, CL_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    // Substations typically don't have explicit OPERATIVA status; assume active
    const tension = p.TENSION_KV ?? 0
    if (typeof tension === 'number' && tension < 110) continue  // skip low-voltage distribution substations
    out.push({
      lat, lon,
      name: (p.NOMBRE || 'CL substation').toString(),
      fuel: `${tension}kV`,
      nace: '351100',
      source: `CNE Substations (${tension}kV)`,
    })
  }
  return out
}

async function main() {
  // #31 round-2: this direct 330 writer must honour the same national-ownership
  // polygon as stampOneWinner — the shared id's bbox/exclusion gate alone can
  // still stamp a neighbour's rows (R9 cannot see 330: no country identity).
  const inClCountry = makeCountryGate('CL')
  console.log(`=== CL Industrial Enrichment — CNE + SERNAGEOMIN + GEM (${YEAR}) ===\n`)

  const cneThermal = loadCnehermal()
  const gem = loadGemPlants()
  const mining = loadMiningTailings()
  const substations = loadSubstations()

  console.log(`  CNE thermal plants:    ${cneThermal.length}`)
  console.log(`  GEM operating plants:  ${gem.length}`)
  console.log(`  SERNAGEOMIN mines (active): ${mining.length}`)
  console.log(`  CNE substations (≥110 kV): ${substations.length}`)

  // Priority: CNE thermal (verified data) > GEM (renewable backfill) > mines > substations
  // Use map keyed by lat,lon to dedupe (CNE thermal often duplicates GEM coal/gas)
  const allSites: IndSite[] = []
  const seen = new Set<string>()
  for (const site of [...cneThermal, ...gem, ...mining, ...substations]) {
    const key = `${site.lat.toFixed(4)}_${site.lon.toFixed(4)}`
    if (seen.has(key)) continue
    seen.add(key)
    allSites.push(site)
  }
  console.log(`  Total unique sites:    ${allSites.length}`)

  // Spatial grid (~0.1° = ~11 km cells)
  const grid = new Map<string, IndSite[]>()
  for (const s of allSites) {
    const key = `${Math.floor(s.lat * 10)}_${Math.floor(s.lon * 10)}`
    if (!grid.has(key)) grid.set(key, [])
    grid.get(key)!.push(s)
  }

  const MY_SOURCE_ID = SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, CL_BBOX) && existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
    } catch {}
  }
  console.log(`  CL-bbox hexes with industrial.arrow: ${hexDirs.length}\n`)

  let totalOsm = 0, matched = 0, newEntries = 0
  const bySource: Record<string, number> = {}

  for (const hex of hexDirs) {
    const arrowPath = resolve(H3R4_DIR, hex, 'industrial.arrow')
    if (!existsSync(arrowPath)) continue
    try {
      await withArrowWrite(arrowPath, table => {
        const n = table.numRows
        if (n === 0) return table
        const osmId = table.getChild('osm_id')
        const centroidLat = table.getChild('centroid_lat') ?? table.getChild('lat')
        const centroidLon = table.getChild('centroid_lon') ?? table.getChild('lon')
        const existingNaceCol = table.getChild('nace_4digit')
        const existingDatasetIdCol = table.getChild('source_id')
        if (!osmId || !centroidLat || !centroidLon) return table
        const newNace = new Uint16Array(n)
        const newDatasetId = new Uint16Array(n)
        const existingSourceId = new Uint16Array(n)
        for (let j = 0; j < n; j++) {
          newNace[j] = (existingNaceCol?.get(j) as number) ?? 0
          existingSourceId[j] = (existingDatasetIdCol?.get(j) as number) ?? 0
          newDatasetId[j] = existingSourceId[j]
        }
        let anyChanged = false

        for (let i = 0; i < n; i++) {
          totalOsm++
          const lat = centroidLat.get(i) as number
          const lon = centroidLon.get(i) as number
          if (lat == null || lon == null) continue
          if (!inBbox(lat, lon, CL_BBOX) || inExcluded(lat, lon) || !inClCountry(lat, lon)) continue

          // 2 km search radius — solar/wind farms have large polygons
          const searchRadius = 2000
          const baseLat = Math.floor(lat * 10)
          const baseLon = Math.floor(lon * 10)
          let best: IndSite | null = null
          let bestDist = searchRadius
          for (let dy = -2; dy <= 2; dy++) {
            for (let dx = -2; dx <= 2; dx++) {
              const cell = grid.get(`${baseLat + dy}_${baseLon + dx}`)
              if (!cell) continue
              for (const s of cell) {
                const d = flatDistM(lat, lon, s.lat, s.lon)
                if (d < bestDist) { bestDist = d; best = s }
              }
            }
          }
          if (best) {
            // NACE values may be 2-digit ('07') or 6-digit ('351100'); pad to 6-digit.
            const nace6Raw = best.nace.length < 6 ? (best.nace + '0000').substring(0, 6) : best.nace
            const nace6 = parseInt(nace6Raw, 10) || 0
            const nace4 = Math.floor(nace6 / 100)
            const existingId = existingSourceId[i]
            if (shouldOverwrite(existingId, MY_SOURCE_ID)) {
              newNace[i] = nace4
              newDatasetId[i] = MY_SOURCE_ID
              if (existingId === 0) newEntries++
              matched++
              anyChanged = true
            }
          }
        }
        if (!anyChanged) return table
        const columns: Record<string, any> = {}
        for (const field of table.schema.fields) {
          if (field.name === 'nace_4digit' || field.name === 'source_id') continue
          columns[field.name] = table.getChild(field.name)!
        }
        columns['nace_4digit'] = makeVector(newNace)
        columns['source_id'] = makeVector(newDatasetId)
        return makeTable(columns)
      })
    } catch {}
  }

  console.log(`=== Results ===`)
  console.log(`  OSM industrial sites scanned: ${totalOsm.toLocaleString()}`)
  console.log(`  Matched:                      ${matched.toLocaleString()}`)
  console.log(`  By source:`)
  for (const [k, v] of Object.entries(bySource).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k.padEnd(15)} ${v}`)
  }
  console.log(`  New/updated arrow rows:       ${newEntries.toLocaleString()}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

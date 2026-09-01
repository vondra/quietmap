/**
 * Enrich CO industrial with GEM + ANM Mining Titles + ANH Oil/Gas Blocks.
 *
 * Sources:
 *   - **GEM Global Integrated Power v1** (Country_area='Colombia'):
 *       services.arcgis.com/P3ePLMYs2RVChkJx/.../Global_Integrated_Power_v1/FeatureServer/0
 *     937 plants total, **251 operating**
 *     Top: Guavio 1250 MW, San Carlos 1240 MW, Hidroituango 1200 MW,
 *     Chivor 1000 MW, Sogamoso 820 MW, Termobarranquilla 791 MW
 *     Fuel breakdown (operating): hydro 49, oil/gas 43, coal 23, solar 705
 *     (mostly pre-construction), wind 114, bioenergy 2
 *     → NACE 35
 *
 *   - **ANM Títulos Mineros 2023**: 7,541 polygons of mining concessions
 *       services9.arcgis.com/pZylgd2zhNey2qXF/.../Titulos_mineros_vigentes__2023__Colombia
 *     5,758 in `Explotación` (active), 1,145 `Exploración`, 329 `Construcción`
 *     Mineral types (top): MINERALES DE ORO (985), ANHIDRITA/CARBÓN (821),
 *     ARENAS/GRAVAS (740), CARBÓN METALÚRGICO (584), ARENAS DE RIO (409),
 *     ARCILLAS (316), MINERALES DE COBRE (298), ANTRACITA/CARBÓN (284)
 *     Mapped to NACE codes:
 *       - Coal/anthracite/coking coal → **NACE 05**
 *       - Metals (gold/copper/silver/iron) → **NACE 07**
 *       - Sand/gravel/clay/quarry → **NACE 08**
 *
 *   - **ANH Hidrocarburos 2023**: 445 oil/gas production blocks
 *       services9.arcgis.com/pZylgd2zhNey2qXF/.../Explotacion_de_hidrocarburos_2023__ANH_
 *     236 in PRODUCCION (active), 202 EXPLORACION
 *     Operators: Ecopetrol, Occidental, Parex, GeoPark, Frontera Energy
 *     Cuencas: Llanos (Cusiana/Cupiagua), VMM, Catatumbo (Caño Limón),
 *              Putumayo, Caguán-Putumayo
 *     → **NACE 06** (Extraction of crude petroleum and natural gas)
 *
 * Polygon-based matching: for ANM titles and ANH blocks, we check whether
 * the OSM industrial centroid falls inside the polygon (point-in-polygon).
 * For point-based GEM plants, we use the nearest-neighbour 2km lookup.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-co.ts
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { makeTable, vectorFromArray, Uint16 } from 'apache-arrow'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { cellToLatLng } from 'h3-js'
import { NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox, pointInRing } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/co`)

const CO_BBOX: [number, number, number, number] = [-4.3, -82.0, 13.5, -66.8]

const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  [0.6, -72.5, 12.5, -59.0],   // Venezuela
  [-4.3, -70.0, -1.0, -66.5],  // Brazil
  [-18.4, -82.0, -1.5, -69.0], // Peru
  [-5.0, -82.0, 1.5, -75.0],   // Ecuador
  [7.2, -83.0, 9.7, -77.2],    // Panama
]

function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}

function inColombia(lat: number, lon: number): boolean {
  return inBbox(lat, lon, CO_BBOX) && !inExcluded(lat, lon)
}

interface Site {
  lat: number; lon: number; name: string; nace: string; source: string
}

interface PolySite {
  rings: [number, number][][]  // [outer, hole1, hole2, ...] but we treat all as outer
  bbox: [number, number, number, number]  // minLat, minLon, maxLat, maxLon
  name: string
  nace: string
  source: string
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

/** The padded-string arithmetic BOTH tiers use to turn a NACE string into the
 *  arrow uint16 — 2-digit ANM/ANH codes pad to 6 then truncate ('05' → 500,
 *  '06' → 600), 6-digit GEM codes pass through ('351100' → 3511). */
function naceStringToUint16(nace: string): number {
  const nace6Raw = nace.length < 6 ? (nace + '0000').substring(0, 6) : nace
  return Math.floor((parseInt(nace6Raw, 10) || 0) / 100)
}

function loadGemPlants(): Site[] {
  const path = resolve(CACHE_DIR, 'power-plants-gem.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: Site[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, CO_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    const fuel = (p.Type || 'unknown').toString().toLowerCase()
    const nace = fuelToNace(fuel)
    if (nace === null) continue  // wind (source_type=10) or unknown → skip
    out.push({
      lat, lon,
      name: (p.Plant___Project_name || 'CO plant').toString(),
      nace,
      source: `GEM CO (${fuel})`,
    })
  }
  return out
}

function classifyMineral(minerales: string): string {
  const m = minerales.toUpperCase()
  // Coal classifications → NACE 05
  if (/CARBÓN|CARBON|ANTRACITA|HULLA|LIGNITO|TURBA/.test(m)) return '05'
  // Metal ores → NACE 07
  if (/ORO|COBRE|HIERRO|PLATA|ZINC|PLOMO|NIQUEL|PLATIN|MOLIBDENO|MANGANESO|COBALTO|ESTAÑO|TUNGSTENO|MERCURIO|EMERALDA|ESMERALDA/.test(m)) return '07'
  // Stone, sand, clay → NACE 08
  if (/ARENA|GRAVA|ARCILLA|CALIZA|MARMOL|GRANITO|YESO|FELDESPATO|CAOLIN|RECEBO|ROCA|CANTERA|BENTONITA|DOLOMITA|ANHIDRITA|FOSFORICA/.test(m)) return '08'
  // Default to metal mining
  return '07'
}

function loadMiningPolys(): PolySite[] {
  const path = resolve(CACHE_DIR, 'mining-titles.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: PolySite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let polys: [number, number][][][] = []
    if (g.type === 'Polygon') polys = [g.coordinates]
    else if (g.type === 'MultiPolygon') polys = g.coordinates
    else continue
    const p = f.properties || {}
    const etapa = (p.ETAPA || '').toString()
    // Only enrich active mines (Explotación + Construcción)
    if (!/Explotaci|Construcci/i.test(etapa)) continue
    const minerales = (p.MINERALES || '').toString()
    const nace = classifyMineral(minerales)
    const name = `Mina ${(p.MUNICIPIOS || '').toString().split(',')[0]} (${minerales.slice(0, 30)})`
    for (const poly of polys) {
      if (!poly || poly.length === 0) continue
      const ring = poly[0]  // outer ring
      let minLat = Infinity, minLon = Infinity, maxLat = -Infinity, maxLon = -Infinity
      for (const [lon, lat] of ring) {
        if (lat < minLat) minLat = lat
        if (lon < minLon) minLon = lon
        if (lat > maxLat) maxLat = lat
        if (lon > maxLon) maxLon = lon
      }
      // Skip if entire polygon is outside CO bbox
      if (maxLat < CO_BBOX[0] || minLat > CO_BBOX[2] || maxLon < CO_BBOX[1] || minLon > CO_BBOX[3]) continue
      out.push({
        rings: poly,
        bbox: [minLat, minLon, maxLat, maxLon],
        name,
        nace,
        source: `ANM CO mining (${minerales.slice(0, 20)})`,
      })
    }
  }
  return out
}

function loadOilGasBlocks(): PolySite[] {
  const path = resolve(CACHE_DIR, 'oil-gas-blocks.geojson')
  if (!existsSync(path)) return []
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: PolySite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let polys: [number, number][][][] = []
    if (g.type === 'Polygon') polys = [g.coordinates]
    else if (g.type === 'MultiPolygon') polys = g.coordinates
    else continue
    const p = f.properties || {}
    const estado = (p.ESTAD_AREA || '').toString().toUpperCase()
    if (!estado.includes('PRODUC')) continue  // Only producing blocks
    const operador = (p.OPERADOR || '').toString()
    const cuenca = (p.CUENCA_SED || '').toString()
    const name = `${p.AREA_NOMBR || 'Bloque'} (${operador}, ${cuenca})`
    for (const poly of polys) {
      if (!poly || poly.length === 0) continue
      const ring = poly[0]
      let minLat = Infinity, minLon = Infinity, maxLat = -Infinity, maxLon = -Infinity
      for (const [lon, lat] of ring) {
        if (lat < minLat) minLat = lat
        if (lon < minLon) minLon = lon
        if (lat > maxLat) maxLat = lat
        if (lon > maxLon) maxLon = lon
      }
      if (maxLat < CO_BBOX[0] || minLat > CO_BBOX[2] || maxLon < CO_BBOX[1] || minLon > CO_BBOX[3]) continue
      out.push({
        rings: poly,
        bbox: [minLat, minLon, maxLat, maxLon],
        name,
        nace: '06',  // Extraction of crude petroleum and natural gas
        source: `ANH CO oil/gas (${cuenca})`,
      })
    }
  }
  return out
}

async function main() {
  // #31 round-2: this direct 330 writer must honour the same national-ownership
  // polygon as stampOneWinner — the shared id's bbox/exclusion gate alone can
  // still stamp a neighbour's rows (R9 cannot see 330: no country identity).
  const inCoCountry = makeCountryGate('CO')
  console.log(`=== CO Industrial Enrichment — GEM + ANM Mining + ANH Oil/Gas (${YEAR}) ===\n`)

  const gemPlants = loadGemPlants()
  console.log(`  GEM operating plants:    ${gemPlants.length}`)

  const miningPolys = loadMiningPolys()
  console.log(`  ANM active mining titles: ${miningPolys.length}`)

  const oilGasPolys = loadOilGasBlocks()
  console.log(`  ANH producing oil/gas blocks: ${oilGasPolys.length}`)

  // The core's empty-guard only covers ALL-empty: a partially-loaded run would
  // reset area stamps it cannot re-stamp (/gg Codex) — any empty source is fatal.
  // (Tier 1's reset also wipes the ANM/ANH containment stamps tier 2 re-creates.)
  const sourceFeatureCounts: Array<[string, number]> = [
    ['power-plants-gem.geojson', gemPlants.length],
    ['mining-titles.geojson', miningPolys.length],
    ['oil-gas-blocks.geojson', oilGasPolys.length],
  ]
  for (const [file, count] of sourceFeatureCounts) {
    if (count === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }

  // Tier order matters: the one-winner core runs FIRST (it resets ALL our old 330
  // stamps inside CO's country polygon (countryGate — the reset is country-wide
  // within CO, which is what re-sweeps the ANM/ANH containment stamps tier 2
  // re-creates), then elects one polygon per GEM plant); the ANM/ANH
  // containment tier runs SECOND, so a mining title / oil block containing a row
  // legitimately overrides a GEM winner (existingId === selfId passes
  // shouldOverwrite). Reversed, the reset would wipe the containment stamps.
  const facilities: MatchFacility[] = []
  for (const s of gemPlants) {
    facilities.push({ lat: s.lat, lon: s.lon, nace4: naceStringToUint16(s.nace), ...NATIONAL_MIX })
  }
  await stampOneWinner({
    facilities,
    isInside: inColombia,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-CO rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, CO_BBOX),
    searchRadiusM: 2000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('CO'),
    // The FATAL empty-source guard above exits before this call whenever any of
    // the three CO files is missing/empty — reaching here IS the parse proof.
    datasetNonEmpty: true,
    label: 'CO',
    h3r4Dir: H3R4_DIR,
  })

  // Spatial grid for polygon bboxes (ANM mining + ANH oil/gas)
  // Each polygon is registered in all 0.5° cells it overlaps
  const polyGrid = new Map<string, PolySite[]>()
  function registerPoly(p: PolySite) {
    const startLat = Math.floor(p.bbox[0] * 2)
    const endLat = Math.floor(p.bbox[2] * 2)
    const startLon = Math.floor(p.bbox[1] * 2)
    const endLon = Math.floor(p.bbox[3] * 2)
    for (let y = startLat; y <= endLat; y++) {
      for (let x = startLon; x <= endLon; x++) {
        const key = `${y}_${x}`
        if (!polyGrid.has(key)) polyGrid.set(key, [])
        polyGrid.get(key)!.push(p)
      }
    }
  }
  for (const p of miningPolys) registerPoly(p)
  for (const p of oilGasPolys) registerPoly(p)
  console.log(`\n  Polygon grid cells: ${polyGrid.size}`)

  const MY_SOURCE_ID = NATIONAL_MIX.id

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, CO_BBOX) && existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
    } catch {}
  }
  console.log(`  CO-bbox hexes with industrial.arrow: ${hexDirs.length}\n`)

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
          if (!inBbox(lat, lon, CO_BBOX) || inExcluded(lat, lon) || !inCoCountry(lat, lon)) continue

          let chosen: { nace: string; source: string } | null = null

          // Point-in-polygon for ANM mining + ANH oil/gas
          const polyKey = `${Math.floor(lat * 2)}_${Math.floor(lon * 2)}`
          const cell = polyGrid.get(polyKey)
          if (cell) {
            for (const p of cell) {
              if (lat < p.bbox[0] || lat > p.bbox[2] || lon < p.bbox[1] || lon > p.bbox[3]) continue
              if (pointInRing(lon, lat, p.rings[0])) {
                chosen = { nace: p.nace, source: p.source }
                break
              }
            }
          }

          if (chosen) {
            const nace4 = naceStringToUint16(chosen.nace)
            const existingId = existingSourceId[i]
            if (shouldOverwrite(existingId, MY_SOURCE_ID)) {
              newNace[i] = nace4
              newDatasetId[i] = MY_SOURCE_ID
              if (existingId === 0) newEntries++
              matched++
              anyChanged = true
              const family = chosen.source.split(' ')[0]
              bySource[family] = (bySource[family] || 0) + 1
            }
          }
        }
        if (!anyChanged) return table
        const columns: Record<string, any> = {}
        for (const field of table.schema.fields) {
          if (field.name === 'nace_4digit' || field.name === 'source_id') continue
          columns[field.name] = table.getChild(field.name)!
        }
        columns['nace_4digit'] = vectorFromArray(newNace, new Uint16())
        columns['source_id'] = vectorFromArray(newDatasetId, new Uint16())
        return makeTable(columns)
      })
    } catch {}
  }

  console.log(`=== Results (ANM/ANH polygon containment tier) ===`)
  console.log(`  OSM industrial sites scanned: ${totalOsm.toLocaleString()}`)
  console.log(`  Matched:                      ${matched.toLocaleString()}`)
  console.log(`  By source:`)
  for (const [k, v] of Object.entries(bySource).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k.padEnd(15)} ${v}`)
  }
  console.log(`  New/updated arrow rows:       ${newEntries.toLocaleString()}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

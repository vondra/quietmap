/**
 * Enrich PE industrial with GEM + INGEMMET + PERUMIN layers.
 *
 * Peru has the **most detailed mining polygon data in the entire pipeline**:
 * actual geometry for open pits, tailings dams, leach pads, waste rock dumps,
 * and supervised mining units — all published via PERUMIN and INGEMMET
 * ArcGIS Online services (community mirrors of Osinergmin data).
 *
 * Sources:
 *   - **GEM Global Integrated Power v1** (Country_area='Peru'):
 *       240 plants total, 85 operating
 *       Top: Kallpa 874 MW CCGT, Chilca 862 MW CCGT, Mantaro 798 MW hydro,
 *            Santiago Antúnez de Mayolo 798 MW hydro, Fenix 570 MW CCGT,
 *            Ventanilla 532 MW CCGT, Cerro del Águila 525 MW hydro,
 *            Chaglla 456 MW hydro
 *       → NACE 35
 *
 *   - **INGEMMET Yacimientos Mineros**:
 *       1,507 mine point records (ESTADO: Activa 61, Produccion 114,
 *       Exploracion 300, Cierre 81, etc.)
 *       Only enrich Activa + Produccion + Ampliacion + Desarrollo states
 *       → NACE 07 (Metal ores — Peru is 2nd copper, 1st silver, 6th gold)
 *
 *   - **PERUMIN_WFL1** (community mirror of Osinergmin mining data):
 *       Layer 2: 335 Unidades Mineras Supervisadas (polygons)
 *              → NACE 07 (major active mine concessions)
 *       Layer 4: 27 Tajos Abiertos (open pit polygons) — Toquepala, Cuajone,
 *              Cerro Verde, Ferrobamba (Las Bambas), Toromocho, Constancia,
 *              Chaupiloma (Yanacocha)
 *              → NACE 07
 *       Layer 5: 12 Relaveras (tailings dam polygons)
 *              → NACE 07
 *       Layer 6: 17 Pila de Lixiviación (heap leach pad polygons)
 *              → NACE 07
 *       Layer 7: 37 Desmonteras (waste rock dump polygons)
 *              → NACE 07
 *       Layer 13: 16 major concession polygons with METALES_MINERALES
 *              → NACE 07
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-pe.ts
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC, makeTable, vectorFromArray, Uint16 } from 'apache-arrow'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { cellToLatLng } from 'h3-js'
import { SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { flatDistM, inBbox, pointInRing } from './lib/spatial.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/pe`)

// Peru bbox
const PE_BBOX: [number, number, number, number] = [-18.4, -82.0, -0.0, -68.5]

// Exclusion zones (neighbouring countries only)
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  // Ecuador (N)
  [-5.0, -82.0, 1.5, -75.0],
  // Colombia (NE Amazon)
  [-4.3, -75.0, 1.5, -66.5],
  // Brazil (E Amazon)
  [-18.4, -75.0, -1.0, -66.5],
  // Bolivia (SE)
  [-22.9, -70.0, -9.5, -57.5],
  // Chile (S)
  [-22.0, -75.0, -17.5, -68.5],
]

function inExcluded(lat: number, lon: number): boolean {
  for (const b of EXCLUDE_ZONES) if (inBbox(lat, lon, b)) return true
  return false
}
interface Site {
  lat: number; lon: number; name: string; nace: string; source: string
}

interface PolySite {
  rings: [number, number][][]
  bbox: [number, number, number, number]
  name: string
  nace: string
  source: string
}

function loadPoints(path: string, nameField: string, nace: string, source: string,
                    statusField?: string, statusOK?: string[]): Site[] {
  const full = resolve(CACHE_DIR, path)
  if (!existsSync(full)) return []
  const fc = JSON.parse(readFileSync(full, 'utf-8'))
  const out: Site[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    if (!inBbox(lat, lon, PE_BBOX) || inExcluded(lat, lon)) continue
    const p = f.properties || {}
    if (statusField && statusOK) {
      const s = (p[statusField] || '').toString().toLowerCase()
      if (!statusOK.some(ok => s.includes(ok))) continue
    }
    out.push({
      lat, lon,
      name: (p[nameField] || 'PE site').toString(),
      nace,
      source,
    })
  }
  return out
}

function loadPolys(path: string, nameField: string, nace: string, source: string,
                   statusField?: string, statusOK?: string[]): PolySite[] {
  const full = resolve(CACHE_DIR, path)
  if (!existsSync(full)) return []
  const fc = JSON.parse(readFileSync(full, 'utf-8'))
  const out: PolySite[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let polys: [number, number][][][] = []
    if (g.type === 'Polygon') polys = [g.coordinates]
    else if (g.type === 'MultiPolygon') polys = g.coordinates
    else continue
    const p = f.properties || {}
    if (statusField && statusOK) {
      const s = (p[statusField] || '').toString().toLowerCase()
      if (!statusOK.some(ok => s.includes(ok))) continue
    }
    const name = (p[nameField] || p.NOMBRE || 'PE site').toString()
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
      if (maxLat < PE_BBOX[0] || minLat > PE_BBOX[2] || maxLon < PE_BBOX[1] || minLon > PE_BBOX[3]) continue
      out.push({
        rings: poly,
        bbox: [minLat, minLon, maxLat, maxLon],
        name,
        nace,
        source,
      })
    }
  }
  return out
}

async function main() {
  // #31 round-2: this direct 330 writer must honour the same national-ownership
  // polygon as stampOneWinner — the shared id's bbox/exclusion gate alone can
  // still stamp a neighbour's rows (R9 cannot see 330: no country identity).
  const inPeCountry = makeCountryGate('PE')
  console.log(`=== PE Industrial Enrichment — GEM + INGEMMET + PERUMIN (${YEAR}) ===\n`)

  // Points: GEM power plants + INGEMMET yacimientos
  const gemPlants = loadPoints(
    'power-plants-gem.geojson', 'Plant___Project_name', '35', 'GEM PE',
    'Status', ['operating'],
  )
  console.log(`  GEM operating plants:     ${gemPlants.length}`)

  const yacimientos = loadPoints(
    'mining-yacimientos.geojson', 'UNIDAD', '07', 'INGEMMET PE yacimiento',
    'ESTADO', ['activa', 'produccion', 'ampliacion', 'desarrollo', 'exploracionavz'],
  )
  console.log(`  INGEMMET active yacimientos: ${yacimientos.length}`)

  // Polygons: PERUMIN layers (priority order: specific infrastructure first,
  // then broad concessions)
  const tajos = loadPolys(
    'mining-tajos.geojson', 'NOMBRE_TAJO', '07', 'PERUMIN PE open-pit',
  )
  console.log(`  PERUMIN open pits (tajos): ${tajos.length}`)

  const relaveras = loadPolys(
    'mining-relaveras.geojson', 'NOMBRE', '07', 'PERUMIN PE tailings',
  )
  console.log(`  PERUMIN tailings dams:     ${relaveras.length}`)

  const leachPads = loadPolys(
    'mining-leach-pads.geojson', 'NOMBRE', '07', 'PERUMIN PE leach-pad',
  )
  console.log(`  PERUMIN leach pads:        ${leachPads.length}`)

  const wasteDumps = loadPolys(
    'mining-waste-dumps.geojson', 'NOMBRE', '07', 'PERUMIN PE waste-dump',
  )
  console.log(`  PERUMIN waste dumps:       ${wasteDumps.length}`)

  const majorConcessions = loadPolys(
    'mining-major-concessions.geojson', 'METALES_MINERALES', '07', 'PERUMIN PE major-concession',
  )
  console.log(`  PERUMIN major concessions: ${majorConcessions.length}`)

  const unidadesMineras = loadPolys(
    'mining-unidades.geojson', 'CH_NOMBRE', '07', 'PERUMIN PE unidad-minera',
    'SITUACION', ['vigente'],
  )
  console.log(`  PERUMIN active unidades:   ${unidadesMineras.length}`)

  // Point grid for GEM + yacimientos
  const pointGrid = new Map<string, Site[]>()
  for (const s of [...gemPlants, ...yacimientos]) {
    const key = `${Math.floor(s.lat * 10)}_${Math.floor(s.lon * 10)}`
    if (!pointGrid.has(key)) pointGrid.set(key, [])
    pointGrid.get(key)!.push(s)
  }

  // Polygon grid — register each polygon in all 0.5° cells it overlaps
  // Priority: specific infrastructure > broad concessions
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
  // Register in priority order (first match wins in point-in-poly test)
  for (const p of tajos) registerPoly(p)
  for (const p of relaveras) registerPoly(p)
  for (const p of leachPads) registerPoly(p)
  for (const p of wasteDumps) registerPoly(p)
  for (const p of majorConcessions) registerPoly(p)
  for (const p of unidadesMineras) registerPoly(p)
  console.log(`  Polygon grid cells: ${polyGrid.size}`)

  const MY_SOURCE_ID = SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, PE_BBOX) && existsSync(resolve(H3R4_DIR, hex, 'industrial.arrow'))) hexDirs.push(hex)
    } catch {}
  }
  console.log(`  PE-bbox hexes with industrial.arrow: ${hexDirs.length}\n`)

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
          if (!inBbox(lat, lon, PE_BBOX) || inExcluded(lat, lon) || !inPeCountry(lat, lon)) continue

          let chosen: { nace: string; source: string } | null = null

          // 1. Point-in-polygon for PERUMIN mining polygons (priority order)
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

          // 2. Nearest point (GEM + INGEMMET) within 2 km
          if (!chosen) {
            const baseLat = Math.floor(lat * 10)
            const baseLon = Math.floor(lon * 10)
            let best: Site | null = null
            let bestDist = 2000
            for (let dy = -2; dy <= 2; dy++) {
              for (let dx = -2; dx <= 2; dx++) {
                const c = pointGrid.get(`${baseLat + dy}_${baseLon + dx}`)
                if (!c) continue
                for (const s of c) {
                  const d = flatDistM(lat, lon, s.lat, s.lon)
                  if (d < bestDist) { bestDist = d; best = s }
                }
              }
            }
            if (best) {
              chosen = { nace: best.nace, source: best.source }
            }
          }

          if (chosen) {
            // nace values are 2-digit ('35', '07'); pad to 6-digit before truncating.
            const nace6Raw = chosen.nace.length < 6 ? (chosen.nace + '0000').substring(0, 6) : chosen.nace
            const nace6 = parseInt(nace6Raw, 10) || 0
            const nace4 = Math.floor(nace6 / 100)
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

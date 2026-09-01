/**
 * Enrich MX roads.arrow with SICT/IMT Datos Viales 2025 (TDPA + vehicle class).
 *
 * Source: **Datos Viales 2025** of the Secretaría de Infraestructura,
 * Comunicaciones y Transportes (SICT) / Instituto Mexicano del Transporte (IMT).
 * The official portal moved from the now-defunct `*.sctcloud.com.mx` viewers to
 * `datosviales2020.routedev.mx` (a Mongo-backed JS map app with no bulk export).
 * We consume the CC-BY-4.0 community scrape that snapshotted it in Apr 2026:
 *   https://github.com/iChocko/Datos-Viales-SICT
 *
 * Universe **U1** (Red Nacional de Carreteras Pavimentadas) gives **4,790
 * segments** with real per-segment annual TDPA (Tránsito Diario Promedio Anual,
 * = AADT both-directions) plus vehicle composition. This is measured count
 * data with a native class split — the strongest road input available for MX.
 *
 *   geodata/geojson/light/u1_segmentos.geojson  LineString geometry + tdpa_2024
 *   csv/u1_segmentos.csv                         comp_* per segment_mongo_id
 *
 * IMT vehicle classes → CNOSSOS-EU 4 categories:
 *   light  = A (autos) + OTROS (unclassified, small)
 *   medium = B (autobuses) + C2 (2-axle trucks)        [CNOSSOS cat 2]
 *   heavy  = C3 + T3S2 + T3S3 + T3S2R4 (3+ axle / artic) [CNOSSOS cat 3]
 *   moto   = M (motocicletas)                           [CNOSSOS cat 4]
 * `comp_*` are FRACTIONS summing to 1.0 (not percentages — verified), so a
 * class AADT is `tdpa_2024 * frac`. TDPA-weighted national split is
 * ~79.5 / 7.1 / 8.2 / 5.1 (light/medium/heavy/moto).
 *
 * Matching: OSM high-order roads (class ≤ 3) → nearest *type-compatible* SICT
 * segment polyline within 200 m (point-to-polyline against the real highway
 * geometry). Proximity alone is not enough: a high-TDPA toll segment passing a
 * dense interchange would otherwise bleed onto every parallel/crossing local
 * street within range (CNOSSOS contract: "a residential street near a motorway
 * must not get motorway traffic"). So each SICT segment is gated to the OSM
 * classes its road type can plausibly be, from red_ok + operacion:
 *   Federal Cuota (autopista de peaje) → motorway(0) / trunk(1)
 *   Federal Libre (carretera federal)  → trunk(1) / primary(2)
 *   Estatal (carretera estatal)        → primary(2) / secondary(3)
 * The SICT U1 layer only contains federal + state highways, so a foreign or
 * low-order road also simply finds no candidate. Unmatched MX roads fall to the
 * engine's MX class defaults (defaults.rs, seeded from this dataset's medians).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-mx.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-mx.ts --enrich-only
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { parse } from 'csv-parse/sync'
import { Int32, Uint16, makeTable, vectorFromArray } from 'apache-arrow'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { SOURCE_ID_MX_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_MX_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/mx/sict-datosviales`)

// Mainland MX bbox — padded from the U1 geometry extent lat[14.6,32.7] lon[-117.1,-86.7].
// Used to gate hex iteration (H3 centroid) and per-segment midpoint.
const MX_BBOX: [number, number, number, number] = [14.3, -117.6, 32.9, -86.4]

// Match radius: OSM road midpoint → SICT highway polyline. 200 m absorbs the
// OSM-vs-SICT digitisation offset on the same road while limiting how far a
// count can jump to a parallel road; the class gate below does the rest.
const MATCH_RADIUS_M = 200
// Only attempt the proximity join for high-order classes (motorway..secondary);
// local roads (tertiary/residential/service) and links are not in the SICT
// network. Passed to writeRoadAadt as the centralized class gate AND declared as
// `roadCoverage` on the registry entry so the invariant scanner can verify it.
const COVERAGE: ReadonlySet<number> = new Set([0, 1, 2, 3])

// Allowed OSM road_class per SICT road type — the false-positive gate. A SICT
// segment can only stamp an OSM road whose class its type can plausibly be, so
// a toll-motorway TDPA can never land on a secondary street near an interchange.
// Bitmask over classes 0..3 (1<<class). road_class codes: 0 mw,1 trunk,2 pri,3 sec.
function allowedMask(redOk: string, operacion: string): number {
  const fed = redOk === 'Federal'
  const cuota = operacion === 'Cuota'
  if (fed && cuota) return (1 << 0) | (1 << 1)   // autopista de peaje → motorway/trunk
  if (fed) return (1 << 1) | (1 << 2)            // carretera federal libre → trunk/primary
  return (1 << 2) | (1 << 3)                     // carretera estatal → primary/secondary
}

// National TDPA-weighted composition — fallback if a segment lacks comp_* (none do today).
const NATIONAL_SPLIT = { light: 0.795, medium: 0.071, heavy: 0.082, moto: 0.052 }

interface SictSeg {
  coords: [number, number][]
  tdpa: number
  light: number   // class fractions (normalized to sum 1)
  medium: number
  heavy: number
  moto: number
  mask: number    // allowed OSM road_class bitmask (false-positive gate)
}

type Frac = { light: number; medium: number; heavy: number; moto: number }

function fnum(x: unknown): number {
  const n = typeof x === 'number' ? x : parseFloat(String(x ?? ''))
  return Number.isFinite(n) ? n : 0
}

/** comp_* → normalized CNOSSOS fractions. Falls back to the national split. */
function compToFrac(c: Record<string, string>): Frac {
  const light = fnum(c.comp_A) + fnum(c.comp_OTROS)
  const medium = fnum(c.comp_B) + fnum(c.comp_C2)
  const heavy = fnum(c.comp_C3) + fnum(c.comp_T3S2) + fnum(c.comp_T3S3) + fnum(c.comp_T3S2R4)
  const moto = fnum(c.comp_M)
  const sum = light + medium + heavy + moto
  if (sum <= 0) return { ...NATIONAL_SPLIT }
  return { light: light / sum, medium: medium / sum, heavy: heavy / sum, moto: moto / sum }
}

function loadComposition(): Map<string, Frac> {
  const path = resolve(CACHE_DIR, 'u1_segmentos.csv')
  const text = readFileSync(path, 'utf-8')
  const records = parse(text, { columns: true, skip_empty_lines: true }) as Record<string, string>[]
  const map = new Map<string, Frac>()
  for (const r of records) {
    if (r.segment_mongo_id) map.set(r.segment_mongo_id, compToFrac(r))
  }
  return map
}

function loadSegments(comp: Map<string, Frac>): SictSeg[] {
  const path = resolve(CACHE_DIR, 'u1_segmentos.geojson')
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: SictSeg[] = []
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g) continue
    let coords: [number, number][] = []
    if (g.type === 'LineString') coords = g.coordinates
    else if (g.type === 'MultiLineString') for (const part of g.coordinates) coords.push(...part)
    if (coords.length < 2) continue

    const p = f.properties || {}
    const tdpa = fnum(p.tdpa_2024)
    if (tdpa <= 0) continue
    const frac = comp.get(p.segment_mongo_id) ?? NATIONAL_SPLIT
    const mask = allowedMask(String(p.red_ok ?? ''), String(p.operacion ?? ''))
    out.push({ coords, tdpa, light: frac.light, medium: frac.medium, heavy: frac.heavy, moto: frac.moto, mask })
  }
  return out
}

// Spatial grid keyed at 0.01° (~1.1 km). We index a segment in every cell its
// geometry PASSES THROUGH, not just its vertex cells: SICT polylines have edges
// up to ~40 km between sparse vertices, and a vertex-only index leaves interior
// cells empty, so an OSM road sitting mid-edge (>1 cell from any vertex) is
// invisible to nearestSeg's 3×3 window and silently drops the match (caught by
// /gg: 8,681 false negatives + 143 wrong-nearest vs a candidate-complete index).
// Sampling each edge at 0.004° (< half a cell) guarantees no crossed cell is
// skipped, so the 3×3 search around any within-200 m receiver always sees it.
const GRID_STEP_DEG = 0.004
function buildLineGrid(features: SictSeg[]): Map<string, SictSeg[]> {
  const grid = new Map<string, SictSeg[]>()
  for (const feat of features) {
    const seen = new Set<string>()
    const addCell = (lat: number, lon: number) => {
      const key = `${Math.floor(lat * 100)}_${Math.floor(lon * 100)}`
      if (seen.has(key)) return
      seen.add(key)
      if (!grid.has(key)) grid.set(key, [])
      grid.get(key)!.push(feat)
    }
    const c = feat.coords
    for (let i = 0; i < c.length; i++) {
      const [lon, lat] = c[i]
      addCell(lat, lon)
      if (i + 1 < c.length) {
        const [lon2, lat2] = c[i + 1]
        const dLat = lat2 - lat, dLon = lon2 - lon
        const steps = Math.ceil(Math.max(Math.abs(dLat), Math.abs(dLon)) / GRID_STEP_DEG)
        for (let s = 1; s < steps; s++) addCell(lat + (dLat * s) / steps, lon + (dLon * s) / steps)
      }
    }
  }
  return grid
}

function nearestSeg(lat: number, lon: number, osmClass: number, grid: Map<string, SictSeg[]>, radiusM: number): SictSeg | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(lat * 100)
  const baseLon = Math.floor(lon * 100)
  let best: SictSeg | null = null
  let bestDist = radiusM
  for (let dy = -reach; dy <= reach; dy++) {
    for (let dx = -reach; dx <= reach; dx++) {
      const cell = grid.get(`${baseLat + dy}_${baseLon + dx}`)
      if (!cell) continue
      for (const feat of cell) {
        if (((feat.mask >> osmClass) & 1) === 0) continue   // type-incompatible
        const d = pointToPolylineDist(lat, lon, feat.coords)
        if (d < bestDist) { bestDist = d; best = feat }
      }
    }
  }
  return best
}

// Self-healing: clear this source's prior stamps before re-applying. The matcher
// is proximity-based and its params (radius, type gate, grid) get tuned across
// runs; writeRoadAadt only overwrites rows it re-matches and never un-stamps, so
// a row matched by an older logic but not the current one would keep a stale
// value. Zeroing our own rows (src == MY_SOURCE_ID — only this enricher writes
// 9484) up front makes every run reproduce the same result from any prior state.
async function clearStaleStamps(hexDirs: string[]): Promise<number> {
  let cleared = 0
  for (const hex of hexDirs) {
    const path = resolve(H3R4_DIR, hex, 'roads.arrow')
    if (!existsSync(path)) continue
    await withArrowWrite(path, (table) => {
      const n = table.numRows
      const exSrc = table.getChild('source_id')
      if (!exSrc) return table
      const exL = table.getChild('aadt_light'), exM = table.getChild('aadt_medium')
      const exH = table.getChild('aadt_heavy'), exMo = table.getChild('aadt_moto')
      const light = new Int32Array(n), medium = new Int32Array(n), heavy = new Int32Array(n), moto = new Int32Array(n), src = new Uint16Array(n)
      let any = false
      for (let i = 0; i < n; i++) {
        const s = (exSrc.get(i) as number) ?? 0
        if (s === MY_SOURCE_ID) { any = true; cleared++ }       // leave zeroed
        else { light[i] = (exL?.get(i) as number) ?? 0; medium[i] = (exM?.get(i) as number) ?? 0; heavy[i] = (exH?.get(i) as number) ?? 0; moto[i] = (exMo?.get(i) as number) ?? 0; src[i] = s }
      }
      if (!any) return table
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const cols: Record<string, any> = {}
      for (const f of table.schema.fields) {
        if (['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id'].includes(f.name)) continue
        cols[f.name] = table.getChild(f.name)!
      }
      cols['aadt_light'] = vectorFromArray(Array.from(light), new Int32()); cols['aadt_medium'] = vectorFromArray(Array.from(medium), new Int32())
      cols['aadt_heavy'] = vectorFromArray(Array.from(heavy), new Int32()); cols['aadt_moto'] = vectorFromArray(Array.from(moto), new Int32())
      cols['source_id'] = vectorFromArray(Array.from(src), new Uint16())
      return makeTable(cols)
    })
  }
  return cleared
}

async function main() {
  console.log(`=== MX Roads Enrichment — SICT/IMT Datos Viales 2025 U1 TDPA (${YEAR}) ===\n`)

  if (!existsSync(resolve(CACHE_DIR, 'u1_segmentos.geojson'))) {
    console.error(`Missing cache: ${CACHE_DIR}/u1_segmentos.geojson`)
    console.error(`Download from https://github.com/iChocko/Datos-Viales-SICT (geodata/geojson/light + csv).`)
    process.exit(1)
  }

  const comp = loadComposition()
  console.log(`  Composition rows (CSV):  ${comp.size}`)
  const segs = loadSegments(comp)
  console.log(`  U1 segments w/ TDPA:     ${segs.length}`)
  const grid = buildLineGrid(segs)
  console.log(`  Grid cells:              ${grid.size}\n`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, MX_BBOX)
  console.log(`  MX-bbox hexes with roads.arrow: ${hexDirs.length}`)

  const cleared = await clearStaleStamps(hexDirs)
  console.log(`  Cleared prior 9484 stamps:      ${cleared.toLocaleString()}`)

  let totalRoads = 0, alreadyEnriched = 0, matched = 0, hexesUpdated = 0
  // bbox alone bleeds into US/Guatemala/Belize border roads — a Mexican class-default
  // (priority 80) could overwrite US HPMS measured. Gate to the MX polygon (gg 2026-06-14).
  const inMX = makeCoastalCountryGate('MX')
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }
        if (!inBbox(row.midLat, row.midLon, MX_BBOX)) return null
        if (!inMX(row.midLat, row.midLon)) return null

        const seg = nearestSeg(row.midLat, row.midLon, row.roadClass, grid, MATCH_RADIUS_M)
        if (!seg) return null

        matched++
        return {
          light: Math.round(seg.tdpa * seg.light),
          medium: Math.round(seg.tdpa * seg.medium),
          heavy: Math.round(seg.tdpa * seg.heavy),
          moto: Math.round(seg.tdpa * seg.moto),
          sourceId: MY_SOURCE_ID,
        }
      },
      undefined,
      COVERAGE,
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${matched} matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:     ${totalRoads.toLocaleString()}`)
  console.log(`  Already enriched (skip): ${alreadyEnriched.toLocaleString()}`)
  console.log(`  Matched to SICT TDPA:    ${matched.toLocaleString()} (${(100 * matched / Math.max(totalRoads, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

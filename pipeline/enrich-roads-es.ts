/**
 * Enrich ES roads.arrow with MITMA Mapa de Tráfico 2022 (Spain national road census).
 *
 * Downloads tramos.js (JS-wrapped GeoJSON) from mapatrafico.transportes.gob.es,
 * parses 7,289 per-segment AADT records covering all autovías + autopistas + N-roads,
 * matches by ref + midpoint proximity, writes aadt_light/medium/heavy/moto + source_id.
 *
 * Source: https://mapatrafico.transportes.gob.es/2022/Visor/datos/GIS/tramos.js
 * License: open data Ministerio de Transportes y Movilidad Sostenible
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-es.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-es.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-es.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_ES_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_ES_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/es`)
const CACHE_TRAMOS = resolve(CACHE_DIR, 'mitma-tramos-2022.js')
const CACHE_JSON = resolve(CACHE_DIR, 'mitma-tramos-parsed-v2.json')   // v2: stores section `coords` polyline (was centroid midLat/midLon)

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

const TRAMOS_URL = 'https://mapatrafico.transportes.gob.es/2022/Visor/datos/GIS/tramos.js'

// Spain bbox (Iberian peninsula + Balearics + Canary islands)
const ES_BBOX: [number, number, number, number] = [27.0, -19.0, 44.0, 5.0]

// Same box in iterateCountryHexes order [minLat,minLon,maxLat,maxLon] (already the
// order ES_BBOX is declared in) — skips the rest of the planet's roads.arrow.
const ES_HEX_BBOX: [number, number, number, number] = ES_BBOX

// ── Types ──

interface TramoSection {
  via: string         // raw road ref e.g. "A-5", "AP-7", "N-340"
  ref: string         // normalized ref for matching
  coords: [number, number][]   // section polyline as [lon, lat] vertices (matched per-segment, not by centroid)
  pkInicio: number
  pkFin: number
  longitud: number
  clase: string       // road class (Autopista, Autovía, etc.)
  imdTot: number
  imdLig: number
  imdPes: number
  aadt_light: number
  aadt_medium: number
  aadt_heavy: number
  aadt_moto: number
}

// ── Step 1: Download MITMA tramos GeoJSON ──

async function downloadTramos(): Promise<TramoSection[]> {
  if (!forceDownload && existsSync(CACHE_JSON)) {
    console.log(`  Using cached parsed JSON: ${CACHE_JSON}`)
    return JSON.parse(readFileSync(CACHE_JSON, 'utf-8'))
  }

  mkdirSync(CACHE_DIR, { recursive: true })

  if (!forceDownload && existsSync(CACHE_TRAMOS)) {
    console.log(`  Using cached tramos.js: ${CACHE_TRAMOS}`)
  } else {
    if (enrichOnly) {
      throw new Error('--enrich-only but tramos.js not cached')
    }
    console.log(`  Downloading MITMA tramos (12.5 MB)...`)
    const res = await fetch(TRAMOS_URL, { signal: AbortSignal.timeout(120_000) })
    if (!res.ok) throw new Error(`HTTP ${res.status} for ${TRAMOS_URL}`)
    const buf = Buffer.from(await res.arrayBuffer())
    writeFileSync(CACHE_TRAMOS, buf)
    console.log(`  Cached: ${(buf.length / 1e6).toFixed(1)} MB`)
  }

  return parseTramos()
}

function parseTramos(): TramoSection[] {
  const raw = readFileSync(CACHE_TRAMOS, 'utf-8')
  const jsonStart = raw.indexOf('{')
  if (jsonStart < 0) throw new Error('No JSON object found in tramos.js')
  // Strip "var tramos_data = " prefix and any trailing semicolon
  let jsonText = raw.substring(jsonStart).trim()
  if (jsonText.endsWith(';')) jsonText = jsonText.slice(0, -1)
  const data = JSON.parse(jsonText)
  console.log(`  Parsed FeatureCollection: ${data.features.length} features`)

  const sections: TramoSection[] = []
  let skipped = 0

  for (const feat of data.features) {
    const props = feat.properties || {}
    const via = (props.via || '').toString().trim()
    const imdTot = parseFloat(props.imdtot || '0')
    const imdLig = parseFloat(props.imdlig || '0')
    const imdPes = parseFloat(props.imdpes || '0')

    // NaN-safe positive check: a few MITMA tramos carry a non-numeric imdtot;
    // `imdTot <= 0` let NaN through (NaN<=0 is false) and the match's ||0
    // fallbacks then stamped 0/0/0/0 under the measured id — the exact shape
    // the #31.4 writer guard rejects (it halted the world chain here).
    if (!via || !(imdTot > 0)) { skipped++; continue }

    // Flatten MultiLineString → [lon, lat] vertices; matched per-segment, not by centroid.
    const rawCoords = feat.geometry?.coordinates
    if (!rawCoords || !Array.isArray(rawCoords) || rawCoords.length === 0) { skipped++; continue }
    const lineCoords = (rawCoords as number[][][]).flat() as [number, number][]
    if (lineCoords.length === 0) { skipped++; continue }

    // bbox filter on the first vertex (sections don't meaningfully straddle the ES bbox edge)
    const [v0Lon, v0Lat] = lineCoords[0]
    if (v0Lat < ES_BBOX[0] || v0Lat > ES_BBOX[2] || v0Lon < ES_BBOX[1] || v0Lon > ES_BBOX[3]) {
      skipped++; continue
    }

    // Normalize ref: "A-5" → "A5", "AP-7" → "AP7", "N-340" → "N340"
    const ref = via.replace(/[-\s]/g, '').toUpperCase()

    // CNOSSOS vehicle split:
    // imdLig already excludes heavy. ~1% of total = motorcycles (CNOSSOS default).
    // imdPes (heavy) split into ~5% buses (medium) + 95% trucks (heavy).
    const motoFrac = 0.01
    const busFracOfHeavy = 0.05
    const aadt_moto = Math.round(imdTot * motoFrac)
    const aadt_medium = Math.round(imdPes * busFracOfHeavy)
    const aadt_heavy = Math.round(imdPes - aadt_medium)
    const aadt_light = Math.max(0, Math.round(imdLig - aadt_moto))

    sections.push({
      via,
      ref,
      coords: lineCoords,
      pkInicio: parseFloat(props.pkinicio_t || '0'),
      pkFin: parseFloat(props.pkfin_t || '0'),
      longitud: parseFloat(props.longitud || '0'),
      clase: props.clase || '',
      imdTot,
      imdLig,
      imdPes,
      aadt_light,
      aadt_medium,
      aadt_heavy,
      aadt_moto,
    })
  }

  console.log(`  Parsed: ${sections.length} sections, ${skipped} skipped (no via/imd/coords)`)
  writeFileSync(CACHE_JSON, JSON.stringify(sections))
  console.log(`  Cached parsed JSON: ${CACHE_JSON}`)
  return sections
}

// ── Step 2: Enrich roads.arrow files ──

async function enrichArrows(sections: TramoSection[]): Promise<void> {
  // Index sections by normalized ref
  const refIndex = new Map<string, TramoSection[]>()
  for (const s of sections) {
    if (!refIndex.has(s.ref)) refIndex.set(s.ref, [])
    refIndex.get(s.ref)!.push(s)
  }
  console.log(`\n  Ref index: ${refIndex.size} unique road refs`)
  console.log(`  Top 10 refs by section count:`)
  const refCounts = [...refIndex.entries()].map(([ref, list]) => [ref, list.length] as [string, number])
  refCounts.sort((a, b) => b[1] - a[1])
  for (const [ref, n] of refCounts.slice(0, 10)) console.log(`    ${ref}: ${n} sections`)

  // Pre-filter Spanish hexes via H3 center. ES_BBOX is already
  // [minLat,minLon,maxLat,maxLon], the order iterateCountryHexes expects.
  const hexDirs = iterateCountryHexes(H3R4_DIR, ES_HEX_BBOX)
  console.log(`  Spanish hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0
  let matched = 0
  let preserved = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hexId = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        // Priority gate: preserve existing if it has higher priority than MITMA.
        // (writeRoadAadt re-checks the gate — this only saves the ref-match work.)
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }

        const osmRef = (row.ref?.toString() || '').trim()
        // Spanish OSM refs use forms like "A-1", "AP-7", "N-340", "M-30"
        // Try original normalized + first part if multi-ref ("A-1;N-I")
        const normRef = osmRef.replace(/[-\s]/g, '').toUpperCase().split(';')[0]

        let best: TramoSection | null = null
        let bestDist = Infinity

        if (normRef && refIndex.has(normRef)) {
          for (const c of refIndex.get(normRef)!) {
            const dist = pointToPolylineDist(row.midLat, row.midLon, c.coords)
            if (dist < bestDist) { bestDist = dist; best = c }
          }
        }

        // Match within 30 km along ref (corridors are long)
        if (!best || bestDist >= 30_000) return null
        // NaN tramos are skipped at parse now; `|| 0` stays as a belt for a
        // single NaN class column. If EVERY class still lands on 0 (tiny
        // imdTot with NaN splits), skip — never stamp zeros under a measured
        // id (#31.4).
        const light = best.aadt_light || 0, medium = best.aadt_medium || 0
        const heavy = best.aadt_heavy || 0, moto = best.aadt_moto || 0
        if (light + medium + heavy + moto === 0) return null
        return { light, medium, heavy, moto, sourceId: MY_SOURCE_ID }
      },
      () => { matched++ },
    )
    totalSeg += r.rows
    if (r.updated) hexesUpdated++

    if (hi % 25 === 0 || hi === hexDirs.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexDirs.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Enrichment Results ===`)
  console.log(`  Total segments scanned: ${totalSeg}`)
  console.log(`  Preserved (continental): ${preserved}`)
  console.log(`  Newly matched: ${matched} (${(100 * matched / Math.max(totalSeg, 1)).toFixed(2)}%)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${hexDirs.length}`)

  // Top AADT corridors
  const top = [...sections].sort((a, b) => b.imdTot - a.imdTot).slice(0, 15)
  console.log(`\n  Top 15 AADT corridors (imd total):`)
  for (const s of top) {
    console.log(`    ${s.via.padEnd(8)} pk ${s.pkInicio.toFixed(1)}–${s.pkFin.toFixed(1)}  imdtot=${s.imdTot.toLocaleString().padStart(8)} pes=${(s.imdPes / s.imdTot * 100).toFixed(1)}%`)
  }
}

// ── Main ──

async function main() {
  console.log(`=== ES Road Traffic Enrichment — MITMA Mapa de Tráfico 2022 ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)
  }

  const sections = await downloadTramos()
  console.log(`\n  Total parsed sections: ${sections.length}`)

  // Class distribution
  const classCounts = new Map<string, number>()
  for (const s of sections) {
    const c = s.clase || 'Unknown'
    classCounts.set(c, (classCounts.get(c) || 0) + 1)
  }
  console.log(`  Class distribution:`)
  for (const [c, n] of [...classCounts.entries()].sort((a, b) => b[1] - a[1])) {
    console.log(`    ${c}: ${n}`)
  }

  await enrichArrows(sections)
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

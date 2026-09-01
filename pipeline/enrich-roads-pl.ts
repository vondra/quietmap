/**
 * Enrich PL roads.arrow with GDDKiA Generalny Pomiar Ruchu (GPR) 2020/2021 data.
 *
 * Sources (Generalna Dyrekcja Dróg Krajowych i Autostrad — GDDKiA):
 *   National roads SHP geometry: 2,290 segments, EPSG:2180 (auto-reprojected to WGS84 by shpjs)
 *   National roads AADT XLS:     2,290 measurement points with full vehicle class split
 *   Provincial roads (DW) AADT XLS: 3,124 segments (no geometry — matched by ref + proximity)
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-pl.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-pl.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-pl.ts --force-download
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { shouldOverwrite } from './lib/provenance.js'
import * as XLSX from 'xlsx'
import shp from 'shpjs'
import { SOURCE_ID_PL_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_PL_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/pl`)
const CACHE_NATIONAL_XLS = resolve(CACHE_DIR, 'gpr-2020-national.xls')
const CACHE_PROVINCIAL_XLS = resolve(CACHE_DIR, 'gpr-2020-provincial.xls')
const CACHE_SHP_ZIP = resolve(CACHE_DIR, 'gpr-2020-segments-shp.zip')
const CACHE_PARSED = resolve(CACHE_DIR, 'gpr-2020-parsed-v3.json')   // v3: header-detected columns (v2 mis-parsed provincial with national layout; see parseGprXls). v2: polyline coords.

const enrichOnly = process.argv.includes('--enrich-only')
const forceDownload = process.argv.includes('--force-download')

// GDDKiA URLs (gov.pl attachment IDs valid as of 2026-04)
const NATIONAL_XLS_URL = 'https://www.gov.pl/attachment/51021048-4885-4ced-a487-3b58cd513abc'
const PROVINCIAL_XLS_URL = 'https://www.gov.pl/attachment/bc113506-2cb6-45fb-9e83-7c8e60aa11fd'
const SHP_ZIP_URL = 'https://www.gov.pl/attachment/540a9afe-df60-4610-bc93-808a925e0ed0'

// Poland mainland bounding box. [minLat,minLon,maxLat,maxLon] — iterateCountryHexes
// skips the rest of the planet so the loader doesn't read every roads.arrow on Earth.
const PL_HEX_BBOX: [number, number, number, number] = [49.0, 14.0, 55.0, 24.5]

interface SegmentRecord {
  nr2020: string         // measurement point ID (joins to SHP)
  ref: string            // road number normalized (e.g. "A1", "S7", "DK1", "DW806")
  isProvincial: boolean
  midLat: number               // representative centroid — grid/display only (NaN for provincial)
  midLon: number
  coords: [number, number][] | null   // section polyline [lon,lat]; null for provincial (ref-only match)
  imdTot: number
  aadt_light: number     // cars + light commercial ≤3.5 t (CNOSSOS cat1)
  aadt_medium: number    // buses (CNOSSOS cat2)
  aadt_heavy: number     // trucks: no-trailer + with-trailer
  aadt_moto: number      // motorcycles
}

// ── Step 1: download cached files if missing ──

async function downloadFile(url: string, path: string, label: string): Promise<void> {
  if (!forceDownload && existsSync(path)) {
    console.log(`  Using cached ${label}: ${path}`)
    return
  }
  if (enrichOnly) throw new Error(`--enrich-only but ${label} not cached`)
  console.log(`  Downloading ${label}...`)
  const res = await fetch(url, { signal: AbortSignal.timeout(120_000), redirect: 'follow' })
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${label} (${url})`)
  const buf = Buffer.from(await res.arrayBuffer())
  writeFileSync(path, buf)
  console.log(`  Cached: ${(buf.length / 1e6).toFixed(2)} MB`)
}

async function downloadAll(): Promise<void> {
  mkdirSync(CACHE_DIR, { recursive: true })
  await downloadFile(NATIONAL_XLS_URL, CACHE_NATIONAL_XLS, 'GPR national XLS')
  await downloadFile(PROVINCIAL_XLS_URL, CACHE_PROVINCIAL_XLS, 'GPR provincial XLS')
  await downloadFile(SHP_ZIP_URL, CACHE_SHP_ZIP, 'GPR national SHP zip')
}

// ── Step 2: parse XLS ──

export interface XlsRecord {
  nr2020: string
  ref: string
  pikp: number
  pikk: number
  imdTot: number
  motorcycles: number
  cars: number
  lightTrucks: number
  trucksNoTrailer: number
  trucksWithTrailer: number
  buses: number
}

/**
 * Parse a GPR 2020 SDRR workbook (national or provincial) into per-point records.
 *
 * Columns are DETECTED FROM HEADER TEXT, not fixed indices: the national sheet
 * has extra "kraj."+"E" columns after "Numer drogi" that the provincial sheet
 * lacks, so a single fixed layout shifts every provincial value one column left
 * — cars landed in `motorcycles` (DW943: moto=1704 instead of 64; audit
 * 2026-06-10 R1). Header rows are the rows above the first numeric ID; needles
 * are diacritic-stripped substrings so "Sam. osob. i mikrobusy" (national) and
 * "Sam. osob. mikrobusy" (provincial) both match.
 */
export function parseGprXls(path: string, label: string): Map<string, XlsRecord> {
  const buf = readFileSync(path)
  const wb = XLSX.read(buf, { type: 'buffer' })
  const sheetName = wb.SheetNames.find(n => n.toLowerCase() === 'tab02') || wb.SheetNames[0]
  const sheet = wb.Sheets[sheetName]
  const range = XLSX.utils.decode_range(sheet['!ref']!)

  // Find data start (first row where col 0 is a number)
  let dataStart = -1
  for (let r = 0; r <= Math.min(30, range.e.r); r++) {
    const cell = sheet[XLSX.utils.encode_cell({ r, c: 0 })]
    if (cell && typeof cell.v === 'number') { dataStart = r; break }
  }
  if (dataStart < 0) throw new Error(`${label}: could not find data start row`)

  // Needles are matched against diacritic-stripped text, so the regex below must
  // strip ń→n, ż→z etc. (needles avoid ł, which doesn't NFD-decompose). Sheet-wide
  // banner rows ("ŚREDNI DOBOWY RUCH ROCZNY (SDRR)") live in column 0 and would
  // poison its text — only rows with ≥3 non-empty string cells are real headers.
  const headerRows: number[] = []
  for (let r = 0; r < dataStart; r++) {
    let nonEmpty = 0
    for (let c = range.s.c; c <= range.e.c; c++) {
      const v = sheet[XLSX.utils.encode_cell({ r, c })]?.v
      if (typeof v === 'string' && v.trim()) nonEmpty++
    }
    if (nonEmpty >= 3) headerRows.push(r)
  }
  const texts: string[] = []
  for (let c = range.s.c; c <= range.e.c; c++) {
    const parts: string[] = []
    for (const r of headerRows) {
      const v = sheet[XLSX.utils.encode_cell({ r, c })]?.v
      if (typeof v === 'string' && v.trim()) parts.push(v)
    }
    texts[c] = parts.join(' ').toLowerCase()
      .normalize('NFD').replace(/[\u0300-\u036f]/g, '')
      .replace(/\s+/g, ' ').trim()
  }
  const findCol = (needle: string, exclude?: string): number =>
    texts.findIndex(t => t && t.includes(needle) && (!exclude || !t.includes(exclude)))
  const cols = {
    id: findCol('numer punktu'),
    ref: findCol('numer drogi'),
    pikp: findCol('pikieta'),                    // "Opis odcinka / Pikietaż / pocz."
    pikk: findCol('konc'),                       // "końc." (NFD → konc)
    sdrr: findCol('sdrr'),
    motorcycles: findCol('motocykle'),
    cars: findCol('sam. osob'),
    lightTrucks: findCol('lekkie sam'),
    trucksNoTrailer: findCol('bez przycz'),
    trucksWithTrailer: findCol('z przycz', 'bez przycz'),  // "bez przycz." contains "z przycz."
    buses: findCol('autobus'),
  }
  const missing = Object.entries(cols).filter(([, c]) => c < 0).map(([k]) => k)
  if (missing.length > 0) {
    throw new Error(`${label}: header columns not found in sheet ${sheetName}: ${missing.join(', ')} — GPR layout drifted, refusing to guess indices`)
  }

  const map = new Map<string, XlsRecord>()
  for (let r = dataStart; r <= range.e.r; r++) {
    const get = (c: number) => sheet[XLSX.utils.encode_cell({ r, c })]?.v
    const num = (c: number) => {
      const v = get(c)
      if (typeof v === 'number') return v
      if (typeof v === 'string') {
        const cleaned = v.replace(',', '.').replace(/\s/g, '')
        if (cleaned === '-' || cleaned === '') return 0
        const n = parseFloat(cleaned)
        return isNaN(n) ? 0 : n
      }
      return 0
    }
    const id = get(cols.id)
    const road = get(cols.ref)
    if (id == null || road == null) continue

    const nr2020 = id.toString().trim()
    const refRaw = road.toString().trim()
    if (!nr2020 || !refRaw) continue

    // Normalize: "A 1" → "A1", "DK 7" → "DK7"; pure digits disambiguated later
    const ref = refRaw.replace(/\s+/g, '').toUpperCase()

    map.set(nr2020, {
      nr2020,
      ref,
      pikp: num(cols.pikp),
      pikk: num(cols.pikk),
      imdTot: num(cols.sdrr),
      motorcycles: num(cols.motorcycles),
      cars: num(cols.cars),
      lightTrucks: num(cols.lightTrucks),
      trucksNoTrailer: num(cols.trucksNoTrailer),
      trucksWithTrailer: num(cols.trucksWithTrailer),
      buses: num(cols.buses),
    })
  }

  // Scramble tripwire: GPR motorcycle share is ~1-2% nationwide; cars-in-moto
  // (the v2 provincial bug) pushes it past 50%. Threshold 15% leaves room for
  // genuinely moto-heavy files while catching any future column shift.
  let motoSum = 0
  let allSum = 0
  for (const rec of map.values()) {
    motoSum += rec.motorcycles
    allSum += rec.motorcycles + rec.cars + rec.lightTrucks + rec.trucksNoTrailer + rec.trucksWithTrailer + rec.buses
  }
  const motoShare = allSum > 0 ? motoSum / allSum : 0
  if (motoShare > 0.15) {
    throw new Error(`${label}: motorcycle share ${(100 * motoShare).toFixed(1)}% > 15% — column-scramble signature (GPR real share ≈1-2%)`)
  }

  console.log(`  ${label}: parsed ${map.size} measurement points (moto share ${(100 * motoShare).toFixed(2)}%)`)
  return map
}

// ── Step 3: load SHP and join with XLS ──

/** Neither the XLS join (independent columns — blank class fields under a
 *  published SDRR total) nor the SDRR-only fallback (total → class-share
 *  split, rounds to zero for a tiny total) can prove a non-zero split up
 *  front — filter once, here, rather than at each of the three push sites
 *  below (#31.4: writeRoadAadt rejects an all-zero payload under a measured
 *  id). Applied to BOTH the fresh-build result and the cached-JSON path
 *  (the cache may predate this filter). */
function dropZeroSplit(segments: SegmentRecord[], label: string): SegmentRecord[] {
  const usable = segments.filter((s) => s.aadt_light + s.aadt_medium + s.aadt_heavy + s.aadt_moto > 0)
  if (usable.length < segments.length) {
    console.log(`  ${label}: ${segments.length - usable.length} segments dropped (SDRR total without class split — cannot stamp zeros under a measured id)`)
  }
  return usable
}

async function buildSegments(): Promise<SegmentRecord[]> {
  if (!forceDownload && existsSync(CACHE_PARSED)) {
    console.log(`  Using cached parsed segments: ${CACHE_PARSED}`)
    const cached: SegmentRecord[] = JSON.parse(readFileSync(CACHE_PARSED, 'utf-8'))
    return dropZeroSplit(cached, 'cached')
  }

  console.log(`  Loading national SHP via shpjs...`)
  const shpBuf = readFileSync(CACHE_SHP_ZIP)
  const result = await shp(shpBuf)
  const fc = Array.isArray(result) ? result[0] : result
  console.log(`  SHP features: ${fc.features.length}`)

  console.log(`  Parsing national XLS...`)
  const nationalXls = parseGprXls(CACHE_NATIONAL_XLS, 'national')

  console.log(`  Parsing provincial XLS...`)
  const provincialXls = parseGprXls(CACHE_PROVINCIAL_XLS, 'provincial')

  const segments: SegmentRecord[] = []
  let nationalMatched = 0
  let nationalUnmatched = 0

  // National roads: join SHP geometry with XLS via NR2020
  for (const feat of fc.features) {
    const props = feat.properties || {}
    const nr2020 = (props.NR2020 || '').toString().trim()
    const xls = nationalXls.get(nr2020)
    if (!xls) {
      // Fall back to SDRR field in DBF (total only, no class split)
      const sdrr = parseFloat(props.SDRR || '0')
      if (sdrr > 0) {
        const coords = extractCentroid(feat.geometry)
        if (coords) {
          const refRaw = (props.NRDROGI || '').toString().trim().toUpperCase().replace(/\s+/g, '')
          const ref = /^[AS]/.test(refRaw) ? refRaw : `DK${refRaw}`
          // Synthetic class split using CNOSSOS defaults
          segments.push(makeFromTotal(nr2020, ref, coords[0], coords[1], extractLineCoords(feat.geometry), sdrr, false))
          nationalUnmatched++
        }
      }
      continue
    }

    const coords = extractCentroid(feat.geometry)
    if (!coords) { nationalUnmatched++; continue }

    // Determine ref prefix from SHP NRDROGI
    const refRaw = (props.NRDROGI || '').toString().trim().toUpperCase().replace(/\s+/g, '')
    let ref: string
    if (/^[AS]/.test(refRaw)) {
      ref = refRaw // motorways A1/S7 keep prefix
    } else if (/^\d/.test(refRaw)) {
      ref = `DK${refRaw}` // national road number prefixed
    } else {
      ref = refRaw
    }

    segments.push(makeRecord(nr2020, ref, coords[0], coords[1], extractLineCoords(feat.geometry), xls, false))
    nationalMatched++
  }

  console.log(`  National: ${nationalMatched} matched (XLS), ${nationalUnmatched} unmatched (SDRR-only fallback)`)

  // Provincial roads: NO geometry — emit records with empty coords for ref-only matching
  // We use a sentinel midLat=0/midLon=0 and rely on ref + OSM proximity (but here we need a position)
  // Strategy: skip provincial without coords; OSM ref-matching handles them by ref alone if midLat=NaN
  // For practical match, we use OSM-side approach: index OSM segments by ref, then for each provincial XLS
  // record, find any OSM segment with that ref and apply (lower priority than national).
  // To keep this script simple, we add provincial records with sentinel coordinates and match via ref.
  let provincialAdded = 0
  for (const xls of provincialXls.values()) {
    // Provincial refs need DW prefix
    const refClean = xls.ref.replace(/^DW/, '').replace(/[^0-9A-Z]/g, '')
    const ref = `DW${refClean}`
    segments.push(makeRecord(xls.nr2020, ref, NaN, NaN, null, xls, true))
    provincialAdded++
  }
  console.log(`  Provincial: ${provincialAdded} added (no geometry, ref-only matching)`)

  const usable = dropZeroSplit(segments, 'fresh-parsed')

  writeFileSync(CACHE_PARSED, JSON.stringify(usable))
  console.log(`  Cached ${usable.length} segments to ${CACHE_PARSED}`)
  return usable
}

function extractCentroid(geom: any): [number, number] | null {
  if (!geom || !geom.coordinates) return null
  const coords = geom.coordinates
  let sumLon = 0, sumLat = 0, n = 0
  if (geom.type === 'LineString') {
    for (const [lon, lat] of coords) { sumLon += lon; sumLat += lat; n++ }
  } else if (geom.type === 'MultiLineString') {
    for (const line of coords) {
      for (const [lon, lat] of line) { sumLon += lon; sumLat += lat; n++ }
    }
  } else return null
  if (n === 0) return null
  return [sumLat / n, sumLon / n]
}

/** Section polyline as [lon, lat] vertices (flattened); [] if no line geometry. */
function extractLineCoords(geom: any): [number, number][] {
  if (!geom || !geom.coordinates) return []
  if (geom.type === 'LineString') return geom.coordinates as [number, number][]
  if (geom.type === 'MultiLineString') return (geom.coordinates as number[][][]).flat() as [number, number][]
  return []
}

function makeRecord(nr2020: string, ref: string, lat: number, lon: number, coords: [number, number][] | null, xls: XlsRecord, isProvincial: boolean): SegmentRecord {
  // CNOSSOS-EU (Dir (EU) 2015/996) vehicle classes:
  //   light  = cars + light commercial ≤3.5 t (LGV)
  //   medium = buses
  //   heavy  = trucks_no_trailer + trucks_with_trailer
  //   moto   = motorcycles
  const aadt_light = Math.round(xls.cars + xls.lightTrucks)
  const aadt_medium = Math.round(xls.buses)
  const aadt_heavy = Math.round(xls.trucksNoTrailer + xls.trucksWithTrailer)
  const aadt_moto = Math.round(xls.motorcycles)
  return {
    nr2020,
    ref,
    isProvincial,
    midLat: lat,
    midLon: lon,
    coords,
    imdTot: xls.imdTot,
    aadt_light,
    aadt_medium,
    aadt_heavy,
    aadt_moto,
  }
}

function makeFromTotal(nr2020: string, ref: string, lat: number, lon: number, coords: [number, number][] | null, total: number, isProvincial: boolean): SegmentRecord {
  // CNOSSOS default split for Polish motorways/national roads when only SDRR is known:
  // light=72%, medium=8%, heavy=18%, moto=2%
  const aadt_moto = Math.round(total * 0.02)
  const aadt_medium = Math.round(total * 0.08)
  const aadt_heavy = Math.round(total * 0.18)
  const aadt_light = Math.round(total - aadt_moto - aadt_medium - aadt_heavy)
  return {
    nr2020,
    ref,
    isProvincial,
    midLat: lat,
    midLon: lon,
    coords,
    imdTot: total,
    aadt_light,
    aadt_medium,
    aadt_heavy,
    aadt_moto,
  }
}

// ── Step 4: enrich roads.arrow ──

async function enrichArrows(segments: SegmentRecord[]): Promise<void> {
  // Poland's bbox blankets ~all of Czechia, and a no-geometry provincial (DW) road
  // is accepted by ref alone — so without a country gate a Czech "I/150" was matched
  // to Polish "DW150" and given Polish AADT (150k segments, Stage-3 audit). Only
  // roads whose midpoint is on Polish soil get PL data. Created here, not at module
  // scope: makeCoastalCountryGate may download+convert the CGAZ boundary file, and the test file
  // imports parseGprXls from this module.
  const inPoland = makeCoastalCountryGate('PL')

  // Index by ref
  const refIndex = new Map<string, SegmentRecord[]>()
  for (const s of segments) {
    if (!refIndex.has(s.ref)) refIndex.set(s.ref, [])
    refIndex.get(s.ref)!.push(s)
  }
  console.log(`\n  Ref index: ${refIndex.size} unique refs`)

  // Pre-filter Polish hexes
  const hexDirs = iterateCountryHexes(H3R4_DIR, PL_HEX_BBOX)
  console.log(`  Polish hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalSeg = 0
  let matched = 0
  let preserved = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  // GPR covers national (A/S/DK → motorway..tertiary) + provincial DW roads
  // (≈ secondary/tertiary). Same coverage set as US HPMS: majors + links only,
  // so a stray ref collision can never stamp a residential/service street
  // (/gg W5 — writeRoadAadt's gate is opt-in, PL previously passed none).
  const PL_COVERAGE: ReadonlySet<number> = new Set([0, 1, 2, 3, 4, 10, 11, 12])

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hexId = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hexId, 'roads.arrow'),
      (row) => {
        totalSeg++
        // Priority gate: preserve existing if it has higher priority than self.
        // (writeRoadAadt re-checks the gate — this fast-exits before the ref match.)
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) {
          preserved++
          return null
        }

        const osmRefRaw = (row.ref?.toString() || '').trim().toUpperCase()
        // Polish OSM refs: "A1", "S7", "DK 1", "DW 806", "85"
        // Try several normalizations, take first ref if multi (";"-separated)
        const firstRef = osmRefRaw.split(';')[0].replace(/\s+/g, '')
        const candidates: string[] = [firstRef]
        // If pure number, also try DK prefix
        if (/^\d+$/.test(firstRef)) candidates.push(`DK${firstRef}`, `DW${firstRef}`)

        let best: SegmentRecord | null = null
        let bestDist = Infinity

        for (const cand of candidates) {
          if (!refIndex.has(cand)) continue
          for (const s of refIndex.get(cand)!) {
            // isProvincial is the no-geometry sentinel — NOT Number.isNaN(midLat):
            // the parsed cache goes through JSON, which turns NaN into null, and
            // isNaN(null) is false — cached reruns would silently drop every DW
            // match (Codex review of C1).
            if (s.isProvincial) {
              // Provincial road without geometry — match by ref alone if no national hit yet
              if (!best || best.isProvincial) {
                best = s
                bestDist = 5_000_000 // sentinel — any national polyline hit beats it
              }
              continue
            }
            const d = pointToPolylineDist(row.midLat, row.midLon, s.coords ?? [])
            if (d < bestDist) { bestDist = d; best = s }
          }
        }

        // National needs its polyline within 30 km; provincial is ref-only
        // (clamped to Polish soil below).
        if (best && (best.isProvincial || bestDist < 30_000)) {
          // The provincial branch above accepts a DW match by ref ALONE (no geometry,
          // any distance); clamp every match to Polish soil so a Czech "150" can't
          // keep "DW150" data.
          if (!inPoland(row.midLat, row.midLon)) return null
          return {
            light: best.aadt_light, medium: best.aadt_medium,
            heavy: best.aadt_heavy, moto: best.aadt_moto, sourceId: MY_SOURCE_ID,
          }
        }
        return null
      },
      () => { matched++ },
      PL_COVERAGE,
    )
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

  // Top corridors
  const top = [...segments]
    .filter(s => !s.isProvincial && s.imdTot > 0)
    .sort((a, b) => b.imdTot - a.imdTot)
    .slice(0, 15)
  console.log(`\n  Top 15 AADT corridors:`)
  for (const s of top) {
    console.log(`    ${s.ref.padEnd(8)} ${s.nr2020}  imdtot=${s.imdTot.toLocaleString().padStart(7)} heavy=${(s.aadt_heavy / s.imdTot * 100).toFixed(1)}%`)
  }
}

// ── Main ──

async function main() {
  console.log(`=== PL Road Traffic Enrichment — GDDKiA GPR 2020/2021 ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) throw new Error(`H3R4 directory not found: ${H3R4_DIR}`)

  await downloadAll()
  const segments = await buildSegments()

  // Stats
  const refs = new Set(segments.map(s => s.ref))
  const refPrefixes = new Map<string, number>()
  for (const s of segments) {
    const prefix = s.ref.match(/^[A-Z]+/)?.[0] || '?'
    refPrefixes.set(prefix, (refPrefixes.get(prefix) || 0) + 1)
  }
  console.log(`\n  Total parsed segments: ${segments.length}`)
  console.log(`  Unique refs: ${refs.size}`)
  console.log(`  Ref prefix distribution:`)
  for (const [p, n] of [...refPrefixes.entries()].sort((a, b) => b[1] - a[1])) {
    console.log(`    ${p}: ${n}`)
  }

  await enrichArrows(segments)
  console.log(`\n=== Done ===`)
}

// Run only when invoked directly — the test file imports parseGprXls.
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}

/**
 * TH from-to name matcher (M6, plan v3.2 §3 ladder level 2) — claims ref-LESS
 * OSM segments whose from-to name uniquely identifies a DRR 2024 census row
 * (`pipeline/lib/th-road-names.ts` does the matching; this script is the
 * I/O: census cache, hex scan, held-out verify gate, and the write).
 *
 * What it does, in order:
 *
 *   1. Loads the DRR 2024 census from the SAME cache as enrich-roads-th.ts
 *      (`data/enrichment/<year>/th/drr-aadt-2024.csv`, MOT CKAN mirror; same
 *      download URL + `--force-download`). Parsed with csv-parse (proper
 *      quoting): the legacy naive splitter in enrich-roads-th.ts silently
 *      drops the 12 rows whose road_name wraps a quoted newline — this
 *      loader recovers them for the name layer (their refs stay unmatchable
 *      for the ref enricher; counted in the report).
 *
 *   2. Scans TH-bbox hexes (same bbox + exclusion zones as the ref
 *      enricher) ONCE and builds:
 *        - province extents: per road_code prefix (สฎ., กจ., …) a robust
 *          bbox over the midpoints of OSM segments carrying that exact ref
 *          prefix — the DATA-DERIVED province gate (no hand table);
 *        - the verify set: class-compatible (2/3/4) segments WITH a ref
 *          (truth known via the ref);
 *        - the production set: class-compatible segments with NO ref and a
 *          name.
 *
 *   3. `--verify` (default): runs the matcher over the verify set with the
 *      ref hidden. Truth = exact census ref (same semantics as the ref
 *      enricher). Reports TP/FP/FN/TN, precision, recall, ambiguity stats,
 *      and sample false positives/negatives. Refs not in the census (DOH
 *      numeric refs like 4171, unknown DRR refs) are collision negatives:
 *      any claim on them is a false positive. **Precision ≥ 0.98 is the
 *      acceptance gate** (plan M6.2: held-out precision/recall + collision
 *      negatives) — `--write` re-runs this in memory and refuses below it.
 *
 *   4. `--claims`: verify + compute all production claims (no writes) and
 *      print them for review (first 20 in detail + full JSON to
 *      /tmp/th-names-claims-<year>.json).
 *
 *   5. `--write`: verify gate → claims → write with the TH national source
 *      id (SOURCE_ID_TH_NATIONAL_ROADS — the matched names ARE the same DRR
 *      census, same measured tier). Post-match conservation passes drop a
 *      census row's claims entirely (loud) when (a) two different OSM place
 *      sets claim the same row, or (b) one row's claimed segments span
 *      > 0.35° — conflicting geometry is a miss, never a guess. Rows
 *      already stamped by an equal/higher measured source are untouched
 *      (shouldOverwrite), ref-having segments are never touched (the ref
 *      enricher's domain), and writeRoadAadt's built-in national-ownership
 *      gate refuses any segment wholly outside TH.
 *
 * No retract arm tonight: first deployment, nothing of ours to heal; re-runs
 * are idempotent (existing === self ⇒ overwrite). Add the retract when the
 * matcher rules change.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-th-names.ts            # = --verify
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-th-names.ts --claims   # dry-run claims
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-th-names.ts --write    # gated write
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-th-names.ts --force-download --verify
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { parse } from 'csv-parse/sync'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_TH_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'
import {
  buildCensusRows,
  buildPlaceIndex,
  buildPrefixExtents,
  classCompatible,
  extractOsmPlaces,
  matchByName,
  type DrrCensusRow,
  type Extent,
} from './lib/th-road-names.js'

const MY_SOURCE_ID = SOURCE_ID_TH_NATIONAL_ROADS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/th`)

const MODE = process.argv.includes('--write') ? 'write' : process.argv.includes('--claims') ? 'claims' : 'verify'
const forceDownload = process.argv.includes('--force-download')

/** Held-out precision required before any write happens (plan M6.2). */
const PRECISION_GATE = 0.98
/** Evidence floor for the gate: at 0 false positives the one-sided 95%
 *  Clopper–Pearson bound only exceeds 98% from ~150 claims up, so a smaller
 *  0-FP sample proves nothing ("no FPs observed" ≠ "98% guaranteed", /gg M6
 *  #7). --write refuses below either bar. */
const PRECISION_GATE_MIN_CLAIMS = 150
/** A census row's claimed segments spanning more than this (degrees, either
 *  axis) is conflicting geometry — the row's claims are dropped. */
const MAX_ROW_SPREAD_DEG = 0.35
/** Production only touches these classes (matcher gates further by DRR number). */
const COVERAGE: ReadonlySet<number> = new Set([2, 3, 4])

// Kept in sync with enrich-roads-th.ts (same territory, same exclusions).
const TH_BBOX: [number, number, number, number] = [5.5, 97.3, 20.5, 105.7]
const TH_HEX_BBOX: [number, number, number, number] = TH_BBOX
const EXCLUDE_ZONES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Myanmar', bbox: [10.0, 97.3, 20.5, 98.3] },
  { name: 'Laos', bbox: [17.9, 101.0, 22.5, 106.0] },
  { name: 'Cambodia', bbox: [10.0, 103.3, 14.7, 107.0] },
  { name: 'Vietnam', bbox: [8.0, 104.5, 23.5, 109.5] },
  { name: 'Malaysia', bbox: [1.0, 99.5, 6.3, 104.5] },
]

function inAnyZone(lat: number, lon: number): boolean {
  for (const z of EXCLUDE_ZONES) if (inBbox(lat, lon, z.bbox)) return true
  return false
}

// ── Census load (same cache + URL as enrich-roads-th.ts; proper CSV parser) ──

interface Census {
  rows: DrrCensusRow[]
  byCode: Map<string, DrrCensusRow>
  totalRecords: number
  noSplit: number
  noPlaces: number
}

async function loadCensus(): Promise<Census> {
  const path = resolve(CACHE_DIR, 'drr-aadt-2024.csv')
  if (!existsSync(path) || forceDownload) {
    mkdirSync(CACHE_DIR, { recursive: true })
    const url = 'https://datagov.mot.go.th/datastore/dump/d0675c68-510b-45e1-b865-1ce261814948?format=csv'
    console.log(`  Downloading DRR 2024 AADT via MOT CKAN datastore...`)
    const res = await fetch(url, { signal: AbortSignal.timeout(60_000), headers: { 'User-Agent': 'Mozilla/5.0' } })
    if (!res.ok) throw new Error(`DRR CKAN HTTP ${res.status}`)
    writeFileSync(path, Buffer.from(await res.arrayBuffer()))
  }
  const records = parse(readFileSync(path, 'utf-8'), {
    columns: true,
    skip_empty_lines: true,
    // The file is LF-only EXCEPT its header line, which ends CRLF (CKAN dump
    // quirk) — and csv-parse's delimiter auto-detection misreads it (whole
    // file collapses into one 54k-column record). Pin '\n' and trim the
    // stray '\r' off unquoted fields (quoted multiline road_names are not
    // trimmed by csv-parse, so their content survives).
    record_delimiter: '\n',
    trim: true,
  }) as Record<string, string>[]
  const { rows, noSplit, noPlaces } = buildCensusRows(records)
  const byCode = new Map(rows.map(r => [r.code, r]))
  return { rows, byCode, totalRecords: records.length, noSplit, noPlaces }
}

// ── Hex scan ──

interface SegCandidate {
  hex: string
  idx: number
  osmId: number | null
  name: string
  roadClass: number
  midLat: number
  midLon: number
  places: Set<string>
}

interface VerifySeg extends SegCandidate {
  ref: string
}

interface ScanResult {
  extents: Map<string, Extent>
  prefixesWithoutExtent: Set<string>
  verify: VerifySeg[]
  candidates: SegCandidate[]
  rowsScanned: number
}

function scan(hexDirs: string[], censusPrefixes: Set<string>): ScanResult {
  const prefixPoints = new Map<string, Array<readonly [number, number]>>()
  const verify: VerifySeg[] = []
  const candidates: SegCandidate[] = []
  let rowsScanned = 0
  const thaiPrefixRef = /^([\u0E01-\u0E5B]{1,3})\.\d+$/

  for (const hex of hexDirs) {
    const t = tableFromIPC(readFileSync(resolve(H3R4_DIR, hex, 'roads.arrow')))
    const n = t.numRows
    rowsScanned += n
    if (n === 0) continue
    const refCol = t.getChild('ref')
    const nameCol = t.getChild('name')
    const clsCol = t.getChild('road_class')
    const sLa = t.getChild('start_lat')!
    const sLo = t.getChild('start_lon')!
    const eLa = t.getChild('end_lat')
    const eLo = t.getChild('end_lon')
    const osmIdCol = t.getChild('osm_id')
    for (let i = 0; i < n; i++) {
      const startLat = sLa.get(i) as number
      const startLon = sLo.get(i) as number
      const endLat = (eLa?.get(i) as number) ?? startLat
      const endLon = (eLo?.get(i) as number) ?? startLon
      const midLat = (startLat + endLat) / 2
      const midLon = (startLon + endLon) / 2
      if (!inBbox(midLat, midLon, TH_BBOX) || inAnyZone(midLat, midLon)) continue
      const ref = ((refCol?.get(i) as string | null) ?? '').trim()
      const roadClass = (clsCol?.get(i) as number) ?? 5

      if (ref) {
        // Province-extent evidence: any Thai-prefix ref locates its prefix.
        const m = thaiPrefixRef.exec(ref)
        if (m && censusPrefixes.has(m[1])) {
          const arr = prefixPoints.get(m[1])
          if (arr) arr.push([midLat, midLon])
          else prefixPoints.set(m[1], [[midLat, midLon]])
        }
        // Verify set: class-compatible, ref-having (name optional — unnamed
        // ref'd rows are honest recall misses).
        if (COVERAGE.has(roadClass)) {
          const name = ((nameCol?.get(i) as string | null) ?? '').trim()
          verify.push({
            hex,
            idx: i,
            osmId: osmIdCol ? Number(osmIdCol.get(i)) : null,
            name,
            roadClass,
            midLat,
            midLon,
            places: name ? extractOsmPlaces(name) : new Set(),
            ref,
          })
        }
        continue
      }

      // Production set: ref-less, named, class-compatible.
      if (!COVERAGE.has(roadClass)) continue
      const name = ((nameCol?.get(i) as string | null) ?? '').trim()
      if (!name) continue
      candidates.push({
        hex,
        idx: i,
        osmId: osmIdCol ? Number(osmIdCol.get(i)) : null,
        name,
        roadClass,
        midLat,
        midLon,
        places: extractOsmPlaces(name),
      })
    }
  }

  const extents = buildPrefixExtents(prefixPoints)
  const prefixesWithoutExtent = new Set([...censusPrefixes].filter(p => !extents.has(p)))
  return { extents, prefixesWithoutExtent, verify, candidates, rowsScanned }
}

// ── Verify ──

interface VerifyReport {
  precision: number
  recall: number
  tp: number
  fp: number
  fn: number
  tn: number
  ambiguous: number
  namedFn: number
  namedTotal: number
}

function runVerify(census: Census, scanRes: ScanResult): VerifyReport {
  const placeIndex = buildPlaceIndex(census.rows)
  let tp = 0, fp = 0, fn = 0, tn = 0, ambiguous = 0, namedFn = 0, namedTotal = 0
  const fpSamples: string[] = []
  const fnSamples: string[] = []
  for (const seg of scanRes.verify) {
    const truth = census.byCode.get(seg.ref) ?? null
    if (seg.places.size > 0) namedTotal++
    const m = matchByName(seg.places, seg.roadClass, seg.midLat, seg.midLon, placeIndex, scanRes.extents)
    if (m.status === 'ambiguous') ambiguous++
    const claim = m.status === 'match' ? m.row : null
    if (claim && truth && claim.code === truth.code) tp++
    else if (claim) {
      fp++
      if (fpSamples.length < 20) {
        fpSamples.push(
          `    FP osm=${seg.osmId} ref=${seg.ref} truth=${truth?.code ?? 'none'} claim=${claim.code} name=${JSON.stringify(seg.name)} @${seg.midLat.toFixed(4)},${seg.midLon.toFixed(4)}`,
        )
      }
    } else if (truth) {
      fn++
      if (seg.places.size > 0) {
        namedFn++
        if (fnSamples.length < 20) {
          fnSamples.push(
            `    FN osm=${seg.osmId} ref=${seg.ref} truth=${truth.code} truthName=${JSON.stringify(truth.name)} name=${JSON.stringify(seg.name)}`,
          )
        }
      }
    } else tn++
  }
  const precision = tp + fp > 0 ? tp / (tp + fp) : 0
  const recall = tp + fn > 0 ? tp / (tp + fn) : 0
  console.log(`\n=== VERIFY (held-out over ${scanRes.verify.length.toLocaleString()} ref-having class-2/3/4 segments) ===`)
  console.log(`  TP=${tp.toLocaleString()} FP=${fp.toLocaleString()} FN=${fn.toLocaleString()} TN=${tn.toLocaleString()} ambiguous=${ambiguous.toLocaleString()}`)
  console.log(`  precision = ${(precision * 100).toFixed(2)}% (gate ≥ ${(PRECISION_GATE * 100).toFixed(0)}% AND ≥ ${PRECISION_GATE_MIN_CLAIMS} true claims)`)
  console.log(`  recall    = ${(recall * 100).toFixed(2)}% of all census-ref segments (${namedFn.toLocaleString()} of the misses are named)`)
  if (fpSamples.length) console.log(`  False-positive samples:\n${fpSamples.join('\n')}`)
  if (fnSamples.length) console.log(`  Named false-negative samples:\n${fnSamples.join('\n')}`)
  return { precision, recall, tp, fp, fn, tn, ambiguous, namedFn, namedTotal }
}

// ── Claims (production) ──

interface Claim extends SegCandidate {
  row: DrrCensusRow
}

function computeClaims(census: Census, scanRes: ScanResult): { claims: Claim[]; ambiguous: number; droppedConflict: number; droppedSpread: number } {
  const placeIndex = buildPlaceIndex(census.rows)
  const claims: Claim[] = []
  let ambiguous = 0
  for (const seg of scanRes.candidates) {
    const m = matchByName(seg.places, seg.roadClass, seg.midLat, seg.midLon, placeIndex, scanRes.extents)
    if (m.status === 'ambiguous') {
      ambiguous++
      if (ambiguous <= 50) console.log(`  AMBIGUOUS (no match): osm=${seg.osmId} name=${JSON.stringify(seg.name)} candidates=${m.codes.join(',')}`)
    } else if (m.status === 'match') {
      claims.push({ ...seg, row: m.row })
    }
  }

  // Conservation pass 1: one census row claimed by >1 DISTINCT OSM place
  // sets = conflicting names → drop the row's claims entirely (loud).
  const sigsByCode = new Map<string, Set<string>>()
  for (const c of claims) {
    const sig = [...c.places].sort().join('|')
    const s = sigsByCode.get(c.row.code)
    if (s) s.add(sig)
    else sigsByCode.set(c.row.code, new Set([sig]))
  }
  const conflictCodes = new Set([...sigsByCode].filter(([, s]) => s.size > 1).map(([code]) => code))
  let kept = claims.filter(c => !conflictCodes.has(c.row.code))
  const droppedConflict = claims.length - kept.length
  for (const code of conflictCodes) {
    console.log(`  CONFLICT (row dropped): ${code} claimed by ${sigsByCode.get(code)!.size} distinct OSM place sets: ${[...sigsByCode.get(code)!].slice(0, 4).join(' ; ')}`)
  }

  // Conservation pass 2: one census row's claimed segments spanning
  // > MAX_ROW_SPREAD_DEG = conflicting geometry → drop the row's claims.
  const byCode = new Map<string, Claim[]>()
  for (const c of kept) {
    const arr = byCode.get(c.row.code)
    if (arr) arr.push(c)
    else byCode.set(c.row.code, [c])
  }
  const spreadCodes = new Set<string>()
  for (const [code, arr] of byCode) {
    let minLa = Infinity, minLo = Infinity, maxLa = -Infinity, maxLo = -Infinity
    for (const c of arr) {
      minLa = Math.min(minLa, c.midLat); maxLa = Math.max(maxLa, c.midLat)
      minLo = Math.min(minLo, c.midLon); maxLo = Math.max(maxLo, c.midLon)
    }
    if (maxLa - minLa > MAX_ROW_SPREAD_DEG || maxLo - minLo > MAX_ROW_SPREAD_DEG) {
      spreadCodes.add(code)
      console.log(`  SPREAD (row dropped): ${code} claims span ${(maxLa - minLa).toFixed(2)}° lat × ${(maxLo - minLo).toFixed(2)}° lon across ${arr.length} segments`)
    }
  }
  const finalClaims = kept.filter(c => !spreadCodes.has(c.row.code))
  const droppedSpread = kept.length - finalClaims.length
  kept = finalClaims

  return { claims: kept, ambiguous, droppedConflict, droppedSpread }
}

// ── Main ──

async function main() {
  console.log(`=== TH Roads — DRR from-to NAME matcher (M6) mode=${MODE} year=${YEAR} ===\n`)

  const census = await loadCensus()
  console.log(`  Census: ${census.totalRecords} records → ${census.rows.length} name-matchable rows (≥2 places + class split); ${census.noSplit} no-split, ${census.noPlaces} without a place pair`)

  const hexDirs = iterateCountryHexes(H3R4_DIR, TH_HEX_BBOX)
  console.log(`  TH-bbox hexes with roads.arrow: ${hexDirs.length}`)

  const t0 = Date.now()
  const censusPrefixes = new Set(census.rows.map(r => r.prefix))
  const scanRes = scan(hexDirs, censusPrefixes)
  console.log(`  Scan: ${scanRes.rowsScanned.toLocaleString()} rows in ${((Date.now() - t0) / 1000).toFixed(1)}s`)
  console.log(`  Province extents: ${scanRes.extents.size}/${censusPrefixes.size} prefixes covered; without OSM ref coverage: ${[...scanRes.prefixesWithoutExtent].join(' ') || 'none'} (name matching disabled there)`)
  console.log(`  Verify set: ${scanRes.verify.length.toLocaleString()} ref-having segments; production set: ${scanRes.candidates.length.toLocaleString()} named ref-less segments`)

  const report = runVerify(census, scanRes)
  const gateOk = report.precision >= PRECISION_GATE && report.tp >= PRECISION_GATE_MIN_CLAIMS
  console.log(`\n  Verify gate: ${gateOk ? 'PASS' : 'FAIL'} (precision ${(report.precision * 100).toFixed(2)}% vs ≥ ${(PRECISION_GATE * 100).toFixed(0)}%; claims ${report.tp.toLocaleString()} vs ≥ ${PRECISION_GATE_MIN_CLAIMS})`)

  if (MODE === 'verify') {
    if (!gateOk) process.exitCode = 1
    return
  }
  if (!gateOk && MODE !== 'claims') {
    // Insufficient evidence is a SAFE NO-OP, not a chain failure (/gg M6
    // Codex: a nonzero exit here aborts the whole chain before later
    // phases). The matcher publishes nothing until the gate passes; the
    // message is the alarm. (--claims stays a dry-run below the gate.)
    console.log(`\n  Insufficient evidence for ${MODE} — writing 0 rows (matcher unpublished until the gate passes; precision ${(report.precision * 100).toFixed(2)}% ≥ ${(PRECISION_GATE * 100).toFixed(0)}%, claims ${report.tp.toLocaleString()} ≥ ${PRECISION_GATE_MIN_CLAIMS})`)
    console.log(`\n=== Results ===`)
    console.log(`  ${scanRes.rowsScanned.toLocaleString()} rows scanned, 0 files updated (gate not met)`)
    return
  }

  const { claims, ambiguous, droppedConflict, droppedSpread } = computeClaims(census, scanRes)
  console.log(`\n=== Claims (production) ===`)
  console.log(`  Candidates: ${scanRes.candidates.length.toLocaleString()} named ref-less segments`)
  console.log(`  Ambiguous (no match): ${ambiguous.toLocaleString()}`)
  console.log(`  Dropped by name-conflict conservation: ${droppedConflict.toLocaleString()} segments`)
  console.log(`  Dropped by geometry-spread conservation: ${droppedSpread.toLocaleString()} segments`)
  console.log(`  Final claims: ${claims.length.toLocaleString()} segments across ${new Set(claims.map(c => c.row.code)).size} census rows`)

  const perClass = new Map<number, number>()
  for (const c of claims) perClass.set(c.roadClass, (perClass.get(c.roadClass) ?? 0) + 1)
  console.log(`  Claims by OSM class: ${[...perClass].sort().map(([k, v]) => `cls${k}=${v.toLocaleString()}`).join(' ')}`)

  // Phangan anchor hex detail (the milestone's A/B anchor).
  const phangan = claims.filter(c => c.hex === '846436bffffffff')
  console.log(`  Phangan hex 846436bffffffff claims: ${phangan.length}`)
  for (const c of phangan.slice(0, 10)) {
    console.log(`    osm=${c.osmId} cls=${c.roadClass} name=${JSON.stringify(c.name)} → ${c.row.code} (${c.row.name}) aadt=${c.row.light}/${c.row.medium}/${c.row.heavy}/${c.row.moto}`)
  }

  console.log(`\n  First 20 claims (review before --write):`)
  for (const c of claims.slice(0, 20)) {
    console.log(`    ${c.hex} osm=${c.osmId} cls=${c.roadClass} name=${JSON.stringify(c.name)} → ${c.row.code} "${c.row.name}" ${c.row.light}/${c.row.medium}/${c.row.heavy}/${c.row.moto} @${c.midLat.toFixed(4)},${c.midLon.toFixed(4)}`)
  }
  const claimsPath = `/tmp/th-names-claims-${YEAR}.json`
  writeFileSync(claimsPath, JSON.stringify(claims.map(c => ({
    hex: c.hex, idx: c.idx, osmId: c.osmId, cls: c.roadClass, name: c.name,
    code: c.row.code, censusName: c.row.name,
    aadt: [c.row.light, c.row.medium, c.row.heavy, c.row.moto],
    mid: [c.midLat, c.midLon],
  })), null, 1))
  console.log(`  Full claims list: ${claimsPath}`)

  if (MODE === 'claims') return

  // ── Write ──
  const byHex = new Map<string, Claim[]>()
  for (const c of claims) {
    const arr = byHex.get(c.hex)
    if (arr) arr.push(c)
    else byHex.set(c.hex, [c])
  }
  let written = 0, hexesUpdated = 0, alreadyEnriched = 0, foreignSkipped = 0
  for (const [hex, hexClaims] of byHex) {
    const byIdx = new Map(hexClaims.map(c => [c.idx, c]))
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row, i) => {
        const c = byIdx.get(i)
        if (!c) return null
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { alreadyEnriched++; return null }
        return { light: c.row.light, medium: c.row.medium, heavy: c.row.heavy, moto: c.row.moto, sourceId: MY_SOURCE_ID }
      },
      undefined,
      COVERAGE,
    )
    written += r.matched
    foreignSkipped += r.skippedForeign
    if (r.updated) hexesUpdated++
  }
  console.log(`\n=== Write ===`)
  console.log(`  Rows written: ${written.toLocaleString()} (source_id ${MY_SOURCE_ID}, same DRR census measured tier)`)
  console.log(`  Hexes updated: ${hexesUpdated}/${byHex.size}`)
  if (alreadyEnriched) console.log(`  Already owned by equal/higher source (skipped): ${alreadyEnriched.toLocaleString()}`)
  if (foreignSkipped) console.log(`  National-gate foreign skips: ${foreignSkipped.toLocaleString()}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })

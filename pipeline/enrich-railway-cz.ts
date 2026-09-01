/**
 * Enrich CZ railways.arrow with real train counts from CZPTT timetable —
 * a thin adapter onto the shared graph-walk driver; CZ no longer has a
 * bespoke chord matcher.
 *
 * Downloads JR2026.zip (national timetable, 13k+ train XMLs), parses each
 * running train's ORDERED location sequence + passenger/freight
 * classification, resolves station GPS via Overpass, forms consecutive-stop
 * pairs AFTER GPS resolution (`czpttSequencesToStationPairs`, exported below),
 * then hands the raw pairs to `enrichRailwaysByGraphWalk` (lib/rail-walk-
 * enrich.ts), which walks each canonical OD pair across the CZ rail graph and
 * writes trains_passenger/trains_freight/source_id/parallel_divisor via the
 * shared `writeRailTrains`.
 *
 * WHAT MOVED, AND WHERE (the pre-2026-07-16 bespoke path, deleted this run):
 *   - `nearestCzpttSegment` (500 m chord match) -> the driver's Dijkstra path
 *     search over the CZ rail-segment graph (lib/rail-graph.ts). Bridges
 *     GPS-less pseudo-locations ("Km 67,500", "vl. v km 75,245",
 *     "odb.výh.101") by construction instead of losing the whole inter-station
 *     stretch to a radius miss (the trať 200 banding bug this migration fixes).
 *   - `OLD_FALLBACK`/`wasOldFallbackStamp` -> the driver's quarantine-gated
 *     retract arm: any row this file's ids (110, 9863) stamped that the walk
 *     no longer claims, outside every failed pair's quarantine region, is
 *     disowned — and the silent residual re-claims genuinely dead lines in
 *     the SAME pass, like the old fallback's "no continue" fallthrough did.
 *   - the bespoke `enrichHexes` (hand-rolled Arrow read/write, the inline
 *     czpttKey parallel-divisor pass, `gpsSegments` chord list) -> gone
 *     entirely. Arrow I/O is `writeRailTrains`/`writeRailParallelDivisor`
 *     (lib/railways-arrow.ts) called once per hex by the driver; divisors are
 *     the walk's own lateral parallel-track spread (rail-graph-
 *     metrics.ts::applyParallelSpread), stagger-immune where the old
 *     midpoint-distance czpttKey pass was not.
 *
 * UNCHANGED: the CZPTT download/cache + Overpass station-GPS fetch (same
 * URLs, same never-cache-empty rule), `normName` station-name matching,
 * source ids (110 stamped / 9863 silent residual, owner decision 2026-07-11
 * option b), the CZ country gate + bbox hex prefilter, the
 * provably-complete-inputs `retractSafe` test, and the import-safe main().
 *
 * CACHE CHANGE: the old `czptt-segment-counts.json` (station-PAIR counts,
 * counted during XML parse) is replaced by `czptt-train-sequences.json`
 * (per-train ORDERED location sequences + classification) — pair formation
 * now happens AFTER GPS resolution, which is the actual fix (see
 * `czpttSequencesToStationPairs`). The old cache is unused from here on;
 * `--enrich-only` reads ONLY the new cache and, if just the old one survives,
 * errors with a pointer to re-run once without `--enrich-only`.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-cz.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-cz.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-cz.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-cz.ts --stamp-only
 *
 * --stamp-only is the inspection-safe mode: it passes
 * `enableDestructive: false` to the driver: the run walk-stamps train counts
 * incl. divisors (a graph-walk stamp's divisor rides in the SAME atomic
 * write as its pax/frt/sourceId, in every mode); no retract (including the country-bleed heal) and no
 * silent residual. Use it to inspect a live run's failure-taxonomy/component
 * health before trusting it with destructive ops. KEPT permanently, not a
 * one-shot flag — a later re-run (fresh CZPTT parse, rail-graph change)
 * reaches for it again.
 */

import { readFileSync, writeFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { SOURCE_ID_CZ_SZCD_GTFS, SOURCE_ID_CZ_TIMETABLE_SILENT } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { logRetractSkippedIncompleteInputs } from './lib/gtfs-enrich-core.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CZ_SZCD_GTFS

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cz`)
const CACHE_TRAINS = resolve(CACHE_DIR, 'czptt-train-sequences.json')
// Pre-#26 cache (station-PAIR counts). Unused here — kept only so
// --enrich-only can point an operator at the fix instead of a bare crash when
// just this fossil survives from before the rewrite.
const CACHE_TRAINS_OLD = resolve(CACHE_DIR, 'czptt-segment-counts.json')
const CACHE_STATIONS = resolve(CACHE_DIR, 'osm-stations.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

const CZPTT_URL = 'https://portal.cisjr.cz/pub/draha/celostatni/szdc/2026/JR2026.zip'
const OVERPASS_URL = 'https://overpass-api.de/api/interpreter'

// Target date for counting trains (Wednesday = typical weekday). JR2026
// validity varies per train (some Dec 2025, some Mar 2026); pick a Wednesday
// well inside validity: April 2026.
const TARGET_DATE = '2026-04-08'

// #31.7 country gate — this hex prefilter bbox (48-51.5 / 11.5-19.5)
// deliberately reaches into DE/AT/PL/SK (Dresden, Vienna) so the driver's
// bleed retract arm (bleedGate = the same CZ gate) can find + disown any foreign row this dataset
// (or its bespoke predecessor) ever stamped out there — same rationale as the
// R9 auditor's segmentWhollyOutside predicate.
const CZ_BBOX: readonly [number, number, number, number] = [48.0, 11.5, 51.5, 19.5]

// ── Types ──

interface StationGPS {
  name: string
  lat: number
  lon: number
}

/** One CZPTT operating point in a train's path — a real station/halt (has a
 *  matching OSM GPS after resolution) or a pseudo-location ("Km 67,500",
 *  "vl. v km 75,245", "odb.výh.101") that never resolves to GPS. */
export interface CzpttTrainStop {
  code: string
  name: string
}

/** One running train's full ORDERED path + classification for TARGET_DATE.
 *  `isFreight` mirrors ObjectType=NA (JR2026 is passenger-only in practice
 *  today; kept for forward-compatibility if SŽ ever publishes freight). */
export interface CzpttTrainSequence {
  stops: CzpttTrainStop[]
  isFreight: boolean
}

// ── Step 1: Download + parse CZPTT into per-train sequences ──

async function getTrainSequences(): Promise<CzpttTrainSequence[]> {
  if (!forceDownload && existsSync(CACHE_TRAINS)) {
    console.log(`  Using cached train sequences: ${CACHE_TRAINS}`)
    return (JSON.parse(readFileSync(CACHE_TRAINS, 'utf-8')) as { sequences: CzpttTrainSequence[] }).sequences
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_TRAINS)) {
      if (existsSync(CACHE_TRAINS_OLD)) {
        console.error(
          `ERROR: --enrich-only found only the pre-#26 pair-count cache (${CACHE_TRAINS_OLD}). ` +
          `This rewrite reads per-train sequences (${CACHE_TRAINS}) instead — the old cache already ` +
          `collapsed trains into segment pairs and cannot be converted. Re-run once WITHOUT ` +
          `--enrich-only to re-parse CZPTT into the new cache, then use --enrich-only again.`,
        )
      } else {
        console.error(`ERROR: --enrich-only but no cache (${CACHE_TRAINS})`)
      }
      process.exit(1)
    }
    return (JSON.parse(readFileSync(CACHE_TRAINS, 'utf-8')) as { sequences: CzpttTrainSequence[] }).sequences
  }

  console.log('  Downloading CZPTT timetable...')
  const zipPath = resolve(CACHE_DIR, 'JR2026.zip')
  const xmlDir = resolve(CACHE_DIR, 'czptt-xml')
  mkdirSync(CACHE_DIR, { recursive: true })

  if (!existsSync(zipPath) || forceDownload) {
    const res = await fetch(CZPTT_URL, { signal: AbortSignal.timeout(120000) })
    if (!res.ok) throw new Error(`CZPTT download error: ${res.status}`)
    writeFileSync(zipPath, Buffer.from(await res.arrayBuffer()))
    console.log(`  Downloaded ${(readFileSync(zipPath).length / 1e6).toFixed(1)} MB`)
  }

  console.log('  Extracting XMLs...')
  mkdirSync(xmlDir, { recursive: true })
  execSync(`unzip -o -q "${zipPath}" -d "${xmlDir}"`, { timeout: 120000 })
  const xmlFiles = readdirSync(xmlDir).filter(f => f.endsWith('.xml'))
  console.log(`  ${xmlFiles.length} train definitions`)

  const targetMs = new Date(TARGET_DATE).getTime()
  const sequences: CzpttTrainSequence[] = []
  let trainsRunning = 0

  // JR2026.zip is passenger-only in practice (verified 2026-04-10: all 13,252
  // XMLs are ObjectType=PA, OperationalTrainNumber never carries a Nex/Pn/Mn
  // freight prefix). SŽ does not publish freight timetables machine-readably.
  // isFreight/ObjectType=NA is checked anyway for forward-compatibility.
  let passengerTrains = 0
  let freightTrains = 0
  let unknownTrains = 0

  for (const file of xmlFiles) {
    const xml = readFileSync(resolve(xmlDir, file), 'utf-8')

    // Check if train runs on target date via PlannedCalendar
    const bitmapMatch = xml.match(/<BitmapDays>([^<]+)</)
    const startMatch = xml.match(/<PlannedCalendar>[\s\S]*?<StartDateTime>([^<]+)</)
    const endMatch = xml.match(/<PlannedCalendar>[\s\S]*?<EndDateTime>([^<]+)</)
    if (!bitmapMatch || !startMatch || !endMatch) continue

    const startDate = new Date(startMatch[1].substring(0, 10))
    const endDate = new Date(endMatch[1].substring(0, 10))
    const bitmap = bitmapMatch[1]

    if (targetMs < startDate.getTime() || targetMs > endDate.getTime()) continue
    const dayOffset = Math.floor((targetMs - startDate.getTime()) / 86400000)
    if (dayOffset >= bitmap.length || bitmap[dayOffset] !== '1') continue

    trainsRunning++

    // PA = Passenger, NA = Freight (if ever published), TR = Train Run reference
    const objTypeMatch = xml.match(/<ObjectType>([^<]+)</)
    const objType = objTypeMatch?.[1] || ''
    const isFreight = objType === 'NA'
    if (isFreight) freightTrains++
    else if (objType === 'PA') passengerTrains++
    else unknownTrains++

    // Extract the FULL ordered station sequence — pair formation happens
    // later, after GPS resolution (czpttSequencesToStationPairs), so
    // pseudo-locations stay here and are dropped only once we know which
    // codes actually have GPS.
    const locRegex = /<LocationPrimaryCode>(\d+)<\/LocationPrimaryCode>\s*<PrimaryLocationName>([^<]+)</g
    const stops: CzpttTrainStop[] = []
    let m
    while ((m = locRegex.exec(xml)) !== null) {
      // Deduplicate consecutive same station (multiple platform entries)
      const code = m[1]
      if (stops.length === 0 || stops[stops.length - 1].code !== code) {
        stops.push({ code, name: m[2] })
      }
    }
    sequences.push({ stops, isFreight })
  }

  console.log(`  ${trainsRunning} trains running on ${TARGET_DATE}`)
  console.log(`    passenger (PA): ${passengerTrains}, freight (NA): ${freightTrains}, other: ${unknownTrains}`)
  if (freightTrains === 0) {
    console.log(`  NOTE: JR2026.zip is passenger-only. Freight data not available from this source.`)
  }

  // Never persist an empty parse — a poisoned cache would starve every later
  // --enrich-only run AND the retract evidence.
  if (sequences.length > 0) {
    writeFileSync(CACHE_TRAINS, JSON.stringify({ sequences }))
  } else {
    console.log(`  NOT caching empty CZPTT parse (${xmlFiles.length} XMLs yielded 0 sequences)`)
  }

  execSync(`rm -rf "${xmlDir}"`) // cleanup XMLs to save space
  return sequences
}

// ── Step 2: Get station GPS from OSM (unchanged) ──

async function getStationGPS(): Promise<Map<string, StationGPS>> {
  if (!forceDownload && existsSync(CACHE_STATIONS)) {
    const data = JSON.parse(readFileSync(CACHE_STATIONS, 'utf-8'))
    return new Map(Object.entries(data))
  }
  if (enrichOnly) {
    if (!existsSync(CACHE_STATIONS)) { console.error('ERROR: --enrich-only but no station cache'); process.exit(1) }
    const data = JSON.parse(readFileSync(CACHE_STATIONS, 'utf-8'))
    return new Map(Object.entries(data))
  }

  console.log('  Downloading station coordinates from OSM...')
  const query = `[out:json];area["ISO3166-1"="CZ"]->.cz;(node["railway"="station"](area.cz);node["railway"="halt"](area.cz););out;`
  const res = await fetch(OVERPASS_URL, {
    method: 'POST',
    body: `data=${encodeURIComponent(query)}`,
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    signal: AbortSignal.timeout(120000),
  })
  const text = await res.text()
  let lines: { lat: number; lon: number; tags?: { name?: string } }[]
  try {
    const json = JSON.parse(text)
    lines = json.elements.filter((e: any) => e.tags?.name)
  } catch {
    // Overpass may be overloaded, try CSV fallback via bbox
    console.log('  Overpass JSON failed, trying bbox query...')
    const res2 = await fetch(OVERPASS_URL, {
      method: 'POST',
      body: 'data=' + encodeURIComponent('[out:json];(node["railway"="station"](48.5,12.0,51.1,18.9);node["railway"="halt"](48.5,12.0,51.1,18.9););out;'),
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      signal: AbortSignal.timeout(120000),
    })
    const json2 = await res2.json() as any
    lines = json2.elements.filter((e: any) => e.tags?.name)
  }

  const stations = new Map<string, StationGPS>()
  for (const el of lines) {
    const name = el.tags!.name!
    stations.set(name, { name, lat: el.lat, lon: el.lon })
  }
  console.log(`  ${stations.size} OSM stations with GPS`)

  // Never persist an empty station set — an Overpass overload can return a
  // valid 200 with zero elements, and a poisoned cache starves every later run.
  if (stations.size > 0) {
    const obj: Record<string, StationGPS> = {}
    for (const [k, v] of stations) obj[k] = v
    writeFileSync(CACHE_STATIONS, JSON.stringify(obj))
  } else {
    console.log('  NOT caching empty OSM station response')
  }

  return stations
}

// ── Step 3: CZPTT code -> GPS via station-name matching (unchanged join) ──

/** Normalize station name for fuzzy matching */
function normName(s: string): string {
  return s.toLowerCase()
    .replace(/[- ]/g, '')
    .replace(/pha\s*hl\.?n\.?/i, 'prahahlavn')
    .replace(/hl\.?n\.?/i, 'hlavní nádraží')
    .replace(/[áà]/g, 'a').replace(/[éè]/g, 'e').replace(/[íì]/g, 'i')
    .replace(/[óò]/g, 'o').replace(/[úùů]/g, 'u').replace(/ý/g, 'y')
    .replace(/č/g, 'c').replace(/ď/g, 'd').replace(/ň/g, 'n')
    .replace(/ř/g, 'r').replace(/š/g, 's').replace(/ť/g, 't').replace(/ž/g, 'z')
    .replace(/ě/g, 'e')
}

/** CZPTT `LocationPrimaryCode` -> OSM GPS, by exact then normalized name match
 *  across every station-code appearance in every train sequence. A code with
 *  no matching OSM station/halt (a pseudo-location, or a real station OSM
 *  never tagged) simply has no entry — `czpttSequencesToStationPairs` treats
 *  that as "bridge over it", never as a failure to retry. */
function buildCodeToGPS(
  sequences: readonly CzpttTrainSequence[],
  stationGPS: Map<string, StationGPS>,
): Map<string, { lat: number; lon: number }> {
  const codeToGPS = new Map<string, { lat: number; lon: number }>()
  const normOSM = new Map<string, StationGPS>()
  for (const [name, gps] of stationGPS) normOSM.set(normName(name), gps)

  let matched = 0, unmatched = 0
  for (const seq of sequences) {
    for (const { code, name } of seq.stops) {
      if (codeToGPS.has(code)) continue
      const osm = stationGPS.get(name) || normOSM.get(normName(name))
      if (osm) {
        codeToGPS.set(code, { lat: osm.lat, lon: osm.lon })
        matched++
      } else {
        unmatched++
      }
    }
  }
  console.log(`  Station name matching: ${matched} found, ${unmatched} not found in OSM`)
  return codeToGPS
}

// ── Pair formation after GPS resolution ──

/**
 * Resolves each train's ordered CZPTT location sequence to its GPS-KNOWN
 * stops only, then emits one RAW station-pair count per consecutive pair.
 * Pseudo-locations ("Km 67,500", "vl. v km 75,245", "odb.výh.101" — CZPTT's
 * term for operating points with no station record) and stations OSM never
 * tagged simply have no entry in `codeToGPS` and drop out — a run
 * A -> pseudo -> B collapses to exactly one A-B pair BY CONSTRUCTION, instead
 * of the old chord-matcher's silent 500 m-radius miss across the whole gap.
 *
 * Adjacent DUPLICATE codes are also collapsed after resolution (two
 * consecutive known stops resolving to the same code — e.g. a re-entered
 * platform record that survives the parse-time consecutive-dedup because a
 * pseudo-location used to sit between them): a repeated stop must never
 * manufacture a zero-length self-pair.
 *
 * Emits RAW per-train, per-pair counts (pax=1/frt=0 passenger, frt=1/pax=0
 * freight) — deliberately NOT pre-summed: the walk driver's own canonical
 * accumulator (rail-graph-metrics.ts::accumulateCanonicalPairs) sums BOTH
 * directions and every trip sharing an OD pair (today's stamps carried only
 * one direction — ~2x undercount on matched rows, plan /gg item 2); summing
 * here would double that summation instead of feeding it raw counts.
 *
 * A train collapsing to fewer than two known stops contributes nothing.
 */
export function czpttSequencesToStationPairs(
  sequences: readonly CzpttTrainSequence[],
  codeToGPS: ReadonlyMap<string, { lat: number; lon: number }>,
): RailStationPairCount[] {
  const pairs: RailStationPairCount[] = []
  for (const seq of sequences) {
    const known: Array<{ code: string; lat: number; lon: number }> = []
    for (const stop of seq.stops) {
      const gps = codeToGPS.get(stop.code)
      if (!gps) continue // pseudo-location or OSM-untagged station — bridged by omission
      if (known.length > 0 && known[known.length - 1].code === stop.code) continue // adjacent duplicate after resolution
      known.push({ code: stop.code, lat: gps.lat, lon: gps.lon })
    }
    if (known.length < 2) continue
    for (let i = 0; i < known.length - 1; i++) {
      pairs.push({
        fromLat: known[i].lat, fromLon: known[i].lon,
        toLat: known[i + 1].lat, toLon: known[i + 1].lon,
        pax: seq.isFreight ? 0 : 1,
        frt: seq.isFreight ? 1 : 0,
      })
    }
  }
  return pairs
}

// ── Main ──

async function main() {
  console.log(`=== CZ Railway Enrichment (${YEAR}) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  const sequences = await getTrainSequences()
  const stationGPS = await getStationGPS()
  const codeToGPS = buildCodeToGPS(sequences, stationGPS)
  const pairs = czpttSequencesToStationPairs(sequences, codeToGPS)
  console.log(`  ${sequences.length} train sequences -> ${pairs.length} raw station-pair counts (pre-walk)`)

  // CRITICAL-1b (/gg Codex, unchanged rationale): a retract may only run over
  // a PROVABLY COMPLETE input snapshot — sequences AND station GPS both
  // loaded non-empty, the name join resolved, and at least one train's
  // sequence survived GPS resolution as a walkable pair. Any cache can
  // silently persist an empty result (empty JR2026 unzip, Overpass 200 with
  // zero elements) — with any hollow, disowning would strand every real stamp
  // with nothing left to re-claim it. Only retract is gated; the walk itself
  // simply has less to work with.
  const incompleteInputs: string[] = []
  if (sequences.length === 0) incompleteInputs.push('CZPTT train sequences empty (czptt-train-sequences.json)')
  if (stationGPS.size === 0) incompleteInputs.push('OSM station GPS empty (osm-stations.json)')
  if (incompleteInputs.length === 0 && codeToGPS.size === 0) incompleteInputs.push('station-name join resolved zero CZPTT codes to GPS')
  if (incompleteInputs.length === 0 && pairs.length === 0) incompleteInputs.push('zero GPS-resolvable station pairs (every train sequence collapsed below 2 known stops)')
  const retractSafe = incompleteInputs.length === 0
  if (!retractSafe) logRetractSkippedIncompleteInputs(incompleteInputs.join('; '))

  // #31.7 national-ownership gate — built HERE, not at module top:
  // makeCountryGate may download CGAZ on first use and imports must stay
  // side-effect-free.
  const countryGate = makeCountryGate('CZ')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: CZ_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate,
    // Nationally-owned ids (110/9863) — this dataset's own country gate IS
    // the union of every territory it may legitimately stamp, so the bleed
    // arm uses the same gate (driver contract, 2026-07-16 item 1).
    bleedGate: countryGate,
    // Owner decision 2026-07-11 (option b, gtfs-silent-decision.md): a
    // quarantine-free stretch the walk never reaches has NO scheduled service
    // by construction (CZPTT is the infrastructure manager's timetable —
    // every operator's SŽ-network train is in it), so it gets a small
    // explicit residual (2 pax + 1 frt/day) under the BASELINE-rank
    // timetable-silent id instead of the engine's branch default.
    silentResidual: { sourceId: SOURCE_ID_CZ_TIMETABLE_SILENT, pax: 2, frt: 1 },
    // Both ids this file has ever stamped (live match + silent residual) are
    // retractable in the same pass — a row the walk no longer claims under
    // either id is disowned (quarantine-gated) and can be re-claimed by
    // the walk or the residual this same run.
    retract: { sourceIds: [MY_SOURCE_ID, SOURCE_ID_CZ_TIMETABLE_SILENT] },
    retractSafe,
    enableDestructive: !stampOnly,
    sidecar: { scope: 'cz', extractFingerprint: `jr2026:${TARGET_DATE}`, feeds: ['czptt-jr2026'] },
  })

  console.log(`\n=== Done (${stampOnly ? 'stamp-only — Step A proof mode' : 'destructive ops enabled'}) ===`)
  console.log(JSON.stringify(stats, null, 2))
}

// Import-safe: run only when invoked directly — importing this file must never
// trigger a download/enrichment pass (pattern from enrich-roads-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}

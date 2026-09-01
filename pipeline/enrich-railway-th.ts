/**
 * Enrich TH railways.arrow with Namtang GTFS — migrated onto the shared
 * graph-walk driver following `enrich-railway-dk.ts`'s single-national-file
 * pattern.
 *
 * Namtang (นามทาง) is the unified Thai transit feed published by the Office of
 * Transport and Traffic Policy and Planning (OTP / สนข., Ministry of
 * Transport). A single ~27 MB zipped GTFS covers:
 *
 *   - **SRT (State Railway of Thailand)** — 189 national heavy-rail routes
 *     (Bangkok <-> Chiang Mai / Ubon Ratchathani / Nong Khai / Sungai Kolok /
 *     Haad Yai) plus Airport Rail Link / SRT Red Line suburban service. GTFS
 *     route_type 2 -> OSM `railway=rail` (rail_type 0) -> the GRAPH-WALK path.
 *   - **BTS Skytrain (BTSC)** — 5 tram/LRT routes, GTFS route_type 0/tram ->
 *     OSM `railway=light_rail` (rail_type 2) -> the per-stop tram join.
 *   - **MRT Bangkok (BEM)** — 4 metro routes (Blue/Purple/Yellow/Pink), GTFS
 *     route_type 1/400-405 ("metro"). OSM tags these `railway=subway`, which
 *     this pipeline never extracts (same known gap as Dubai/Taipei/Singapore/
 *     Seoul/Tokyo/HK/Mexico City metros). MRT stops are deliberately IGNORED
 *     — `computeTramStopFrequencies`' familyOf maps METRO_TYPES → null,
 *     exactly the pre-migration standalone 'metro' family's (non-)behavior.
 *     The first migration draft collapsed them into BTS's 'tram' bucket via
 *     core's `routeFamily` and argued both were dead ends against subway
 *     geometry; the 2026-07-16 reviews disproved that: the metro STOP is not
 *     dead — within the 500 m join radius of an interchange it can win the
 *     match onto BTS light-rail track and stamp MRT frequencies there. An
 *     interchange does not put metro trains on tram track (same physics fix
 *     as AE's Dubai Metro: 87 stops / 26 tram rows / 654 trains-day there).
 *   - BMTA + regional buses (211 routes), 943 DLT intercity + 154 TSB + 98 TC
 *     bus routes — excluded from rail enrichment.
 *
 * Source: `https://namtang-api.otp.go.th/download/namtang-gtfs.zip` (anonymous,
 * updated daily, 27 MB zipped / 160 MB uncompressed). Mirror:
 * `https://github.com/asiripanich/bangkok-gtfs` (daily snapshots since
 * April 2022).
 *
 * HEAVY RAIL (rail_type 0, SRT) runs on `lib/rail-walk-enrich.ts`:
 * `computeStopPairFrequenciesForFeed` (lib/gtfs-stop-pairs.ts) turns
 * stop_times.txt into station-PAIR frequencies, walked along the shortest path
 * by `enrichRailwaysByGraphWalk`. Its `expandFrequencies` option (default: on
 * iff frequencies.txt exists) is the GENERIC form of the headway-multiplier
 * this file used to hand-roll — the gtfs-stop-pairs.ts module doc cites THIS
 * file as the "TH-multiplier style" origin of that pattern, so the pairs path
 * gets it for free with no reimplementation here.
 *
 * TRAM/light-rail (rail_type 1/2 — BTS; MRT metro is EXCLUDED, see its
 * bullet above) keeps the pre-Phase-4 `nearestGridStop` 500 m stop-join
 * (`buildTramExtraMatch`, shared with every other national enricher).
 * `buildTramExtraMatch` filters to `family === 'tram'` INTERNALLY — the
 * load-bearing guard for TH's legacy 3-family bare-array stop cache (76 tram
 * + 184 rail + 107 metro records): passed unfiltered, its ARL heavy-rail
 * stops handed 4 Suvarnabhumi APM rail_type=2 rows 190 ARL trains/day
 * (verified on live Arrow data by the 2026-07-16 review); the call-site
 * filter below stays as harmless double protection. The per-stop counter
 * (`computeTramStopFrequencies` below) is kept as a THIN local wrapper over
 * core's primitives rather than `computeStopFrequenciesForFeed` (core's
 * version has NO frequencies.txt support): BTS genuinely publishes
 * headway-based service — a single template trip in stop_times.txt repeated
 * every `headway_secs`, not one row per real departure — so without this
 * expansion its stop counts would understate real frequency by roughly two
 * orders of magnitude (same shape as the mx.ts frequencies.txt comment).
 *
 * TH's source id is NATIONALLY OWNED (like DK's, unlike europe's shared
 * `SOURCE_ID_GLOBAL_GTFS_TRANSIT`): `bleedGate` is set to the SAME country
 * gate as `countryGate`. No predecessor source id survives from the
 * pre-2026-07-10 class-default fallback design (TH only ever used ONE id):
 * the driver's own generic retract (disown a previously-owned row the
 * walk/tram-join no longer covers, given `retractSafe`) already subsumes the
 * old OLD_FALLBACK exact-tuple check (SRT/BTS/MRT class-default tuples), so
 * that table is deleted outright rather than preserved via `sourceIds`
 * composition.
 *
 * WHY (unchanged): segments the feed doesn't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; TH is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-th.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-th.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-th.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-th.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-europe.ts/enrich-railway-dk.ts) — enableDestructive=false:
 * the run still walk-stamps heavy-rail counts AND divisors and still runs the
 * tram stop-join, but NO retract and NO country-bleed heal fire.
 */

import { writeFileSync, existsSync, mkdirSync, createReadStream } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { createInterface } from 'node:readline'
import { makeCountryGate } from './lib/country-polygon.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import {
  RAIL_TYPES, TRAM_TYPES, parseCsvLine, parseCsvStream, parseTime,
  computeActiveTripFamiliesForFeed, loadStopsWithCoords, resolveStopViaParent,
  dedupeStopsByLocation, buildTramExtraMatch,
  declaredRouteFamiliesForFeed, describeIncompleteFamilies,
  describeIncompleteFeeds, logRetractSkippedIncompleteInputs, readMergedStopCache, writeMergedStopCache,
  GTFS_BORDER_MARGIN_DEG, type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { SOURCE_ID_TH_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_TH_NATIONAL_RAILWAY

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/th`)
// UNCHANGED across this migration (manifest note, RAIL_CACHED_DOWNLOAD.th marker,
// chain/manifest.ts): now holds ONLY the tram/light-rail per-stop counter (heavy
// rail moved to computeStopPairFrequenciesForFeed's own gtfs-rail-pairs-v1.json
// cache alongside it, never replacing it).
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-stop-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

const FEED_ID = 'namtang'
const FEED_URL = 'https://namtang-api.otp.go.th/download/namtang-gtfs.zip'

// Scope: the exact CGAZ TH polygon via the central writeRailTrains countryGate (#31.7).
// Conservative bbox that doesn't clip Chiang Mai (18.8N/98.99E), Korat (14.97N/102.1E),
// Udon Thani (17.41N/102.79E), Surat Thani (9.1N/99.3E).
const TH_BBOX: [number, number, number, number] = [5.5, 97.3, 20.5, 105.7]
// Neighbour exclusion boxes DELETED (#32, /gg #31 round-2 Codex): the central
// writeRailTrains countryGate (exact CGAZ polygon) owns national scope now, and
// the hand rectangles provably clipped DOMESTIC territory (the CN 'Vietnam' box
// held Nanning, IN 'Pakistan' held Ahmedabad, TH 'Malaysia' held Sungai Kolok).

/** Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in (SRT runs to Sungai Kolok/Malaysia
 *  and Vientiane/Laos), or a through-train's foreign endpoint never snaps and
 *  the whole pair quarantines for no real reason. Stop/pair PARSING itself
 *  (`computeTramStopFrequencies`, `computeStopPairFrequenciesForFeed`) gets
 *  the UNPADDED `TH_BBOX` — the margin is applied internally by
 *  `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  TH_BBOX[0] - GTFS_BORDER_MARGIN_DEG, TH_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  TH_BBOX[2] + GTFS_BORDER_MARGIN_DEG, TH_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

// ── Step 1: Download GTFS feed (UNCHANGED — TH's own single-hardcoded-feed shape) ──

async function downloadGtfs(): Promise<string> {
  const extractDir = resolve(CACHE_DIR, 'gtfs-namtang')
  if (!forceDownload && existsSync(resolve(extractDir, 'stops.txt'))) {
    console.log(`  Using cached GTFS: ${extractDir}`)
    return extractDir
  }
  if (enrichOnly && !existsSync(resolve(extractDir, 'stops.txt'))) {
    throw new Error('--enrich-only but no cached GTFS')
  }
  mkdirSync(CACHE_DIR, { recursive: true })
  const zipPath = resolve(CACHE_DIR, 'namtang-gtfs.zip')
  if (!existsSync(zipPath) || forceDownload) {
    console.log(`  Downloading Namtang GTFS from ${FEED_URL}...`)
    const res = await fetch(FEED_URL, { signal: AbortSignal.timeout(600_000), headers: { 'User-Agent': 'Mozilla/5.0' } })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    writeFileSync(zipPath, Buffer.from(await res.arrayBuffer()))
    console.log(`  Downloaded.`)
  }
  mkdirSync(extractDir, { recursive: true })
  execSync(`unzip -o -q "${zipPath}" -d "${extractDir}"`, { timeout: 120_000 })
  return extractDir
}

// ── Step 2: tram/light-rail per-stop frequencies (heavy rail moved to the
//    graph-walk pairs below) ──

/** Per-stop TRAM/light-rail frequency counter — feeds `buildTramExtraMatch`'s
 *  500 m stop-join (rail_type 1/2 only). Namtang's METRO routes (MRT Bangkok,
 *  GTFS route_type 1/400-405) are EXCLUDED — familyOf maps METRO_TYPES →
 *  null, restoring the pre-migration deliberate ignore (2026-07-16 review
 *  fix; see the module doc's MRT bullet: a 'tram'-labelled MRT stop can win
 *  the 500 m join onto BTS track at an interchange, which is physically
 *  wrong).
 *
 *  Kept as a THIN local wrapper — not core's `computeStopFrequenciesForFeed`,
 *  which has no frequencies.txt support — because BTS genuinely publishes
 *  headway-based service (module doc). SRT heavy rail no longer needs a
 *  per-stop counter at all: `computeStopPairFrequenciesForFeed`'s own
 *  `expandFrequencies` option already reimplements this exact TH-multiplier
 *  pattern generically for the pairs path. */
async function computeTramStopFrequencies(extractDir: string): Promise<StopTrainCount[]> {
  const { tripFam } = await computeActiveTripFamiliesForFeed(
    extractDir,
    // TRAM_TYPES only — NOT `routeFamily`, whose metro→tram grouping would
    // revive MRT counts here (the rebuild-path half of the review fix; the
    // legacy-cache half is buildTramExtraMatch's internal family filter).
    (routeType) => (TRAM_TYPES.has(routeType) ? 'tram' : null),
  )
  if (tripFam.size === 0) return []

  // frequencies.txt: expand headway-based BTS trips into a daily departure count.
  const tripBoosts = new Map<string, number>()
  const freqPath = resolve(extractDir, 'frequencies.txt')
  if (existsSync(freqPath)) {
    const freqs = await parseCsvStream(freqPath)
    for (const f of freqs) {
      const tripId = f['trip_id']
      if (!tripFam.has(tripId)) continue
      const startSec = parseTime(f['start_time'] || '')
      const endSec = parseTime(f['end_time'] || '')
      const headway = parseInt(f['headway_secs'] || '0', 10)
      if (startSec < 0 || endSec < 0 || headway <= 0) continue
      const count = Math.max(1, Math.floor(Math.max(0, endSec - startSec) / headway))
      tripBoosts.set(tripId, (tripBoosts.get(tripId) || 0) + count)
    }
    console.log(`  [tram] frequencies.txt boosts: ${tripBoosts.size} trips`)
  }
  const boostFor = (tripId: string): number => tripBoosts.get(tripId) ?? 1

  // stop_times.txt (streamed for memory)
  const stopDepartures = new Map<string, number>()
  const stStream = createReadStream(resolve(extractDir, 'stop_times.txt'), { encoding: 'utf-8' })
  const stRl = createInterface({ input: stStream, crlfDelay: Infinity })
  let stHeaders: string[] | null = null
  let tripIdIdx = -1, stopIdIdx = -1
  for await (const rawLine of stRl) {
    const line = stHeaders === null ? rawLine.replace(/^\uFEFF/, '') : rawLine
    if (line.trim() === '') continue
    if (!stHeaders) {
      stHeaders = parseCsvLine(line)
      tripIdIdx = stHeaders.indexOf('trip_id')
      stopIdIdx = stHeaders.indexOf('stop_id')
      if (tripIdIdx < 0 || stopIdIdx < 0) throw new Error('stop_times.txt missing trip_id/stop_id')
      continue
    }
    const fields = parseCsvLine(line)
    const tripId = fields[tripIdIdx]
    if (!tripFam.has(tripId)) continue
    const stopId = fields[stopIdIdx]
    stopDepartures.set(stopId, (stopDepartures.get(stopId) || 0) + boostFor(tripId))
  }
  console.log(`  [tram] ${stopDepartures.size} unique stops with tram/light-rail departures`)

  const stops = await loadStopsWithCoords(extractDir, TH_BBOX)
  const results: StopTrainCount[] = []
  let resolvedViaParent = 0
  for (const [stopId, count] of stopDepartures) {
    const { stop, viaParent } = resolveStopViaParent(stops, stopId)
    if (!stop) continue
    if (viaParent) resolvedViaParent++
    results.push({
      stop_id: stop.stop_id, lat: stop.lat, lon: stop.lon, name: stop.name, h3r4: stop.h3r4,
      family: 'tram', trains_passenger: count, trains_freight: 0,
    })
  }
  const deduped = dedupeStopsByLocation(results)
  console.log(`  [tram] ${deduped.length} unique tram/light-rail stops (${resolvedViaParent} resolved via parent station)`)
  return deduped
}

/** TH's route_type → family classifier for the declared-families completeness
 *  check (`declaredRouteFamiliesForFeed`) — the SAME classification the two
 *  parsers actually apply: pairs side = `computeStopPairFrequenciesForFeed`'s
 *  default RAIL_TYPES-only familyOf (SRT route_type 2 → 'rail'), stop side =
 *  `computeTramStopFrequencies`' TRAM_TYPES-only familyOf (BTS → 'tram').
 *  METRO_TYPES → null on BOTH sides (MRT is deliberately ignored, module
 *  doc), so a declared metro family never demands completeness evidence
 *  nobody parses. */
function thDeclaredRouteFamily(routeType: number): 'rail' | 'tram' | null {
  if (RAIL_TYPES.has(routeType)) return 'rail'
  return TRAM_TYPES.has(routeType) ? 'tram' : null
}

// ── Main ──

async function main() {
  console.log(`=== TH Railway Enrichment — Namtang GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // downloadGtfs is always needed — even a merged-stop-cache hit below still
  // needs a real extractDir for the pair parser's own per-extractDir cache.
  // It THROWS on failure, so past this point the single feed is always
  // present — no missing-feed completeness term exists here (unlike dk/ae's
  // failure-tolerant multi-feed loaders).
  const dir = await downloadGtfs()
  const stopCacheHit = !forceDownload && existsSync(CACHE_FREQUENCIES)

  // ── Single-feed parse: declared families + heavy-rail STATION PAIRS (+ tram
  //    stops on cache miss). BIDIRECTIONAL completeness (2026-07-16 review
  //    item 4): every family the feed's routes.txt declares under
  //    `thDeclaredRouteFamily` must have parsed non-empty (declares rail →
  //    pairs>0, declares tram → tramStops>0), evaluated by the shared
  //    `describeIncompleteFamilies`. A malformed routes.txt THROWS (item 3,
  //    `declaredRouteFamiliesForFeed`) and is recorded as a PARSE FAILURE —
  //    never as "legitimately rail-less". Pairs are re-validated every run
  //    (their own per-extractDir cache in computeStopPairFrequenciesForFeed
  //    is fingerprinted against the GTFS inputs, item 2 — NOT recomputed from
  //    stop_times each run, the old comment here claiming "always computed
  //    fresh" was wrong); `expandFrequencies` defaults on (frequencies.txt
  //    exists in this feed) — the generic TH-multiplier the pairs path
  //    inherited, see module doc. The tram direction is checked live on
  //    stop-cache miss and vouched by the v2 cache's recorded provenance on a
  //    hit (recording rule below is itself bidirectional, so a recorded feed
  //    means BOTH directions held at write).
  let pairs: RailStationPairCount[] = []
  let freshTramStops: StopTrainCount[] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  try {
    const declared = await declaredRouteFamiliesForFeed(dir, thDeclaredRouteFamily)
    const pairResult = await computeStopPairFrequenciesForFeed(dir, { bbox: TH_BBOX, optionsKey: 'th-busiest-wed' })
    pairs = pairResult.pairs
    let tramN: number | null = null
    if (!stopCacheHit) {
      freshTramStops = await computeTramStopFrequencies(dir)
      tramN = freshTramStops.length
    }
    const issue = describeIncompleteFamilies(FEED_ID, declared, pairs.length, tramN)
    if (issue) feedIssues.push(issue)
    else completeFeedIds.push(FEED_ID)
    console.log(`  [${FEED_ID}] ${pairs.length} heavy-rail station pairs; declares [${[...declared].sort().join('+') || 'no rail family'}]`)
  } catch (err: any) {
    console.error(`  [${FEED_ID}] PARSE FAILED: ${err.message}`)
    feedIssues.push(`${FEED_ID}: parse failure — ${err.message}`)
  }

  // ── Tram/light-rail STOPS: the v2 cache (the pre-existing download marker;
  //    unchanged path) or this run's own parse. ──
  let tramStops: StopTrainCount[]
  let stopsUnsafeDetail: string
  if (stopCacheHit) {
    console.log(`  Using cached tram/light-rail stop frequencies: ${CACHE_FREQUENCIES}`)
    const cached = readMergedStopCache<StopTrainCount>(CACHE_FREQUENCIES)
    // Call-site family filter: the legacy 3-family bare-array cache carries
    // 76 tram + 184 rail + 107 metro records, and only the tram ones may feed
    // the stop-join. buildTramExtraMatch filters internally too (the
    // load-bearing guard — see the module doc's Suvarnabhumi APM finding);
    // this filter is harmless double protection and keeps the count below honest.
    tramStops = cached.stops.filter(s => s.family === 'tram')
    // Legacy bare-array cache: with a SINGLE feed, non-empty tram stops are
    // themselves the completeness proof of the tram direction (the one feed
    // parsed non-empty when the cache was written); the rail direction is
    // re-proven live by the pairs check above every run.
    stopsUnsafeDetail = cached.feedsLoadedNonEmpty === null
      ? (tramStops.length > 0 ? '' : `cached snapshot has no tram stops: ${CACHE_FREQUENCIES}`)
      : describeIncompleteFeeds([FEED_ID], cached.feedsLoadedNonEmpty)
    console.log(`  ${tramStops.length} tram/light-rail stops in cache`)
  } else {
    tramStops = freshTramStops
    stopsUnsafeDetail = '' // live evaluation already sits in feedIssues
    if (feedIssues.length === 0) {
      // Recording rule (item 4): the feed lands in the cache's provenance only
      // when it passed the FULL bidirectional check this run — a later
      // cache-served run inherits completeness both ways, not any-family.
      writeMergedStopCache(CACHE_FREQUENCIES, completeFeedIds, tramStops)
      console.log(`  Cached tram/light-rail frequencies to ${CACHE_FREQUENCIES}`)
    } else {
      // Never persist a partial snapshot: a poisoned cache would silently starve
      // every later cache-served run (both enrichment and the retract evidence).
      console.log(`  NOT caching partial/empty snapshot (${feedIssues.join('; ')})`)
    }
  }

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE
  // snapshot for BOTH families this single feed carries — a working tram part
  // must never mask an empty heavy-rail pairs part (or vice versa), since ONE
  // retract object below covers every row regardless of family.
  const retractUnsafeDetail = [...feedIssues, stopsUnsafeDetail].filter(Boolean).join('; ')
  const retractSafe = retractUnsafeDetail === ''
  if (!retractSafe) logRetractSkippedIncompleteInputs(retractUnsafeDetail)

  if (pairs.length === 0 && tramStops.length === 0) {
    console.log(`\nNo GTFS data to enrich. Exiting.`)
    return
  }

  console.log(`\n  ${pairs.length} heavy-rail station pairs, ${tramStops.length} tram/light-rail stops`)

  // COUNTRY GATE (#26C): a national feed can carry international through-services
  // (SRT runs to Sungai Kolok/Malaysia and Vientiane/Laos), so this id must not
  // speak for track outside Thailand — same rationale as every other national
  // enricher (mechanism: the PL feed once stamped 11,856 km of CZ track,
  // 7fac2349). #31.7 central country gate — see writeRailTrains / rail-walk-enrich.ts.
  const inTh = makeCountryGate('TH')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inTh,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Thailand is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inTh,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — TH has no retired predecessor id to absorb. The pre-2026-07-10
    // OLD_FALLBACK class-default retract is deleted outright (not preserved via
    // sourceIds composition): the driver's own generic retract — disown a
    // previously-owned row the walk/tram-join no longer covers, given
    // retractSafe — already subsumes it and is strictly more correct.
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: TH is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'th',
      extractFingerprint: `th-gtfs:${FEED_ID}`,
      feeds: [FEED_ID],
    },
  })
  console.log(`\n  walk stats: ${JSON.stringify(stats)}`)

  console.log(`\n=== Done (${stampOnly ? 'stamp-only — Step A proof mode' : 'destructive ops enabled'}) ===`)
}

// Import-safe: run only when invoked directly — importing this file must never
// trigger a download/enrichment pass (pattern from enrich-roads-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}

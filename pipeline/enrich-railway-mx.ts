/**
 * Enrich MX railways.arrow with Mexican GTFS feeds — migrated onto the shared
 * graph-walk driver, following `enrich-railway-dk.ts`'s single-national-file
 * pattern.
 *
 * CDMX SEMOVI publishes a unified GTFS containing Metro, Metrobús, Trolebús,
 * Tren Ligero, Cablebús, Tren Suburbano, Pumabús, RTP, and concession
 * corridors. Distributed via mdb-latest mirror (origin datos.cdmx.gob.mx is
 * firewalled). The second feed, Toluca + Metropolitan Area (Movimex), is
 * mostly bus-only but kept as a candidate second contributor — it simply
 * yields zero rail/tram stops and pairs when it carries none, and a bus-only
 * routes.txt declares neither rail family, which exempts it from the
 * bidirectional retract-completeness check (`describeIncompleteFamilies`) —
 * it is never demanded to produce the pairs/stops it does not carry, so it
 * can no longer pin retractSafe false forever.
 *
 * HEAVY RAIL (rail_type 0 — Tren Suburbano and any other route_type-2/10x
 * service) runs on `lib/rail-walk-enrich.ts`: `computeStopPairFrequenciesForFeed`
 * (lib/gtfs-stop-pairs.ts) turns stop_times.txt into station-PAIR
 * frequencies, walked along the shortest path by `enrichRailwaysByGraphWalk`.
 * Its `expandFrequencies` option auto-detects `frequencies.txt` per extract
 * dir and expands headway-based trips into a daily departure count FOR FREE
 * — no custom code needed here for the pairs side, even though CDMX SEMOVI's
 * Metro/Tren Ligero use exactly that headway-block pattern.
 *
 * TRAM/light-rail (rail_type 1/2) keeps the pre-Phase-4 `nearestGridStop`
 * 500 m stop-join (`buildTramExtraMatch`, `lib/gtfs-enrich-core.ts`). CDMX
 * Metro (GTFS route_type 1) and Tren Ligero (route_type 0) both map to the
 * GTFS 'tram' family via `routeFamily` — i.e. the frequency-boosted stops
 * this file cares about are ALWAYS tram/light_rail family, never heavy rail
 * (confirmed by reading the pre-migration family-routing gate: `rail_type
 * === 0` only ever drew from the 'rail'-family grid). But the per-stop
 * counter feeding that join still needs its OWN frequencies.txt boost: core's
 * shared `computeStopFrequenciesForFeed` has NO frequencies.txt support (only
 * the pairs parser does), so counting a Metro/Tren Ligero template trip's
 * stop_times as one departure understates Línea 1 by ~225x (it runs
 * ~285-570 trains/day in reality, not the 2 stop_times its template trip
 * contributes). Since GTFS stop-level pax counts feed
 * `buildTramExtraMatch`'s `trains_passenger` field directly, this boost is
 * preserved via a THIN wrapper (`computeStopFrequenciesWithHeadwayBoost`
 * below) that reuses core's CSV/calendar/parent-station primitives
 * (`computeActiveTripFamiliesForFeed`, `loadStopsWithCoords`,
 * `resolveStopViaParent`) for everything EXCEPT the boost — the truly dead
 * duplication (routes/calendar/trips resolution, stops+parent-station
 * fallback, byte-identical to core) is deleted; only the genuinely
 * MX-specific frequencies.txt-boost + stop_times counting loop survives.
 *
 * MX's source id is NATIONALLY OWNED (like DK, unlike europe's shared
 * `SOURCE_ID_GLOBAL_GTFS_TRANSIT`): `bleedGate` is set to the SAME country
 * gate as `countryGate`. No predecessor source id survives from the
 * pre-2026-07-10 class-default fallback design (grep confirms MX only ever
 * used ONE id, `SOURCE_ID_MX_NATIONAL_RAILWAY`): the driver's own generic
 * retract already subsumes the old OLD_FALLBACK exact-tuple check, so that
 * class-default table is deleted outright.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; MX is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-mx.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-mx.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-mx.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-mx.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-europe.ts / enrich-railway-dk.ts) — enableDestructive=false:
 * the run still walk-stamps heavy-rail counts AND divisors and still runs
 * the tram stop-join, but NO retract and NO country-bleed heal fire.
 */

import { writeFileSync, existsSync, mkdirSync, createReadStream } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { createInterface } from 'node:readline'
import { SOURCE_ID_MX_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import {
  routeFamily, parseCsvLine, parseCsvStream, parseTime, formatDate,
  computeActiveTripFamiliesForFeed, loadStopsWithCoords, resolveStopViaParent,
  dedupeStopsByLocation, buildTramExtraMatch,
  declaredRouteFamiliesForFeed, describeIncompleteFamilies,
  describeIncompleteFeeds, logRetractSkippedIncompleteInputs, readMergedStopCache, writeMergedStopCache,
  GTFS_BORDER_MARGIN_DEG, type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_MX_NATIONAL_RAILWAY

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/mx`)
// UNCHANGED across this migration (chain/manifest.ts's RAIL_CACHED_DOWNLOAD.mx
// marker `${YEAR}/mx/gtfs-family-frequencies.json` points here) — the pair
// parser adds its OWN per-extractDir cache (gtfs-rail-pairs-v1.json) alongside
// it, never replacing it.
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-family-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

export interface FeedConfig {
  id: string
  name: string
  urls: string[]
}

const FEEDS: FeedConfig[] = [
  {
    id: 'cdmx-semovi',
    name: 'CDMX SEMOVI unified (Metro + Metrobús + Trolebús + Tren Ligero + Cablebús + Suburbano)',
    urls: [
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/mx-unknown-pumabus-gtfs-1830.zip?alt=media',
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/mx-distrito-federal-corredores-concesionados-gtfs-1099.zip?alt=media',
    ],
  },
  {
    id: 'toluca-movimex',
    name: 'Toluca + Metropolitan Area (Movimex)',
    urls: [
      'https://datos.movimex.gob.mx/gtfs/toluca.gtfs.zip',
    ],
  },
]

// Mexico bbox — both feeds' stops already sit well inside it (neither carries a
// distinct per-feed boundingBox the way europe's registry does), so this doubles
// as the union base for GRAPH_BBOX below.
export const MX_BBOX: [number, number, number, number] = [14.5, -118.4, 32.7, -86.7]

/** Same margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in. Stop/pair PARSING itself
 *  (`computeStopFrequenciesWithHeadwayBoost`, `computeStopPairFrequenciesForFeed`)
 *  gets the UNPADDED `MX_BBOX` — the margin is applied internally by
 *  `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  MX_BBOX[0] - GTFS_BORDER_MARGIN_DEG, MX_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  MX_BBOX[2] + GTFS_BORDER_MARGIN_DEG, MX_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

// ── Step 1: Download GTFS feeds (UNCHANGED — per-country cache/download quirks) ──

/**
 * Download all configured GTFS feeds and return a list of extraction directories.
 * Each feed is cached in its own subdirectory so multiple feeds can coexist.
 */
async function downloadAllGtfs(): Promise<Array<{ feed: FeedConfig; dir: string }>> {
  const results: Array<{ feed: FeedConfig; dir: string }> = []

  for (const feed of FEEDS) {
    const extractDir = resolve(CACHE_DIR, `gtfs-${feed.id}`)

    if (!forceDownload && existsSync(resolve(extractDir, 'stops.txt'))) {
      console.log(`  [${feed.id}] Using cached GTFS: ${extractDir}`)
      results.push({ feed, dir: extractDir })
      continue
    }
    if (enrichOnly) {
      if (!existsSync(resolve(extractDir, 'stops.txt'))) {
        console.log(`  [${feed.id}] --enrich-only but no cached GTFS, skipping`)
        continue
      }
      results.push({ feed, dir: extractDir })
      continue
    }

    mkdirSync(CACHE_DIR, { recursive: true })
    const zipPath = resolve(CACHE_DIR, `gtfs-${feed.id}.zip`)

    let downloaded = false
    for (const url of feed.urls) {
      try {
        console.log(`  [${feed.id}] Downloading from ${url}...`)
        const res = await fetch(url, {
          signal: AbortSignal.timeout(600_000),
          headers: { 'Accept': 'application/zip, application/octet-stream, */*' },
          redirect: 'follow',
        })
        if (!res.ok) {
          console.log(`  [${feed.id}] HTTP ${res.status}, trying next...`)
          continue
        }
        const buf = Buffer.from(await res.arrayBuffer())
        writeFileSync(zipPath, buf)
        console.log(`  [${feed.id}] Downloaded: ${(buf.length / 1e6).toFixed(1)} MB`)
        downloaded = true
        break
      } catch (err: any) {
        console.log(`  [${feed.id}] Failed: ${err.message}, trying next...`)
      }
    }

    if (!downloaded) {
      console.log(`  [${feed.id}] All URLs failed — skipping this feed`)
      continue
    }

    mkdirSync(extractDir, { recursive: true })
    execSync(`unzip -o -q "${zipPath}" -d "${extractDir}"`, { timeout: 120_000 })

    for (const f of ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt']) {
      if (!existsSync(resolve(extractDir, f))) {
        console.log(`  [${feed.id}] Missing ${f}, skipping feed`)
        downloaded = false
        break
      }
    }

    execSync(`rm -f "${zipPath}"`)

    if (downloaded) results.push({ feed, dir: extractDir })
  }

  if (results.length === 0) {
    throw new Error('Failed to download any Mexican GTFS feed')
  }

  console.log(`  ${results.length}/${FEEDS.length} MX feeds available`)
  return results
}

// ── Step 2: per-stop frequency counter with MX's frequencies.txt headway boost ──

/**
 * MX-specific per-stop frequency counter with frequencies.txt headway-boost —
 * used ONLY to feed the TRAM/METRO `nearestGridStop` join (`buildTramExtraMatch`);
 * heavy rail's frequency-boost comes for free from
 * `computeStopPairFrequenciesForFeed`'s own `expandFrequencies` auto-detection
 * (see module doc).
 *
 * WHY this can't just call core's `computeStopFrequenciesForFeed`: CDMX SEMOVI
 * publishes Metro/Tren Ligero as frequency-based trips — a single template
 * trip in stop_times.txt repeated every `headway_secs` seconds
 * (frequencies.txt), not as individually-scheduled trips. Counting each
 * stop_time as one departure understates Línea 1 by ~225x (it runs ~285-570
 * trains/day in reality, not the 2 stop_times its template trip contributes).
 * Core's `computeStopFrequenciesForFeed` has NO frequencies.txt support at all
 * (only the pairs parser, gtfs-stop-pairs.ts, does) — so this thin wrapper
 * reuses core's CSV/calendar/parent-station primitives
 * (`computeActiveTripFamiliesForFeed`, `loadStopsWithCoords`,
 * `resolveStopViaParent`) for everything EXCEPT the boost, and keeps its own
 * frequencies.txt read + stop_times counting loop on top — the genuinely
 * MX-specific part. Even though the boosted stops are tram/light_rail family
 * only (Metro/Tren Ligero never carries OSM rail_type 0 — confirmed by
 * reading the pre-migration family-routing gate), the boost still matters:
 * GTFS stop-level pax counts feed `buildTramExtraMatch`'s `trains_passenger`
 * field directly, so an un-boosted Metro stop would under-report its train
 * frequency by ~2 orders of magnitude in the tram join.
 */
export async function computeStopFrequenciesWithHeadwayBoost(
  feed: FeedConfig,
  extractDir: string,
  bbox: readonly [number, number, number, number],
): Promise<StopTrainCount[]> {
  console.log(`\n  [${feed.id}] Parsing GTFS files (with frequencies.txt headway boost)...`)
  const startTime = Date.now()

  const { tripFam, targetDate, calendarPresent, activeServiceIds } =
    await computeActiveTripFamiliesForFeed(extractDir, routeFamily)
  if (calendarPresent) {
    console.log(`  [${feed.id}] Target date: ${targetDate ? formatDate(targetDate) : '(unresolved)'}`)
  } else {
    console.log(`  [${feed.id}] WARNING: No calendar files. Counting all trips.`)
  }
  console.log(`  [${feed.id}] ${activeServiceIds.size} active service IDs on target date`)
  console.log(`  [${feed.id}] ${tripFam.size} rail/tram trips on target day`)

  if (tripFam.size === 0) {
    console.log(`  [${feed.id}] WARNING: No active rail trips. Returning empty.`)
    return []
  }

  // ── frequencies.txt: headway-based service expansion (the MX-specific part —
  // see this function's doc for why core's shared counter can't do this) ──
  const freqPath = resolve(extractDir, 'frequencies.txt')
  const tripDepartures = new Map<string, number>() // trip_id -> departures/day (default 1)
  if (existsSync(freqPath)) {
    const freqRaw = await parseCsvStream(freqPath)
    for (const r of freqRaw) {
      const tid = r['trip_id']
      if (!tid || !tripFam.has(tid)) continue
      const startSec = parseTime(r['start_time'] || '')
      const endSec = parseTime(r['end_time'] || '')
      const headway = parseInt(r['headway_secs'] || '0', 10)
      if (startSec < 0 || endSec < 0 || headway <= 0) continue
      const span = endSec - startSec
      if (span <= 0) continue
      tripDepartures.set(tid, (tripDepartures.get(tid) || 0) + Math.round(span / headway))
    }
    console.log(`  [${feed.id}] frequencies.txt: ${tripDepartures.size} trips expanded by headway (else 1 departure)`)
  }

  // ── stop_times.txt (stream for large files) ──
  console.log(`  [${feed.id}] Reading stop_times.txt (streaming)...`)
  const stopDepartures = new Map<string, { rail: number; tram: number }>()

  const stStream = createReadStream(resolve(extractDir, 'stop_times.txt'), { encoding: 'utf-8' })
  const stRl = createInterface({ input: stStream, crlfDelay: Infinity })
  let stHeaders: string[] | null = null
  let stLines = 0
  let stMatched = 0
  let tripIdIdx = -1
  let stopIdIdx = -1
  let lastProgressTime = Date.now()

  for await (const rawLine of stRl) {
    const line = stHeaders === null ? rawLine.replace(/^\uFEFF/, '') : rawLine
    if (line.trim() === '') continue

    if (!stHeaders) {
      stHeaders = parseCsvLine(line)
      tripIdIdx = stHeaders.indexOf('trip_id')
      stopIdIdx = stHeaders.indexOf('stop_id')
      if (tripIdIdx < 0 || stopIdIdx < 0) {
        throw new Error(`stop_times.txt missing trip_id/stop_id. Found: ${stHeaders.join(', ')}`)
      }
      continue
    }

    stLines++
    const fields = parseCsvLine(line)
    const tripId = fields[tripIdIdx]
    const fam = tripFam.get(tripId)
    if (!fam) continue

    const stopId = fields[stopIdIdx]
    let counts = stopDepartures.get(stopId)
    if (!counts) { counts = { rail: 0, tram: 0 }; stopDepartures.set(stopId, counts) }
    counts[fam] += tripDepartures.get(tripId) ?? 1 // headway-expanded departures, else 1
    stMatched++

    if (Date.now() - lastProgressTime > 10_000) {
      console.log(`    ... ${(stLines / 1e6).toFixed(1)}M lines, ${stMatched} rail stop-times`)
      lastProgressTime = Date.now()
    }
  }
  console.log(`  [${feed.id}] ${stLines} stop_times lines, ${stMatched} rail stop-times, ${stopDepartures.size} unique stops`)

  // ── stops.txt + parent-station resolution (core's shared primitive) ──
  console.log(`  [${feed.id}] Reading stops.txt...`)
  const stops = await loadStopsWithCoords(extractDir, bbox)
  console.log(`  [${feed.id}] ${stops.stopsMap.size} stops with valid coords`)
  if (stops.skippedOutOfBounds > 0) console.log(`  [${feed.id}] Skipped (out of bounds): ${stops.skippedOutOfBounds}`)

  // ── Build final list ──
  const results: StopTrainCount[] = []
  let resolvedViaParent = 0

  for (const [stopId, counts] of stopDepartures) {
    const { stop, viaParent } = resolveStopViaParent(stops, stopId)
    if (!stop) continue
    if (viaParent) resolvedViaParent++

    for (const family of ['rail', 'tram'] as const) {
      if (counts[family] === 0) continue
      results.push({
        stop_id: stop.stop_id,
        lat: stop.lat,
        lon: stop.lon,
        name: stop.name,
        h3r4: stop.h3r4,
        family,
        trains_passenger: counts[family],
        trains_freight: 0,
      })
    }
  }

  const deduped = dedupeStopsByLocation(results)
  console.log(`  [${feed.id}] ${deduped.length} stops with train counts (${resolvedViaParent} resolved via parent station)`)

  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)
  console.log(`  [${feed.id}] GTFS parsing took ${elapsed}s`)

  return deduped
}

// ── Main ──

async function main() {
  console.log(`=== MX Railway Enrichment — Multi-feed GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // downloadAllGtfs is always needed — even a merged-stop-cache hit below still
  // needs a real extractDir per feed for the pair parser's own per-extractDir cache.
  const feeds = await downloadAllGtfs()
  const stopCacheHit = !forceDownload && existsSync(CACHE_FREQUENCIES)

  // ── Per-feed parse: declared families + heavy-rail STATION PAIRS (+ stops on
  //    cache miss). BIDIRECTIONAL completeness (2026-07-16 review item 4):
  //    every family a feed's own routes.txt declares must have parsed non-empty
  //    (declares rail → pairs>0, declares tram → tramStops>0; declares neither
  //    → exempt), evaluated by the shared `describeIncompleteFamilies`. That
  //    "declares neither" exemption is what un-sticks MX: bus-only
  //    toluca-movimex used to be demanded on BOTH sides, so retractSafe stayed
  //    false forever and the 120,434 legacy OLD_FALLBACK stamps could never be
  //    retracted. A malformed routes.txt THROWS (item 3,
  //    `declaredRouteFamiliesForFeed`) and is recorded as a PARSE FAILURE —
  //    never as "legitimately rail-less". Pairs are re-validated every run
  //    (their own per-extractDir cache in computeStopPairFrequenciesForFeed is
  //    fingerprinted against the GTFS inputs, item 2 — NOT recomputed from
  //    stop_times each run, the old comment here claiming "always computed
  //    fresh" was wrong); the tram direction is checked live on stop-cache
  //    miss and vouched by the v2 cache's recorded provenance on a hit
  //    (recording rule below is itself bidirectional, so a recorded feed means
  //    BOTH directions held at write). Pairs use default
  //    familyOf/dateSelection/expandFrequencies — they match MX's own needs
  //    (rail-only routes, midpoint-Wednesday, auto-detect frequencies.txt),
  //    no per-feed override needed, unlike europe's per-feed allow-lists.
  const perFeedPairs: RailStationPairCount[][] = []
  const perFeedCounts: StopTrainCount[][] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  for (const { feed, dir } of feeds) {
    try {
      const declared = await declaredRouteFamiliesForFeed(dir, routeFamily)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, { bbox: MX_BBOX, optionsKey: 'mx-national' })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        // computeStopFrequenciesWithHeadwayBoost reads routes.txt through
        // core's computeActiveTripFamiliesForFeed, which THROWS on a malformed
        // file — this try/catch covers it, so a broken extract records a
        // PARSE FAILURE instead of masquerading as a bus-only feed.
        const counts = await computeStopFrequenciesWithHeadwayBoost(feed, dir, MX_BBOX)
        perFeedCounts.push(counts)
        tramN = counts.filter(s => s.family === 'tram').length
      }
      const issue = describeIncompleteFamilies(feed.id, declared, pairs.length, tramN)
      if (issue) feedIssues.push(issue)
      else completeFeedIds.push(feed.id)
      console.log(`  [${feed.id}] ${pairs.length} heavy-rail station pairs; declares [${[...declared].sort().join('+') || 'no rail family'}]`)
    } catch (err: any) {
      console.error(`  [${feed.id}] PARSE FAILED: ${err.message}`)
      feedIssues.push(`${feed.id}: parse failure — ${err.message}`)
    }
  }
  const pairs = perFeedPairs.flat()
  // Feeds not present at all this run (download failed / --enrich-only with no
  // cached extract).
  const missingDetail = describeIncompleteFeeds(FEEDS.map(f => f.id), feeds.map(({ feed }) => feed.id))

  // ── Tram stops: the merged v2 cache (the pre-existing download marker;
  //    unchanged path) or this run's own per-feed parses. ──
  let mergedStops: StopTrainCount[]
  let stopsUnsafeDetail: string
  if (stopCacheHit) {
    console.log(`  Using cached merged stop frequencies: ${CACHE_FREQUENCIES}`)
    const cached = readMergedStopCache<StopTrainCount>(CACHE_FREQUENCIES)
    mergedStops = cached.stops
    stopsUnsafeDetail = cached.feedsLoadedNonEmpty === null
      ? `legacy merged cache without feed provenance — delete ${CACHE_FREQUENCIES} to rebuild from the cached feed extracts`
      : describeIncompleteFeeds(FEEDS.map(f => f.id), cached.feedsLoadedNonEmpty)
    console.log(`  ${mergedStops.length} stops in cache`)
  } else {
    mergedStops = dedupeStopsByLocation(perFeedCounts.flat())
    stopsUnsafeDetail = '' // live per-feed evaluation already sits in feedIssues
    if (feedIssues.length === 0 && missingDetail === '') {
      // Recording rule (item 4): a feed lands in the cache's provenance only
      // when it passed the FULL bidirectional check this run — a bus-only feed
      // like toluca-movimex counts as complete (it declares no family), so its
      // zero stops no longer block the cache write.
      writeMergedStopCache(CACHE_FREQUENCIES, completeFeedIds, mergedStops)
      console.log(`  Cached merged frequencies to ${CACHE_FREQUENCIES}`)
    } else {
      // Never persist a partial snapshot: a poisoned cache would silently starve
      // every later cache-served run (both enrichment and the retract evidence).
      console.log(`  NOT caching partial merged snapshot (${[...feedIssues, missingDetail].filter(Boolean).join('; ')})`)
    }
  }

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE
  // snapshot for BOTH families this run carries — neither direction may mask
  // the other, since ONE retract object below covers every row regardless of
  // family.
  const retractUnsafeDetail = [missingDetail, ...feedIssues, stopsUnsafeDetail].filter(Boolean).join('; ')
  const retractSafe = retractUnsafeDetail === ''
  if (!retractSafe) logRetractSkippedIncompleteInputs(retractUnsafeDetail)

  const tramStops = mergedStops.filter(s => s.family === 'tram')

  if (pairs.length === 0 && tramStops.length === 0) {
    console.log(`\nNo GTFS data to enrich. Exiting.`)
    return
  }

  console.log(`\n  ${pairs.length} heavy-rail station pairs, ${tramStops.length} tram/light-rail stops`)

  // COUNTRY GATE (#26C): a national feed can carry international through-services,
  // so the raw stop/pair lists may contain foreign stations — joining those would
  // stamp a neighbour's track under this feed's id, and the same-rank higher-id
  // tiebreak can beat the neighbour's own national source (mechanism: the PL feed
  // stamped 11,856 km of CZ track, 7fac2349). A national feed only speaks for its
  // own country's network. #31.7 central country gate — see writeRailTrains /
  // rail-walk-enrich.ts.
  const inMX = makeCountryGate('MX')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inMX,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Mexico is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inMX,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — MX has no retired predecessor id to absorb (confirmed: only
    // SOURCE_ID_MX_NATIONAL_RAILWAY was ever stamped by this file). The
    // pre-2026-07-10 OLD_FALLBACK class-default retract is deleted outright
    // (not preserved via sourceIds composition): the driver's own generic
    // retract — disown a previously-owned row the walk/tram-join no longer
    // covers, given retractSafe — already subsumes it.
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: MX is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'mx',
      extractFingerprint: `mx-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
      feeds: feeds.map(({ feed }) => feed.id),
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

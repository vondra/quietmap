/**
 * Enrich FI railways.arrow with multi-feed Finnish GTFS — migrated onto the
 * shared graph-walk driver, following `enrich-railway-dk.ts`'s per-country
 * pattern and extended for FI's four-feed roster.
 *
 * Sources: rata.digitraffic.fi (Fintraffic VR national rail),
 * infopalvelut.storage.hsldev.com (HSL Helsinki commuter + metro + tram),
 * data.itsfactory.fi (Tampere tram + bus), data.foli.fi (Turku/Föli — bus +
 * ferry only, kept for completeness).
 *
 * HEAVY RAIL (rail_type 0) runs on `lib/rail-walk-enrich.ts`
 * (`computeStopPairFrequenciesForFeed` -> station-PAIR frequencies ->
 * `enrichRailwaysByGraphWalk`'s shortest-path stamp). TRAM/METRO (rail_type
 * 1/2) keeps the pre-Phase-4 `nearestGridStop` 500 m stop-join
 * (`buildTramExtraMatch`, hoisted to `lib/gtfs-enrich-core.ts`), wired in as
 * the walk driver's `extraMatch` fallback arm. Helsinki metro has NO distinct
 * 3rd bucket here (unlike ae/th): `routeFamily` already groups GTFS
 * route_type 1 (subway/metro) with tram — HSL's 4 route_type=1 routes ARE
 * the Helsinki metro and fall into `tram` through the shared classifier, no
 * fi-local override needed (verified against the real HSL extract).
 *
 * HSL × VR OVERLAP — REVERTED (2026-07-16 review item 6, verified against
 * `data/enrichment/2026/fi/gtfs-*`, not estimated): the migration briefly
 * zeroed HSL's heavy-rail PAIR contribution (`hslExcludeHeavyRail`, deleted)
 * because all 113 of HSL's rail-tagged stop occurrences sit within 300 m of a
 * fintraffic-vr stop under the identical station name (Helsinki, Pasila,
 * Tikkurila, Kerava, ...). Stop overlap does NOT carry frequency equivalence:
 * HSL's chosen service day (2026-05-13, its calendar-midpoint Wednesday) runs
 * 929 active route_type=109 trips across 12 lines (A,D,E,I,K,L,P,R,T,U,Y,Z —
 * 13 letters declared incl. H, which has 0 trips that day), while
 * fintraffic-vr's own midpoint-Wednesday lands on 2027-02-17 — the
 * pathological far-future midpoint of its 2026-04-04..2027-12-31 calendar
 * span — with only 6 active route_type=109 trips (lines I/K). Zeroing HSL
 * therefore gutted Helsinki suburban rail almost entirely, so BOTH feeds now
 * contribute pairs through the default rail classifier. Accepted tradeoff:
 * where the two describe the same physical stretch, VR's ~6 trips/day
 * double-count on top of HSL's 929 — negligible vs. losing the 929. DATA NOTE
 * (sweep-phase worklist, deliberately NOT fixed here): VR's target-day
 * pathology — the shared midpoint-Wednesday picker selects a nearly
 * service-empty far-future day for any feed whose calendar span stretches
 * years ahead — deserves a per-feed date-selection review.
 *
 * Tampere and Föli-Turku are geographically/modally disjoint from
 * fintraffic-vr and HSL (Tampere: 2 tram + 115 bus routes, zero rail-family;
 * Föli-Turku: 157 bus + 2 ferry, zero rail AND zero tram) — plain
 * concatenation of their pairs/stops is correct.
 *
 * retractSafe completeness is DATA-DRIVEN per feed (dk.ts pattern, 2026-07-16
 * review item 4 — replaces the deleted hand-maintained
 * `STOPS_RELEVANT_FEED_IDS`/`PAIRS_RELEVANT_FEED_IDS` sets):
 * `declaredRouteFamiliesForFeed` reads which families each feed's own
 * routes.txt declares, and `describeIncompleteFamilies` demands every declared
 * family parsed non-empty BIDIRECTIONALLY (declares rail → station pairs > 0;
 * declares tram → tram stops > 0; declares neither → exempt). Föli-Turku's
 * bus+ferry routes.txt declares neither family, so it is automatically exempt
 * from both directions (the old hand-sets encoded exactly this by hand);
 * Tampere declares tram only and is judged on its tram stops; HSL and VR are
 * judged on what they declare. A malformed routes.txt THROWS and is recorded
 * as a parse failure — never as "legitimately rail-less".
 *
 * FI's source id is NATIONALLY OWNED: `bleedGate` = `countryGate` (a row
 * wholly outside Finland is disownable on geometry alone). No predecessor
 * source id survives (FI only ever used ONE id) — the driver's own generic
 * retract subsumes the deleted pre-2026-07-10 OLD_FALLBACK class-default
 * check.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; FI is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-fi.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-fi.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-fi.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-fi.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-europe.ts) — enableDestructive=false: the run still
 * walk-stamps heavy-rail counts AND divisors and still runs the tram
 * stop-join, but NO retract and NO country-bleed heal fire.
 */

import { writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { SOURCE_ID_FI_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import {
  computeStopFrequenciesForFeed, dedupeStopsByLocation, buildTramExtraMatch, routeFamily,
  declaredRouteFamiliesForFeed, describeIncompleteFamilies,
  describeIncompleteFeeds, logRetractSkippedIncompleteInputs, readMergedStopCache, writeMergedStopCache,
  GTFS_BORDER_MARGIN_DEG, type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_FI_NATIONAL_RAILWAY

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/fi`)
// Versioned filename: the family-aware schema added a mandatory `family` field, so a
// pre-migration cache must NOT be reused (a family-less stop would fall into tramGrid →
// heavy rail loses its count, trams re-inherit it). Kept UNCHANGED across this migration
// (manifest note): this remains the RAIL_CACHED_DOWNLOAD marker file chain/manifest.ts
// checks — the pair parser adds its OWN per-extractDir cache alongside it, never replacing it.
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-family-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

interface FeedConfig {
  id: string
  name: string
  urls: string[]
}

const FEEDS: FeedConfig[] = [
  {
    id: 'fintraffic-vr',
    name: 'Fintraffic VR (national passenger rail incl. commuter)',
    urls: [
      'https://rata.digitraffic.fi/api/v1/trains/gtfs-passenger.zip',
    ],
  },
  {
    id: 'hsl-helsinki',
    name: 'HSL Helsinki Region Transport (commuter rail + metro + tram + bus)',
    urls: [
      'https://infopalvelut.storage.hsldev.com/gtfs/hsl.zip',
    ],
  },
  {
    id: 'tampere',
    name: 'Tampere (tram + bus)',
    urls: [
      'http://data.itsfactory.fi/journeys/files/gtfs/latest/gtfs_tampere.zip',
    ],
  },
  {
    id: 'foli-turku',
    name: 'Föli Turku (bus, kept for completeness — no tram)',
    urls: [
      'http://data.foli.fi/gtfs/gtfs.zip',
    ],
  },
]

// Finland mainland bounding box
const FI_BBOX: [number, number, number, number] = [59.7, 19.1, 70.1, 31.6]

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
          headers: {
            'Accept': 'application/zip, application/octet-stream, */*',
            'Accept-Encoding': 'gzip', // Fintraffic Digitraffic requires this
          },
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
    throw new Error('Failed to download any Finnish GTFS feed')
  }

  console.log(`  ${results.length}/${FEEDS.length} FI feeds available`)
  return results
}

// ── Step 2/3: pair the padded country bbox with the graph-walk driver ──

/** Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in. Stop/pair PARSING itself
 *  (`computeStopFrequenciesForFeed`, `computeStopPairFrequenciesForFeed`) gets
 *  the UNPADDED `FI_BBOX`, shared across all 4 feeds (unchanged from the
 *  pre-Phase-4 file — none of these feeds carries its own tighter bbox) — the
 *  margin is applied internally by `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  FI_BBOX[0] - GTFS_BORDER_MARGIN_DEG, FI_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  FI_BBOX[2] + GTFS_BORDER_MARGIN_DEG, FI_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

// ── Main ──

async function main() {
  console.log(`=== FI Railway Enrichment — Multi-feed GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
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
  //    → exempt — Föli-Turku's automatic exemption), evaluated by the shared
  //    `describeIncompleteFamilies`. A malformed routes.txt THROWS (item 3,
  //    `declaredRouteFamiliesForFeed`) and is recorded as a PARSE FAILURE —
  //    never as "legitimately rail-less". Every feed — INCLUDING hsl-helsinki,
  //    whose narrowing was reverted (module doc's HSL × VR OVERLAP) — uses the
  //    default RAIL_TYPES-only pair classifier. Pairs are re-validated every
  //    run (their own per-extractDir cache in computeStopPairFrequenciesForFeed
  //    is fingerprinted against the GTFS inputs, item 2 — NOT recomputed from
  //    stop_times each run, the old comment here claiming "always computed
  //    fresh" was wrong); the tram direction is checked live on stop-cache miss
  //    and vouched by the v2 cache's recorded provenance on a hit (recording
  //    rule below is itself bidirectional, so a recorded feed means BOTH
  //    directions held at write).
  const perFeedPairs: RailStationPairCount[][] = []
  const perFeedCounts: StopTrainCount[][] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  for (const { feed, dir } of feeds) {
    try {
      const declared = await declaredRouteFamiliesForFeed(dir, routeFamily)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, { bbox: FI_BBOX })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, FI_BBOX)
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
  // Plain concatenation across feeds — disjoint city networks sum correctly, and
  // the HSL/VR overlap is an ACCEPTED ~6 trips/day double-count (module doc). The
  // walk's own canonical accumulator sums by OD key regardless of how many arrays
  // contributed.
  const pairs = perFeedPairs.flat()
  // Feeds not present at all this run (--enrich-only with no cached extract, or a
  // tolerated download failure).
  const missingDetail = describeIncompleteFeeds(FEEDS.map(f => f.id), feeds.map(({ feed }) => feed.id))

  // ── Tram stops: the merged v2 cache (the pre-existing download marker;
  //    unchanged path) or this run's own per-feed parses. ──
  let mergedStops: StopTrainCount[]
  let stopsUnsafeDetail: string
  if (stopCacheHit) {
    console.log(`  Using cached merged stop frequencies: ${CACHE_FREQUENCIES}`)
    const cached = readMergedStopCache<StopTrainCount>(CACHE_FREQUENCIES)
    mergedStops = cached.stops
    // 4 configured feeds — no FEEDS.length===1 shortcut (that's only sound for a
    // single-feed country file): a legacy bare-array cache here is unconditionally
    // retract-unsafe until rebuilt.
    stopsUnsafeDetail = cached.feedsLoadedNonEmpty === null
      ? `legacy merged cache without feed provenance — delete ${CACHE_FREQUENCIES} to rebuild from the cached feed extracts`
      : describeIncompleteFeeds(FEEDS.map(f => f.id), cached.feedsLoadedNonEmpty)
    console.log(`  ${mergedStops.length} stops in cache`)
  } else {
    mergedStops = dedupeStopsByLocation(perFeedCounts.flat())
    stopsUnsafeDetail = '' // live per-feed evaluation already sits in feedIssues
    if (feedIssues.length === 0 && missingDetail === '') {
      // Recording rule (item 4): a feed lands in the cache's provenance only
      // when it passed the FULL bidirectional check this run — a later
      // cache-served run inherits completeness both ways, not any-family.
      // Föli-Turku is recorded despite zero stops: declaring neither family,
      // it is complete by exemption.
      writeMergedStopCache(CACHE_FREQUENCIES, completeFeedIds, mergedStops)
      console.log(`  Cached merged frequencies to ${CACHE_FREQUENCIES}`)
    } else {
      // Never persist a partial snapshot: a poisoned cache would silently starve
      // every later cache-served run (both enrichment and the retract evidence).
      console.log(`  NOT caching partial merged snapshot (${[...feedIssues, missingDetail].filter(Boolean).join('; ')})`)
    }
  }

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE
  // snapshot for BOTH families — a working tram part must never mask an empty
  // heavy-rail pairs part (or vice versa), since ONE retract object below covers
  // every row regardless of family.
  const retractUnsafeDetail = [missingDetail, ...feedIssues, stopsUnsafeDetail].filter(Boolean).join('; ')
  const retractSafe = retractUnsafeDetail === ''
  if (!retractSafe) logRetractSkippedIncompleteInputs(retractUnsafeDetail)

  const tramStops = mergedStops.filter(s => s.family === 'tram')

  if (pairs.length === 0 && tramStops.length === 0) {
    console.log(`\nNo GTFS data to enrich. Exiting.`)
    return
  }

  console.log(`\n  ${pairs.length} heavy-rail station pairs, ${tramStops.length} tram/light-rail/metro stops`)

  // COUNTRY GATE (#26C): a national feed can carry international through-services,
  // so this id must not speak for track outside Finland — same rationale as every
  // other national enricher (mechanism: the PL feed once stamped 11,856 km of CZ
  // track, 7fac2349). #31.7 central country gate — see writeRailTrains /
  // rail-walk-enrich.ts.
  const inFi = makeCountryGate('FI')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inFi,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Finland is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inFi,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — FI has no retired predecessor id to absorb. The pre-2026-07-10
    // OLD_FALLBACK class-default retract is deleted outright (not preserved via
    // sourceIds composition): the driver's own generic retract — disown a
    // previously-owned row the walk/tram-join no longer covers, given
    // retractSafe — already subsumes it and is strictly more correct (it
    // re-evaluates real coverage every run instead of pattern-matching one
    // frozen class-default tuple).
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: FI is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'fi',
      extractFingerprint: `fi-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

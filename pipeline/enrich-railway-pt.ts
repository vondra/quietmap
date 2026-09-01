/**
 * Enrich PT railways.arrow with train frequencies from Portuguese GTFS feeds —
 * migrated onto the shared graph-walk driver following `enrich-railway-dk.ts`'s
 * pattern extended to multiple feeds like
 * `enrich-railway-europe.ts`).
 *
 * Downloads multiple Portuguese rail/metro GTFS feeds (Comboios de Portugal,
 * Metro do Porto, Metro Sul Tejo, Carris Metropolitana de Lisboa) and merges
 * stop frequencies, then matches GTFS stops to OSM railway segments.
 *
 * Overlap check (2026-07-16, real route_type data inspected from the cached
 * extracts): `cp-comboios` is the ONLY feed that ever carries a `RAIL_TYPES`
 * route (route_type 2 ×128, 109 ×61 — CP's own regional/suburban/intercity
 * classification). `metro-porto` is entirely route_type 1 (subway/metro),
 * `metro-sul-tejo` entirely route_type 0 (tram/light-rail) — both TRAM_TYPES,
 * never RAIL_TYPES. `carris-metropolitana` is entirely route_type 3 (bus) —
 * despite its name suggesting "AML buses + commuter rail", the actual extract
 * carries ZERO rail-family routes (it is the Lisbon-area BUS network only,
 * commuter RAIL there is CP's own). With only one feed ever contributing to
 * the 'rail' family, plain concat across all 4 is trivially correct — no
 * `familyOf` narrowing needed (unlike PL's warsaw-ztm/SKM mirror case).
 *
 * HEAVY RAIL (rail_type 0) runs on `lib/rail-walk-enrich.ts`:
 * `computeStopPairFrequenciesForFeed` (lib/gtfs-stop-pairs.ts) turns
 * stop_times.txt into station-PAIR frequencies per feed, concatenated (every
 * feed contributes through the walk driver's own canonical accumulator) and
 * walked along the shortest path by `enrichRailwaysByGraphWalk`. TRAM/light-rail
 * (rail_type 1/2 — Metro do Porto, Metro Sul do Tejo) keeps the pre-Phase-4
 * `nearestGridStop` 500 m stop-join (`buildTramExtraMatch`, hoisted to
 * `lib/gtfs-enrich-core.ts` so this file shares the SAME implementation every
 * other national enricher uses), wired in as the walk driver's `extraMatch`
 * fallback arm.
 *
 * PT's source id is NATIONALLY OWNED (unlike europe's shared
 * `SOURCE_ID_GLOBAL_GTFS_TRANSIT`): `bleedGate` is set to the SAME country
 * gate as `countryGate` — a row wholly outside Portugal is disownable on
 * geometry alone, since this id never legitimately stamps outside PT. No
 * predecessor source id survives from the pre-2026-07-10 class-default
 * fallback design (PT only ever used ONE id): the driver's own generic retract
 * (disown a previously-owned row the walk/tram-join no longer covers, given
 * `retractSafe`) already subsumes the old OLD_FALLBACK exact-tuple check, so
 * that class-default table is deleted outright rather than preserved via
 * `sourceIds` composition.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; PT is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pt.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pt.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pt.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pt.ts --stamp-only
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
import { SOURCE_ID_PT_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
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

const MY_SOURCE_ID = SOURCE_ID_PT_NATIONAL_RAILWAY

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/pt`)
// Versioned filename: the family-aware schema added a mandatory `family` field, so a
// pre-migration cache must NOT be reused (a family-less stop would fall into tramGrid →
// heavy rail loses its count, trams re-inherit it). Kept UNCHANGED across this migration
// (manifest note): this remains the RAIL_CACHED_DOWNLOAD marker file chain/manifest.ts
// checks — the pair parser adds its OWN per-extractDir cache alongside it, never replacing it.
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-family-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

// Portugal multi-feed: national rail (CP) + metro Porto + metro sul Tejo +
// Carris Metropolitana (Lisboa metropolitan area buses)
interface FeedConfig {
  id: string
  name: string
  urls: string[]
}

const FEEDS: FeedConfig[] = [
  {
    id: 'cp-comboios',
    name: 'Comboios de Portugal (CP) — national rail',
    urls: [
      'https://publico.cp.pt/gtfs/gtfs.zip',
    ],
  },
  {
    id: 'metro-porto',
    name: 'Metro do Porto',
    urls: [
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/pt-porto-metro-do-porto-gtfs-2357.zip?alt=media',
    ],
  },
  {
    id: 'metro-sul-tejo',
    name: 'Metro Sul do Tejo (Almada)',
    urls: [
      'https://mts.pt/imt/MTS-20240129.zip',
    ],
  },
  {
    id: 'carris-metropolitana',
    name: 'Carris Metropolitana de Lisboa (AML buses)',
    urls: [
      'https://api.carrismetropolitana.pt/v2/gtfs',
    ],
  },
]

// Portugal mainland bounding box (excludes Azores/Madeira)
const PT_BBOX: [number, number, number, number] = [36.5, -10.0, 42.5, -6.0] // [minLat, minLon, maxLat, maxLon]

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
    throw new Error('Failed to download any Portuguese GTFS feed')
  }

  console.log(`  ${results.length}/${FEEDS.length} PT feeds available`)
  return results
}

// ── Step 2/3: pair the padded country bbox with the graph-walk driver ──

/** Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in, or a cross-border through-train's
 *  foreign endpoint never snaps and the whole pair quarantines for no real
 *  reason (CP runs Celta/Sud legs into Vigo/Spain). Stop/pair PARSING itself
 *  (`computeStopFrequenciesForFeed`, `computeStopPairFrequenciesForFeed`)
 *  gets the UNPADDED `PT_BBOX` — the margin is applied internally by
 *  `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  PT_BBOX[0] - GTFS_BORDER_MARGIN_DEG, PT_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  PT_BBOX[2] + GTFS_BORDER_MARGIN_DEG, PT_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

// ── Main ──

async function main() {
  console.log(`=== PT Railway Enrichment — Multi-feed GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
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
  //    "declares neither" exemption is what un-sticks PT: `carris-metropolitana`
  //    is pure bus (952 route_type=3 routes, see module doc overlap check) —
  //    the old stop-side check demanded non-empty stops from it anyway, so
  //    retractSafe stayed false forever and the 27,595 legacy OLD_FALLBACK
  //    stamps could never be retracted. Now it declares no family and is
  //    exempt both ways. A malformed routes.txt THROWS (item 3,
  //    `declaredRouteFamiliesForFeed`) and is recorded as a PARSE FAILURE —
  //    never as "legitimately rail-less". Pairs are re-validated every run
  //    (their own per-extractDir cache in computeStopPairFrequenciesForFeed is
  //    fingerprinted against the GTFS inputs, item 2 — NOT recomputed from
  //    stop_times each run, the old comment here claiming "always computed
  //    fresh" was wrong); the tram direction is checked live on stop-cache
  //    miss and vouched by the v2 cache's recorded provenance on a hit
  //    (recording rule below is itself bidirectional, so a recorded feed means
  //    BOTH directions held at write). Plain pair concat across feeds: only
  //    cp-comboios ever carries a rail-family route (see module doc overlap
  //    check) — the walk's own canonical accumulator sums by station-pair key
  //    regardless.
  const perFeedPairs: RailStationPairCount[][] = []
  const perFeedCounts: StopTrainCount[][] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  for (const { feed, dir } of feeds) {
    try {
      const declared = await declaredRouteFamiliesForFeed(dir, routeFamily)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, { bbox: PT_BBOX, optionsKey: 'pt-default' })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, PT_BBOX)
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
      // like carris-metropolitana counts as complete (it declares no family),
      // so its zero stops no longer block the cache write.
      writeMergedStopCache(CACHE_FREQUENCIES, completeFeedIds, mergedStops)
      console.log(`  Cached merged frequencies to ${CACHE_FREQUENCIES}`)
    } else {
      // Never persist a partial snapshot: a poisoned cache would silently starve
      // every later cache-served run (both enrichment and the retract evidence).
      console.log(`  NOT caching partial merged snapshot (${[...feedIssues, missingDetail].filter(Boolean).join('; ')})`)
    }
  }

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE
  // snapshot for BOTH families every feed carries — neither direction may mask
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

  // COUNTRY GATE (#26C): a national feed can carry international through-services
  // (CP runs Celta/Sud legs into Vigo/Spain), so this id must not speak for
  // track outside Portugal — same rationale as every other national enricher
  // (mechanism: the PL feed once stamped 11,856 km of CZ track, 7fac2349).
  // #31.7 central country gate — see writeRailTrains / rail-walk-enrich.ts.
  const inPt = makeCountryGate('PT')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inPt,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Portugal is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inPt,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — PT has no retired predecessor id to absorb. The pre-2026-07-10
    // OLD_FALLBACK class-default retract is deleted outright (not preserved via
    // sourceIds composition): the driver's own generic retract — disown a
    // previously-owned row the walk/tram-join no longer covers, given
    // retractSafe — already subsumes it and is strictly more correct (it
    // re-evaluates real coverage every run instead of pattern-matching one
    // frozen class-default tuple).
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: PT is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'pt',
      extractFingerprint: `pt-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

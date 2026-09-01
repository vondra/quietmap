/**
 * Enrich CA railways.arrow with real train frequencies from Canadian GTFS feeds —
 * migrated onto the shared graph-walk driver following `enrich-railway-dk.ts`'s
 * single-feed pattern, extended here for eight city/national feeds: via-rail, go-transit,
 * ttc-toronto, stm-montreal, translink-vancouver, oc-transpo, calgary-transit,
 * edmonton-ets.
 *
 * HEAVY RAIL (rail_type 0) now runs on `lib/rail-walk-enrich.ts`:
 * `computeStopPairFrequenciesForFeed` (lib/gtfs-stop-pairs.ts) turns each feed's
 * stop_times.txt into station-PAIR frequencies, walked along the shortest path by
 * `enrichRailwaysByGraphWalk`. THREE feeds declare heavy rail (route_type 2), not
 * two: via-rail, go-transit AND translink-vancouver — TransLink is not just
 * SkyTrain, its routes.txt line 229 carries the West Coast Express commuter train
 * (route_id 6770, route_type=2, 10 active trips on the chosen 2026-03-25 target
 * day). The migration's hand-maintained "only VIA + GO carry heavy rail" set
 * (`HEAVY_RAIL_FEED_IDS`, deleted 2026-07-16 review item 5) silently exempted WCE
 * from retract completeness — a broken WCE parse could have been masked by working
 * SkyTrain data — so which feeds owe which family is now DATA-DRIVEN:
 * `declaredRouteFamiliesForFeed` reads it from each feed's own routes.txt (see
 * the completeness paragraph below). The remaining 5 feeds are metro/streetcar/LRT
 * systems whose GTFS tags them route_type 1 (subway/metro) or 0/900s (tram), never
 * mapped to 'rail' by the DEFAULT `familyOf`. That is CORRECT, not a coverage gap:
 * (1) TTC/STM's underground and TransLink's SkyTrain track is OSM
 * `railway=subway`, which `osm-extract` never even extracts into railways.arrow
 * (classify/ways.rs accepts only rail/tram/light_rail/narrow_gauge/funicular);
 * (2) Calgary C-Train/Edmonton LRT/OC Transpo's O-Train are OSM
 * `railway=light_rail` — tram/light-rail's remit, not heavy rail's. Unlike
 * europe's au-vic Metro Trains, none of CA's 8 feeds needs a `metroAsRail`-style
 * override: the PRE-migration `enrichHexes` already routed every metro/tram GTFS
 * type into `tramGrid`, never `railGrid` (routeFamily groups METRO_TYPES with tram
 * by default) — the walk's default rail-only familyOf simply preserves that, so no
 * per-feed FeedConfig registry is needed; the flat `{id, name, urls}` shape stays
 * as-is.
 *
 * Plain concatenation across all 8 feeds' pairs/tram stops is correct: the city
 * transit systems each run in their own disjoint city with no shared physical
 * track (mirrors europe's au-vic/au-qld reasoning). via-rail, go-transit and
 * TransLink's West Coast Express are independent GTFS exports of DIFFERENT real
 * trains (no shared trip_ids, unlike europe's fr/fr-idf pair) — even where their
 * corridors physically overlap (e.g. Toronto Union/Lakeshore for VIA+GO), summing
 * their pairs is the correct total: distinct train movements sharing track, not
 * two descriptions of the same trip.
 *
 * TRAM/light-rail (rail_type 1/2) keeps the pre-Phase-4 `nearestGridStop` 500 m
 * stop-join (`buildTramExtraMatch`, hoisted to `lib/gtfs-enrich-core.ts`), wired in
 * as the walk driver's `extraMatch` fallback arm — this is where the 6 city transit
 * feeds do all their real work; via-rail/go-transit contribute essentially nothing
 * here (no tram-family routes of their own).
 *
 * CA's source id is NATIONALLY OWNED (SOURCE_ID_CA_NATIONAL_RAILWAY, unchanged):
 * `bleedGate` is set to the SAME country gate as `countryGate` — a row wholly
 * outside Canada is disownable on geometry alone (VIA/Amtrak joint runs cross into
 * the US, same mechanism as the PL-feed-stamped-CZ-track bug, 7fac2349). No
 * predecessor source id survives from the pre-2026-07-10 class-default fallback
 * design (CA only ever used ONE id): the driver's own generic retract already
 * subsumes the old OLD_FALLBACK exact-tuple check, so that class-default table is
 * deleted outright rather than preserved via `sourceIds` composition.
 *
 * retractSafe completeness is DATA-DRIVEN per feed (dk.ts pattern, 2026-07-16
 * review items 4+5): `declaredRouteFamiliesForFeed` reads which families each
 * feed's own routes.txt declares, and `describeIncompleteFamilies` demands every
 * declared family parsed non-empty BIDIRECTIONALLY (declares rail → station
 * pairs > 0; declares tram → tram stops > 0; declares neither → exempt). This
 * replaced the hand-maintained heavy-rail feed set, which was both data-wrong
 * (TransLink's West Coast Express, above) and unidirectional (a feed whose tram
 * parse silently emptied could still read complete). A malformed routes.txt
 * THROWS and is recorded as a parse failure — never as "legitimately rail-less".
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on purpose —
 * the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns unknowns;
 * CA is not silent-residual-capable (that exists only for CZ's timetable, owner
 * decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ca.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ca.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ca.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ca.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-dk.ts) — enableDestructive=false: the run still walk-stamps
 * heavy-rail counts AND divisors and still runs the tram stop-join, but NO retract
 * and NO country-bleed heal fire.
 */

import { writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { SOURCE_ID_CA_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
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

const MY_SOURCE_ID = SOURCE_ID_CA_NATIONAL_RAILWAY

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ca`)
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
    id: 'via-rail',
    name: 'VIA Rail Canada (national passenger)',
    urls: [
      'https://www.viarail.ca/sites/all/files/gtfs/viarail.zip',
    ],
  },
  {
    id: 'go-transit',
    name: 'GO Transit Metrolinx (Toronto/Hamilton commuter)',
    urls: [
      'https://assets.metrolinx.com/raw/upload/Documents/Metrolinx/Open%20Data/GO-GTFS.zip',
    ],
  },
  {
    id: 'ttc-toronto',
    name: 'TTC Toronto (subway + streetcar + bus)',
    urls: [
      'http://opendata.toronto.ca/toronto.transit.commission/ttc-routes-and-schedules/OpenData_TTC_Schedules.zip',
    ],
  },
  {
    id: 'stm-montreal',
    name: 'STM Montréal (metro + bus)',
    urls: [
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/ca-quebec-societe-de-transport-de-montreal-stm-gtfs-2126.zip?alt=media',
    ],
  },
  {
    id: 'translink-vancouver',
    name: 'TransLink Vancouver (SkyTrain + SeaBus + bus)',
    urls: [
      'https://gtfs-static.translink.ca/gtfs/google_transit.zip',
    ],
  },
  {
    id: 'oc-transpo',
    name: 'OC Transpo Ottawa (O-Train + bus)',
    urls: [
      'https://oct-gtfs-emasagcnfmcgeham.z01.azurefd.net/public-access/GTFSExport.zip',
    ],
  },
  {
    id: 'calgary-transit',
    name: 'Calgary Transit (C-Train LRT + bus)',
    urls: [
      'https://data.calgary.ca/download/npk7-z3bj/application%2Fzip',
    ],
  },
  {
    id: 'edmonton-ets',
    name: 'Edmonton ETS (LRT + bus)',
    urls: [
      'https://gtfs.edmonton.ca/TMGTFSRealTimeWebService/GTFS/GTFS.zip',
    ],
  },
]

// Canada bbox — the stop/pair PARSING envelope (unpadded). GRAPH_BBOX below pads it
// for the walk's own graph-building scope, same split as enrich-railway-dk.ts.
const CA_BBOX: [number, number, number, number] = [41.5, -141.0, 84.0, -52.0]

// Same 0.5° margin as loadStopsWithCoords's GTFS geometry envelope (item 6,
// gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a cross-border
// stop's pair could land in (VIA/Amtrak joint runs cross into the US), or such a
// pair's foreign endpoint never snaps and the whole pair quarantines for no real
// reason. Stop/pair PARSING itself gets the UNPADDED CA_BBOX — the margin is applied
// internally by loadStopsWithCoords.
const GRAPH_BBOX: [number, number, number, number] = [
  CA_BBOX[0] - GTFS_BORDER_MARGIN_DEG, CA_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  CA_BBOX[2] + GTFS_BORDER_MARGIN_DEG, CA_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

// ── Step 1: Download GTFS feeds (UNCHANGED — per-feed cache/download quirks) ──

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
    throw new Error('Failed to download any Canadian GTFS feed')
  }

  console.log(`  ${results.length}/${FEEDS.length} CA feeds available`)
  return results
}

// ── Main ──

async function main() {
  console.log(`=== CA Railway Enrichment — Multi-feed GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // downloadAllGtfs is always needed — even a merged-stop-cache hit below still needs
  // a real extractDir per feed for the pair parser's own per-extractDir cache.
  const feeds = await downloadAllGtfs()
  const stopCacheHit = !forceDownload && existsSync(CACHE_FREQUENCIES)

  // ── Per-feed parse: declared families + heavy-rail STATION PAIRS (+ stops on
  //    cache miss). BIDIRECTIONAL completeness (2026-07-16 review items 4+5):
  //    every family a feed's own routes.txt declares must have parsed non-empty
  //    (declares rail → pairs>0, declares tram → tramStops>0; declares neither
  //    → exempt), evaluated by the shared `describeIncompleteFamilies`. This is
  //    what catches TransLink's West Coast Express — the deleted hand-set
  //    exempted it (module doc). A malformed routes.txt THROWS (item 3,
  //    `declaredRouteFamiliesForFeed`) and is recorded as a PARSE FAILURE —
  //    never as "legitimately rail-less". Pairs are re-validated every run
  //    (their own per-extractDir cache in computeStopPairFrequenciesForFeed is
  //    fingerprinted against the GTFS inputs, item 2 — NOT recomputed from
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
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, {
        bbox: CA_BBOX,
        // DEFAULT familyOf (rail-only) for every one of the 8 feeds — no metroAsRail
        // override needed, see the module doc's HEAVY RAIL paragraph.
        optionsKey: 'ca-rail-default',
      })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, CA_BBOX)
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
    // No single-feed shortcut (unlike enrich-railway-dk.ts's FEEDS.length===1 case) —
    // CA has 8 feeds, so a legacy bare-array cache without feed provenance is ALWAYS
    // retract-unsafe.
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
      writeMergedStopCache(CACHE_FREQUENCIES, completeFeedIds, mergedStops)
      console.log(`  Cached merged frequencies to ${CACHE_FREQUENCIES}`)
    } else {
      // Never persist a partial snapshot: a poisoned cache would silently starve
      // every later cache-served run (both enrichment and the retract evidence).
      console.log(`  NOT caching partial merged snapshot (${[...feedIssues, missingDetail].filter(Boolean).join('; ')})`)
    }
  }

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE snapshot
  // for BOTH families — a working tram part must never mask an empty heavy-rail pairs
  // part (or vice versa), since ONE retract object below covers every row regardless
  // of family.
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
  // (VIA/Amtrak joint runs cross into the US), so this id must not speak for track
  // outside Canada — same rationale as every other national enricher (mechanism: the
  // PL feed once stamped 11,856 km of CZ track, 7fac2349). #31.7 central country
  // gate — see writeRailTrains / rail-walk-enrich.ts.
  const inCa = makeCountryGate('CA')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inCa,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Canada is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inCa,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — CA has no retired predecessor id to absorb. The pre-2026-07-10
    // OLD_FALLBACK class-default retract is deleted outright (not preserved via
    // sourceIds composition): the driver's own generic retract — disown a
    // previously-owned row the walk/tram-join no longer covers, given
    // retractSafe — already subsumes it and is strictly more correct (it
    // re-evaluates real coverage every run instead of pattern-matching one
    // frozen class-default tuple).
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: CA is NOT silent-capable (silentResidual omitted) — a quarantine-free
    // row the walk never reaches simply stays at source_id=0 for the engine's own
    // class default; only CZ's timetable evidence (owner decision option b)
    // justifies a residual.
    sidecar: {
      scope: 'ca',
      extractFingerprint: `ca-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

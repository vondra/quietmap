/**
 * Enrich PL railways.arrow with train frequencies from Polish GTFS feeds —
 * migrated onto the shared graph-walk driver following `enrich-railway-dk.ts`'s
 * pattern extended to multiple feeds like
 * `enrich-railway-europe.ts`).
 *
 * Sources:
 *   mkuran.pl/gtfs/polish_trains.zip — unified daily-updated feed for all Polish
 *     train operators (PKP IC, PolRegio, Koleje Mazowieckie/Śląskie/Dolnośląskie/
 *     Wielkopolskie/Małopolskie, ŁKA, SKM Trójmiasto, SKM Warszawa, Arriva, etc.)
 *   mkuran.pl/gtfs/warsaw.zip — Warszawa ZTM tram + metro
 *   gtfs.ztp.krakow.pl/GTFS_KRK_T.zip — Kraków tram
 *   mkuran.pl/gtfs/gzm.zip — Górnośląska Metropolia (Silesian tram + bus)
 *   mkuran.pl/gtfs/wkd.zip — Warszawska Kolej Dojazdowa (suburban)
 *
 * MIRROR-PUBLISH OVERLAP (confirmed 2026-07-16 from the cached extracts, the
 * europe.ts fr-idf scenario repeated): `warsaw-ztm`'s routes.txt carries FIVE
 * route_type=2 routes — S1 Otwock–Pruszków, S2/S3/S4/S40 — that are byte-for-byte
 * the SAME SKM Warszawa lines `polish-trains` already carries under its own
 * `SKM` agency (route_ids `SKM_S1..SKM_S40`, identical termini). Warszawa's
 * WTP/ZTM feed republishes SKM's timetable for its own integrated-ticket
 * planner; `polish-trains` is the national authority for SKM (module doc
 * already lists "SKM Warszawa" among its aggregated operators). Counting both
 * would double-stamp every SKM Warszawa corridor. Fix (mirrors fr-idf): narrow
 * `warsaw-ztm`'s heavy-rail PAIRS contribution to null — its real payload for
 * this pipeline is tram (0) + metro (1), which the pairs mechanism never
 * counts anyway (`RAIL_TYPES`-only default), so this only suppresses the 5
 * duplicate SKM routes, never trams/metro (those stay on the per-stop
 * `nearestGridStop` join below, unaffected by `pairsFamilyOf`). `wkd-warszawa`
 * (WKD, a separate historic narrow corridor Warszawa Śródmieście–Grodzisk
 * Mazowiecki, not part of PKP infrastructure and NOT listed among
 * `polish-trains`' aggregated agencies) and `krakow-tram`/`silesia-gzm`
 * (tram+bus only, zero RAIL_TYPES routes) carry no such overlap.
 *
 * HEAVY RAIL (rail_type 0) runs on `lib/rail-walk-enrich.ts`:
 * `computeStopPairFrequenciesForFeed` (lib/gtfs-stop-pairs.ts) turns
 * stop_times.txt into station-PAIR frequencies per feed, concatenated (every
 * feed contributes through the walk driver's own canonical accumulator) and
 * walked along the shortest path by `enrichRailwaysByGraphWalk`. TRAM/light-rail
 * (rail_type 1/2 — Warszawa Metro + trams, Kraków/Silesia trams) keeps the
 * pre-Phase-4 `nearestGridStop` 500 m stop-join (`buildTramExtraMatch`, hoisted
 * to `lib/gtfs-enrich-core.ts` so this file shares the SAME implementation
 * every other national enricher uses), wired in as the walk driver's
 * `extraMatch` fallback arm.
 *
 * PL's source id is NATIONALLY OWNED (unlike europe's shared
 * `SOURCE_ID_GLOBAL_GTFS_TRANSIT`): `bleedGate` is set to the SAME country
 * gate as `countryGate` — a row wholly outside Poland is disownable on
 * geometry alone, since this id never legitimately stamps outside PL. No
 * predecessor source id survives from the pre-2026-07-10 class-default
 * fallback design (PL only ever used ONE id): the driver's own generic retract
 * (disown a previously-owned row the walk/tram-join no longer covers, given
 * `retractSafe`) already subsumes the old OLD_FALLBACK exact-tuple check, so
 * that class-default table is deleted outright rather than preserved via
 * `sourceIds` composition.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; PL is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pl.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pl.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pl.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-pl.ts --stamp-only
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
import { SOURCE_ID_PL_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import {
  computeStopFrequenciesForFeed, dedupeStopsByLocation, buildTramExtraMatch, routeFamily,
  declaredRouteFamiliesForFeed, describeIncompleteFamilies,
  describeIncompleteFeeds, logRetractSkippedIncompleteInputs, readMergedStopCache, writeMergedStopCache,
  GTFS_BORDER_MARGIN_DEG, RAIL_TYPES, type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_PL_NATIONAL_RAILWAY

const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/pl`)
// Versioned filename: the family-aware schema added a mandatory `family` field, so a
// pre-migration cache must NOT be reused (a family-less stop would fall into tramGrid →
// heavy rail loses its count, trams re-inherit it). Kept UNCHANGED across this migration
// (manifest note): this remains the RAIL_CACHED_DOWNLOAD marker file chain/manifest.ts
// checks — the pair parser adds its OWN per-extractDir cache alongside it, never replacing it.
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-family-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

// Poland multi-feed: unified national trains + Warszawa tram/metro + Kraków tram +
// Silesia tram + WKD suburban
interface FeedConfig {
  id: string
  name: string
  urls: string[]
  /** Narrow this feed's heavy-rail PAIRS contribution — omit for the default
   *  RAIL_TYPES-only classifier. `warsaw-ztm` sets this to always-null: its 5
   *  route_type=2 routes mirror-publish SKM Warszawa lines `polish-trains`
   *  already carries (see module doc) — narrowing prevents double-stamping
   *  those corridors, mirroring europe.ts's fr-idf narrowing. The
   *  declared-families completeness check (`declaredRouteFamiliesForFeed`)
   *  uses this SAME narrowed classifier for its rail direction, so a narrowed
   *  feed never declares 'rail' and is exempt from the "pairs parsed
   *  non-empty" requirement by design, not by failure — while its tram/metro
   *  routes still declare 'tram' via the shared `routeFamily`, keeping its
   *  tram stops demanded. */
  pairsFamilyOf?: (routeType: number) => 'rail' | null
}

const FEEDS: FeedConfig[] = [
  {
    id: 'polish-trains',
    name: 'Polish Trains unified (PKP IC + PolRegio + KM + KS + KD + KW + KML + ŁKA + SKM + Arriva)',
    urls: [
      'https://mkuran.pl/gtfs/polish_trains.zip',
    ],
  },
  {
    id: 'warsaw-ztm',
    name: 'Warszawa ZTM tram + metro',
    urls: [
      'https://mkuran.pl/gtfs/warsaw.zip',
    ],
    // See module doc "MIRROR-PUBLISH OVERLAP": this feed's route_type=2 rows
    // ARE SKM Warszawa, already counted via polish-trains.
    pairsFamilyOf: () => null,
  },
  {
    id: 'krakow-tram',
    name: 'Kraków ZTP tram',
    urls: [
      'https://gtfs.ztp.krakow.pl/GTFS_KRK_T.zip',
    ],
  },
  {
    id: 'silesia-gzm',
    name: 'Górnośląska Metropolia (Silesian tram + bus)',
    urls: [
      'https://mkuran.pl/gtfs/gzm.zip',
    ],
  },
  {
    id: 'wkd-warszawa',
    name: 'Warszawska Kolej Dojazdowa (suburban)',
    urls: [
      'https://mkuran.pl/gtfs/wkd.zip',
    ],
  },
]

// Poland mainland bounding box
const PL_BBOX: [number, number, number, number] = [49.0, 14.0, 55.0, 24.5]

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
    throw new Error('Failed to download any Polish GTFS feed')
  }

  console.log(`  ${results.length}/${FEEDS.length} PL feeds available`)
  return results
}

// ── Step 2/3: pair the padded country bbox with the graph-walk driver ──

/** Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in, or a cross-border through-train's
 *  foreign endpoint never snaps and the whole pair quarantines for no real
 *  reason. Stop/pair PARSING itself (`computeStopFrequenciesForFeed`,
 *  `computeStopPairFrequenciesForFeed`) gets the UNPADDED `PL_BBOX` — the
 *  margin is applied internally by `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  PL_BBOX[0] - GTFS_BORDER_MARGIN_DEG, PL_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  PL_BBOX[2] + GTFS_BORDER_MARGIN_DEG, PL_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

const defaultPairFamilyOf = (routeType: number): 'rail' | null => (RAIL_TYPES.has(routeType) ? 'rail' : null)

// ── Main ──

async function main() {
  console.log(`=== PL Railway Enrichment — Multi-feed GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
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
  //    → exempt), evaluated by the shared `describeIncompleteFamilies`. A
  //    malformed routes.txt THROWS (item 3, `declaredRouteFamiliesForFeed`) and
  //    is recorded as a PARSE FAILURE — never as "legitimately rail-less" (the
  //    old file-local feedDeclaresHeavyRail returned false for a header-only
  //    file, indistinguishable from tram-only; deleted for the shared
  //    mechanism). Pairs are re-validated every run (their own per-extractDir
  //    cache in computeStopPairFrequenciesForFeed is fingerprinted against the
  //    GTFS inputs, item 2 — NOT recomputed from stop_times each run, the old
  //    comment here claiming "always computed fresh" was wrong); the tram
  //    direction is checked live on stop-cache miss and vouched by the v2
  //    cache's recorded provenance on a hit (recording rule below is itself
  //    bidirectional, so a recorded feed means BOTH directions held at write).
  //    `warsaw-ztm` is narrowed to null (see module doc SKM mirror-publish
  //    overlap); the other 4 feeds use the default RAIL_TYPES-only classifier
  //    and are genuinely disjoint networks (national trains vs Kraków tram vs
  //    Silesia tram vs WKD's own historic corridor, not part of
  //    polish-trains' aggregated agencies) — the walk's own canonical
  //    accumulator sums by station-pair key.
  const perFeedPairs: RailStationPairCount[][] = []
  const perFeedCounts: StopTrainCount[][] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  for (const { feed, dir } of feeds) {
    // COMBINED classifier for the declared-families check: the rail direction
    // uses this feed's ACTUAL pair classifier (`pairsFamilyOf` — always-null
    // for warsaw-ztm, so its narrowed-away SKM mirror routes are never
    // demanded back as pair evidence), the tram direction the shared
    // `routeFamily` the stop counter parses with (warsaw-ztm's tram/metro
    // routes DO declare 'tram', so its tram stops are still required).
    const pairFamilyOf = feed.pairsFamilyOf ?? defaultPairFamilyOf
    const declaredFamilyOf = (rt: number): 'rail' | 'tram' | null =>
      pairFamilyOf(rt) === 'rail' ? 'rail' : routeFamily(rt) === 'tram' ? 'tram' : null
    try {
      const declared = await declaredRouteFamiliesForFeed(dir, declaredFamilyOf)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, {
        bbox: PL_BBOX,
        familyOf: pairFamilyOf,
        optionsKey: feed.pairsFamilyOf ? `pl-${feed.id}-narrowed` : 'pl-default',
      })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, PL_BBOX)
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

  // COUNTRY GATE (#26C): PKP feeds carry international through-services, so a
  // raw stop/pair list can reach Praha, Ostrava, Wien… — joining those would
  // stamp a neighbour's track under this id (mechanism: the PL feed once
  // stamped 11,856 km of CZ track, 7fac2349). A national feed only speaks for
  // its own country's network. #31.7 central country gate — see
  // writeRailTrains / rail-walk-enrich.ts.
  const inPl = makeCountryGate('PL')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inPl,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Poland is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inPl,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — PL has no retired predecessor id to absorb. The pre-2026-07-10
    // OLD_FALLBACK class-default retract is deleted outright (not preserved via
    // sourceIds composition): the driver's own generic retract — disown a
    // previously-owned row the walk/tram-join no longer covers, given
    // retractSafe — already subsumes it and is strictly more correct (it
    // re-evaluates real coverage every run instead of pattern-matching one
    // frozen class-default tuple).
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: PL is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'pl',
      extractFingerprint: `pl-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

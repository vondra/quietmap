/**
 * Enrich BE railways.arrow with urban metro/tram GTFS feeds — migrated onto
 * the shared graph-walk driver following `enrich-railway-dk.ts`'s pattern.
 *
 * Continental SNCB national HEAVY rail is already applied via
 * `enrich-railway-europe.ts`. This script ADDS Brussels (STIB), Flanders (De
 * Lijn tram), and Wallonia (TEC tram/pre-metro) urban rail/tram coverage.
 *
 * NONE of these 3 feeds ever carries a heavy-rail (GTFS route_type 2 or
 * 100-109) route — verified against each feed's own routes.txt: stib-brussels
 * is {tram(0): 18, subway/pre-metro(1): 4, bus(3): 64}, delijn-flanders is
 * {tram(0): 54, bus(3): 1964}, tec-wallonia is {bus(3): 924,
 * subway/pre-metro(1): 3, tram(0): 1} — zero rail-type entries in any of the
 * three. So `computeStopPairFrequenciesForFeed`'s default rail-only familyOf
 * always yields EMPTY pairs here, by construction, not by omission: this
 * file's heavy-rail contribution to the graph walk is legitimately nothing,
 * same as before this migration (SNCB owns Belgian heavy rail via europe.ts).
 * No fr-idf-style overlap narrowing is needed for the same reason — there is
 * no heavy-rail family here to double-publish in the first place.
 *
 * Retract-safety completeness is DATA-DRIVEN (2026-07-16 review fix): the
 * shared `declaredRouteFamiliesForFeed` + `describeIncompleteFamilies`
 * mechanism holds each feed to exactly the families its OWN routes.txt
 * declares. Today all three declare only tram (or nothing), so the pairs
 * direction is automatically exempt and retractSafe effectively rests on the
 * tram stops — the same outcome the old hand-rolled tram-only judgement
 * produced, but no longer hand-maintained: if a future feed update ADDS a
 * heavy-rail route, the mechanism DEMANDS non-empty pairs for that feed
 * (the old guard only logged), and a malformed routes.txt throws as a parse
 * failure instead of reading as "legitimately rail-less".
 *
 * TRAM/light-rail (rail_type 1/2) keeps the pre-Phase-4 `nearestGridStop`
 * 500 m stop-join (`buildTramExtraMatch`), wired in as the walk driver's
 * `extraMatch` fallback arm — this remains the file's WHOLE contribution.
 *
 * BE's source id is NATIONALLY OWNED (unlike europe's shared
 * SOURCE_ID_GLOBAL_GTFS_TRANSIT, id 2009): `bleedGate` is set to the SAME
 * country gate as `countryGate` — a row wholly outside Belgium is disownable
 * on geometry alone, since this id never legitimately stamps outside BE. No
 * predecessor source id survives from the pre-2026-07-10 class-default
 * fallback design (BE only ever used ONE id, confirmed via retract-call
 * grep): the driver's own generic retract already subsumes the old
 * OLD_FALLBACK exact-tuple check, so that class-default table is deleted
 * outright rather than preserved via `sourceIds` composition.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; BE is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-be.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-be.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-be.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-be.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-europe.ts) — enableDestructive=false: the run still runs
 * the tram stop-join, but NO retract and NO country-bleed heal fire.
 */

import { writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { SOURCE_ID_BE_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
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
import { DATA_YEAR as YEAR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_BE_NATIONAL_RAILWAY

const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/be`)
// Versioned filename: the family-aware schema added a mandatory `family` field, so a
// pre-migration cache must NOT be reused (a family-less stop would fall into tramGrid →
// heavy rail loses its count, trams re-inherit it). Kept UNCHANGED across this migration
// (manifest note): this remains the RAIL_CACHED_DOWNLOAD marker file chain/manifest.ts
// checks — the pair parser adds its OWN per-extractDir cache alongside it, never replacing it.
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-family-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

// Belgium urban rail multi-feed: STIB (Brussels metro+tram), De Lijn (Flanders
// tram), TEC (Wallonia tram/pre-metro). National SNCB is already in continental.
interface FeedConfig {
  id: string
  name: string
  urls: string[]
}

const FEEDS: FeedConfig[] = [
  {
    id: 'stib-brussels',
    name: 'STIB/MIVB Brussels (metro 4 lines + tram 18 lines + bus)',
    urls: [
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/be-bruxelles-capitale-societe-des-transports-intercommunaux-de-bruxellesmaatschappij-voor-het-intercommunaal-vervoer-te-brussel-stibmivb-gtfs-1088.zip?alt=media',
      'https://stibmivb.opendatasoft.com/api/datasets/1.0/gtfs-files-production/alternative_exports/gtfszip/',
    ],
  },
  {
    id: 'delijn-flanders',
    name: 'De Lijn (Flanders tram: Antwerpen, Gent, Coast Tram)',
    urls: [
      'http://gtfs.irail.be/de-lijn/de_lijn-gtfs.zip',
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/be-vlaams-gewest-de-lijn-gtfs-684.zip?alt=media',
    ],
  },
  {
    id: 'tec-wallonia',
    name: 'TEC Wallonia (Charleroi pre-metro light rail + bus)',
    urls: [
      'http://opendata.tec-wl.be/Current%20GTFS/TEC-GTFS.zip',
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/be-unknown-societe-regionale-wallonne-du-transport-gtfs-1212.zip?alt=media',
    ],
  },
]

// Belgium bounding box
const BE_BBOX: [number, number, number, number] = [49.4, 2.4, 51.6, 6.5]

// Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
// gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
// cross-border stop's pair could land in. Kept for shape parity with every
// other national enricher even though this file's own pairs are always empty
// (see module doc) — a future non-tram BE feed slots in without restructuring.
const GRAPH_BBOX: [number, number, number, number] = [
  BE_BBOX[0] - GTFS_BORDER_MARGIN_DEG, BE_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  BE_BBOX[2] + GTFS_BORDER_MARGIN_DEG, BE_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
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
    throw new Error('Failed to download any Belgian GTFS feed')
  }

  console.log(`  ${results.length}/${FEEDS.length} BE feeds available`)
  return results
}

// ── Main ──

async function main() {
  console.log(`=== BE Railway Enrichment — Multi-feed urban GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
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
  //    → exempt), evaluated by the shared `describeIncompleteFamilies`. For BE
  //    pairs are EXPECTED EMPTY — no feed declares a heavy-rail route_type
  //    (module doc; SNCB owns Belgian heavy rail via europe.ts), so the rail
  //    direction is exempt per feed BY DATA rather than by the old hand-rolled
  //    tram-only judgement; a feed update that adds heavy rail is DEMANDED to
  //    produce pairs, not just logged. A malformed routes.txt THROWS (item 3,
  //    `declaredRouteFamiliesForFeed`) and is recorded as a PARSE FAILURE —
  //    never as "legitimately rail-less". Pairs are re-validated every run
  //    (their own per-extractDir cache in computeStopPairFrequenciesForFeed is
  //    fingerprinted against the GTFS inputs, item 2); the tram direction is
  //    checked live on stop-cache miss and vouched by the v2 cache's recorded
  //    provenance on a hit (recording rule below is itself bidirectional, so a
  //    recorded feed means BOTH directions held at write).
  const perFeedPairs: RailStationPairCount[][] = []
  const perFeedCounts: StopTrainCount[][] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  for (const { feed, dir } of feeds) {
    try {
      const declared = await declaredRouteFamiliesForFeed(dir, routeFamily)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, { bbox: BE_BBOX, optionsKey: 'be-default' })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, BE_BBOX)
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
  // Feeds not present at all this run (--enrich-only with no cached extract,
  // or every download URL failed).
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
  // snapshot for EVERY family a feed declares — with today's tram-only BE
  // feeds this reduces to the tram stop evidence, but the formula stays the
  // full bidirectional one so a future heavy-rail addition gates itself.
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
  // (BE_BBOX blankets Lille, Maastricht, Aachen), so this id must not speak for
  // track outside Belgium — same rationale as every other national enricher
  // (mechanism: the PL feed once stamped 11,856 km of CZ track, 7fac2349).
  // #31.7 central country gate — see writeRailTrains / rail-walk-enrich.ts.
  const inBe = makeCountryGate('BE')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inBe,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside Belgium is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inBe,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — BE has no retired predecessor id to absorb. The pre-2026-07-10
    // OLD_FALLBACK class-default retract is deleted outright (not preserved via
    // sourceIds composition): the driver's own generic retract — disown a
    // previously-owned row the tram-join no longer covers, given retractSafe —
    // already subsumes it and is strictly more correct.
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: BE is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'be',
      extractFingerprint: `be-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

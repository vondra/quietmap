/**
 * Enrich ES railways.arrow with real train frequencies from 3 complementary
 * Spanish rail GTFS feeds — migrated onto the shared graph-walk driver following
 * `enrich-railway-dk.ts`'s single-file multi-feed pattern.
 *
 * Feeds (all Spain-national/regional in scope — unlike
 * `enrich-railway-europe.ts`'s per-COUNTRY feeds, all three of these describe
 * networks entirely inside Spain, so no per-feed boundingBox is needed):
 *   - `renfe-av-ld-md`  — RENFE Alta Velocidad/Larga Distancia/Media Distancia
 *     (national high-speed + intercity + regional-express, standard gauge).
 *   - `renfe-cercanias` — RENFE Cercanías commuter rail (Madrid, Barcelona,
 *     Valencia, Bilbao, Asturias, Sevilla, Málaga, Murcia, Cádiz).
 *   - `fgc-catalunya`   — FGC (Ferrocarrils de la Generalitat de Catalunya),
 *     a SEPARATE narrow-gauge operator (Barcelona-Vallès + Llobregat-Anoia
 *     lines) sharing no track with either RENFE feed.
 *
 * NO cross-feed overlap: AV/LD/MD explicitly excludes Cercanías services (two
 * distinct ssl.renfe.com endpoints, two distinct GTFS feeds by design —
 * unlike fr's national SNCF feed, which DOES already include Transilien/RER,
 * forcing fr-idf to be declared tram-only in enrich-railway-europe.ts), and
 * FGC runs entirely disjoint infrastructure from both RENFE feeds. Plain
 * concatenation of all 3 feeds' pairs is therefore correct with no narrowing
 * — `accumulateCanonicalPairs` downstream (inside the walk) sums by canonical
 * OD key regardless of how many source arrays contributed.
 *
 * HEAVY RAIL (rail_type 0) runs on `lib/rail-walk-enrich.ts`:
 * `computeStopPairFrequenciesForFeed` (lib/gtfs-stop-pairs.ts, DEFAULT
 * RAIL_TYPES-only familyOf — matches this file's pre-migration classification
 * exactly) turns each feed's stop_times.txt into station-PAIR frequencies,
 * concatenated across all 3 feeds and walked along the shortest path by
 * `enrichRailwaysByGraphWalk`. TRAM/light-rail (rail_type 1/2) keeps the
 * pre-Phase-4 `nearestGridStop` 500 m stop-join (`buildTramExtraMatch`,
 * shared from `lib/gtfs-enrich-core.ts`), wired in as the walk driver's
 * `extraMatch` fallback arm.
 *
 * DELETED local shadow (2026-07-16): this file used to carry its own
 * near-duplicate copy of the generic CSV/calendar/parent-station GTFS
 * parsing now centralized in `computeStopFrequenciesForFeed`
 * (gtfs-enrich-core.ts). Two real deltas were checked against it and
 * resolved rather than blindly ported:
 *   1. A "no rail routes found → count ALL routes" fallback. Verified DEAD
 *      against the live cached extracts (2026-04-11): route_type
 *      distributions are renfe-av-ld-md 659×2, renfe-cercanias 560×2 + 14×3
 *      (replacement buses, correctly excluded), fgc-catalunya 14×2 + 4×1
 *      (metro) + 3×7 (funicular) — RAIL_TYPES (2, 100-109) is present in
 *      EVERY feed, so the fallback never fires. Dropped rather than ported:
 *      simpler, and dead code carried forward is a bug waiting to misfire on
 *      a future feed change no one is watching for.
 *   2. FGC's own route_type=1 routes (L6/L7/L8/L12 — the Barcelona-Vallès
 *      line's central-tunnel "metro-branded" section) were previously
 *      silently dropped entirely by the RAIL_TYPES-only shadow. Switching to
 *      core's shared per-stop counter (default `routeFamily`, which maps
 *      METRO_TYPES to 'tram') now classifies them as tram/light-rail stops —
 *      a bounded, low-risk widening: `buildTramExtraMatch` only ever
 *      searches OSM rail_type 1/2 track within 500 m, so if FGC's physical
 *      track is actually tagged rail_type=0 (heavy rail, as these lines
 *      share infrastructure with FGC's own R5/R6/S-series trains), the join
 *      simply finds nothing and nothing changes; if a genuine nearby
 *      tram/light_rail segment exists, it now gets real data instead of
 *      silence. Heavy-rail PAIRS intentionally keep the narrower
 *      RAIL_TYPES-only default (no metroAsRail override, unlike europe.ts's
 *      au-vic) — these 4 routes' true OSM rail_type is unverified, and a
 *      wrong 'rail' classification would risk the graph walk mis-stamping
 *      onto a real trunk line.
 *
 * Bbox margin fix: the pre-migration file padded its OWN stops.txt bounds
 * check by a hardcoded ±1° — exactly the "old mismatch (stops 1°, graph
 * 0.5°)" that `GTFS_BORDER_MARGIN_DEG`'s doc (gtfs-enrich-core.ts) warns
 * caused cross-border pairs to snap-fail. Now: `ES_BBOX` is passed UNPADDED
 * to the stop/pair parsers (the standard 0.5° margin applies internally, via
 * `loadStopsWithCoords`), and `GRAPH_BBOX` (0.5°-padded) is the ONLY other
 * bbox, used solely for the graph-walk driver's hex scope — one margin, two
 * consumers, no drift.
 *
 * ES's source id is NATIONALLY OWNED (its retract has only ever used its own
 * id — no predecessor found in this codebase): `bleedGate` is set to the SAME
 * country gate as `countryGate`, same rationale as `enrich-railway-dk.ts`.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns; ES is not silent-residual-capable (that exists only for CZ's
 * timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-es.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-es.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-es.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-es.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-dk.ts) — enableDestructive=false: the run still walk-stamps
 * heavy-rail counts AND divisors and still runs the tram stop-join, but NO
 * retract and NO country-bleed heal fire.
 */

import { writeFileSync, existsSync, mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { SOURCE_ID_ES_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
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

const MY_SOURCE_ID = SOURCE_ID_ES_NATIONAL_RAILWAY

const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/es`)
// Filename kept UNCHANGED across this migration (manifest note): this remains
// the chain/manifest.ts RAIL_CACHED_DOWNLOAD.es marker file
// (`${YEAR}/es/renfe-stop-frequencies.json`) — the pair parser adds its OWN
// per-extractDir cache alongside it (gtfs-rail-pairs-v1.json), never
// replacing it.
const CACHE_STOP_FREQ = resolve(CACHE_DIR, 'renfe-stop-frequencies.json')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

interface FeedConfig {
  id: string
  name: string
  urls: string[]  // try in order; first 200 wins
}

const FEEDS: FeedConfig[] = [
  {
    id: 'renfe-av-ld-md',
    name: 'Renfe Alta Velocidad / Larga Distancia / Media Distancia',
    urls: [
      'https://ssl.renfe.com/gtransit/Fichero_AV_LD/google_transit.zip',
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/es-renfe-alta-velocidad-larga-distancia-media-distancia-gtfs-2620.zip?alt=media',
    ],
  },
  {
    id: 'renfe-cercanias',
    name: 'Renfe Cercanías (commuter — Madrid, Barcelona, Valencia, Bilbao, Asturias, etc.)',
    urls: [
      'https://ssl.renfe.com/ftransit/Fichero_CER_FOMENTO/fomento_transit.zip',
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/es-unknown-renfe-cercanias-gtfs-2653.zip?alt=media',
    ],
  },
  {
    id: 'fgc-catalunya',
    name: 'FGC — Ferrocarrils de la Generalitat de Catalunya',
    urls: [
      'https://www.fgc.cat/google/google_transit.zip',
      'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/es-catalunya-ferrocarrils-de-la-generalitat-de-catalunya-gtfs-1856.zip?alt=media',
    ],
  },
]

// Spain bounding box (raw — UNPADDED; the standard GTFS_BORDER_MARGIN_DEG
// margin is applied internally by loadStopsWithCoords for stop/pair parsing —
// see GRAPH_BBOX below for the graph-walk driver's own padded copy).
const ES_BBOX: [number, number, number, number] = [35.5, -10.0, 44.0, 5.0]

// ── Step 1: Download all configured GTFS feeds (UNCHANGED — per-feed cache/download quirks) ──

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
          signal: AbortSignal.timeout(300_000),
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
      console.log(`  [${feed.id}] All URLs failed — skipping`)
      continue
    }

    mkdirSync(extractDir, { recursive: true })
    execSync(`unzip -o -q "${zipPath}" -d "${extractDir}"`, { timeout: 120_000 })

    let hasFiles = true
    for (const f of ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt']) {
      if (!existsSync(resolve(extractDir, f))) {
        console.log(`  [${feed.id}] Missing ${f}, skipping`)
        hasFiles = false
        break
      }
    }

    execSync(`rm -f "${zipPath}"`)

    if (hasFiles) results.push({ feed, dir: extractDir })
  }

  if (results.length === 0) {
    throw new Error('Failed to download any Spanish GTFS feed')
  }

  console.log(`\n  ${results.length}/${FEEDS.length} ES feeds available`)
  return results
}

// ── Step 2/3: pair the padded country bbox with the graph-walk driver ──

/** Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in (RENFE AV/LD runs into France), or
 *  a through-train's foreign endpoint never snaps and the whole pair
 *  quarantines for no real reason. Stop/pair PARSING itself gets the
 *  UNPADDED `ES_BBOX` — the margin is applied internally by
 *  `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  ES_BBOX[0] - GTFS_BORDER_MARGIN_DEG, ES_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  ES_BBOX[2] + GTFS_BORDER_MARGIN_DEG, ES_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

// ── Main ──

async function main() {
  console.log(`=== ES Railway Enrichment — Multi-feed GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // downloadAllGtfs is always needed — even a merged-stop-cache hit below still
  // needs a real extractDir per feed for the pair parser's own per-extractDir cache.
  const feeds = await downloadAllGtfs()
  const stopCacheHit = !forceDownload && existsSync(CACHE_STOP_FREQ)

  // ── Per-feed parse: declared families + heavy-rail STATION PAIRS (+ stops on
  //    cache miss). BIDIRECTIONAL completeness (2026-07-16 review item 4):
  //    every family a feed's own routes.txt declares must have parsed non-empty
  //    (declares rail → pairs>0, declares tram → tramStops>0; declares neither
  //    → exempt), evaluated by the shared `describeIncompleteFamilies`. A
  //    malformed routes.txt THROWS (item 3, `declaredRouteFamiliesForFeed`) and
  //    is recorded as a PARSE FAILURE — never as "legitimately rail-less".
  //    Pairs keep the DEFAULT pair familyOf (RAIL_TYPES-only) — same
  //    classification the deleted local shadow used for heavy rail, see module
  //    doc point 2 — and are re-validated every run (their own per-extractDir
  //    cache in computeStopPairFrequenciesForFeed is fingerprinted against the
  //    GTFS inputs, item 2 — NOT recomputed from stop_times each run, the old
  //    comment here claiming "always computed fresh" was wrong); the tram
  //    direction is checked live on stop-cache miss and vouched by the v2
  //    cache's recorded provenance on a hit (recording rule below is itself
  //    bidirectional, so a recorded feed means BOTH directions held at write).
  const perFeedPairs: RailStationPairCount[][] = []
  const perFeedCounts: StopTrainCount[][] = []
  const completeFeedIds: string[] = []
  const feedIssues: string[] = []
  for (const { feed, dir } of feeds) {
    try {
      const declared = await declaredRouteFamiliesForFeed(dir, routeFamily)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, { bbox: ES_BBOX, optionsKey: 'es-default' })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, ES_BBOX)
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
  // No cross-feed overlap (module doc) — plain concatenation, summed by
  // canonical OD key downstream inside the walk.
  const pairs = perFeedPairs.flat()
  // Feeds not present at all this run (--enrich-only with no cached extract,
  // or every download URL failed).
  const missingDetail = describeIncompleteFeeds(FEEDS.map(f => f.id), feeds.map(({ feed }) => feed.id))

  // ── Tram stops: the merged v2 cache (the pre-existing download marker;
  //    unchanged path) or this run's own per-feed parses. ──
  let mergedStops: StopTrainCount[]
  let stopsUnsafeDetail: string
  if (stopCacheHit) {
    console.log(`  Using cached merged stop frequencies: ${CACHE_STOP_FREQ}`)
    const cached = readMergedStopCache<StopTrainCount>(CACHE_STOP_FREQ)
    mergedStops = cached.stops
    stopsUnsafeDetail = cached.feedsLoadedNonEmpty === null
      ? `legacy merged cache without feed provenance — delete ${CACHE_STOP_FREQ} to rebuild from the cached feed extracts`
      : describeIncompleteFeeds(FEEDS.map(f => f.id), cached.feedsLoadedNonEmpty)
    console.log(`  ${mergedStops.length} stops in cache`)
  } else {
    mergedStops = dedupeStopsByLocation(perFeedCounts.flat())
    stopsUnsafeDetail = '' // live per-feed evaluation already sits in feedIssues
    if (feedIssues.length === 0 && missingDetail === '') {
      // Recording rule (item 4): a feed lands in the cache's provenance only
      // when it passed the FULL bidirectional check this run — a later
      // cache-served run inherits completeness both ways, not any-family.
      writeMergedStopCache(CACHE_STOP_FREQ, completeFeedIds, mergedStops)
      console.log(`  Cached merged frequencies to ${CACHE_STOP_FREQ}`)
    } else {
      // Never persist a partial snapshot: a poisoned cache would silently starve
      // every later cache-served run (both enrichment and the retract evidence).
      console.log(`  NOT caching partial merged snapshot (${[...feedIssues, missingDetail].filter(Boolean).join('; ')})`)
    }
  }

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE
  // snapshot for BOTH the stops part and the pairs part — a working one must
  // never mask an empty other, since ONE retract object below covers every
  // row regardless of family.
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
  // (RENFE AV/LD runs into France, Celta into Portugal), so this id must not
  // speak for track outside Spain — same rationale as every other national
  // enricher (mechanism: the PL feed once stamped 11,856 km of CZ track, 7fac2349).
  // #31.7 central country gate — see writeRailTrains / rail-walk-enrich.ts.
  const inEs = makeCountryGate('ES')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inEs,
    // Nationally-owned id (no predecessor found — this id's retract has only
    // ever used itself): the SAME gate doubles as the bleed arm, same as
    // enrich-railway-dk.ts.
    bleedGate: inEs,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — ES has no retired predecessor id to absorb. The
    // pre-2026-07-10 OLD_FALLBACK class-default retract is deleted outright
    // (not preserved via sourceIds composition): the driver's own generic
    // retract — disown a previously-owned row the walk/tram-join no longer
    // covers, given retractSafe — already subsumes it.
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: ES is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'es',
      extractFingerprint: `es-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

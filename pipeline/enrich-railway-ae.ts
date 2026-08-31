/**
 * Enrich AE railways.arrow with Dubai RTA's unified GTFS — migrated onto the
 * shared graph-walk driver, following `enrich-railway-dk.ts`'s
 * single-national-file pattern.
 *
 * Dubai Roads & Transport Authority publishes ONE unified GTFS (7z archive)
 * via Dubai Pulse containing Metro (Red 1, Red 2, Green), Tram (T1), Water
 * Bus, and 163 bus routes — 1 feed, 1 archive, no fallback URLs (unlike DK's
 * Rejseplanen list).
 *
 * Critical pipeline limitation (unchanged since before this rewrite): Dubai
 * Metro (Red Line + Green Line) is tagged `railway=subway` in OSM. The
 * `osm-extract` crate only accepts rail/tram/light_rail/narrow_gauge/
 * funicular, so Dubai Metro is NOT in railways.arrow at all — no OSM segment
 * of any rail_type exists for it to match, whether via the graph walk or the
 * tram stop-join. Same bug affects Taipei, Kaohsiung, Seoul, Singapore,
 * Tokyo, Mexico City. Only the Dubai Tram T1 (Al Sufouh / Al Marina, OSM
 * `railway=tram`) has track to enrich.
 *
 * HEAVY RAIL (rail_type 0) runs on `lib/rail-walk-enrich.ts` like every other
 * migrated country, but this feed structurally NEVER produces heavy-rail
 * station pairs: routes.txt carries only route_type 0 (tram, 1 route: T1),
 * 1 (metro/subway, 3 routes: MRed1/MRed2/MGrn), 3 (bus, 182 routes), 4
 * (water bus, 15 routes) — never 2 or 100-109 (verified against the live
 * extract, `data/enrichment/2026/ae/gtfs-rta-dubai/GTFS_20250823/routes.txt`)
 * — and Etihad Rail freight / the Abu Dhabi airport APM / Yas Island people
 * mover / Palm Jumeirah Monorail publish no public timetable at all
 * (unchanged since before this rewrite). No `metroAsRail`-style override
 * (see `enrich-railway-europe.ts`'s `railFamilyFor`) is wired here: mapping
 * GTFS metro (route_type 1) to 'rail' would still snap onto NOTHING, since
 * Dubai Metro has zero graph segments either way (see the pipeline
 * limitation above) — an override would only add dead code, not real
 * coverage. `computeStopPairFrequenciesForFeed` is still called (default
 * rail-only `familyOf`) so the shape matches every other national enricher,
 * and the day Etihad Rail ever publishes a timetable this file needs no
 * structural change beyond a new feed entry.
 *
 * TRAM (rail_type 1/2) keeps the pre-Phase-4 `nearestGridStop` 500 m
 * stop-join (`buildTramExtraMatch`, `lib/gtfs-enrich-core.ts`) — same as
 * every other migrated country. Pre-migration this file carried a THIRD
 * family bucket ('metro', separate from 'tram') whose stops were
 * deliberately IGNORED — "metro stops are never matched — Dubai Metro is
 * railway=subway (not extracted)", a comment present since this file's very
 * first version (confirmed via `git log --follow -p`). That deliberate
 * ignore is PRESERVED by `aeRouteFamilyExcludingMetro` (below): METRO_TYPES
 * → null on the stop side, so metro stops never enter the counts at all.
 * The first migration draft instead collapsed metro into 'tram' (core
 * `routeFamily`'s convention) and argued the Metro↔Tram interchange
 * contribution (Damac Properties / Sobha Realty near the T1 alignment) was
 * a correctness improvement; the 2026-07-16 reviews DISPROVED that on the
 * live extract: a cache rebuild would have turned 87 Dubai Metro stops into
 * 'tram'-family rows and stamped 26 tram rows — e.g. 654 metro trains/day
 * onto Dubai Tram (T1) track. An interchange within 500 m does NOT put
 * metro trains on tram track — the metro runs on its own (unextracted,
 * railway=subway) alignment. `buildTramExtraMatch`'s internal
 * `family === 'tram'` filter is the second, independent guard: 'metro'-tagged
 * rows from a pre-migration legacy cache never enter the match grid either.
 *
 * AE's source id is NATIONALLY OWNED (`SOURCE_ID_AE_NATIONAL_RAILWAY`,
 * unlike europe's shared `SOURCE_ID_GLOBAL_GTFS_TRANSIT`): `bleedGate` is set
 * to the SAME country gate as `countryGate` — a row wholly outside the UAE is
 * disownable on geometry alone, since this id never legitimately stamps
 * outside it (AE_BBOX brushes Oman's Musandam enclave and the Al Ain border,
 * hence the #31.7 country gate this file already carried pre-migration). No
 * predecessor source id survives from the pre-2026-07-10 class-default
 * fallback design (checked: AE only ever used ONE id, MY_SOURCE_ID, both
 * before and after this rewrite) — the driver's own generic retract (disown
 * a previously-owned row the walk/tram-join no longer covers, given
 * `retractSafe`) already subsumes the old OLD_FALLBACK exact-tuple check
 * (which distinguished Etihad Rail freight tiers from Dubai's light-rail/
 * tram/funicular class defaults), so that table is deleted outright rather
 * than preserved via `sourceIds` composition.
 *
 * WHY (unchanged): segments the feed doesn't reach STAY at source_id=0 on
 * purpose — the engine default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns
 * unknowns (Etihad Rail freight, Ruwais refinery sidings, Abu Dhabi airport
 * APM, Yas Island people mover, Palm Jumeirah Monorail all stay there); AE is
 * not silent-residual-capable (that exists only for CZ's timetable, owner
 * decision 2026-07-11 option b).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ae.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ae.ts --force-download
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ae.ts --enrich-only
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ae.ts --stamp-only
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-europe.ts/enrich-railway-dk.ts) — enableDestructive=false:
 * the run still walk-stamps heavy-rail counts AND divisors (moot for AE
 * today — see above) and still runs the tram stop-join, but NO retract and
 * NO country-bleed heal fire.
 */

import { writeFileSync, readdirSync, existsSync, mkdirSync, chmodSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { SOURCE_ID_AE_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import {
  computeStopFrequenciesForFeed, dedupeStopsByLocation, buildTramExtraMatch, routeFamily, METRO_TYPES,
  declaredRouteFamiliesForFeed, describeIncompleteFamilies,
  describeIncompleteFeeds, logRetractSkippedIncompleteInputs, readMergedStopCache, writeMergedStopCache,
  GTFS_BORDER_MARGIN_DEG, type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_AE_NATIONAL_RAILWAY

const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/ae`)
// Filename UNCHANGED across this migration (manifest note): this remains the
// RAIL_CACHED_DOWNLOAD.ae marker file chain/manifest.ts checks
// (`${YEAR}/ae/gtfs-stop-frequencies.json`) — never rename or move it. A
// cache written by the PRE-migration 3-family ('rail'|'tram'|'metro') parser
// still reads fine here (same on-disk shape; `family` is just a string): its
// 'metro'-tagged rows are dropped by BOTH the call-site `family === 'tram'`
// filter below and `buildTramExtraMatch`'s internal family filter, and a
// rebuild (--force-download, or deleting the file) excludes metro at the
// SOURCE (`aeRouteFamilyExcludingMetro` maps METRO_TYPES → null) — metro
// stops stay ignored on every path, exactly their PRE-migration behavior.
const CACHE_FREQUENCIES = resolve(CACHE_DIR, 'gtfs-stop-frequencies.json')
const SEVENZ_BIN = resolve(import.meta.dirname, '../pipeline/node_modules/7zip-bin/linux/x64/7za')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

interface FeedConfig {
  id: string
  name: string
  url: string
}

const FEEDS: FeedConfig[] = [
  {
    id: 'rta-dubai',
    name: 'Dubai RTA unified (Metro Red/Green + Tram T1 + Water Bus + buses)',
    url: 'https://www.dubaipulse.gov.ae/dataset/73765e8f-e8c4-443c-9687-288072ed9d12/resource/11515bd3-bdba-466f-ab65-f057bd123ab5/download/gtfs.7z',
  },
]

// UAE bbox (inclusive of Musandam enclave)
const AE_BBOX: [number, number, number, number] = [22.3, 51.0, 26.3, 56.7]

/** Same 0.5° margin as `loadStopsWithCoords`'s GTFS geometry envelope (item 6,
 *  gtfs-enrich-core.ts) — the graph-walk scope must reach every hex a
 *  cross-border stop's pair could land in. Stop/pair PARSING itself
 *  (`computeStopFrequenciesForFeed`/`computeStopPairFrequenciesForFeed`) gets
 *  the UNPADDED `AE_BBOX` — the margin is applied internally by
 *  `loadStopsWithCoords`. */
const GRAPH_BBOX: [number, number, number, number] = [
  AE_BBOX[0] - GTFS_BORDER_MARGIN_DEG, AE_BBOX[1] - GTFS_BORDER_MARGIN_DEG,
  AE_BBOX[2] + GTFS_BORDER_MARGIN_DEG, AE_BBOX[3] + GTFS_BORDER_MARGIN_DEG,
]

/** AE's route_type → family classifier, used for BOTH the per-stop counter
 *  (`computeStopFrequenciesForFeed`'s familyOf) and the declared-families
 *  completeness check (`declaredRouteFamiliesForFeed`): RAIL_TYPES → 'rail',
 *  TRAM_TYPES → 'tram', METRO_TYPES → null — metro is EXCLUDED, restoring
 *  this file's pre-migration deliberate ignore of Dubai Metro stops. The
 *  default `routeFamily` groups metro with tram; the 2026-07-16 reviews
 *  PROVED on the live extract that adopting it here would, on the next cache
 *  rebuild, turn 87 Dubai Metro stops into 'tram'-family rows and stamp 26
 *  tram rows — e.g. 654 metro trains/day onto Dubai Tram (T1) track. An
 *  interchange within 500 m does NOT put metro trains on tram track.
 *  Declared-'rail' under this classifier matches exactly what the pair
 *  parser parses (`computeStopPairFrequenciesForFeed`'s default familyOf is
 *  RAIL_TYPES-only), so the completeness check demands pairs only from a
 *  feed that really declares heavy rail — this replaces the hand-maintained
 *  TRAM_ONLY_FEED_IDS set (rta-dubai declares no rail route_type, so it is
 *  exempt from the pairs requirement automatically). */
function aeRouteFamilyExcludingMetro(routeType: number): 'rail' | 'tram' | null {
  const fam = routeFamily(routeType)
  return fam === 'tram' && METRO_TYPES.has(routeType) ? null : fam
}

// ── Step 1: Download GTFS 7z (UNCHANGED — per-country cache/download quirks:
//    7z archive via Dubai Pulse CKAN, nested GTFS_{YYYYMMDD}/ extraction dir) ──

async function downloadAllGtfs(): Promise<Array<{ feed: FeedConfig; dir: string }>> {
  const results: Array<{ feed: FeedConfig; dir: string }> = []

  // Ensure 7za is executable
  try { chmodSync(SEVENZ_BIN, 0o755) } catch {}

  for (const feed of FEEDS) {
    const extractParent = resolve(CACHE_DIR, `gtfs-${feed.id}`)

    if (!forceDownload && existsSync(extractParent)) {
      // find the actual GTFS dir (could be nested like GTFS_20250823/)
      const subs = readdirSync(extractParent).filter(f => existsSync(resolve(extractParent, f, 'stops.txt')))
      if (subs.length > 0) {
        console.log(`  [${feed.id}] Using cached GTFS: ${resolve(extractParent, subs[0])}`)
        results.push({ feed, dir: resolve(extractParent, subs[0]) })
        continue
      }
      if (existsSync(resolve(extractParent, 'stops.txt'))) {
        console.log(`  [${feed.id}] Using cached GTFS: ${extractParent}`)
        results.push({ feed, dir: extractParent })
        continue
      }
    }
    if (enrichOnly) {
      if (!existsSync(extractParent)) {
        console.log(`  [${feed.id}] --enrich-only but no cached GTFS, skipping`)
        continue
      }
    }

    mkdirSync(CACHE_DIR, { recursive: true })
    const archivePath = resolve(CACHE_DIR, `${feed.id}.7z`)

    if (!existsSync(archivePath) || forceDownload) {
      console.log(`  [${feed.id}] Downloading from ${feed.url}...`)
      try {
        const res = await fetch(feed.url, {
          signal: AbortSignal.timeout(600_000),
          headers: {
            'Accept': 'application/x-7z-compressed, application/octet-stream, */*',
            'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36',
            'Accept-Language': 'en-US,en;q=0.9',
            'Accept-Encoding': 'gzip, deflate, br',
            'Referer': 'https://www.dubaipulse.gov.ae/dataset/dubai-gtfs',
          },
          redirect: 'follow',
        })
        if (!res.ok) {
          console.log(`  [${feed.id}] HTTP ${res.status}, skipping feed`)
          continue
        }
        const buf = Buffer.from(await res.arrayBuffer())
        writeFileSync(archivePath, buf)
        console.log(`  [${feed.id}] Downloaded: ${(buf.length / 1e6).toFixed(1)} MB`)
      } catch (err: any) {
        console.log(`  [${feed.id}] Failed: ${err.message}, skipping feed`)
        continue
      }
    }

    mkdirSync(extractParent, { recursive: true })
    try {
      execSync(`"${SEVENZ_BIN}" x -y -o"${extractParent}" "${archivePath}" > /dev/null`, { timeout: 120_000 })
    } catch (err: any) {
      console.log(`  [${feed.id}] 7z extraction failed: ${err.message}`)
      continue
    }

    // 7z nests inside GTFS_{YYYYMMDD}/ — find the real directory
    const subs = readdirSync(extractParent).filter(f => {
      try { return existsSync(resolve(extractParent, f, 'stops.txt')) } catch { return false }
    })
    let gtfsDir: string
    if (subs.length > 0) gtfsDir = resolve(extractParent, subs[0])
    else if (existsSync(resolve(extractParent, 'stops.txt'))) gtfsDir = extractParent
    else {
      console.log(`  [${feed.id}] Could not find stops.txt in extracted archive`)
      continue
    }

    // NOTE: this used to `continue` the INNER file loop — a no-op that let a feed
    // missing trips.txt/routes.txt through to the parser. Flag + outer continue,
    // the same shape enrich-railway-be.ts uses.
    let requiredFilesOk = true
    for (const f of ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt']) {
      if (!existsSync(resolve(gtfsDir, f))) {
        console.log(`  [${feed.id}] Missing ${f}, skipping feed`)
        requiredFilesOk = false
        break
      }
    }
    if (!requiredFilesOk) continue

    results.push({ feed, dir: gtfsDir })
  }

  if (results.length === 0) {
    console.log(`  No GTFS feeds available — nothing will be stamped.`)
  } else {
    console.log(`  ${results.length}/${FEEDS.length} AE feeds available`)
  }
  return results
}

// ── Main ──

async function main() {
  console.log(`=== AE Railway Enrichment — Dubai RTA unified GTFS (${YEAR}, Phase 4: graph-walk) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache:    ${CACHE_DIR}\n`)

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
  //    every family a feed's own routes.txt declares under
  //    `aeRouteFamilyExcludingMetro` must have parsed non-empty (declares rail
  //    → pairs>0, declares tram → tramStops>0; declares neither → exempt),
  //    evaluated by the shared `describeIncompleteFamilies`. rta-dubai
  //    declares no rail route_type, so its structurally-empty pairs result is
  //    exempt automatically — data-driven, replacing the deleted
  //    TRAM_ONLY_FEED_IDS set — and the day an AE feed DOES declare heavy
  //    rail (Etihad Rail publishing a timetable) the pairs requirement
  //    arrives without editing anything. A malformed routes.txt THROWS
  //    (item 3, `declaredRouteFamiliesForFeed`) and is recorded as a PARSE
  //    FAILURE — never as "legitimately rail-less". Pairs are re-validated
  //    every run (their own per-extractDir cache in
  //    computeStopPairFrequenciesForFeed is fingerprinted against the GTFS
  //    inputs, item 2 — NOT recomputed from stop_times each run, the old
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
      const declared = await declaredRouteFamiliesForFeed(dir, aeRouteFamilyExcludingMetro)
      const { pairs } = await computeStopPairFrequenciesForFeed(dir, { bbox: AE_BBOX })
      perFeedPairs.push(pairs)
      let tramN: number | null = null
      if (!stopCacheHit) {
        const counts = await computeStopFrequenciesForFeed(feed, dir, AE_BBOX, aeRouteFamilyExcludingMetro)
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
  // Feeds not present at all this run (download failed, or --enrich-only with
  // no cached extract) — downloadAllGtfs tolerates per-feed failure.
  const missingDetail = describeIncompleteFeeds(FEEDS.map(f => f.id), feeds.map(({ feed }) => feed.id))

  // ── Tram stops: the merged v2 cache (the pre-existing download marker;
  //    unchanged path) or this run's own per-feed parses. ──
  let mergedStops: StopTrainCount[]
  let stopsUnsafeDetail: string
  if (stopCacheHit) {
    console.log(`  Using cached merged stop frequencies: ${CACHE_FREQUENCIES}`)
    const cached = readMergedStopCache<StopTrainCount>(CACHE_FREQUENCIES)
    mergedStops = cached.stops
    // Legacy bare-array cache: with a SINGLE configured feed, non-empty stops are
    // themselves the completeness proof (the one feed parsed non-empty when the
    // cache was written). The FEEDS.length===1 term is the tripwire that voids
    // this shortcut the day a second AE feed is added.
    stopsUnsafeDetail = cached.feedsLoadedNonEmpty === null
      ? (FEEDS.length === 1 && mergedStops.length > 0 ? '' : `legacy merged cache without feed provenance — delete ${CACHE_FREQUENCIES} to rebuild from the cached feed extract`)
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
  // snapshot for BOTH families this run's ONE retract object will govern — a
  // working tram part must never mask an empty heavy-rail part (or vice
  // versa), since ONE retract object below covers every row regardless of family.
  const retractUnsafeDetail = [missingDetail, ...feedIssues, stopsUnsafeDetail].filter(Boolean).join('; ')
  const retractSafe = retractUnsafeDetail === ''
  if (!retractSafe) logRetractSkippedIncompleteInputs(retractUnsafeDetail)

  // Call-site 'tram' filter kept as harmless double protection over
  // buildTramExtraMatch's internal one — and it is what keeps a legacy
  // 3-family cache's 'metro' rows out of the stop count logged below.
  const tramStops = mergedStops.filter(s => s.family === 'tram')

  if (pairs.length === 0 && tramStops.length === 0) {
    console.log(`\nNo GTFS data to enrich. Exiting.`)
    return
  }

  console.log(`\n  ${pairs.length} heavy-rail station pairs, ${tramStops.length} tram stops`)

  // #31.7 national-ownership gate: AE_BBOX brushes Oman (Musandam + the Al Ain
  // border), so this id must not speak for track outside the UAE — same
  // rationale as every other national enricher.
  const inAe = makeCountryGate('AE')

  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox: GRAPH_BBOX,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate: inAe,
    // Nationally-owned id (unlike europe's shared SOURCE_ID_GLOBAL_GTFS_TRANSIT):
    // the SAME gate doubles as the bleed arm — a row wholly outside the UAE is
    // disownable on geometry alone, since this id never legitimately stamps there.
    bleedGate: inAe,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — AE has no retired predecessor id to absorb. The
    // pre-2026-07-10 OLD_FALLBACK class-default retract (Etihad Rail freight
    // tiers vs Dubai's light-rail/tram/funicular defaults) is deleted outright
    // (not preserved via sourceIds composition): the driver's own generic
    // retract — disown a previously-owned row the walk/tram-join no longer
    // covers, given retractSafe — already subsumes it and is strictly more
    // correct (it re-evaluates real coverage every run instead of
    // pattern-matching one frozen class-default tuple).
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: AE is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default; only CZ's timetable
    // evidence (owner decision option b) justifies a residual.
    sidecar: {
      scope: 'ae',
      extractFingerprint: `ae-gtfs:${feeds.map(({ feed }) => feed.id).join('+')}`,
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

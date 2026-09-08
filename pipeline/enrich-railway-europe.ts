/** Country-scoped global GTFS railway enrichment from immutable inputs into z9. */

import { mkdirSync } from 'node:fs'
import { basename, relative, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import {
  buildTramExtraMatch, computeStopFrequenciesForFeed, declaredRouteFamiliesForFeed,
  dedupeStopsByLocation, describeIncompleteFamilies, findBusiestWednesday,
  type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { enrichZ9RailwaysByGraphWalk, type Z9RailWalkResult } from './lib/rail-walk-enrich.js'
import {
  GLOBAL_GTFS_FEEDS, countryGtfsBbox, gtfsSourceDirectories, railFamilyFor,
  validateGtfsSourceFreshness, type GlobalGtfsFeed, type GtfsSourceFreshness,
} from './lib/railway-gtfs-feeds.js'
import { SOURCE_ID_GLOBAL_GTFS_TRANSIT } from './lib/sources.js'

interface PlannedFeed {
  feed: GlobalGtfsFeed
  directories: readonly string[]
}

interface LoadedFeed {
  id: string
  directories: number
  declaredFamilies: readonly string[]
  sourceFreshness: readonly GtfsSourceFreshness[]
  pairs: RailStationPairCount[]
  tramStops: StopTrainCount[]
  pairCacheHits: number
}

export interface GlobalGtfsCountryResult {
  country: string
  feeds: ReadonlyArray<Omit<LoadedFeed, 'pairs' | 'tramStops'> & {
    pairs: number
    tramStops: number
  }>
  pairs: number
  tramStops: number
  walk: Z9RailWalkResult
}

function planCountryFeeds(sourceDirectory: string, country: string): PlannedFeed[] {
  const feeds = GLOBAL_GTFS_FEEDS.filter(feed => feed.country === country)
  if (feeds.length === 0) throw new Error(`no global GTFS feed for country '${country}'`)
  return feeds.map(feed => {
    const directories = gtfsSourceDirectories(sourceDirectory, feed)
    if (directories.length === 0) {
      throw new Error(`GTFS feed ${feed.id} is missing required source files under ${sourceDirectory}`)
    }
    return { feed, directories }
  })
}

function pairCachePath(
  cacheDirectory: string,
  sourceDirectory: string,
  feed: GlobalGtfsFeed,
  directory: string,
): string {
  const feedRoot = resolve(sourceDirectory, feed.id)
  const label = (relative(feedRoot, directory) || basename(directory))
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .replace(/^-|-$/g, '') || 'root'
  const target = resolve(cacheDirectory, feed.id, `${label}.pairs.json`)
  mkdirSync(resolve(cacheDirectory, feed.id), { recursive: true })
  return target
}

async function loadFeed(
  plan: PlannedFeed,
  sourceDirectory: string,
  cacheDirectory: string,
  asOfDate: string,
): Promise<LoadedFeed> {
  const { feed, directories } = plan
  const declared = new Set<'rail' | 'tram'>()
  const sourceFreshness: GtfsSourceFreshness[] = []
  const pairs: RailStationPairCount[] = []
  const tramStops: StopTrainCount[] = []
  let pairCacheHits = 0

  for (const directory of directories) {
    const classify = (routeType: number) => railFamilyFor(routeType, feed)
    const directoryFamilies = await declaredRouteFamiliesForFeed(directory, classify)
    for (const family of directoryFamilies) declared.add(family)
    sourceFreshness.push(await validateGtfsSourceFreshness(feed, directory, asOfDate))
    if (directoryFamilies.size === 0) continue

    const stopCounts = await computeStopFrequenciesForFeed(
      feed,
      directory,
      feed.bbox,
      classify,
      findBusiestWednesday,
    )
    tramStops.push(...stopCounts.filter(stop => stop.family === 'tram'))

    const pairResult = await computeStopPairFrequenciesForFeed(directory, {
      bbox: feed.bbox,
      familyOf: routeType => railFamilyFor(routeType, feed) === 'rail' ? 'rail' : null,
      dateSelection: findBusiestWednesday,
      optionsKey: 'europe-busiest-wed',
      cachePath: pairCachePath(cacheDirectory, sourceDirectory, feed, directory),
    })
    pairs.push(...pairResult.pairs)
    if (pairResult.provenance.fromCache) pairCacheHits++
  }

  const dedupedTramStops = dedupeStopsByLocation(tramStops)
  const incomplete = describeIncompleteFamilies(
    feed.id,
    declared,
    pairs.length,
    dedupedTramStops.length,
  )
  if (incomplete) throw new Error(`incomplete GTFS input: ${incomplete}`)
  return {
    id: feed.id,
    directories: directories.length,
    declaredFamilies: [...declared].sort(),
    sourceFreshness,
    pairs,
    tramStops: dedupedTramStops,
    pairCacheHits,
  }
}

export async function enrichGlobalGtfsCountry(options: {
  sourceDirectory: string
  preparedDirectory: string
  cacheDirectory: string
  country: string
}): Promise<GlobalGtfsCountryResult> {
  const sourceDirectory = resolve(options.sourceDirectory)
  const preparedDirectory = resolve(options.preparedDirectory)
  const cacheDirectory = resolve(options.cacheDirectory)
  const country = options.country.toUpperCase()
  if (!/^[A-Z]{2}$/.test(country)) throw new Error(`invalid ISO2 country '${options.country}'`)

  const plans = planCountryFeeds(sourceDirectory, country)
  const asOfDate = new Date().toISOString().slice(0, 10).replace(/-/g, '')
  const loaded: LoadedFeed[] = []
  for (const plan of plans) {
    loaded.push(await loadFeed(plan, sourceDirectory, cacheDirectory, asOfDate))
  }

  const pairs = loaded.flatMap(feed => feed.pairs)
  const tramStops = dedupeStopsByLocation(loaded.flatMap(feed => feed.tramStops))
  const walk = await enrichZ9RailwaysByGraphWalk({
    preparedDirectory,
    bbox: countryGtfsBbox(country),
    pairs,
    sourceId: SOURCE_ID_GLOBAL_GTFS_TRANSIT,
    countryIso: country,
    extraMatch: buildTramExtraMatch(tramStops, SOURCE_ID_GLOBAL_GTFS_TRANSIT),
    retractSafe: true,
  })
  return {
    country,
    feeds: loaded.map(({ pairs: feedPairs, tramStops: feedTramStops, ...feed }) => ({
      ...feed,
      pairs: feedPairs.length,
      tramStops: feedTramStops.length,
    })),
    pairs: pairs.length,
    tramStops: tramStops.length,
    walk,
  }
}

function cliOptions(argv: readonly string[]): {
  sourceDirectory: string
  preparedDirectory: string
  cacheDirectory: string
  country: string
} {
  const { values } = parseArgs({
    args: [...argv],
    strict: true,
    allowPositionals: false,
    options: {
      'source-dir': { type: 'string' },
      'prepared-dir': { type: 'string' },
      'cache-dir': { type: 'string' },
      country: { type: 'string' },
    },
  })
  if (!values['source-dir'] || !values['prepared-dir'] ||
      !values['cache-dir'] || !values.country) {
    throw new Error(
      'usage: enrich-railway-europe.ts --source-dir DIR --prepared-dir DIR ' +
      '--cache-dir DIR --country CC',
    )
  }
  return {
    sourceDirectory: values['source-dir'],
    preparedDirectory: values['prepared-dir'],
    cacheDirectory: values['cache-dir'],
    country: values.country,
  }
}

async function main(): Promise<void> {
  console.log(JSON.stringify(await enrichGlobalGtfsCountry(cliOptions(process.argv.slice(2)))))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}

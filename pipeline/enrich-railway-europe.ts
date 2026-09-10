/** Country-scoped GTFS railway enrichment from immutable global or national inputs. */

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
  countryGtfsBbox, feedsForRegistry, gtfsSourceDirectories, railFamilyFor,
  validateGtfsSourceFreshness, type GlobalGtfsFeed, type GtfsRegistry,
  type GtfsSourceFreshness,
} from './lib/railway-gtfs-feeds.js'

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
  registry: GtfsRegistry
  asOfDate: string
  sourceId: number
  feeds: ReadonlyArray<Omit<LoadedFeed, 'pairs' | 'tramStops'> & { pairs: number; tramStops: number }>
  pairs: number
  tramStops: number
  walk: Z9RailWalkResult
}

function planCountryFeeds(
  sourceDirectory: string,
  country: string,
  registry: GtfsRegistry,
  cacheDirectory: string,
): PlannedFeed[] {
  const feeds = feedsForRegistry(registry).filter(feed => feed.country === country)
  if (feeds.length === 0) throw new Error(`no ${registry} GTFS feed for country '${country}'`)
  return feeds.map(feed => {
    const directories = gtfsSourceDirectories(sourceDirectory, feed, cacheDirectory)
    if (directories.length === 0) {
      throw new Error(`GTFS feed ${feed.id} is missing required source files under ${sourceDirectory}`)
    }
    return { feed, directories }
  })
}

function pairCachePath(
  cacheDirectory: string,
  sourceDirectory: string,
  registry: GtfsRegistry,
  feed: GlobalGtfsFeed,
  directory: string,
): string {
  const feedRoot = resolve(sourceDirectory, feed.sourcePath ?? feed.id)
  const label = (feed.sourceArchive ? `archive-${feed.sourceArchive.sha256.slice(0, 16)}` :
    relative(feedRoot, directory) || basename(directory))
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .replace(/^-|-$/g, '') || 'root'
  const parent = resolve(cacheDirectory, registry, feed.id)
  mkdirSync(parent, { recursive: true })
  return resolve(parent, `${label}.pairs.json`)
}

async function loadFeed(
  plan: PlannedFeed,
  sourceDirectory: string,
  cacheDirectory: string,
  registry: GtfsRegistry,
  asOfDate: string,
): Promise<LoadedFeed> {
  const { feed, directories } = plan
  const declared = new Set<'rail' | 'tram'>()
  const sourceFreshness: GtfsSourceFreshness[] = []
  const pairs: RailStationPairCount[] = []
  const tramStops: StopTrainCount[] = []
  let pairCacheHits = 0

  for (const directory of directories) {
    const stopFamily = (routeType: number) => railFamilyFor(routeType, feed)
    const pairFamily = (routeType: number): 'rail' | null =>
      feed.includeRailPairs !== false && stopFamily(routeType) === 'rail' ? 'rail' : null
    const declaredFamily = (routeType: number): 'rail' | 'tram' | null =>
      pairFamily(routeType) ?? (stopFamily(routeType) === 'tram' ? 'tram' : null)
    const dateSelection = feed.serviceDay === 'busiest-wednesday' ? findBusiestWednesday : undefined
    const directoryFamilies = await declaredRouteFamiliesForFeed(directory, declaredFamily)
    for (const family of directoryFamilies) declared.add(family)
    sourceFreshness.push(await validateGtfsSourceFreshness(feed, directory, asOfDate))

    const directoryTramStops = directoryFamilies.has('tram')
      ? (await computeStopFrequenciesForFeed(feed, directory, feed.bbox, stopFamily, dateSelection))
        .filter(stop => stop.family === 'tram')
      : []
    const pairResult = directoryFamilies.has('rail')
      ? await computeStopPairFrequenciesForFeed(directory, {
          bbox: feed.bbox,
          familyOf: pairFamily,
          dateSelection,
          optionsKey: `${registry}-complete-family-day-v2-${feed.serviceDay}-${feed.includeRailPairs === false ? 'tram-only' : 'rail'}`,
          cachePath: pairCachePath(cacheDirectory, sourceDirectory, registry, feed, directory),
        })
      : null
    const directoryPairs = pairResult?.pairs ?? []
    const incomplete = describeIncompleteFamilies(
      `${feed.id}/${basename(directory)}`,
      directoryFamilies,
      directoryPairs.length,
      directoryTramStops.length,
    )
    if (incomplete) throw new Error(`incomplete GTFS input: ${incomplete}`)
    pairs.push(...directoryPairs)
    tramStops.push(...directoryTramStops)
    if (pairResult?.provenance.fromCache) pairCacheHits++
  }

  return {
    id: feed.id,
    directories: directories.length,
    declaredFamilies: [...declared].sort(),
    sourceFreshness,
    pairs,
    tramStops: dedupeStopsByLocation(tramStops),
    pairCacheHits,
  }
}

export async function enrichGlobalGtfsCountry(options: {
  sourceDirectory: string
  preparedDirectory: string
  cacheDirectory: string
  country: string
  registry?: GtfsRegistry
  asOfDate: string
}): Promise<GlobalGtfsCountryResult> {
  const sourceDirectory = resolve(options.sourceDirectory)
  const preparedDirectory = resolve(options.preparedDirectory)
  const cacheDirectory = resolve(options.cacheDirectory)
  const country = options.country.toUpperCase()
  const registry = options.registry ?? 'global'
  if (!/^[A-Z]{2}$/.test(country)) throw new Error(`invalid ISO2 country '${options.country}'`)

  const registryFeeds = feedsForRegistry(registry)
  const plans = planCountryFeeds(sourceDirectory, country, registry, cacheDirectory)
  const sourceIds = new Set(plans.map(plan => plan.feed.sourceId))
  if (sourceIds.size !== 1) throw new Error(`${registry} GTFS country '${country}' has inconsistent source ids`)
  const sourceId = [...sourceIds][0]
  const asOfDate = options.asOfDate
  if (!/^\d{8}$/.test(asOfDate)) throw new Error(`invalid GTFS as-of date '${asOfDate}'`)
  const loaded: LoadedFeed[] = []
  for (const plan of plans) {
    loaded.push(await loadFeed(plan, sourceDirectory, cacheDirectory, registry, asOfDate))
  }

  const pairs = loaded.flatMap(feed => feed.pairs)
  const tramStops = dedupeStopsByLocation(loaded.flatMap(feed => feed.tramStops))
  const walk = await enrichZ9RailwaysByGraphWalk({
    preparedDirectory,
    bbox: countryGtfsBbox(country, registryFeeds),
    pairs,
    sourceId,
    countryIso: country,
    extraMatch: buildTramExtraMatch(tramStops, sourceId),
    retractSafe: true,
  })
  return {
    country,
    registry,
    asOfDate,
    sourceId,
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
  registry: GtfsRegistry
  asOfDate: string
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
      registry: { type: 'string', default: 'global' },
      'as-of-date': { type: 'string' },
    },
  })
  if (!values['source-dir'] || !values['prepared-dir'] || !values['cache-dir'] || !values.country ||
      !values['as-of-date'] || !/^\d{8}$/.test(values['as-of-date']) ||
      (values.registry !== 'global' && values.registry !== 'national')) {
    throw new Error(
      'usage: enrich-railway-europe.ts --source-dir DIR --prepared-dir DIR ' +
      '--cache-dir DIR --country CC --as-of-date YYYYMMDD [--registry global|national]',
    )
  }
  return {
    sourceDirectory: values['source-dir'],
    preparedDirectory: values['prepared-dir'],
    cacheDirectory: values['cache-dir'],
    country: values.country,
    registry: values.registry,
    asOfDate: values['as-of-date'],
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

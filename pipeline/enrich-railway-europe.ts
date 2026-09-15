/** Country-scoped GTFS railway enrichment from immutable global or national inputs. */

import { mkdirSync } from 'node:fs'
import { runGtfsCountries, awaitCountryPublication } from './lib/rail-country-workers.js'
import { basename, relative, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import {
  buildTramExtraMatch, computeStopFrequenciesForFeed, declaredRouteFamiliesForFeed,
  dedupeStopsByLocation, describeIncompleteFamilies,
  type StopTrainCount,
} from './lib/gtfs-enrich-core.js'
import { openGtfsServices, type GtfsService, type GtfsServiceStore } from './lib/gtfs-service-store.js'
import { enrichZ9RailwaysByServices, type Z9RailWalkResult } from './lib/rail-walk-enrich.js'
import {
  countryGtfsBbox, feedsForRegistry, gtfsSourceDirectories, railFamilyFor, serviceDaySelection,
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
  stores: GtfsServiceStore[]
  railServices: number
  railServicesWithoutStopTimes: number
  tramStops: StopTrainCount[]
  serviceCacheHits: number
}

export interface GlobalGtfsCountryResult {
  country: string
  registry: GtfsRegistry
  asOfDate: string
  sourceId: number
  feeds: ReadonlyArray<Omit<LoadedFeed, 'stores' | 'tramStops'> & { tramStops: number }>
  services: number
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

function serviceCachePath(
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
  return resolve(parent, `${label}.services.sqlite`)
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
  const stores: GtfsServiceStore[] = []
  const tramStops: StopTrainCount[] = []
  let serviceCacheHits = 0
  let railServices = 0
  let railServicesWithoutStopTimes = 0

  try {
    for (const directory of directories) {
      const stopFamily = (routeType: number) => railFamilyFor(routeType, feed)
      const pairFamily = (routeType: number): 'rail' | null =>
        feed.includeRailPairs !== false && stopFamily(routeType) === 'rail' ? 'rail' : null
      const declaredFamily = (routeType: number): 'rail' | 'tram' | null =>
        pairFamily(routeType) ?? (stopFamily(routeType) === 'tram' ? 'tram' : null)
      const dateSelection = serviceDaySelection(feed)
      const directoryFamilies = await declaredRouteFamiliesForFeed(directory, declaredFamily)
      for (const family of directoryFamilies) declared.add(family)
      // One window for this directory: the freshness verdict feeds service-day selection and
      // the service-store cache identity, so enrichment can never sample a day the gate clipped.
      const freshness = await validateGtfsSourceFreshness(feed, directory, asOfDate)
      sourceFreshness.push(freshness)

      const directoryTramStops = directoryFamilies.has('tram')
        ? (await computeStopFrequenciesForFeed(feed, directory, feed.bbox, stopFamily, dateSelection, freshness))
          .filter(stop => stop.family === 'tram')
        : []
      const store = directoryFamilies.has('rail')
        ? await openGtfsServices(directory, {
            familyOf: pairFamily,
            dateSelection,
            serviceWindow: freshness,
            optionsKey: `${registry}-complete-family-day-v2-${feed.serviceDay}-${feed.includeRailPairs === false ? 'tram-only' : 'rail'}`,
            cachePath: serviceCachePath(cacheDirectory, sourceDirectory, registry, feed, directory),
          })
        : null
      const directoryServices = store?.provenance.tripsWithStopTimes ?? 0
      const incomplete = describeIncompleteFamilies(
        `${feed.id}/${basename(directory)}`,
        directoryFamilies,
        directoryServices,
        directoryTramStops.length,
      )
      if (incomplete) {
        store?.[Symbol.dispose]()
        throw new Error(`incomplete GTFS input: ${incomplete}`)
      }
      if (store) {
        stores.push(store)
        railServices += directoryServices
        railServicesWithoutStopTimes += store.provenance.activeTripCount - directoryServices
        if (store.provenance.fromCache) serviceCacheHits++
      }
      tramStops.push(...directoryTramStops)
    }
  } catch (error) {
    for (const store of stores) store[Symbol.dispose]()
    throw error
  }

  return {
    id: feed.id,
    directories: directories.length,
    declaredFamilies: [...declared].sort(),
    sourceFreshness,
    stores,
    railServices,
    railServicesWithoutStopTimes,
    tramStops: dedupeStopsByLocation(tramStops),
    serviceCacheHits,
  }
}

export interface GlobalGtfsCountryOptions {
  sourceDirectory: string
  preparedDirectory: string
  cacheDirectory: string
  country: string
  registry?: GtfsRegistry
  asOfDate: string
  beforeWrite?: () => Promise<void>
}

export async function enrichGlobalGtfsCountry(options: GlobalGtfsCountryOptions): Promise<GlobalGtfsCountryResult> {
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
  try {
    for (const plan of plans) {
      loaded.push(await loadFeed(plan, sourceDirectory, cacheDirectory, registry, asOfDate))
    }
    const tramStops = dedupeStopsByLocation(loaded.flatMap(feed => feed.tramStops))
    const services = function* (): Generator<GtfsService> {
      for (const feed of loaded) {
        for (const store of feed.stores) {
          for (const service of store.services()) {
            yield store.provenance.frequenciesPresent ? service : { ...service, departureMultiplier: 1 }
          }
        }
      }
    }
    const walk = await enrichZ9RailwaysByServices({
      preparedDirectory,
      bbox: countryGtfsBbox(country, registryFeeds),
      services: services(),
      sourceId,
      countryIso: country,
      extraMatch: buildTramExtraMatch(tramStops, sourceId),
      retractSafe: true,
      beforeWrite: options.beforeWrite,
    })
    return {
      country,
      registry,
      asOfDate,
      sourceId,
      feeds: loaded.map(({ stores: _stores, tramStops: feedTramStops, ...feed }) => ({
        ...feed,
        tramStops: feedTramStops.length,
      })),
      services: loaded.reduce((sum, feed) => sum + feed.railServices, 0),
      tramStops: tramStops.length,
      walk,
    }
  } finally {
    for (const feed of loaded) for (const store of feed.stores) store[Symbol.dispose]()
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
  const disconnected = () => process.exit(1)
  if (process.send) process.once('disconnect', disconnected)
  try {
    const options = cliOptions(process.argv.slice(2))
    if (options.country.includes(',')) await runGtfsCountries(options)
    else console.log(JSON.stringify(await enrichGlobalGtfsCountry({
      ...options, beforeWrite: process.send ? awaitCountryPublication : undefined,
    })))
  } finally {
    process.off('disconnect', disconnected)
    if (process.connected) process.disconnect!()
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}

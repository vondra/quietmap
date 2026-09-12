/** Download, validate and atomically admit immutable GTFS source snapshots. */

import {
  closeSync, cpSync, createReadStream, createWriteStream, existsSync, mkdirSync, mkdtempSync,
  openSync, readSync, readdirSync, renameSync, rmSync, statSync, writeFileSync,
} from 'node:fs'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { basename, dirname, resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { pathToFileURL } from 'node:url'
import {
  computeActiveTripFamiliesForFeed, declaredRouteFamiliesForFeed, describeInactiveFamilies, findBusiestWednesday,
  readGtfsFeedWindow,
} from './lib/gtfs-enrich-core.js'
import {
  feedsForRegistry, gtfsDownloadUrls, gtfsSourceDirectories, railFamilyFor,
  sevenZipExecutable, validateGtfsSourceFreshness, type GlobalGtfsFeed, type GtfsRegistry,
} from './lib/railway-gtfs-feeds.js'

const REQUIRED_FILES = ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt'] as const

export interface GtfsRefreshReceipt {
  registry: GtfsRegistry
  feed: string
  country: string
  sourceId: number
  requestedUrl: string
  resolvedUrl: string
  downloadedAt: string
  asOfDate: string
  archiveSha256: string
  archiveBytes: number
  firstServiceDate: string | null
  lastServiceDate: string
  targetDate: string
  activeTrips: number
  declaredFamilies: string[]
  archivedPreviousSource: string | null
}

function run(command: string, args: readonly string[]): Promise<void> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, [...args], { stdio: ['ignore', 'ignore', 'pipe'] })
    let error = ''
    child.stderr.on('data', chunk => { error += String(chunk) })
    child.on('error', reject)
    child.on('close', code => {
      if (code === 0) resolvePromise()
      else reject(new Error(`${basename(command)} exited ${code}: ${error.trim()}`))
    })
  })
}

async function download(url: string, destination: string): Promise<string> {
  const response = await fetch(url, {
    redirect: 'follow',
    signal: AbortSignal.timeout(600_000),
    headers: { 'User-Agent': 'Quiet Map source refresh', Accept: 'application/zip, application/octet-stream, */*' },
  })
  if (!response.ok || !response.body) throw new Error(`HTTP ${response.status}`)
  await pipeline(Readable.from(response.body), createWriteStream(destination, { flags: 'wx' }))
  return response.url
}

function directoryHasGtfs(directory: string): boolean {
  return REQUIRED_FILES.every(name => existsSync(resolve(directory, name)))
}

function findGtfsDirectories(root: string, depth = 0): string[] {
  const matches = directoryHasGtfs(root) ? [root] : []
  if (depth >= 4) return matches
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (entry.isDirectory() && entry.name !== '__MACOSX') {
      matches.push(...findGtfsDirectories(resolve(root, entry.name), depth + 1))
    }
  }
  return matches
}

function sha256(path: string): Promise<string> {
  return new Promise((resolvePromise, reject) => {
    const hash = createHash('sha256')
    const stream = createReadStream(path)
    stream.on('error', reject)
    stream.on('data', chunk => hash.update(chunk))
    stream.on('end', () => resolvePromise(hash.digest('hex')))
  })
}

async function candidateFromUrl(
  staging: string,
  feed: GlobalGtfsFeed,
  url: string,
  asOfDate: string,
): Promise<{ candidate: string; receipt: Omit<GtfsRefreshReceipt, 'registry' | 'archivedPreviousSource'> }> {
  const archive = resolve(staging, 'source.archive')
  const resolvedUrl = await download(url, archive)
  const extracted = resolve(staging, 'extracted')
  mkdirSync(extracted)
  await run(sevenZipExecutable(), ['x', '-y', `-o${extracted}`, archive])
  const directories = findGtfsDirectories(extracted)
  if (directories.length !== 1) {
    throw new Error(`archive contains ${directories.length} GTFS roots; expected exactly one`)
  }
  const candidate = resolve(staging, 'candidate')
  cpSync(directories[0], candidate, { recursive: true })
  const freshness = await validateGtfsSourceFreshness(feed, candidate, asOfDate)
  if (freshness.historical) {
    throw new Error(`download is still the accepted historical ${freshness.lastServiceDate} snapshot`)
  }
  const familyOf = (routeType: number) => railFamilyFor(routeType, feed)
  const declaredFamilies = [...await declaredRouteFamiliesForFeed(candidate, familyOf)].sort()
  const dateSelection = feed.serviceDay === 'busiest-wednesday' ? findBusiestWednesday : undefined
  const active = await computeActiveTripFamiliesForFeed(candidate, familyOf, dateSelection)
  const incomplete = describeInactiveFamilies(
    feed.id, new Set(declaredFamilies), new Set(active.tripFam.values()),
  )
  if (incomplete) throw new Error(incomplete)
  const sourceMetadata = resolve(candidate, '_source')
  mkdirSync(sourceMetadata)
  const magic = Buffer.alloc(6)
  const descriptor = openSync(archive, 'r')
  try { readSync(descriptor, magic, 0, magic.length, 0) } finally { closeSync(descriptor) }
  const extension = magic[0] === 0x37 && magic[1] === 0x7a ? '7z' : 'zip'
  cpSync(archive, resolve(sourceMetadata, `source.${extension}`))
  const window = await readGtfsFeedWindow(candidate)
  const receipt = {
    feed: feed.id,
    country: feed.country,
    sourceId: feed.sourceId,
    requestedUrl: url,
    resolvedUrl,
    downloadedAt: new Date().toISOString(),
    asOfDate,
    archiveSha256: await sha256(archive),
    archiveBytes: statSync(archive).size,
    firstServiceDate: window.firstServiceDate,
    lastServiceDate: freshness.lastServiceDate,
    targetDate: active.targetDate,
    activeTrips: active.tripFam.size,
    declaredFamilies,
  }
  return { candidate, receipt }
}

function sourceTarget(sourceDirectory: string, feed: GlobalGtfsFeed): string {
  return resolve(sourceDirectory, feed.sourcePath ?? feed.id)
}

function writeReceipt(path: string, receipt: GtfsRefreshReceipt): void {
  mkdirSync(dirname(path), { recursive: true })
  const temporary = `${path}.tmp-${process.pid}`
  writeFileSync(temporary, JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx' })
  renameSync(temporary, path)
}

export async function refreshGtfsFeed(options: {
  registry: GtfsRegistry
  feed: GlobalGtfsFeed
  sourceDirectory: string
  archiveDirectory: string
  receiptDirectory: string
  asOfDate: string
}): Promise<GtfsRefreshReceipt> {
  const target = sourceTarget(options.sourceDirectory, options.feed)
  const incoming = resolve(dirname(target), '.incoming')
  mkdirSync(incoming, { recursive: true })
  const failures: string[] = []
  for (const url of gtfsDownloadUrls(options.feed)) {
    const staging = mkdtempSync(resolve(incoming, `${options.feed.id}-`))
    try {
      const { candidate, receipt: partial } = await candidateFromUrl(
        staging, options.feed, url, options.asOfDate,
      )
      const archived = resolve(
        options.archiveDirectory, options.registry, options.feed.country.toLowerCase(), basename(target),
      )
      const archivedPreviousSource = existsSync(target) ? archived : null
      const fullReceipt = { ...partial, registry: options.registry, archivedPreviousSource }
      writeFileSync(resolve(candidate, '_source/receipt.json'), JSON.stringify(fullReceipt, null, 2) + '\n')
      if (archivedPreviousSource) {
        if (existsSync(archivedPreviousSource)) throw new Error(`archive target already exists: ${archivedPreviousSource}`)
        mkdirSync(dirname(archivedPreviousSource), { recursive: true })
        renameSync(target, archivedPreviousSource)
      }
      try {
        renameSync(candidate, target)
      } catch (error) {
        if (archivedPreviousSource && !existsSync(target)) renameSync(archivedPreviousSource, target)
        throw error
      }
      writeReceipt(resolve(options.receiptDirectory, `${options.registry}-${options.feed.id}.json`), fullReceipt)
      rmSync(staging, { recursive: true, force: true })
      return fullReceipt
    } catch (error) {
      failures.push(`${url}: ${error instanceof Error ? error.message : String(error)}`)
      rmSync(staging, { recursive: true, force: true })
    }
  }
  throw new Error(failures.join(' | '))
}

async function currentSourceIsFresh(
  sourceDirectory: string, feed: GlobalGtfsFeed, asOfDate: string,
): Promise<boolean> {
  const directories = gtfsSourceDirectories(sourceDirectory, feed)
  if (directories.length === 0) return false
  try {
    for (const directory of directories) {
      if ((await validateGtfsSourceFreshness(feed, directory, asOfDate)).historical) return false
    }
    return true
  } catch {
    return false
  }
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    strict: true,
    options: {
      registry: { type: 'string' },
      feed: { type: 'string', multiple: true },
      'source-dir': { type: 'string' },
      'archive-dir': { type: 'string' },
      'receipt-dir': { type: 'string' },
      'as-of-date': { type: 'string' },
      'only-stale': { type: 'boolean', default: false },
    },
  })
  if ((values.registry !== 'global' && values.registry !== 'national') ||
      !values['source-dir'] || !values['archive-dir'] || !values['receipt-dir'] ||
      !values['as-of-date'] || !/^\d{8}$/.test(values['as-of-date'])) {
    throw new Error(
      'usage: refresh-railway-gtfs.ts --registry global|national --source-dir DIR ' +
      '--archive-dir DIR --receipt-dir DIR --as-of-date YYYYMMDD [--only-stale] [--feed ID]',
    )
  }
  const registry = values.registry as GtfsRegistry
  const requested = new Set(values.feed ?? [])
  const feeds = feedsForRegistry(registry).filter(feed => requested.size === 0 || requested.has(feed.id))
  if (requested.size > 0 && feeds.length !== requested.size) throw new Error('unknown or duplicate --feed')
  let failed = 0
  for (const feed of feeds) {
    if (values['only-stale'] && await currentSourceIsFresh(values['source-dir'], feed, values['as-of-date'])) {
      console.log(JSON.stringify({ registry, feed: feed.id, status: 'fresh' }))
      continue
    }
    try {
      const receipt = await refreshGtfsFeed({
        registry,
        feed,
        sourceDirectory: values['source-dir'],
        archiveDirectory: values['archive-dir'],
        receiptDirectory: values['receipt-dir'],
        asOfDate: values['as-of-date'],
      })
      console.log(JSON.stringify({ ...receipt, status: 'refreshed' }))
    } catch (error) {
      failed++
      console.log(JSON.stringify({
        registry, feed: feed.id, status: 'failed',
        error: error instanceof Error ? error.message : String(error),
      }))
    }
  }
  process.exitCode = failed ? 1 : 0
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(error => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}

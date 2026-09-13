/** Atomic GTFS source refresh preserves the previous canonical tree and pins source bytes. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, test } from 'node:test'
import { path7za } from '7zip-bin'
import { refreshGtfsFeed } from './refresh-railway-gtfs.js'
import { computeActiveTripFamiliesForFeed } from './lib/gtfs-enrich-core.js'
import { openGtfsServices } from './lib/gtfs-service-store.js'
import {
  NATIONAL_GTFS_FEEDS, gtfsTextSourceSha256, railFamilyFor, serviceDaySelection,
  validateGtfsSourceFreshness, type GlobalGtfsFeed,
} from './lib/railway-gtfs-feeds.js'

const TEMP = mkdtempSync(join(tmpdir(), 'gtfs-refresh-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))

function makeArchive(name = 'archive-source', lastServiceDate = '20261231'): { bytes: Buffer; source: string } {
  const source = join(TEMP, name)
  mkdirSync(source)
  writeFileSync(join(source, 'routes.txt'), 'route_id,route_type\nrail,2\n')
  writeFileSync(join(source, 'calendar.txt'),
    'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
    `daily,1,1,1,1,1,1,1,20240101,${lastServiceDate}\n`)
  writeFileSync(join(source, 'trips.txt'), 'route_id,service_id,trip_id\nrail,daily,trip\n')
  writeFileSync(join(source, 'stops.txt'),
    'stop_id,stop_name,stop_lat,stop_lon\na,A,50,14\nb,B,50.01,14.01\n')
  writeFileSync(join(source, 'stop_times.txt'),
    'trip_id,stop_id,stop_sequence\ntrip,a,1\ntrip,b,2\n')
  const archive = join(TEMP, `${name}.zip`)
  chmodSync(path7za, 0o755)
  execFileSync(path7za, ['a', '-tzip', archive, '.'], { cwd: source, stdio: 'ignore' })
  return { bytes: readFileSync(archive), source }
}

test('validated source admission atomically archives the old tree and records its archive digest', async () => {
  const { bytes } = makeArchive()
  const server = createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'application/zip', 'Content-Length': bytes.length })
    response.end(bytes)
  })
  await new Promise<void>(resolvePromise => server.listen(0, '127.0.0.1', resolvePromise))
  try {
    const address = server.address()
    assert.ok(address && typeof address === 'object')
    const sourceDirectory = join(TEMP, 'canonical')
    const target = join(sourceDirectory, 'gr', 'gtfs-test')
    mkdirSync(target, { recursive: true })
    writeFileSync(join(target, 'old-marker'), 'old')
    const feed: GlobalGtfsFeed = {
      id: 'test', country: 'GR', name: 'Fixture', url: `http://127.0.0.1:${address.port}/source.zip`,
      bbox: [49, 13, 51, 15], routeTypes: new Set([2]), sourceId: 100,
      sourcePath: 'gr/gtfs-test', serviceDay: 'midpoint-wednesday',
    }
    const receipt = await refreshGtfsFeed({
      registry: 'national', feed, sourceDirectory,
      archiveDirectory: join(TEMP, 'archive'), receiptDirectory: join(TEMP, 'receipts'),
      asOfDate: '20260909',
    })
    assert.equal(receipt.archiveSha256, createHash('sha256').update(bytes).digest('hex'))
    assert.equal(receipt.lastServiceDate, '20261231')
    assert.equal(receipt.activeTrips, 1)
    assert.equal(readFileSync(join(TEMP, 'archive/national/gr/gtfs-test/old-marker'), 'utf8'), 'old')
    assert.equal(readFileSync(join(target, '_source/receipt.json'), 'utf8'), JSON.stringify(receipt, null, 2) + '\n')
    assert.deepEqual(readFileSync(join(target, '_source/source.zip')), bytes)
  } finally {
    await new Promise<void>((resolvePromise, reject) =>
      server.close(error => error ? reject(error) : resolvePromise()))
  }
})


test('refresh refuses to relabel an accepted historical snapshot as current', async () => {
  const { bytes, source } = makeArchive('historical-source', '20251231')
  const server = createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'application/zip', 'Content-Length': bytes.length })
    response.end(bytes)
  })
  await new Promise<void>(resolvePromise => server.listen(0, '127.0.0.1', resolvePromise))
  try {
    const address = server.address()
    assert.ok(address && typeof address === 'object')
    const sourceDirectory = join(TEMP, 'historical-canonical')
    const target = join(sourceDirectory, 'ae', 'gtfs-test')
    mkdirSync(target, { recursive: true })
    writeFileSync(join(target, 'old-marker'), 'preserved')
    const feed: GlobalGtfsFeed = {
      id: 'test', country: 'AE', name: 'Historical fixture',
      url: `http://127.0.0.1:${address.port}/source.zip`,
      bbox: [22, 51, 27, 57], routeTypes: new Set([2]), sourceId: 2001,
      sourcePath: 'ae/gtfs-test', serviceDay: 'midpoint-wednesday',
      acceptedHistoricalSource: {
        gtfsTextSha256: gtfsTextSourceSha256(source),
        lastServiceDate: '20251231', sourceYear: 2025,
      },
    }
    await assert.rejects(refreshGtfsFeed({
      registry: 'national', feed, sourceDirectory,
      archiveDirectory: join(TEMP, 'historical-archive'),
      receiptDirectory: join(TEMP, 'historical-receipts'), asOfDate: '20260909',
    }), /download is still the accepted historical 20251231 snapshot/)
    assert.equal(readFileSync(join(target, 'old-marker'), 'utf8'), 'preserved')
  } finally {
    await new Promise<void>((resolvePromise, reject) =>
      server.close(error => error ? reject(error) : resolvePromise()))
  }
})

/** GZM's real publisher shape: `feed_info` stamped with the snapshot day while
 *  `calendar_dates.txt` (no `calendar.txt`) declares a month of daily services. */
function makeSnapshotStampedArchive(name: string): { bytes: Buffer; source: string } {
  const source = join(TEMP, name)
  mkdirSync(source)
  writeFileSync(join(source, 'routes.txt'), 'route_id,route_type\nrail,2\ntram,0\n')
  writeFileSync(join(source, 'feed_info.txt'), 'feed_start_date,feed_end_date\n20260909,20260909\n')
  writeFileSync(join(source, 'calendar_dates.txt'),
    'service_id,date,exception_type\n' +
    'tram-only,20260909,1\n' +
    'rail-daily,20260916,1\ntram-daily,20260916,1\n' +
    'rail-daily,20260923,1\ntram-daily,20260923,1\n' +
    'rail-daily,20261009,1\n' +
    'cancelled,20261231,2\n')
  writeFileSync(join(source, 'trips.txt'),
    'trip_id,route_id,service_id\nrail-wed,rail,rail-daily\ntram-wed,tram,tram-daily\n' +
    'tram-snap,tram,tram-only\n')
  writeFileSync(join(source, 'stops.txt'),
    'stop_id,stop_name,stop_lat,stop_lon\na,A,50.3,19\nb,B,50.31,19.01\n')
  writeFileSync(join(source, 'stop_times.txt'),
    'trip_id,stop_id,stop_sequence\nrail-wed,a,1\nrail-wed,b,2\ntram-wed,a,1\ntram-wed,b,2\n' +
    'tram-snap,a,1\ntram-snap,b,2\n')
  const archive = join(TEMP, `${name}.zip`)
  chmodSync(path7za, 0o755)
  execFileSync(path7za, ['a', '-tzip', archive, '.'], { cwd: source, stdio: 'ignore' })
  return { bytes: readFileSync(archive), source }
}

test('one GZM calendar window governs both refresh admission and enrichment selection', async () => {
  const { bytes } = makeSnapshotStampedArchive('gzm-snapshot-source')
  const url = 'http://127.0.0.1:%PORT%/gzm.zip'
  const server = createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'application/zip', 'Content-Length': bytes.length })
    response.end(bytes)
  })
  await new Promise<void>(resolvePromise => server.listen(0, '127.0.0.1', resolvePromise))
  try {
    const address = server.address()
    assert.ok(address && typeof address === 'object')
    const served = url.replace('%PORT%', String(address.port))
    // The REAL registry entry: its `serviceWindowFromCalendars` fact and route types govern.
    const registryFeed = NATIONAL_GTFS_FEEDS.find(feed => feed.id === 'silesia-gzm')!
    const feed: GlobalGtfsFeed = { ...registryFeed, url: served, downloadUrls: [served] }
    const sourceDirectory = join(TEMP, 'gzm-canonical')
    const receipt = await refreshGtfsFeed({
      registry: 'national', feed, sourceDirectory,
      archiveDirectory: join(TEMP, 'gzm-archive'), receiptDirectory: join(TEMP, 'gzm-receipts'),
      asOfDate: '20260910',
    })
    // The snapshot-day stamp no longer clips the horizon, and the sampled day is an actual
    // declared service day inside it — 20260909 runs rail only, so it is not a complete day.
    assert.equal(receipt.firstServiceDate, '20260909')
    assert.equal(receipt.lastServiceDate, '20261009')
    assert.equal(receipt.targetDate, '20260916')
    assert.equal(receipt.activeTrips, 2)

    const admitted = join(sourceDirectory, feed.sourcePath!)
    const familyOf = (routeType: number) => railFamilyFor(routeType, feed)
    const dateSelection = serviceDaySelection(feed)
    const window = await validateGtfsSourceFreshness(feed, admitted, '20260910')
    assert.deepEqual(window, { firstServiceDate: '20260909', lastServiceDate: '20261009' })
    const enrichment = await computeActiveTripFamiliesForFeed(admitted, familyOf, dateSelection, window)
    assert.equal(enrichment.targetDate, receipt.targetDate, 'enrichment samples the day admission validated')
    assert.equal(enrichment.tripFam.size, receipt.activeTrips)

    // Without the derived window the same directory still clips to the snapshot day — the
    // divergence the shared window removes — and the cache must not serve that clipped day.
    const clippedWindow = { firstServiceDate: '20260909', lastServiceDate: '20260909' }
    const clipped = await computeActiveTripFamiliesForFeed(admitted, familyOf, dateSelection, clippedWindow)
    assert.equal(clipped.targetDate, '20260909', 'the stamp-day window still samples the incomplete snapshot day')
    assert.deepEqual([...clipped.tripFam.values()], ['tram'])

    const cachePath = join(TEMP, 'gzm-services.sqlite')
    {
      using built = await openGtfsServices(admitted, { serviceWindow: window, cachePath })
      assert.equal(built.provenance.fromCache, false)
      assert.equal(built.provenance.targetDate, receipt.targetDate)
    }
    {
      using cached = await openGtfsServices(admitted, { serviceWindow: window, cachePath })
      assert.equal(cached.provenance.fromCache, true)
      assert.equal(cached.provenance.targetDate, receipt.targetDate)
    }
    {
      using narrowerWindow = await openGtfsServices(admitted, { serviceWindow: clippedWindow, cachePath })
      assert.equal(narrowerWindow.provenance.fromCache, false, 'a different window is a different cache identity')
      assert.equal(narrowerWindow.provenance.activeTripCount, 0, 'the clipped window leaves no complete rail day')
    }
  } finally {
    await new Promise<void>((resolvePromise, reject) =>
      server.close(error => error ? reject(error) : resolvePromise()))
  }
})

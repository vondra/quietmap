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
import { gtfsTextSourceSha256, type GlobalGtfsFeed } from './lib/railway-gtfs-feeds.js'

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

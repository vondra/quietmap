/** Source-to-z9 integration tests for the country-scoped global GTFS producer. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import {
  copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichGlobalGtfsCountry } from './enrich-railway-europe.js'
import { writeRailwaysFixture } from './lib/rail-test-fixture.js'

const TEMP = mkdtempSync(join(tmpdir(), 'global-gtfs-z9-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))

function squareDirectory(prepared: string, latitude: number, longitude: number): string {
  const x = Math.floor((longitude + 180) / 360 * 512)
  const radians = latitude * Math.PI / 180
  const y = Math.floor((1 - Math.asinh(Math.tan(radians)) / Math.PI) / 2 * 512)
  return join(prepared, 'z9', String(x), String(y))
}

function writeGreekGtfs(source: string, routes: string): void {
  const directory = join(source, 'gr')
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, 'routes.txt'), routes)
  writeFileSync(
    join(directory, 'calendar.txt'),
    'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
    'daily,1,1,1,1,1,1,1,20260101,20991231\n',
  )
  writeFileSync(
    join(directory, 'trips.txt'),
    'route_id,service_id,trip_id\nrail,daily,rail-trip\ntram,daily,tram-trip\n',
  )
  writeFileSync(
    join(directory, 'stops.txt'),
    'stop_id,stop_name,stop_lat,stop_lon\n' +
    'a,Rail A,38.0000,23.0000\nb,Rail B,38.0050,23.0050\n' +
    'c,Tram C,38.0010,23.0010\nd,Tram D,38.0015,23.0015\n',
  )
  writeFileSync(
    join(directory, 'stop_times.txt'),
    'trip_id,stop_id,stop_sequence\n' +
    'rail-trip,a,1\nrail-trip,b,2\ntram-trip,c,1\ntram-trip,d,2\n',
  )
}

function makePrepared(name: string): { prepared: string; path: string } {
  const prepared = join(TEMP, name)
  const directory = squareDirectory(prepared, 38, 23)
  mkdirSync(directory, { recursive: true })
  const path = join(directory, 'railways.arrow')
  copyFileSync(writeRailwaysFixture(`${name}.arrow`, [
    {
      latitude: 38,
      longitude: 23,
      endLatitude: 38.005,
      endLongitude: 23.005,
      lengthMetres: 708,
      railType: 0,
      country: 'GR',
    },
    {
      latitude: 38.001,
      longitude: 23.001,
      endLatitude: 38.0015,
      endLongitude: 23.0015,
      lengthMetres: 71,
      railType: 1,
      country: 'GR',
    },
  ]), path)
  return { prepared, path }
}

function values(path: string, column: string): unknown[] {
  const table = tableFromIPC(readFileSync(path))
  return [...Array(table.numRows)].map((_, index) => table.getChild(column)!.get(index))
}

test('real GTFS files stamp heavy rail and tram through one z9 writer and rerun identically', async () => {
  const source = join(TEMP, 'source-valid')
  writeGreekGtfs(source, 'route_id,route_type\nrail,2\ntram,0\n')
  const { prepared, path } = makePrepared('prepared-valid')
  const options = {
    sourceDirectory: source,
    preparedDirectory: prepared,
    cacheDirectory: join(TEMP, 'cache-valid'),
    country: 'gr',
    asOfDate: '20260909',
  }

  const first = await enrichGlobalGtfsCountry(options)
  assert.deepEqual(
    {
      country: first.country,
      feeds: first.feeds.map(feed => feed.id),
      pairs: first.pairs,
      tramStops: first.tramStops,
      walked: first.walk.pairsWalked,
      walkStamped: first.walk.walkStamped,
      extraStamped: first.walk.extraStamped,
    },
    {
      country: 'GR',
      feeds: ['gr'],
      pairs: 1,
      tramStops: 2,
      walked: 1,
      walkStamped: 1,
      extraStamped: 1,
    },
  )
  assert.deepEqual(values(path, 'trains_passenger'), [1, 1])
  assert.deepEqual(values(path, 'trains_freight'), [0, 0])
  assert.deepEqual(values(path, 'source_id'), [100, 100])
  const before = readFileSync(path)
  const second = await enrichGlobalGtfsCountry(options)
  assert.equal(second.feeds[0].pairCacheHits, 1)
  assert.deepEqual(readFileSync(path), before)

  writeGreekGtfs(source, 'route_id,route_type\nbus,3\n')
  const retracted = await enrichGlobalGtfsCountry(options)
  assert.equal(retracted.walk.retracted, 2)
  assert.deepEqual(values(path, 'trains_passenger'), [0, 0])
  assert.deepEqual(values(path, 'source_id'), [0, 0])
  assert.equal(tableFromIPC(readFileSync(path)).getChild('parallel_divisor'), null)
})

test('an invalid feed fails before touching an existing prepared Arrow', async () => {
  const source = join(TEMP, 'source-invalid')
  writeGreekGtfs(source, 'route_id,route_name\nrail,broken\n')
  const { prepared, path } = makePrepared('prepared-invalid')
  const before = readFileSync(path)
  await assert.rejects(
    enrichGlobalGtfsCountry({
      sourceDirectory: source,
      preparedDirectory: prepared,
      cacheDirectory: join(TEMP, 'cache-invalid'),
      country: 'GR',
      asOfDate: '20260909',
    }),
    /routes\.txt parse failure/,
  )
  assert.deepEqual(readFileSync(path), before)
})

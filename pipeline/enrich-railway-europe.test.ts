/** Source-to-z9 integration tests for the country-scoped global GTFS producer. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import {
  appendFileSync, cpSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from 'node:fs'
import { join, relative } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichGlobalGtfsCountry } from './enrich-railway-europe.js'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { listRailIntervals } from './lib/rail-traffic-store.js'
import { writeSyntheticRailTopology } from './lib/transport-test-fixture.js'
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

function makePrepared(name: string): { prepared: string; path: string; square: string } {
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
  writeSyntheticRailTopology(prepared, [relative(prepared, directory)])
  return { prepared, path, square: relative(prepared, directory) }
}

test('real GTFS files stamp heavy rail and tram through one z9 writer and rerun identically', async () => {
  const source = join(TEMP, 'source-valid')
  writeGreekGtfs(source, 'route_id,route_type\nrail,2\ntram,0\n')
  const { prepared, path, square } = makePrepared('prepared-valid')
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
      services: first.services,
      tramStops: first.tramStops,
      routed: first.walk.servicesGraphEstimated + first.walk.servicesRelationEstimated,
      walkStamped: first.walk.walkStamped,
      extraStamped: first.walk.extraStamped,
    },
    {
      country: 'GR',
      feeds: ['gr'],
      services: 1,
      tramStops: 2,
      routed: 1,
      walkStamped: 1,
      extraStamped: 1,
    },
  )
  const stamped = listRailIntervals(prepared, square)
  assert.equal(stamped.length, 2)
  assert.deepEqual(stamped.map(row => row.passenger).sort((a, b) => b - a), [1, 1])
  assert.ok(stamped.every(row => row.freightStatus === 0))
  const before = readFileSync(path)
  const second = await enrichGlobalGtfsCountry(options)
  assert.equal(second.feeds[0].serviceCacheHits, 1)
  assert.deepEqual(readFileSync(path), before)

  writeGreekGtfs(source, 'route_id,route_type\nbus,3\n')
  const retracted = await enrichGlobalGtfsCountry(options)
  assert.equal(retracted.walk.retracted, 2)
  assert.equal(listRailIntervals(prepared, square).length, 0)
  assert.deepEqual(readFileSync(path), before)
})

test('an incomplete rail snapshot cannot replace or retract previously prepared traffic', async () => {
  for (const missing of ['stop coordinates', 'trip stop_times']) {
    const source = join(TEMP, `source-missing-${missing}`)
    writeGreekGtfs(source, 'route_id,route_type\nrail,2\ntram,0\n')
    const { prepared, path } = makePrepared(`prepared-missing-${missing}`)
    const options = { sourceDirectory: source, preparedDirectory: prepared,
      cacheDirectory: join(TEMP, `cache-missing-${missing}`), country: 'GR', asOfDate: '20260909' }
    await enrichGlobalGtfsCountry(options)
    const before = readFileSync(path)
    appendFileSync(join(source, 'gr', 'trips.txt'), 'rail,daily,incomplete-trip\n')
    if (missing === 'stop coordinates') {
      appendFileSync(join(source, 'gr', 'stop_times.txt'),
        'incomplete-trip,a,1\nincomplete-trip,unknown,2\nincomplete-trip,b,3\n')
    }
    for (let attempt = 0; attempt < 2; attempt++) {
      await assert.rejects(enrichGlobalGtfsCountry(options),
        /active rail trip 'incomplete-trip' has (unresolved stop 'unknown'|no stop_times)/)
      assert.deepEqual(readFileSync(path), before, 'a non-empty surviving trip is not a complete source snapshot')
    }
  }
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

test('an orphan with a source shape preserves prior corridor counts while valid services elsewhere proceed', async () => {
  const source = join(TEMP, 'source-localized-orphan')
  writeGreekGtfs(source, 'route_id,route_type\nrail,2\n')
  const prepared = join(TEMP, 'prepared-localized-orphan')
  const directory = squareDirectory(prepared, 38, 23)
  mkdirSync(directory, { recursive: true })
  const path = join(directory, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('localized-orphan.arrow', [0, 0.2].map(offset => ({
    latitude: 38 + offset, longitude: 23,
    endLatitude: 38.005 + offset, endLongitude: 23.005,
    lengthMetres: 708, railType: 0, country: 'GR',
  }))), path)
  const square = relative(prepared, directory)
  writeSyntheticRailTopology(prepared, [square])
  const options = { sourceDirectory: source, preparedDirectory: prepared,
    cacheDirectory: join(TEMP, 'cache-localized-orphan'), country: 'GR', asOfDate: '20260909' }
  await enrichGlobalGtfsCountry(options)
  const prior = listRailIntervals(prepared, square)
  assert.equal(prior.length, 1)
  const before = readFileSync(path)
  writeFileSync(join(source, 'gr', 'trips.txt'),
    'route_id,service_id,trip_id,shape_id\nrail,daily,orphan,missing-stations\nrail,daily,new-trip,\n')
  writeFileSync(join(source, 'gr', 'shapes.txt'),
    'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n' +
    'missing-stations,38,23,1\nmissing-stations,38.005,23.005,2\n')
  appendFileSync(join(source, 'gr', 'stops.txt'),
    'e,New A,38.2,23\nf,New B,38.205,23.005\n')
  writeFileSync(join(source, 'gr', 'stop_times.txt'),
    'trip_id,stop_id,stop_sequence\nnew-trip,e,1\nnew-trip,f,2\n')
  writeFileSync(join(source, 'gr', 'frequencies.txt'),
    'trip_id,start_time,end_time,headway_secs\norphan,06:00:00,07:00:00,600\n')
  for (const cacheHits of [0, 1]) {
    const result = await enrichGlobalGtfsCountry(options)
    assert.equal(result.feeds[0].serviceCacheHits, cacheHits)
    assert.equal(result.feeds[0].railServicesWithoutStopTimes, 1)
    assert.equal(result.walk.serviceDailyDepartures?.total, 7)
    assert.equal(result.walk.serviceDailyDepartures?.failures.snapFailed, 6)
    assert.equal(result.walk.serviceDailyDepartures?.graphEstimated, 1)
    assert.ok(result.walk.quarantinedKilometres > 0)
    const intervals = listRailIntervals(prepared, square)
    assert.equal(intervals.length, 2)
    assert.deepEqual(intervals.find(row => row.osmId === prior[0].osmId), prior[0])
    assert.deepEqual(readFileSync(path), before)
  }
})

test('ready countries release slots, replay serial facts, and persist before a later failure', async () => {
  const source = join(TEMP, 'source-parallel')
  writeGreekGtfs(source, 'route_id,route_type\nrail,2\ntram,0\n')
  const stops = join(source, 'gr', 'stops.txt')
  writeFileSync(stops, readFileSync(stops, 'utf8').replaceAll('38.', '42.').replaceAll('23.', '19.'))
  for (const country of ['hr', 'fi']) cpSync(join(source, 'gr'), join(source, country), { recursive: true })
  writeFileSync(join(source, 'fi', 'routes.txt'), 'route_id,route_type\nrail,2\n')
  const outputs = ['serial', 'parallel'].map(name => {
    const prepared = join(TEMP, `countries-${name}`)
    const directory = squareDirectory(prepared, 42, 19)
    mkdirSync(directory, { recursive: true })
    const path = join(directory, 'railways.arrow')
    copyFileSync(writeRailwaysFixture(`countries-${name}.arrow`, [
      { latitude: 42, longitude: 19, endLatitude: 42.005, endLongitude: 19.005,
        lengthMetres: 708, railType: 0, country: 'GR' },
      { latitude: 42.001, longitude: 19.001, endLatitude: 42.0015, endLongitude: 19.0015,
        lengthMetres: 71, railType: 1, country: 'HR' },
    ]), path)
    const finnish = squareDirectory(prepared, 60, 24)
    mkdirSync(finnish, { recursive: true })
    copyFileSync(writeRailwaysFixture(`countries-${name}-fi.arrow`, [
      { osmId: 60_000, latitude: 60, longitude: 24, country: 'FI' },
    ]), join(finnish, 'railways.arrow'))
    writeSyntheticRailTopology(prepared, [directory, finnish].map(path => relative(prepared, path)))
    return { prepared, path }
  })
  const base = { sourceDirectory: source, cacheDirectory: join(TEMP, 'cache-parallel'), asOfDate: '20260909' }
  for (const country of ['GR', 'HR', 'FI']) {
    await enrichGlobalGtfsCountry({ ...base, preparedDirectory: outputs[0].prepared, country })
  }
  const before = readFileSync(outputs[1].path)
  // Hold GR until third-country FI is ready: a batch barrier would deadlock.
  const readyMarker = join(TEMP, 'third-country-ready')
  const preload = join(TEMP, 'country-ready-order.mjs')
  writeFileSync(preload, `
    import { existsSync, writeFileSync } from 'node:fs';
    import { setTimeout } from 'node:timers/promises';
    const country = process.argv[process.argv.indexOf('--country') + 1];
    if (process.send) {
      const send = process.send.bind(process);
      process.send = (...args) => {
        if (args[0] === 'ready' && country === 'FI') writeFileSync(${JSON.stringify(readyMarker)}, 'ready');
        if (args[0] === 'ready' && country === 'GR') {
          (async () => {
            while (!existsSync(${JSON.stringify(readyMarker)})) await setTimeout(10);
            send(...args);
          })();
          return true;
        }
        return send(...args);
      };
    }
  `)
  // Bound tiny fixture heaps so this exercises two real workers on small CI runners too.
  const run = () => spawnSync(process.execPath, [
    ...process.execArgv, '--max-old-space-size=256', '--import', preload, fileURLToPath(new URL('./enrich-railway-europe.ts', import.meta.url)),
    '--source-dir', source, '--prepared-dir', outputs[1].prepared,
    '--cache-dir', base.cacheDirectory, '--as-of-date', base.asOfDate, '--country', 'GR,HR,FI',
  ], { encoding: 'utf8', timeout: 15_000, killSignal: 'SIGKILL', env: { ...process.env, QM_ROAD_WORKERS: '2' } })
  const success = run()
  assert.equal(success.status, 0, success.stderr)
  assert.match(success.stdout, /"workers":2/)
  const facts = (prepared: string) => listRailIntervals(prepared).map(row => JSON.stringify(row)).sort()
  const expected = facts(outputs[0].prepared)
  assert.ok(expected.length > 0)
  assert.deepEqual(facts(outputs[1].prepared), expected)
  assert.deepEqual(readFileSync(outputs[1].path), before)
  const results = success.stdout.split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line))
  assert.equal(results.find(row => row.walk)?.country, 'HR', 'ready HR publishes while GR still routes')
  assert.ok(results.findIndex(row => row.phase && row.country === 'FI') <
    results.findIndex(row => row.walk && row.country === 'GR'), 'third country starts before first finishes')
  // HR commits a changed snapshot before invalid FI starts; held GR cannot publish.
  rmSync(readyMarker)
  appendFileSync(join(source, 'hr', 'trips.txt'), 'rail,daily,extra-trip\n')
  appendFileSync(join(source, 'hr', 'stop_times.txt'), 'extra-trip,a,1\nextra-trip,b,2\n')
  writeFileSync(join(source, 'fi', 'routes.txt'), 'route_id,route_name\nrail,broken\n')
  const failed = run()
  assert.equal(failed.status, 1, failed.stderr)
  assert.match(failed.stderr, /FI: railway worker exited/)
  await enrichGlobalGtfsCountry({ ...base, preparedDirectory: outputs[0].prepared, country: 'HR' })
  assert.notDeepEqual(facts(outputs[0].prepared), expected)
  assert.deepEqual(facts(outputs[1].prepared), facts(outputs[0].prepared))
  assert.deepEqual(readFileSync(outputs[1].path), before)
  writeFileSync(join(source, 'fi', 'routes.txt'), 'route_id,route_type\nrail,2\n')
  const replay = run()
  assert.equal(replay.status, 0, replay.stderr)
  assert.deepEqual(facts(outputs[1].prepared), facts(outputs[0].prepared))
})

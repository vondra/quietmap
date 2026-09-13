/** Selected whole GTFS services retain ordered stops, shapes, cache identity and parse failures. */

import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { spawnSync } from 'node:child_process'
import { openGtfsServices } from './gtfs-service-store.js'

const TMP = mkdtempSync(join(tmpdir(), 'gtfs-service-store-test-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

function writeGtfsFixture(dir: string, files: Record<string, string>): void {
  mkdirSync(dir, { recursive: true })
  for (const [name, contents] of Object.entries(files)) {
    writeFileSync(join(dir, name), contents.trim() + '\n')
  }
}

const NO_CALENDAR_ROUTES = 'route_id,route_type\nR1,2\n'

test('out-of-order stop_sequence is sorted numerically', async () => {
  const dir = join(TMP, 'seq-sort')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,C,3\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,50.1,14.1\nC,Charlie,50.2,14.2\n',
  })
  using store = await openGtfsServices(dir)
  assert.deepEqual(store.service('T1')!.stops.map(stop => stop.sourceStopId), ['A', 'B', 'C'])
})

test('parent-station resolution happens before coordless stops are dropped', async () => {
  const dir = join(TMP, 'parent-resolution')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,PLATFORM1,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon,parent_station\n' +
      'STATION1,Station One,50.0000,14.0000,\nPLATFORM1,Platform 1,,,STATION1\nB,Bravo,50.1000,14.1000,\n',
  })
  using store = await openGtfsServices(dir)
  const stops = store.service('T1')!.stops
  assert.equal(stops[0].sourceStopId, 'PLATFORM1')
  assert.equal(stops[0].stopId, 'STATION1')
  assert.deepEqual([stops[0].lat, stops[0].lon], [50, 14])
})

test('frequencies.txt expands a headway-defined trip into its daily repeat count', async () => {
  const dir = join(TMP, 'freq')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,50.1,14.1\n',
    'frequencies.txt': 'trip_id,start_time,end_time,headway_secs\nT1,06:00:00,07:00:00,600\n',
  })
  using store = await openGtfsServices(dir)
  assert.equal(store.provenance.frequenciesPresent, true)
  assert.equal(store.service('T1')!.departureMultiplier, 6)
})

test('frequencies.txt rejects a backwards interval instead of silently counting one trip', async () => {
  const dir = join(TMP, 'freq-backwards')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,50.1,14.1\n',
    'frequencies.txt': 'trip_id,start_time,end_time,headway_secs\nT1,07:00:00,06:00:00,600\n',
  })
  await assert.rejects(openGtfsServices(dir), /non-positive interval/)
})

test('shape attachment retains every source vertex, including a short excursion', async () => {
  const dir = join(TMP, 'shape-attach')
  const n = 700
  const shapeRows: string[] = []
  for (let i = 0; i < n; i++) {
    const lat = (i === 2 ? 50.5 : 50.0 + (i / (n - 1)) * 0.7).toFixed(6)
    const lon = (14.0 + (i / (n - 1)) * 0.7).toFixed(6)
    shapeRows.push(`SHAPE1,${lat},${lon},${i}`)
  }
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id,shape_id\nT1,R1,svc,SHAPE1\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0000,14.0000\nB,Bravo,50.1000,14.1000\n',
    'shapes.txt': 'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n' + shapeRows.join('\n') + '\n',
  })
  using store = await openGtfsServices(dir)
  assert.equal(store.service('T1')!.shape.length, n)
  assert.deepEqual([store.service('T1')!.shape[2].lat, store.service('T1')!.shape[2].lon], [50.5, 14.002003])
})

test('rail-only family filter excludes tram trips', async () => {
  const dir = join(TMP, 'rail-only')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nR1,2\nTRAM,0\n',
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\nT2,TRAM,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\nT2,C,1\nT2,D,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,A,50,14\nB,B,50.1,14.1\nC,C,50.2,14.2\nD,D,50.3,14.3\n',
  })
  using store = await openGtfsServices(dir)
  assert.ok(store.service('T1'))
  assert.equal(store.service('T2'), undefined)
})

test('cache round-trip uses an explicit path and leaves source inputs immutable by default', async () => {
  const dir = join(TMP, 'cache-roundtrip')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,50.1,14.1\n',
  })
  const cachePath = join(TMP, 'roundtrip.sqlite')
  const before = readdirSync(dir).sort()
  using uncached = await openGtfsServices(dir)
  assert.equal(uncached.provenance.fromCache, false)
  using first = await openGtfsServices(dir, { cachePath })
  assert.equal(first.provenance.fromCache, false)
  using second = await openGtfsServices(dir, { cachePath })
  assert.equal(second.provenance.fromCache, true)
  assert.deepEqual(readdirSync(dir).sort(), before)
  rmSync(cachePath)
  using rebuilt = await openGtfsServices(dir, { cachePath })
  assert.equal(rebuilt.provenance.fromCache, false)
})

test('source cache identity includes caller policy; input fingerprint invalidates coordinates and shapes', async () => {
  const dir = join(TMP, 'fingerprint')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id,shape_id\nT1,R1,svc,\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\nB,Bravo,50.1,14.1\n',
  })
  const cachePath = join(TMP, 'fingerprint.sqlite')
  {
    using first = await openGtfsServices(dir, { cachePath })
    assert.equal(first.provenance.fromCache, false)
  }
  {
    using second = await openGtfsServices(dir, { cachePath })
    assert.equal(second.provenance.fromCache, true)
  }
  const policy = { cachePath, optionsKey: 'europe-busiest-wed' }
  {
    using first = await openGtfsServices(dir, policy)
    assert.equal(first.provenance.fromCache, false)
  }
  {
    using second = await openGtfsServices(dir, policy)
    assert.equal(second.provenance.fromCache, true)
  }
  writeFileSync(join(dir, 'stops.txt'),
    'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\nB,Bravo,50.22,14.22\n')
  {
    using rebuilt = await openGtfsServices(dir, { cachePath })
    assert.equal(rebuilt.provenance.fromCache, false)
  }
})

test('malformed routes.txt throws; a valid bus-only feed is empty', async () => {
  const headerOnly = join(TMP, 'malformed-header-only')
  writeGtfsFixture(headerOnly, {
    'routes.txt': 'route_id,route_type\n',
    'trips.txt': 'trip_id,route_id,service_id\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\n',
  })
  await assert.rejects(openGtfsServices(headerOnly), /routes\.txt parse failure/)
  const noRouteType = join(TMP, 'malformed-no-route-type')
  writeGtfsFixture(noRouteType, {
    'routes.txt': 'route_id,route_short_name\nR1,S1\n',
    'trips.txt': 'trip_id,route_id,service_id\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\n',
  })
  await assert.rejects(openGtfsServices(noRouteType), /no route_type column/)
  const dir = join(TMP, 'legit-bus-only')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nB1,3\n',
    'trips.txt': 'trip_id,route_id,service_id\nT1,B1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\n',
  })
  using store = await openGtfsServices(dir)
  assert.equal([...store.services()].length, 0)
  assert.equal(store.provenance.activeTripCount, 0)
})

test('active stop-time rows and unused shapes do not exhaust the parser heap', () => {
  const dir = join(TMP, 'unused-shape-memory')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id,shape_id\nT1,R1,svc,ACTIVE\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,51,15\n',
    'shapes.txt': '\uFEFFshape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nACTIVE,51,15,2\n',
  })
  for (let trip = 0; trip < 5000; trip++) {
    appendFileSync(join(dir, 'trips.txt'), `REPEATED${trip},R1,svc,\n`)
    appendFileSync(join(dir, 'stop_times.txt'), Array.from({ length: 1000 }, (_, sequence) =>
      `REPEATED${trip},${sequence % 2 ? 'B' : 'A'},${sequence}\n`).join(''))
  }
  const shapes = join(dir, 'shapes.txt')
  const unused = Array.from({ length: 1000 }, (_, i) => `UNUSED,0,0,${i}\n`).join('')
  for (let i = 0; i < 2000; i++) appendFileSync(shapes, unused)
  appendFileSync(shapes, 'ACTIVE,50,14,1\n')
  const result = spawnSync(process.execPath, [
    '--max-old-space-size=128', '--import', import.meta.resolve('tsx'),
    '--input-type=module', '--eval',
    `import { openGtfsServices } from ${JSON.stringify(new URL('./gtfs-service-store.js', import.meta.url).href)};
     using store = await openGtfsServices(${JSON.stringify(dir)});
     let services = 0; for (const _ of store.services()) services++;
     console.log(JSON.stringify({ ...store.provenance, services }));`,
  ], { encoding: 'utf-8', timeout: 60_000 })
  assert.equal(result.status, 0, result.error?.message ?? result.stderr)
  const parsed = JSON.parse(result.stdout)
  assert.equal(parsed.tripsWithShape, 1)
  assert.equal(parsed.stopTimesLines, 5_000_002)
  assert.equal(parsed.services, 5001)
})

test('whole-service cache preserves repeated visits, source identities, times and shape distances', async () => {
  const dir = join(TMP, 'whole-service')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id,direction_id,shape_id\nT1,R1,svc,1,S\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence,arrival_time,departure_time,shape_dist_traveled\n' +
      'T1,A,30,25:10:00,25:11:00,20\nT1,PLATFORM,10,23:55:00,23:56:00,0\nT1,B,20,24:30:00,,10\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon,parent_station\n' +
      'A,Alpha,50,14,\nPLATFORM,Platform,,,A\nB,Bravo,51,15,\n',
    'shapes.txt': 'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence,shape_dist_traveled\n' +
      'S,50,14,30,20\nS,51,15,20,10\nS,50,14,10,0\n',
  })
  const cachePath = join(TMP, 'whole-service.sqlite')
  for (const fromCache of [false, true]) {
    using store = await openGtfsServices(dir, { cachePath })
    assert.equal(store.provenance.fromCache, fromCache)
    const svc = store.service('T1')!
    assert.deepEqual(svc.stops.map(stop => [stop.sequence, stop.sourceStopId, stop.stopId,
      stop.arrivalTime, stop.departureTime, stop.shapeDistance]), [
      [10, 'PLATFORM', 'A', '23:55:00', '23:56:00', 0],
      [20, 'B', 'B', '24:30:00', '', 10],
      [30, 'A', 'A', '25:10:00', '25:11:00', 20],
    ])
    assert.deepEqual(svc.shape, [
      { sequence: 10, lat: 50, lon: 14, shapeDistance: 0 },
      { sequence: 20, lat: 51, lon: 15, shapeDistance: 10 },
      { sequence: 30, lat: 50, lon: 14, shapeDistance: 20 },
    ])
  }
})

test('malformed source order or shape cannot be silently repaired and cached', async () => {
  for (const [label, stopTimes, shape] of [
    ['invalid-sequence', 'T1,A,1oops\nT1,B,2\n', 'S,50,14,1\nS,51,15,2\n'],
    ['duplicate-sequence', 'T1,A,1\nT1,B,1\n', 'S,50,14,1\nS,51,15,2\n'],
    ['invalid-shape', 'T1,A,1\nT1,B,2\n', 'S,50,14,1\nS,NaN,15,2\nS,51,15,3\n'],
    ['missing-shape', 'T1,A,1\nT1,B,2\n', 'UNUSED,50,14,1\n'],
  ]) {
    const dir = join(TMP, label), cachePath = join(TMP, `${label}.sqlite`)
    writeGtfsFixture(dir, {
      'routes.txt': NO_CALENDAR_ROUTES,
      'trips.txt': 'trip_id,route_id,service_id,shape_id\nT1,R1,svc,S\n',
      'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,51,15\n',
      'stop_times.txt': 'trip_id,stop_id,stop_sequence\n' + stopTimes,
      'shapes.txt': 'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n' + shape,
    })
    await assert.rejects(openGtfsServices(dir, { cachePath }))
    assert.equal(existsSync(cachePath), false)
  }
})

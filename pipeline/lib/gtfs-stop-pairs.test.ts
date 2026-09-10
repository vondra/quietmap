/**
 * Unit tests for computeStopPairFrequenciesForFeed (pipeline/lib/gtfs-stop-pairs.ts) —
 * synthetic tmpdir GTFS fixtures, fully offline (no network, no real feed download).
 *
 * Each test gets its OWN extractDir (subdirectory of a shared tmpdir root) so the
 * caller-owned derived cache never leaks between unrelated test
 * cases; the dedicated cache-round-trip test is the only one that deliberately calls
 * the parser twice against the same extractDir.
 *
 * Run: `cd pipeline && npx tsx --test lib/gtfs-stop-pairs.test.ts`
 */

import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { computeStopPairFrequenciesForFeed, type RailStationPairCount } from './gtfs-stop-pairs.js'

const TMP = mkdtempSync(join(tmpdir(), 'gtfs-stop-pairs-test-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

/** Write a minimal synthetic GTFS extract into `dir` (created fresh) from a map of
 *  filename -> CSV text (header row + data rows). Mirrors the fixture helper in
 *  gtfs-enrich-core.test.ts. */
function writeGtfsFixture(dir: string, files: Record<string, string>): void {
  mkdirSync(dir, { recursive: true })
  for (const [name, contents] of Object.entries(files)) {
    writeFileSync(join(dir, name), contents.trim() + '\n')
  }
}

/** Find the accumulated pair whose endpoints match (a,b) in EITHER order — pairs are
 *  canonicalized, so tests shouldn't need to know which side won the from/to slot. */
function findPair(pairs: RailStationPairCount[], a: [number, number], b: [number, number]): RailStationPairCount | undefined {
  return pairs.find(p => {
    const matchesForward = p.fromLat === a[0] && p.fromLon === a[1] && p.toLat === b[0] && p.toLon === b[1]
    const matchesReverse = p.fromLat === b[0] && p.fromLon === b[1] && p.toLat === a[0] && p.toLon === a[1]
    return matchesForward || matchesReverse
  })
}

const NO_CALENDAR_ROUTES = 'route_id,route_type\nR1,2\n' // rail-only route, no calendar files anywhere below
// (no calendar.txt/calendar_dates.txt in these fixtures => computeActiveTripFamiliesForFeed's
// "no calendar data at all" fallback counts every trip — keeps fixtures focused on the
// pair-parsing mechanics under test, not calendar-day selection.)

test('out-of-order stop_sequence is sorted numerically before pairing (file order is not trustworthy)', async () => {
  const dir = join(TMP, 'seq-sort')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    // Scrambled file order: seq 3, then 1, then 2.
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,C,3\n' +
      'T1,A,1\n' +
      'T1,B,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n' +
      'C,Charlie,50.2000,14.2000\n',
  })

  const { pairs } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 2, 'A-B and B-C, not A-C or any pair involving the scrambled file order')
  assert.ok(findPair(pairs, [50.0, 14.0], [50.1, 14.1]), 'A-B pair (numeric seq 1->2)')
  assert.ok(findPair(pairs, [50.1, 14.1], [50.2, 14.2]), 'B-C pair (numeric seq 2->3)')
  assert.ok(!findPair(pairs, [50.0, 14.0], [50.2, 14.2]), 'A-C would only appear if file order (3,1,2) had been trusted')
})

test('parent-station resolution happens BEFORE coordless stops are dropped', async () => {
  const dir = join(TMP, 'parent-resolution')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,PLATFORM1,1\n' +
      'T1,B,2\n',
    'stops.txt':
      // PLATFORM1 has NO coordinates of its own but declares parent_station=STATION1,
      // which does. Both a child stop and its parent-with-coords, plus an unrelated stop.
      'stop_id,stop_name,stop_lat,stop_lon,parent_station\n' +
      'STATION1,Station One,50.0000,14.0000,\n' +
      'PLATFORM1,Platform 1,,,STATION1\n' +
      'B,Bravo,50.1000,14.1000,\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 1)
  assert.ok(findPair(pairs, [50.0, 14.0], [50.1, 14.1]), 'PLATFORM1 resolved to STATION1 coords, paired with B')
  assert.equal(provenance.droppedUnresolvedStops, 0, 'the child stop was resolved via its parent, not dropped')
})

test('adjacent duplicate stations (same resolved coords, back-to-back) collapse to one node', async () => {
  const dir = join(TMP, 'adjacent-dedup')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,PLATFORM1,2\n' + // resolves via parent to the SAME station as PLATFORM2 (adjacent dup)
      'T1,PLATFORM2,3\n' +
      'T1,C,4\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon,parent_station\n' +
      'A,Alpha,50.0000,14.0000,\n' +
      'STATION1,Station One,50.1000,14.1000,\n' +
      'PLATFORM1,Platform 1,,,STATION1\n' +
      'PLATFORM2,Platform 2,,,STATION1\n' +
      'C,Charlie,50.2000,14.2000,\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 2, 'A-STATION1 and STATION1-C only — no zero-length STATION1-STATION1 pair')
  assert.ok(findPair(pairs, [50.0, 14.0], [50.1, 14.1]))
  assert.ok(findPair(pairs, [50.1, 14.1], [50.2, 14.2]))
  assert.equal(provenance.collapsedAdjacentDuplicates, 1, 'PLATFORM1/PLATFORM2 collapsed into one visit to STATION1')
})

test('a truly unresolvable (coordless, no parent) stop is dropped and pairs bridge across it', async () => {
  const dir = join(TMP, 'coordless-bridge')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,GHOST,2\n' + // no coords, no parent_station — genuinely unresolvable
      'T1,B,3\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'GHOST,,,\n' +
      'B,Bravo,50.1000,14.1000\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 1, 'exactly one pair: A-B, spanning the dropped GHOST stop')
  assert.ok(findPair(pairs, [50.0, 14.0], [50.1, 14.1]))
  assert.equal(provenance.droppedUnresolvedStops, 1)
})

test('direction SUM: A->B on one trip and B->A on another merge into ONE canonical pair with summed pax', async () => {
  const dir = join(TMP, 'direction-sum')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\nT2,R1,svc\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,B,2\n' + // A -> B
      'T2,B,1\n' +
      'T2,A,2\n', // B -> A (reverse direction, different trip)
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 1, 'A->B and B->A collapse onto the same canonical pair key')
  const pair = findPair(pairs, [50.0, 14.0], [50.1, 14.1])
  assert.ok(pair)
  assert.equal(pair!.pax, 2, 'both directions summed (1 + 1), not just one direction kept')
  assert.equal(pair!.frt, 0, 'GTFS is passenger-only')
  assert.equal(provenance.pairEventsBeforeDedup, 2)
  assert.equal(provenance.pairsAfterDedup, 1)
})

test('frequencies.txt expands a headway-defined trip into its daily repeat count (TH-multiplier style)', async () => {
  const dir = join(TMP, 'frequencies-expand')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,B,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n',
    // 06:00-10:00 (4h = 14400s) every 600s (10 min) => 24 runs.
    'frequencies.txt': 'trip_id,start_time,end_time,headway_secs\nT1,06:00:00,10:00:00,600\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  const pair = findPair(pairs, [50.0, 14.0], [50.1, 14.1])
  assert.ok(pair)
  assert.equal(pair!.pax, 24, 'floor(14400/600) = 24 daily runs, not 1')
  assert.equal(provenance.frequenciesExpanded, true, 'default is to expand when frequencies.txt exists')
})

test('frequencies.txt rejects a backwards interval instead of silently counting one trip', async () => {
  const dir = join(TMP, 'frequencies-backwards')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n',
    'frequencies.txt':
      'trip_id,start_time,end_time,headway_secs\nT1,22:00:00,06:00:00,600\n',
  })
  await assert.rejects(
    computeStopPairFrequenciesForFeed(dir),
    /non-positive interval for trip 'T1'/,
  )
})

test('calendar zero-active-services fix applies inside the pair parser too: zero pairs, not every trip counted', async () => {
  const dir = join(TMP, 'calendar-zero-active-pairs')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'inactive,0,0,0,0,0,0,0,20260101,20261231\n',
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,inactive\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,B,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 0, 'calendar present + no active service dates = zero pairs')
  assert.equal(provenance.calendarPresent, true)
  assert.equal(provenance.activeTripCount, 0)
  assert.ok(!existsSync(join(dir, 'gtfs-rail-pairs-v1.json')), 'never-cache-empty: this zero-pair parse must not be cached')
})

test('shape attachment downsamples to <= maxShapePoints and keeps first/last points exactly', async () => {
  const dir = join(TMP, 'shape-attach')
  const N = 700
  const shapeRows: string[] = []
  for (let i = 0; i < N; i++) {
    // A simple straight line of N points from (50.0, 14.0) to (50.7, 14.7).
    const lat = (50.0 + (i / (N - 1)) * 0.7).toFixed(6)
    const lon = (14.0 + (i / (N - 1)) * 0.7).toFixed(6)
    shapeRows.push(`SHAPE1,${lat},${lon},${i}`)
  }
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id,shape_id\nT1,R1,svc,SHAPE1\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,B,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n',
    'shapes.txt': 'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n' + shapeRows.join('\n') + '\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  const pair = findPair(pairs, [50.0, 14.0], [50.1, 14.1])
  assert.ok(pair)
  assert.ok(pair!.shapePolyline, 'trips.txt has shape_id and shapes.txt exists -> shape attached')
  assert.ok(pair!.shapePolyline!.length <= 500, `downsampled to <= 500 points, got ${pair!.shapePolyline!.length}`)
  assert.deepEqual(pair!.shapePolyline![0], [50.0, 14.0], 'first shape point preserved exactly')
  assert.deepEqual(pair!.shapePolyline![pair!.shapePolyline!.length - 1], [50.7, 14.7], 'last shape point preserved exactly')
  assert.equal(provenance.tripsWithShape, 1)
})

test('rail-only family filter excludes tram trips (default familyOf)', async () => {
  const dir = join(TMP, 'rail-only-filter')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nR1,2\nR2,0\n', // R1 rail, R2 tram
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\nT2,R2,svc\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,B,2\n' +
      'T2,A,1\n' +
      'T2,C,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n' +
      'C,Charlie,50.9000,14.9000\n',
  })

  const { pairs } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 1, 'only the rail trip (T1) contributes a pair')
  assert.ok(findPair(pairs, [50.0, 14.0], [50.1, 14.1]), 'A-B from the rail trip')
  assert.ok(!findPair(pairs, [50.0, 14.0], [50.9, 14.9]), 'A-C from the tram trip must be excluded')
})

test('cache round-trip uses an explicit path and leaves source inputs immutable by default', async () => {
  const dir = join(TMP, 'cache-round-trip')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\nB,Bravo,50.1,14.1\n',
  })
  const uncached = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(uncached.provenance.fromCache, false)
  assert.equal(existsSync(join(dir, 'gtfs-rail-pairs-v1.json')), false)

  const cachePath = join(TMP, 'cache-round-trip.json')
  const first = await computeStopPairFrequenciesForFeed(dir, { cachePath })
  assert.equal(first.provenance.fromCache, false)
  assert.ok(existsSync(cachePath))
  const second = await computeStopPairFrequenciesForFeed(dir, { cachePath })
  assert.equal(second.provenance.fromCache, true)
  assert.deepEqual(second.pairs, first.pairs)
})

test('shape conflict: a SECOND different shapeId reaching an already-shaped pair drops the polyline entirely (2026-07-16 /gg review item 11a — keeps the ambiguity probe alive)', async () => {
  const dir = join(TMP, 'shape-conflict')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id,shape_id\nT1,R1,svc,SHAPE1\nT2,R1,svc,SHAPE2\n',
    'stop_times.txt':
      'trip_id,stop_id,stop_sequence\n' +
      'T1,A,1\n' +
      'T1,B,2\n' +
      'T2,A,1\n' +
      'T2,B,2\n',
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'A,Alpha,50.0000,14.0000\n' +
      'B,Bravo,50.1000,14.1000\n',
    'shapes.txt':
      'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\n' +
      'SHAPE1,50.0000,14.0000,0\n' +
      'SHAPE1,50.1000,14.1000,1\n' +
      'SHAPE2,50.0000,14.0000,0\n' +
      'SHAPE2,50.0500,14.2000,1\n' + // a DIFFERENT corridor shape reaching the same A-B pair
      'SHAPE2,50.1000,14.1000,2\n',
  })

  const { pairs, provenance } = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(pairs.length, 1, 'still one canonical A-B pair')
  const pair = findPair(pairs, [50.0, 14.0], [50.1, 14.1])
  assert.ok(pair)
  assert.equal(pair!.shapePolyline, undefined, 'conflicting shapeIds drop the polyline entirely — the walk\'s ambiguity probe must run unconstrained, not pick one corridor arbitrarily')
  assert.equal(provenance.tripsWithShape, 2, 'both trips DID carry a shape at the trip level — the conflict is detected at the PAIR merge, not trip parsing')
})

test('optionsKey fingerprints an explicit derived cache', async () => {
  const dir = join(TMP, 'options-fingerprint')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\nB,Bravo,50.1,14.1\n',
  })
  const cachePath = join(TMP, 'options-fingerprint.json')
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath, optionsKey: 'default' })).provenance.fromCache, false)
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath, optionsKey: 'europe-busiest-wed' })).provenance.fromCache, false)
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath, optionsKey: 'europe-busiest-wed' })).provenance.fromCache, true)
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath, optionsKey: 'default' })).provenance.fromCache, false)
})

test('input fingerprint invalidates the pair cache when coordinates or shapes change', async () => {
  const dir = join(TMP, 'inputs-fingerprint')
  writeGtfsFixture(dir, {
    'routes.txt': NO_CALENDAR_ROUTES,
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\nT1,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\nB,Bravo,50.1,14.1\n',
  })
  const cachePath = join(TMP, 'inputs-fingerprint.json')
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath })).provenance.fromCache, false)
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath })).provenance.fromCache, true)
  writeFileSync(
    join(dir, 'stops.txt'),
    'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\nB,Bravo,50.22,14.22\n',
  )
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath })).provenance.fromCache, false)
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath })).provenance.fromCache, true)
  writeFileSync(
    join(dir, 'shapes.txt'),
    'shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nS,50.0,14.0,1\n',
  )
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath })).provenance.fromCache, false)
  writeFileSync(join(dir, 'feed_info.txt'), 'feed_start_date,feed_end_date\n20260901,20260930\n')
  assert.equal((await computeStopPairFrequenciesForFeed(dir, { cachePath })).provenance.fromCache, false)
})

test('malformed routes.txt THROWS instead of returning the empty result a legitimately rail-less feed yields (2026-07-16 review item 3)', async () => {
  const headerOnly = join(TMP, 'malformed-header-only')
  writeGtfsFixture(headerOnly, {
    'routes.txt': 'route_id,route_type\n',
    'trips.txt': 'trip_id,route_id,service_id\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\n',
  })
  await assert.rejects(() => computeStopPairFrequenciesForFeed(headerOnly), /routes\.txt parse failure/, 'header-only routes.txt is a parse failure, never "no rail routes"')

  const noRouteType = join(TMP, 'malformed-no-route-type')
  writeGtfsFixture(noRouteType, {
    'routes.txt': 'route_id,route_short_name\nR1,S1\n',
    'trips.txt': 'trip_id,route_id,service_id\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\n',
  })
  await assert.rejects(() => computeStopPairFrequenciesForFeed(noRouteType), /no route_type column/, 'a routes.txt without route_type is a parse failure')
})

test('legitimately rail-less routes.txt (valid header, bus-only rows) still yields the EMPTY result, no throw (item 3\'s other direction)', async () => {
  const dir = join(TMP, 'legit-bus-only')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nB1,3\nB2,3\n', // bus-only — Carris/Toluca shape
    'trips.txt': 'trip_id,route_id,service_id\nT1,B1,svc\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\nT1,A,1\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50.0,14.0\n',
  })
  const result = await computeStopPairFrequenciesForFeed(dir)
  assert.equal(result.pairs.length, 0, 'zero rail routes with a VALID header is a legitimate empty, not an error')
  assert.equal(result.provenance.fromCache, false)
})

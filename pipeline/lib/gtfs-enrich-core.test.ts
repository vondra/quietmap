/** GTFS service selection and spatial matching regressions. */

import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  computeActiveTripFamiliesForFeed, computeStopFrequenciesForFeed, findBusiestWednesday,
  findTargetWednesday, loadStopsWithCoords, parseGtfsDate, readMergedStopCache, routeFamily,
  writeMergedStopCache, dedupeStopsByLocation, buildTramExtraMatch,
  declaredRouteFamiliesForFeed, describeIncompleteFamilies, type StopTrainCount,
} from './gtfs-enrich-core.js'
import type { RailwayRow } from './railways-arrow.js'

const TMP = mkdtempSync(join(tmpdir(), 'gtfs-enrich-core-test-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

/** Write a minimal synthetic GTFS extract into `dir` (created fresh) from a map of
 *  filename -> CSV text (header row + data rows, no trailing newline required). */
function writeGtfsFixture(dir: string, files: Record<string, string>): void {
  mkdirSync(dir, { recursive: true })
  for (const [name, contents] of Object.entries(files)) {
    writeFileSync(join(dir, name), contents.trim() + '\n')
  }
}

test('merged-stop cache v2 round-trip preserves stops AND feed provenance', () => {
  const path = join(TMP, 'v2.json')
  const stops = [{ stop_id: 's1', lat: 50.85, lon: 4.35, trains_passenger: 42 }]
  writeMergedStopCache(path, ['stib-brussels', 'tec-wallonia'], stops)
  const cached = readMergedStopCache<(typeof stops)[number]>(path)
  assert.deepEqual(cached.stops, stops)
  assert.deepEqual(cached.feedsLoadedNonEmpty, ['stib-brussels', 'tec-wallonia'])

})

test('routeFamily: basic GTFS codes route rail vs tram/metro vs dropped (DE de_full profile)', () => {
  // gtfs.de de_full flattens DELFI NeTEx to BASIC route_types only (verified
  // 2026-07-11: 2 rail, 0 tram, 1 subway, 3 bus, 4 ferry, 7 funicular — no
  // TPEG 100-117). Locks the family routing enrich-railway-de.ts rides on:
  // ICE/IC/RE/S-Bahn (2) must never land in the tram grid, U-Bahn (1) groups
  // with tram (OSM light_rail), road/water/funicular modes must drop out
  // (OSM funicular rows keep the engine's own 40/day default instead).
  assert.equal(routeFamily(2), 'rail')
  assert.equal(routeFamily(0), 'tram')
  assert.equal(routeFamily(1), 'tram')
  assert.equal(routeFamily(3), null, 'bus never enriches a rail row')
  assert.equal(routeFamily(4), null, 'ferry dropped')
  assert.equal(routeFamily(7), null, 'funicular dropped — engine default owns it')
})

test('routeFamily: TPEG extended codes keep the same family split', () => {
  // Representatives of the extended sets (feeds like ÖBB/opentransportdata
  // publish these): railway subtypes → rail, tram/metro subtypes → tram.
  assert.equal(routeFamily(102), 'rail', 'long-distance rail (TPEG 102)')
  assert.equal(routeFamily(109), 'rail', 'suburban railway (TPEG 109)')
  assert.equal(routeFamily(900), 'tram')
  assert.equal(routeFamily(402), 'tram', 'metro groups with tram (OSM light_rail)')
  assert.equal(routeFamily(715), null, 'bus subtype dropped')
})

// ── computeActiveTripFamiliesForFeed ──
// The shared routes+calendar+trips resolver used by both computeStopFrequenciesForFeed
// (below) and the station-pair parser (gtfs-stop-pairs.ts).

const RAIL_ROUTES_CSV = 'route_id,route_type\nR1,2\n'
const TRIPS_CSV = (serviceId: string) => `trip_id,route_id,service_id\nT1,R1,${serviceId}\n`

test('computeActiveTripFamiliesForFeed: calendar present + zero active services on target date = ZERO trips (2026-07-15 fix)', async () => {
  const dir = join(TMP, 'calendar-zero-active')
  // An all-zero calendar row has no recurring service and no calendar_dates additions.
  // calendar.txt still exists, so its empty active set remains authoritative.
  writeGtfsFixture(dir, {
    'routes.txt': RAIL_ROUTES_CSV,
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'inactive,0,0,0,0,0,0,0,20260101,20261231\n',
    'trips.txt': TRIPS_CSV('inactive'),
  })

  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily)
  assert.equal(result.calendarPresent, true, 'calendar.txt exists — this is the "we know the active set" branch')
  assert.equal(result.activeServiceIds.size, 0, 'the declared calendar has no active service day')
  assert.equal(
    result.tripFam.size, 0,
    'BUG FIX: calendar present + zero active services must yield ZERO trips, not "count everything" — ' +
    'the pre-2026-07-15 code gated on `activeServiceIds.size > 0`, so this exact case silently counted T1 as running',
  )
})

test('computeActiveTripFamiliesForFeed: exact-dates calendar.txt (all weekday flags 0) takes the calendar_dates path — SE Trafiklab convention', async () => {
  const dir = join(TMP, 'calendar-exact-dates')
  // The 2026-07-16 SE finding: every calendar.txt row is all-zeros (rows only declare a
  // validity span; activation lives in calendar_dates.txt add exceptions). The weekday
  // branch would sample zero services on every Wednesday and fall to the span-midpoint
  // fallback — a date the operator's rolling exception horizon may not even reach (SL
  // trams: 0 active => "feed empty" => the completeness gate blocked the world sweep).
  // 20260114 is a Wednesday.
  writeGtfsFixture(dir, {
    'routes.txt': RAIL_ROUTES_CSV,
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'exact_dates_svc,0,0,0,0,0,0,0,20260101,20261231\n',
    'calendar_dates.txt':
      'service_id,date,exception_type\n' +
      'exact_dates_svc,20260114,1\n' +
      'exact_dates_svc,20260115,1\n',
    'trips.txt': TRIPS_CSV('exact_dates_svc'),
  })

  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily)
  assert.equal(result.calendarPresent, true)
  assert.equal(result.targetDate, '20260114', 'busiest WEDNESDAY from calendar_dates, never the span midpoint')
  assert.equal(result.activeServiceIds.has('exact_dates_svc'), true)
  assert.equal(result.tripFam.size, 1, 'the exact-dates service counts — before the fix this feed read as empty')
})

test('computeActiveTripFamiliesForFeed: calendar present + a matching active service still counts that trip (fix does not over-correct)', async () => {
  const dir = join(TMP, 'calendar-nonzero-active')
  writeGtfsFixture(dir, {
    'routes.txt': RAIL_ROUTES_CSV,
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'daily,1,1,1,1,1,1,1,20260101,20261231\n',
    'trips.txt': TRIPS_CSV('daily'),
  })

  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily)
  assert.equal(result.calendarPresent, true)
  assert.ok(result.activeServiceIds.has('daily'))
  assert.equal(result.tripFam.get('T1'), 'rail', 'a trip whose service DOES run on the target date is still counted')
})

test('computeActiveTripFamiliesForFeed: no calendar files at all still counts every trip (preserved fallback)', async () => {
  const dir = join(TMP, 'no-calendar-files')
  writeGtfsFixture(dir, {
    'routes.txt': RAIL_ROUTES_CSV,
    'trips.txt': TRIPS_CSV('whatever'), // no calendar.txt/calendar_dates.txt define this service at all
  })

  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily)
  assert.equal(result.calendarPresent, false, 'neither calendar.txt nor calendar_dates.txt exists')
  assert.equal(result.tripFam.get('T1'), 'rail', 'the pre-existing "no calendar data" fallback still counts all trips')
})

test('computeActiveTripFamiliesForFeed: a custom dateSelection hook overrides the default midpoint-Wednesday picker', async () => {
  const dir = join(TMP, 'custom-date-selection')
  writeGtfsFixture(dir, {
    'routes.txt': RAIL_ROUTES_CSV,
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'svc,1,1,1,1,1,1,1,20260101,20260201\n',
    'trips.txt': TRIPS_CSV('svc'),
  })

  const forcedDate = '20260115'
  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily, () => forcedDate)
  assert.equal(result.targetDate, forcedDate, 'europe-style busiest-Wednesday sampler (or any hook) wins over the default heuristic')
})

test('empty calendar selection is deterministic and does not consult the current date', () => {
  assert.equal(findTargetWednesday([]), '')
  assert.equal(findBusiestWednesday([]), '')
})

test('findBusiestWednesday preserves the continental producer service-density choice', () => {
  const rows = [
    { start_date: '20260101', end_date: '20261231', wednesday: '1' },
    { start_date: '20260601', end_date: '20260731', wednesday: '1' },
    { start_date: '20260601', end_date: '20260731', wednesday: '1' },
  ]
  assert.equal(findBusiestWednesday(rows), '20260603')
})

test('service selection uses a real operating day when eligible rail has no Wednesday service', async () => {
  const dir = join(TMP, 'no-wednesday-service')
  writeGtfsFixture(dir, {
    'routes.txt': RAIL_ROUTES_CSV,
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'rail_thursday,0,0,0,1,0,0,0,20260901,20260930\n' +
      'unrelated_bus_wednesday,0,0,1,0,0,0,0,20260901,20260930\n',
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,rail_thursday\n',
  })
  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily, findBusiestWednesday)
  assert.equal(result.targetDate, '20260903')
  assert.deepEqual([...result.tripFam.keys()], ['T1'])
})

test('busiest service selection applies exceptions and retains every declared family', async () => {
  const dir = join(TMP, 'busiest-complete-family-day')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nrail,2\ntram,0\n',
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'rail_peak,0,0,1,0,0,0,0,20260107,20260107\n' +
      'rail_daily,0,0,1,0,0,0,0,20260107,20260128\n' +
      'tram_daily,0,0,1,0,0,0,0,20260107,20260128\n',
    'calendar_dates.txt':
      'service_id,date,exception_type\ntram_daily,20260107,2\n',
    'trips.txt':
      'trip_id,route_id,service_id\npeak,rail,rail_peak\nrail,rail,rail_daily\ntram,tram,tram_daily\n',
  })
  const result = await computeActiveTripFamiliesForFeed(dir, routeFamily, findBusiestWednesday)
  assert.equal(result.targetDate, '20260114', '20260107 has more scheduled rows but its tram is removed by an exception')
  assert.deepEqual([...new Set(result.tripFam.values())].sort(), ['rail', 'tram'])
})

test('GTFS calendar arithmetic is UTC and independent of the host time zone', () => {
  assert.equal(parseGtfsDate('20260603'), Date.UTC(2026, 5, 3))
  assert.equal(new Date(parseGtfsDate('20260603')).getUTCDay(), 3)
  assert.equal(findBusiestWednesday([
    { start_date: '20260603', end_date: '20260603', wednesday: '1' },
  ]), '20260603')
})

test('findBusiestWednesday scans a large calendar without argument-spread overflow', () => {
  const rows = Array.from({ length: 150_000 }, () => ({
    start_date: '20260107', end_date: '20260107', wednesday: '1',
  }))
  assert.equal(findBusiestWednesday(rows), '20260107')
})

test('stop frequencies expand GTFS headway templates once per daily departure', async () => {
  const dir = join(TMP, 'stop-headway-expansion')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nT,0\n',
    'calendar.txt':
      'service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n' +
      'daily,1,1,1,1,1,1,1,20260901,20260930\n',
    'trips.txt': 'trip_id,route_id,service_id\ntrip,T,daily\n',
    'frequencies.txt': 'trip_id,start_time,end_time,headway_secs\ntrip,06:00:00,08:00:00,600\n',
    'stop_times.txt': 'trip_id,stop_id,stop_sequence\ntrip,A,1\ntrip,B,2\n',
    'stops.txt': 'stop_id,stop_name,stop_lat,stop_lon\nA,Alpha,50,14\nB,Bravo,50.1,14.1\n',
  })
  const stops = await computeStopFrequenciesForFeed({ id: 'headway' }, dir, [49, 13, 51, 15])
  assert.deepEqual(stops.map(stop => stop.trains_passenger), [12, 12])
})

test('GTFS stops accept the equator and prime meridian but reject invalid ranges', async () => {
  const dir = join(TMP, 'coordinate-validation')
  writeGtfsFixture(dir, {
    'stops.txt':
      'stop_id,stop_name,stop_lat,stop_lon\n' +
      'equator,Equator,0,14\n' +
      'greenwich,Greenwich,51,0\n' +
      'bad-lat,Bad latitude,91,14\n' +
      'bad-lon,Bad longitude,51,181\n',
  })
  const result = await loadStopsWithCoords(dir)
  assert.deepEqual([...result.stopsMap.keys()], ['equator', 'greenwich'])
  assert.equal(result.skippedNoCoords, 2)
})

test('computeActiveTripFamiliesForFeed: familyOf hook filters out non-matching route types (rail-only view)', async () => {
  const dir = join(TMP, 'family-hook-filter')
  writeGtfsFixture(dir, {
    'routes.txt': 'route_id,route_type\nR1,2\nR2,0\n', // R1 rail, R2 tram
    'trips.txt': 'trip_id,route_id,service_id\nT1,R1,svc\nT2,R2,svc\n',
  })

  const railOnly = (routeType: number): 'rail' | null => (routeType === 2 ? 'rail' : null)
  const result = await computeActiveTripFamiliesForFeed(dir, railOnly)
  assert.equal(result.tripFam.size, 1)
  assert.equal(result.tripFam.get('T1'), 'rail')
  assert.equal(result.tripFam.has('T2'), false, 'tram trip excluded by the rail-only familyOf hook')
})

// ── dedupeStopsByLocation / buildTramExtraMatch (2026-07-16 Phase 4 hoist) ──
// Shared by enrich-railway-europe.ts AND every national enrich-railway-{cc}.ts —
// see those files' own tests for the au-vic/registry-flavored integration checks;
// these pin the pure logic at its one source of truth.

const stop = (lat: number, lon: number, pax: number, family: 'rail' | 'tram' = 'rail'): StopTrainCount =>
  ({ stop_id: `${lat}_${lon}_${family}`, lat, lon, name: 'x', family, trains_passenger: pax, trains_freight: 0 })

test('dedupeStopsByLocation: sums same coord+family, keeps distinct coords and families apart', () => {
  const out = dedupeStopsByLocation([
    stop(50.0, 14.0, 10),
    stop(50.0, 14.0, 5),           // same coord+family → sums to 15
    stop(50.0001, 14.0, 3),        // distinct coord → stays separate
    stop(50.0, 14.0, 7, 'tram'),   // same coord, different family → separate
  ])
  assert.equal(out.length, 3)
  const rail = out.filter(s => s.family === 'rail').sort((a, b) => b.trains_passenger - a.trains_passenger)
  assert.equal(rail[0].trains_passenger, 15)
  assert.equal(rail[1].trains_passenger, 3)
  assert.equal(out.find(s => s.family === 'tram')!.trains_passenger, 7)
})

const FAKE_RAIL_ROW = (railType: number, existingDivisor = 1): RailwayRow => ({
  railType, usage: 0, service: 0, existingSourceId: 0,
  existingPassenger: 0, existingFreight: 0, existingDivisor,
  startLat: 50.0, startLon: 14.0, endLat: 50.0, endLon: 14.0, midLat: 50.0, midLon: 14.0, name: '',
})

test('buildTramExtraMatch crosses z9 square boundaries because the stop index is global', () => {
  const tramStops: StopTrainCount[] = [
    { stop_id: 'S1', lat: 50.0001, lon: 14.0001, name: 'Adjacent stop', family: 'tram', trains_passenger: 42, trains_freight: 1 },
  ]
  const result = buildTramExtraMatch(tramStops, 12345)(FAKE_RAIL_ROW(2, 3), 0, 'z9/275/173')
  assert.deepEqual(result, { passenger: 42, freight: 1, sourceId: 12345, divisor: 1 })
})

test('buildTramExtraMatch never offers tram counts to heavy rail', () => {
  const tramStops: StopTrainCount[] = [
    { stop_id: 'S1', lat: 50.0001, lon: 14.0001, name: 'Stop', family: 'tram', trains_passenger: 42, trains_freight: 0 },
  ]
  assert.equal(buildTramExtraMatch(tramStops, 1)(FAKE_RAIL_ROW(0), 0, 'z9/275/173'), null)
})

test('buildTramExtraMatch returns null for an empty stop index', () => {
  assert.equal(buildTramExtraMatch([], 1)(FAKE_RAIL_ROW(1), 0, 'z9/275/173'), null)
})

test('buildTramExtraMatch filters mixed input to the tram family internally', () => {
  const mixed: StopTrainCount[] = [
    { stop_id: 'RAIL', lat: 50.00005, lon: 14.00005, name: 'Rail', family: 'rail', trains_passenger: 190, trains_freight: 4 },
    { stop_id: 'TRAM', lat: 50.0006, lon: 14.0006, name: 'Tram', family: 'tram', trains_passenger: 42, trains_freight: 0 },
  ]
  const result = buildTramExtraMatch(mixed, 100)(FAKE_RAIL_ROW(2), 0, 'z9/275/173')
  assert.equal(result?.passenger, 42)
})

// ── declaredRouteFamiliesForFeed / describeIncompleteFamilies (review items 3+4) ──

test('declaredRouteFamiliesForFeed: reads the declared family set from routes.txt; bus-only declares neither', async () => {
  const mixedDir = join(TMP, 'declared-mixed')
  writeGtfsFixture(mixedDir, { 'routes.txt': 'route_id,route_type\nR1,2\nT1,0\nB1,3\n' })
  const declared = await declaredRouteFamiliesForFeed(mixedDir, routeFamily)
  assert.deepEqual([...declared].sort(), ['rail', 'tram'])

  const busDir = join(TMP, 'declared-bus-only')
  writeGtfsFixture(busDir, { 'routes.txt': 'route_id,route_type\nB1,3\nB2,715\n' })
  const busDeclared = await declaredRouteFamiliesForFeed(busDir, routeFamily)
  assert.equal(busDeclared.size, 0, 'a bus-only feed (PT Carris / MX Toluca shape) declares NO rail families — exempt from both completeness directions')
})

test('declaredRouteFamiliesForFeed: respects a narrowed classifier — a warsaw-ztm-style pair-null override never demands back the narrowed-away family', async () => {
  const dir = join(TMP, 'declared-narrowed')
  writeGtfsFixture(dir, { 'routes.txt': 'route_id,route_type\nS1,2\nT1,0\n' })
  // Pair side narrowed to always-null (mirror-publish exclusion), tram side normal:
  const declared = await declaredRouteFamiliesForFeed(dir, (rt) => (routeFamily(rt) === 'tram' ? 'tram' : null))
  assert.deepEqual([...declared], ['tram'], 'the excluded rail family is NOT declared — completeness will not require pairs from a feed whose pair contribution is deliberately zero')
})

test('malformed routes.txt throws from the shared reader (header-only / missing route_type) — computeActiveTripFamiliesForFeed rides the same reader (item 3)', async () => {
  const headerOnly = join(TMP, 'declared-header-only')
  writeGtfsFixture(headerOnly, { 'routes.txt': 'route_id,route_type\n' })
  await assert.rejects(() => declaredRouteFamiliesForFeed(headerOnly, routeFamily), /header-only or empty/)
  await assert.rejects(() => computeActiveTripFamiliesForFeed(headerOnly, routeFamily), /header-only or empty/)

  const noType = join(TMP, 'declared-no-route-type')
  writeGtfsFixture(noType, { 'routes.txt': 'route_id,route_short_name\nR1,IC\n' })
  await assert.rejects(() => declaredRouteFamiliesForFeed(noType, routeFamily), /no route_type column/)
  await assert.rejects(() => computeActiveTripFamiliesForFeed(noType, routeFamily), /no route_type column/)
})

test('describeIncompleteFamilies: BIDIRECTIONAL — each declared family independently requires its parsed output non-empty (item 4)', () => {
  const both = new Set(['rail', 'tram'])
  assert.equal(describeIncompleteFamilies('f', both, 100, 50), '', 'both declared, both parsed — complete')
  assert.match(describeIncompleteFamilies('f', both, 0, 50), /declares rail but 0 station pairs/, 'working tram must NOT mask empty rail (the original masking direction)')
  assert.match(describeIncompleteFamilies('f', both, 100, 0), /declares tram but 0 tram stops/, 'working rail must NOT mask empty tram (the direction the old any-family check missed)')
  assert.equal(describeIncompleteFamilies('f', new Set(['rail']), 5, 0), '', 'rail-only feed: empty tram is its normal state')
  assert.equal(describeIncompleteFamilies('f', new Set(['tram']), 0, 5), '', 'tram-only feed: empty pairs is its normal state')
  assert.equal(describeIncompleteFamilies('f', new Set(), 0, 0), '', 'bus-only feed: exempt from both — retract can finally activate over MX/PT')
  assert.equal(describeIncompleteFamilies('f', both, 100, null), '', 'tramStopCount null = merged-cache-served run; tram direction is vouched by the cache\'s own recorded provenance')
  assert.match(describeIncompleteFamilies('f', both, 0, null), /declares rail/, 'the rail/pairs direction is still enforced on cache-served runs (pairs are always fresh)')
})


test('feed_info bounds both recurring and exception-only service selection', async () => {
  for (const exactDates of [false, true]) {
    const dir = join(TMP, `feed-info-window-${exactDates}`)
    writeGtfsFixture(dir, {
      'routes.txt': RAIL_ROUTES_CSV,
      'trips.txt': 'trip_id,route_id,service_id\nT1,R1,current\nT2,R1,old\nT3,R1,older\n',
      'feed_info.txt': 'feed_start_date,feed_end_date\n20260901,20260930\n',
      ...(exactDates ? {
        'calendar_dates.txt': 'service_id,date,exception_type\nold,20260107,1\nolder,20260107,1\ncurrent,20260909,1\n',
      } : {
        'calendar.txt': 'service_id,wednesday,start_date,end_date\nold,1,20260101,20261231\nolder,1,20260101,20260131\ncurrent,1,20260901,20260930\n',
      }),
    })
    const result = await computeActiveTripFamiliesForFeed(dir, routeFamily, findBusiestWednesday)
    assert.ok(result.targetDate >= '20260901' && result.targetDate <= '20260930', result.targetDate)
    assert.ok(result.tripFam.has('T1'))
    assert.ok(!result.tripFam.has('T3'))
  }
})

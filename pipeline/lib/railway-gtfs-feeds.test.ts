/** Registry and immutable source-layout contract for the global GTFS producer. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  GLOBAL_GTFS_FEEDS, countryGtfsBbox, gtfsSourceDirectories, railFamilyFor,
  validateGtfsSourceFreshness,
} from './railway-gtfs-feeds.js'

const TEMP = mkdtempSync(join(tmpdir(), 'railway-gtfs-feeds-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))

function writeRequiredGtfs(directory: string): void {
  mkdirSync(directory, { recursive: true })
  for (const name of ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt']) {
    writeFileSync(join(directory, name), 'header\n')
  }
  writeFileSync(
    join(directory, 'calendar.txt'),
    'service_id,end_date\ncurrent,20991231\n',
  )
}

test('registry is exactly the 23-feed, 21-country dev1 global aggregate', () => {
  assert.deepEqual(GLOBAL_GTFS_FEEDS.map(feed => feed.id), [
    'de', 'ch', 'at', 'nl', 'se', 'no', 'fi', 'be', 'in', 'us', 'ca', 'fr',
    'lu', 'gr', 'lv-pv', 'ee', 'bg-sofia', 'hr', 'hu', 'sk', 'fr-idf',
    'au-vic', 'au-qld',
  ])
  assert.equal(new Set(GLOBAL_GTFS_FEEDS.map(feed => feed.country)).size, 21)
  assert.deepEqual(
    GLOBAL_GTFS_FEEDS.filter(feed => feed.country === 'FR').map(feed => feed.id),
    ['fr', 'fr-idf'],
  )
  assert.deepEqual(
    GLOBAL_GTFS_FEEDS.filter(feed => feed.country === 'AU').map(feed => feed.id),
    ['au-vic', 'au-qld'],
  )
})

test('registry keeps the two deliberate family overrides', () => {
  const franceTram = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'fr-idf')!
  const victoria = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'au-vic')!
  assert.equal(railFamilyFor(2, franceTram), null)
  assert.equal(railFamilyFor(0, franceTram), 'tram')
  assert.equal(railFamilyFor(400, victoria), 'rail')
  assert.equal(railFamilyFor(0, victoria), null)
})

test('source discovery requires all three Victoria railway mode directories', () => {
  const de = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'de')!
  const au = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'au-vic')!
  writeRequiredGtfs(join(TEMP, 'de', 'extracted'))
  writeRequiredGtfs(join(TEMP, 'au-vic', '1', 'extracted'))
  writeRequiredGtfs(join(TEMP, 'au-vic', '10', 'extracted'))
  writeRequiredGtfs(join(TEMP, 'au-vic', '2', 'extracted'))
  mkdirSync(join(TEMP, 'au-vic', '3'), { recursive: true })
  assert.deepEqual(gtfsSourceDirectories(TEMP, de), [join(TEMP, 'de', 'extracted')])
  assert.deepEqual(gtfsSourceDirectories(TEMP, au), [
    join(TEMP, 'au-vic', '1', 'extracted'),
    join(TEMP, 'au-vic', '2', 'extracted'),
    join(TEMP, 'au-vic', '10', 'extracted'),
  ])
  rmSync(join(TEMP, 'au-vic', '2'), { recursive: true })
  assert.deepEqual(gtfsSourceDirectories(TEMP, au), [])
})

test('country bbox unions all feeds and pads the shared half-degree border', () => {
  assert.deepEqual(countryGtfsBbox('FR'), [40.8, -5.7, 51.6, 10.1])
  assert.throws(() => countryGtfsBbox('ZZ'), /no global GTFS feed/)
})

test('freshness rejects every expired source, including the historical Greece fixture', async () => {
  const stale = join(TEMP, 'stale')
  writeRequiredGtfs(stale)
  writeFileSync(join(stale, 'calendar.txt'), 'service_id,end_date\nold,20240101\n')
  const de = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'de')!
  const gr = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'gr')!
  await assert.rejects(
    validateGtfsSourceFreshness(de, stale, '20260905'),
    /expired 20240101/,
  )
  await assert.rejects(
    validateGtfsSourceFreshness(gr, stale, '20260905'),
    /GTFS feed gr expired 20240101/,
  )
})

test('freshness scans large exception calendars without argument-spread overflow', async () => {
  const directory = join(TEMP, 'large-calendar')
  writeRequiredGtfs(directory)
  rmSync(join(directory, 'calendar.txt'))
  const lines = ['service_id,date,exception_type']
  for (let index = 0; index < 150_000; index++) {
    lines.push(`service-${index},20991231,1`)
  }
  writeFileSync(join(directory, 'calendar_dates.txt'), lines.join('\n'))
  const de = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'de')!
  assert.equal(
    (await validateGtfsSourceFreshness(de, directory, '20260905')).lastServiceDate,
    '20991231',
  )
})

test('freshness ignores calendar-date removals when finding the service horizon', async () => {
  const directory = join(TEMP, 'removed-service-date')
  writeRequiredGtfs(directory)
  rmSync(join(directory, 'calendar.txt'))
  writeFileSync(
    join(directory, 'calendar_dates.txt'),
    'service_id,date,exception_type\nold,20240101,1\ncancelled,20991231,2\n',
  )
  const de = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'de')!
  await assert.rejects(
    validateGtfsSourceFreshness(de, directory, '20260905'),
    /expired 20240101/,
  )
})

/** Registry and immutable source-layout contract for the global GTFS producer. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  GLOBAL_GTFS_FEEDS, NATIONAL_GTFS_FEEDS, countryGtfsBbox, gtfsDownloadUrls, gtfsSourceDirectories, gtfsTextSourceSha256, railFamilyFor,
  sevenZipExecutable,
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

test('global registry keeps the 22 productive feeds across 21 countries', () => {
  assert.deepEqual(GLOBAL_GTFS_FEEDS.map(feed => feed.id), [
    'de', 'ch', 'at', 'nl', 'se', 'no', 'fi', 'be', 'in', 'us', 'ca', 'fr',
    'lu', 'gr', 'lv-pv', 'ee', 'bg-sofia', 'hr', 'hu', 'sk', 'au-vic', 'au-qld',
  ])
  assert.equal(new Set(GLOBAL_GTFS_FEEDS.map(feed => feed.country)).size, 21)
  assert.deepEqual(
    GLOBAL_GTFS_FEEDS.filter(feed => feed.country === 'FR').map(feed => feed.id),
    ['fr'],
  )
  assert.deepEqual(
    GLOBAL_GTFS_FEEDS.filter(feed => feed.country === 'AU').map(feed => feed.id),
    ['au-vic', 'au-qld'],
  )
})

test('registries retain only productive, explicit family overrides', () => {
  const victoria = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'au-vic')!
  const thailand = NATIONAL_GTFS_FEEDS.find(feed => feed.id === 'namtang')!
  assert.equal(railFamilyFor(400, victoria), 'rail')
  assert.equal(railFamilyFor(0, victoria), null)
  assert.equal(railFamilyFor(1, thailand), null)
})

test('every immutable feed records its refresh URL and Greece no longer points at the 2019 archive', () => {
  for (const feed of [...GLOBAL_GTFS_FEEDS, ...NATIONAL_GTFS_FEEDS]) {
    assert.ok(gtfsDownloadUrls(feed).every(url => /^https?:\/\//.test(url)), feed.id)
  }
  assert.equal(
    GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'gr')!.url,
    'https://jbb.ghsq.de/gtfs/gr-hellenic-train.gtfs.zip',
  )
  assert.deepEqual(
    gtfsDownloadUrls(NATIONAL_GTFS_FEEDS.find(feed => feed.id === 'toscana-trenitalia')!),
    ['https://dati.toscana.it/dataset/8bb8f8fe-fe7d-41d0-90dc-49f2456180d1/resource/4f85393b-357d-443d-8378-65de4198505f/download/trenitalia.gtfs'],
  )
})

test('national registry consolidates 42 current and three pinned historical GTFS feeds in 17 countries', () => {
  assert.equal(NATIONAL_GTFS_FEEDS.length, 45)
  assert.equal(new Set(NATIONAL_GTFS_FEEDS.map(feed => feed.country)).size, 17)
  assert.deepEqual(
    [...new Set(NATIONAL_GTFS_FEEDS.map(feed => feed.country))].sort(),
    ['AE', 'AR', 'AU', 'BE', 'CA', 'DE', 'DK', 'ES', 'FI', 'IE', 'IL', 'IT', 'MX', 'PL', 'PT', 'SE', 'TH'],
  )
  assert.equal(NATIONAL_GTFS_FEEDS.find(feed => feed.id === 'warsaw-ztm')!.includeRailPairs, false)
  assert.equal(NATIONAL_GTFS_FEEDS.filter(feed => feed.acceptedHistoricalSource).length, 3)
  for (const country of new Set(NATIONAL_GTFS_FEEDS.map(feed => feed.country))) {
    assert.equal(new Set(NATIONAL_GTFS_FEEDS.filter(feed => feed.country === country).map(feed => feed.sourceId)).size, 1)
  }
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

test('identity-pinned archive extracts only into the derived cache', () => {
  const source = join(TEMP, 'archive-source')
  const input = join(TEMP, 'archive-input')
  const cache = join(TEMP, 'archive-cache')
  mkdirSync(source)
  writeRequiredGtfs(input)
  const archive = join(source, 'feed.zip')
  const created = spawnSync(sevenZipExecutable(), ['a', archive, '.'], { cwd: input, encoding: 'utf8' })
  assert.equal(created.status, 0, created.stderr)
  const archiveSha256 = createHash('sha256').update(readFileSync(archive)).digest('hex')
  const base = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'de')!
  const feed = { ...base, id: 'archive-test', sourcePath: 'missing', sourceArchive: {
    relativePath: 'feed.zip', sha256: archiveSha256,
  } }
  const directories = gtfsSourceDirectories(source, feed, cache)
  assert.equal(directories.length, 1)
  assert.ok(directories[0].startsWith(cache))
  assert.deepEqual(gtfsSourceDirectories(source, feed, cache), directories)
  writeFileSync(archive, 'changed')
  assert.throws(() => gtfsSourceDirectories(source, feed, cache), /archive identity mismatch/)
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
  const archived = { ...de, id: 'archived-test', acceptedHistoricalSource: {
    gtfsTextSha256: gtfsTextSourceSha256(stale), lastServiceDate: '20240101', sourceYear: 2024,
  } }
  assert.deepEqual(await validateGtfsSourceFreshness(archived, stale, '20260905'),
    { lastServiceDate: '20240101', historical: true })
  writeFileSync(join(stale, 'routes.txt'), 'changed\n')
  await assert.rejects(validateGtfsSourceFreshness(archived, stale, '20260905'), /historical source identity/)
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


test('freshness rejects a future feed instead of using its broad recurring calendar', async () => {
  const directory = join(TEMP, 'future-feed')
  writeRequiredGtfs(directory)
  writeFileSync(join(directory, 'feed_info.txt'), 'feed_start_date,feed_end_date\n20260910,20260916\n')
  const us = GLOBAL_GTFS_FEEDS.find(feed => feed.id === 'us')!
  await assert.rejects(validateGtfsSourceFreshness(us, directory, '20260909'), /starts 20260910/)
})

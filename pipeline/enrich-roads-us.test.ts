/** FHWA source/cache and real US z9 traffic-enrichment regressions. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { enrichUsRoads, loadUsSegments, parseUsPage, runUsEnrichment } from './enrich-roads-us.js'
import { iso2Code, listPreparedSquares } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'
import type { RoadLoaderArguments } from './lib/road-loader-cli.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-us-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

function feature(aadt: unknown = 10000, functionalClass: unknown = 1, longitude = -84, latitude = 34) {
  return {
    properties: { AADT: aadt, F_SYSTEM: functionalClass },
    geometry: { type: 'LineString', coordinates: [[longitude, latitude], [longitude, latitude]] },
  }
}

function cache(name: string): RoadLoaderArguments {
  const root = join(DIRECTORY, name)
  mkdirSync(join(root, 'us'), { recursive: true })
  return { preparedDirectory: root, enrichmentDirectory: root, enrichOnly: true, forceDownload: false }
}

function fresh(name: string): RoadLoaderArguments {
  const root = join(DIRECTORY, name)
  return { preparedDirectory: root, enrichmentDirectory: root, enrichOnly: false, forceDownload: true }
}

function page(options: RoadLoaderArguments, offset: number, contents: unknown): void {
  writeFileSync(join(options.enrichmentDirectory, 'us', `hpms-page-${offset}.json`), JSON.stringify(contents))
}

test('FHWA parser preserves all five class splits and genuine positive half-count truncation', () => {
  const parsed = parseUsPage({ features: [1, 2, 3, 4, 5].map(rank => feature(10000, rank)) })
  assert.deepEqual(parsed.segments.map(({ light, medium, heavy, moto }) => [light, medium, heavy, moto]), [
    [8700, 240, 960, 100], [8900, 200, 800, 100], [9100, 160, 640, 100],
    [9300, 120, 480, 100], [9400, 100, 400, 100],
  ])
  // Actual hpms-page-12000.json record near Redding, California.
  const decimal = parseUsPage({ features: [feature(47277.5, 1, -122.291209293, 40.45164878)] }).segments[0]
  assert.deepEqual(
    [decimal.aadt, decimal.light, decimal.medium, decimal.heavy, decimal.moto],
    [47277, 41131, 1135, 4538, 473],
  )
  assert.equal(decimal.longitude, -122.291209293)
  assert.equal(parseUsPage({ features: [feature(100, 6), feature(0)] }).segments.length, 0)
})

test('FHWA malformed pages/counts/coordinates fail instead of manufacturing empty source data', () => {
  for (const invalid of [null, {}, { error: { code: 500 }, features: [] }, { features: [null] }]) {
    assert.throws(() => parseUsPage(invalid), /FHWA/)
  }
  for (const invalid of [-1, 'bad', true, Infinity, 2 ** 31, null, '']) {
    assert.throws(() => parseUsPage({ features: [feature(invalid)] }), /FHWA/)
  }
  assert.throws(() => parseUsPage({ features: [feature(100, 1.5)] }), /F_SYSTEM/)
  assert.throws(() => parseUsPage({ features: [feature(100, 1, -181)] }), /coordinate/)
  const multiline = {
    ...feature(), geometry: { type: 'MultiLineString', coordinates: [[[-84, 34], [-84.02, 34.02]]] },
  }
  const center = parseUsPage({ features: [multiline] }).segments[0]
  assert.ok(Math.abs(center.longitude + 84.01) < 1e-12)
  assert.ok(Math.abs(center.latitude - 34.01) < 1e-12)
})

test('FHWA offline snapshot requires every page and accepts its small empty terminal page', async () => {
  const options = cache('offline')
  page(options, 0, { features: [feature()] })
  await assert.rejects(loadUsSegments(options), /cache page missing/)
  page(options, 2000, { type: 'FeatureCollection', features: [] })
  assert.equal((await loadUsSegments(options)).length, 1)
  page(options, 4000, { features: [feature()] })
  await assert.rejects(loadUsSegments(options), /beyond terminal/)
  const short = cache('nonempty-after-short')
  page(short, 0, { features: [feature()] })
  page(short, 2000, { features: [feature()] })
  await assert.rejects(loadUsSegments(short), /nonempty page follows a short/)
})

test('FHWA validates the entire cache before the first prepared Arrow write', async () => {
  const options = cache('atomic-source-validation')
  const square = join(options.preparedDirectory, 'z9', '136', '204')
  mkdirSync(square, { recursive: true })
  const target = join(square, 'roads.arrow')
  copyFileSync(writeRoadsFixture('us-invalid-cache.arrow', [0], {
    origin: [-84, 34], countryCodes: [iso2Code('US')],
  }), target)
  const before = readFileSync(target)
  page(options, 0, { features: Array.from({ length: 2000 }, () => feature()) })
  page(options, 2000, { error: { message: 'broken later page' } })
  await assert.rejects(runUsEnrichment(options), /valid features array/)
  assert.deepEqual(readFileSync(target), before)
})

test('FHWA downloader installs pages only after complete source admission', async context => {
  const options = { ...cache('download'), enrichOnly: false, forceDownload: true }
  type Scenario = 'old' | 'late-network-failure' | 'late-malformed-body' | 'fresh-network-failure' | 'fresh-success'
  let scenario: Scenario = 'old'
  const offsets: number[] = []
  context.mock.method(globalThis, 'fetch', async (input: string | URL | Request) => {
    const query = new URL(String(input)).searchParams
    const offset = Number(query.get('resultOffset'))
    assert.equal(query.get('orderByFields'), 'OBJECTID')
    offsets.push(offset)
    if (scenario === 'late-network-failure' || scenario === 'fresh-network-failure') {
      if (offset === 0) return new Response(JSON.stringify({ features: [feature(200)] }))
      throw new Error('synthetic late FHWA network failure')
    }
    if (scenario === 'late-malformed-body') {
      if (offset === 0) return new Response(JSON.stringify({ features: [feature(300)] }))
      return new Response(JSON.stringify({ error: { code: 500 } }))
    }
    if (scenario === 'old' || scenario === 'fresh-success') {
      return new Response(JSON.stringify({ features: offset === 0 ? [feature(scenario === 'old' ? 100 : 500)] : [] }))
    }
    throw new Error(`unhandled test scenario: ${scenario}`)
  })

  assert.equal((await loadUsSegments(options)).length, 1)
  assert.deepEqual(offsets, [0, 2000])
  const sourcePaths = [0, 2000].map(offset => join(options.enrichmentDirectory, 'us', `hpms-page-${offset}.json`))
  const original = sourcePaths.map(path => ({ path, bytes: readFileSync(path), inode: statSync(path).ino }))
  const assertOriginalCache = () => {
    for (const { path, bytes, inode } of original) {
      assert.deepEqual(readFileSync(path), bytes)
      assert.equal(statSync(path).ino, inode)
    }
  }
  const offline = { ...options, enrichOnly: true, forceDownload: false }

  offsets.length = 0
  scenario = 'late-network-failure'
  await assert.rejects(loadUsSegments(options), /synthetic late FHWA network failure/)
  assert.deepEqual(offsets, [0, 2000])
  assertOriginalCache()
  assert.equal((await loadUsSegments(offline))[0].aadt, 100)

  offsets.length = 0
  scenario = 'late-malformed-body'
  await assert.rejects(loadUsSegments(options), /valid features array/)
  assert.deepEqual(offsets, [0, 2000])
  assertOriginalCache()
  assert.equal((await loadUsSegments(offline))[0].aadt, 100)

  const freshFailure = fresh('fresh-network-failure')
  offsets.length = 0
  scenario = 'fresh-network-failure'
  await assert.rejects(loadUsSegments(freshFailure), /synthetic late FHWA network failure/)
  assert.deepEqual(offsets, [0, 2000])
  assert.equal(existsSync(join(freshFailure.enrichmentDirectory, 'us')), false)

  const freshSuccess = fresh('fresh-success')
  offsets.length = 0
  scenario = 'fresh-success'
  assert.equal((await loadUsSegments(freshSuccess))[0].aadt, 500)
  assert.deepEqual(offsets, [0, 2000])
  assert.equal(existsSync(join(freshSuccess.enrichmentDirectory, 'us', 'hpms-page-0.json')), true)
  assert.equal(existsSync(join(freshSuccess.enrichmentDirectory, 'us', 'hpms-page-2000.json')), true)
})

test('actual US z9 Arrow matching protects road class, source priority and baked territory', async () => {
  const options = cache('roundtrip')
  const square = join(options.preparedDirectory, 'z9', '136', '204')
  mkdirSync(square, { recursive: true })
  const target = join(square, 'roads.arrow')
  copyFileSync(writeRoadsFixture('us-roundtrip.arrow', [0, 3, 5, 0, 0, 0, 10], {
    origin: [-84, 34],
    countryCodes: ['US', 'US', 'US', 'CA', 'PR', 'US', 'US'].map(iso2Code),
    // A registered same-tier newer source exercises the existing priority gate.
    sourceIds: [0, 0, 21, 21, 0, 24, 0],
  }), target)
  const features = Array.from({ length: 7 }, (_, index) =>
    feature(398000, 1, -83.99975 + index * 0.001, 34.00025 + index * 0.001))
  features.push(feature(10000, 4, -83.9984, 34.0016))
  page(options, 0, { features })
  page(options, 2000, { features: [] })
  assert.deepEqual(listPreparedSquares(options.preparedDirectory, [34, -84, 34.01, -83.99]), ['z9/136/204'])
  const result = await runUsEnrichment(options)
  assert.deepEqual(
    { matched: result.matched, skipped: result.skipped, skippedForeign: result.skippedForeign },
    { matched: 3, skipped: 1, skippedForeign: 2 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual(Array.from({ length: 7 }, (_, i) => table.getChild('source_id')!.get(i)), [21, 21, 0, 0, 0, 24, 21])
  assert.deepEqual(
    ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].map(name => table.getChild(name)!.get(0)),
    [346260, 9552, 38208, 3980],
  )
  assert.equal(table.getChild('aadt_light')!.get(1), 9300)
  assert.equal(table.getChild('aadt_light')!.get(5), 1005)
  assert.equal(result.retracted, 2) // Out-of-coverage and foreign own stamps are obsolete.
  assert.equal(table.schema.metadata.get('roads_contract'), 'country_baked_v1')
  assert.equal(table.schema.metadata.get('grid'), 'z30')
  const [[south, west, north, east]] = JSON.parse(table.schema.metadata.get('qm_batch_bboxes')!)
  assert.ok(south <= 34.00025 && north >= 34.00625 && north < 35)
  assert.ok(west <= -83.99975 && east >= -83.99375 && east < -83)
  const before = readFileSync(target)
  assert.equal((await runUsEnrichment(options)).squaresUpdated, 0)
  assert.deepEqual(readFileSync(target), before)
  const unrelated = parseUsPage({ features: [feature(10000, 1, -80, 40)] }).segments
  const removed = await enrichUsRoads(options.preparedDirectory, unrelated)
  assert.equal(removed.matched, 0); assert.equal(removed.retracted, 3)
  const after = tableFromIPC(readFileSync(target))
  assert.deepEqual([...after.getChild('source_id')!], [0, 0, 0, 0, 0, 24, 0])
  for (const field of table.schema.fields) {
    if (['source_id', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto'].includes(field.name)) continue
    assert.deepEqual(after.getChild(field.name)!.toArray(), table.getChild(field.name)!.toArray())
  }
  assert.deepEqual(after.schema.metadata, table.schema.metadata)
  assert.deepEqual(after.batches.map(batch => batch.numRows), table.batches.map(batch => batch.numRows))
  const stable = readFileSync(target), inode = statSync(target).ino
  assert.equal((await enrichUsRoads(options.preparedDirectory, unrelated)).squaresUpdated, 0)
  assert.deepEqual(readFileSync(target), stable); assert.equal(statSync(target).ino, inode)

})

test('Puerto Rico measurements do not bypass the current registry and baked-country authority', async () => {
  const options = cache('puerto-rico')
  const square = join(options.preparedDirectory, 'z9', '162', '229')
  mkdirSync(square, { recursive: true })
  const target = join(square, 'roads.arrow')
  copyFileSync(writeRoadsFixture('us-puerto-rico.arrow', [0], {
    origin: [-66, 18], countryCodes: [iso2Code('PR')],
  }), target)
  const before = readFileSync(target)
  const { segments } = parseUsPage({ features: [feature(10000, 1, -65.99975, 18.00025)] })
  const result = await enrichUsRoads(options.preparedDirectory, segments)
  assert.equal(result.skippedForeign, 1)
  assert.equal(result.matched, 0)
  assert.deepEqual(readFileSync(target), before)
})

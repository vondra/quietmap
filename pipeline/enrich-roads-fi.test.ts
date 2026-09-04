/** FI loader tests for complete parsing, class-safe matching and ownership. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import {
  enrichFinnishRoads, fiRoadNumberRank, parseFiPages, type FiRoadSegment,
} from './enrich-roads-fi.js'
import { iso2Code } from './lib/prepared-grid.js'
import { writeRoadsFixture } from './lib/road-test-fixture.js'

const TEST_DIRECTORY = mkdtempSync(join(tmpdir(), 'enrich-roads-fi-test-'))
after(() => rmSync(TEST_DIRECTORY, { recursive: true, force: true }))

function feature(properties: Record<string, unknown> = {}, coordinates: unknown = [
  [[24, 60, 5], [24.02, 60.02, 6]],
]): unknown {
  return {
    properties: { kvl: 1000, kvl_raskas: 100, alkusijainti_tie: 4, ...properties },
    geometry: { type: 'MultiLineString', coordinates },
  }
}

test('FI parser preserves current class split, centroid and road-number ranks', () => {
  const parsed = parseFiPages([{ features: [feature()] }]).segments
  assert.equal(parsed.length, 1)
  assert.ok(Math.abs(parsed[0].latitude - 60.01) < 1e-12)
  assert.ok(Math.abs(parsed[0].longitude - 24.01) < 1e-12)
  assert.deepEqual({ ...parsed[0], latitude: 60.01, longitude: 24.01 }, {
    roadNumber: 4, latitude: 60.01, longitude: 24.01, rank: 0,
    aadt: 1000, light: 890, medium: 0, heavy: 100, moto: 10,
  })
  assert.deepEqual([4, 101, 102, 100, 999, 1000].map(fiRoadNumberRank), [0, 0, 0, 2, 2, 4])
})

test('FI parser drops absent road identity and rejects malformed counts and coordinates', () => {
  assert.deepEqual(parseFiPages([{ features: [feature({ alkusijainti_tie: 0 })] }]).segments, [])
  assert.throws(() => parseFiPages([{ features: [feature({ kvl_raskas: -1 })] }]), /invalid Finnish KVL integer/)
  assert.throws(() => parseFiPages([{ features: [feature({ kvl_raskas: true })] }]), /invalid Finnish KVL integer/)
  assert.throws(() => parseFiPages([{ features: [feature({}, [[[24, 'bad']]])] }]), /invalid Finnish KVL coordinate/)
  assert.throws(() => parseFiPages([{ features: [feature({}, [[[240, 60]]])] }]), /invalid Finnish KVL coordinate/)
})

test('FI parser rejects published heavy counts above the exact total without clamping', () => {
  const parsed = parseFiPages([{ features: [
    feature(),
    feature({ internal_id: 17352, alkusijainti_tie: 21819, kvl: 109, kvl_raskas: 520 }),
  ] }])
  assert.equal(parsed.segments.length, 1)
  assert.equal(parsed.inconsistentClassTotalsSkipped, 1)
})

function segment(overrides: Partial<FiRoadSegment>): FiRoadSegment {
  return {
    roadNumber: 4, latitude: 50.00025, longitude: 14.00025, rank: 1,
    aadt: 1000, light: 890, medium: 0, heavy: 100, moto: 10,
    ...overrides,
  }
}

test('z9 FI pass selects the nearest compatible class and heals a foreign FI stamp', async () => {
  const prepared = join(TEST_DIRECTORY, 'prepared')
  const square = join(prepared, 'z9', '290', '148')
  mkdirSync(square, { recursive: true })
  const source = writeRoadsFixture('fi-loader.arrow', [1, 1], {
    countryCodes: [iso2Code('FI'), iso2Code('SE')],
    sourceIds: [0, 1039],
  })
  const target = join(square, 'roads.arrow')
  copyFileSync(source, target)
  const incompatible = segment({ rank: 4, light: 9999 })
  const compatible = segment({ latitude: 50.0005, rank: 1, light: 890 })

  const result = await enrichFinnishRoads(prepared, [incompatible, compatible])
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 1, retracted: 1, skippedForeign: 1 },
  )
  const table = tableFromIPC(readFileSync(target))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [1039, 0])
  assert.equal(table.getChild('aadt_light')!.get(0), 890)
  assert.equal(table.schema.metadata.get('roads_contract'), 'country_baked_v1')
})

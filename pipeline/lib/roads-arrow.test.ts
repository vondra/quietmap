/** Core safety contracts for the z9 road enrichment writer. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { tableFromIPC } from 'apache-arrow'
import { iso2Code } from './prepared-grid.js'
import { bytes, writeRoadsFixture } from './road-test-fixture.js'
import {
  disjointVehicleClassCountsFitPublishedTotal, osmRoadClassRank, writeRoadAadt, type RoadRow,
} from './roads-arrow.js'

const STAMP_ID = 10 // measured continental road source
const MAJOR_CLASSES = new Set([0, 1, 2, 3, 4, 10, 11, 12])
const payload = (sourceId = STAMP_ID) => ({ light: 9999, medium: 888, heavy: 77, moto: 6, sourceId })

test('coverage gate never offers or changes out-of-coverage road classes', async () => {
  const path = writeRoadsFixture('coverage.arrow', [0, 2, 5, 7, 9, 10])
  const offered: RoadRow[] = []
  const result = await writeRoadAadt(path, row => { offered.push(row); return payload() }, undefined, MAJOR_CLASSES)
  assert.deepEqual(
    { rows: result.rows, matched: result.matched, skipped: result.skipped, updated: result.updated },
    { rows: 6, matched: 3, skipped: 3, updated: true },
  )
  assert.deepEqual(offered.map(row => row.roadClass).sort((a, b) => a - b), [0, 2, 10])
  const table = tableFromIPC(bytes(path))
  for (let index = 0; index < table.numRows; index++) {
    const covered = MAJOR_CLASSES.has(table.getChild('road_class')!.get(index) as number)
    assert.equal(table.getChild('aadt_light')!.get(index), covered ? 9999 : 1000 + index)
    assert.equal(table.getChild('source_id')!.get(index), covered ? STAMP_ID : 0)
  }
  assert.equal(table.getChild('ref')!.get(3), 'R3')
})

test('road-class rank keeps majors, collapses links and excludes minors', () => {
  assert.deepEqual([0, 1, 2, 3, 4].map(osmRoadClassRank), [0, 1, 2, 3, 4])
  assert.deepEqual([10, 11, 12].map(osmRoadClassRank), [0, 1, 2])
  assert.deepEqual([5, 6, 7, 8, 9].map(osmRoadClassRank), [6, 6, 6, 6, 6])
})

test('disjoint class totals distinguish exact from independent integer rounding', () => {
  const counts = [91, 3, 7, 1]
  assert.equal(disjointVehicleClassCountsFitPublishedTotal(100, counts, 'exact'), false)
  assert.equal(disjointVehicleClassCountsFitPublishedTotal(100, counts, 'independently-rounded'), true)
  assert.equal(disjointVehicleClassCountsFitPublishedTotal(99, counts, 'independently-rounded'), false)
})

test('AADT outside the on-disk non-negative Int32 domain aborts without typed-array coercion', async () => {
  for (const [name, light] of [
    ['nan', NaN], ['negative', -1], ['fractional', 1.5], ['overflow', 2 ** 31],
  ] as const) {
    const path = writeRoadsFixture(`malformed-${name}.arrow`, [0, 2])
    const before = bytes(path)
    await assert.rejects(writeRoadAadt(path, () => ({ ...payload(), light })), /invalid match/)
    assert.deepEqual(bytes(path), before)
  }
})

test('missing payload field aborts instead of coercing undefined to zero', async () => {
  const path = writeRoadsFixture('malformed-missing.arrow', [0])
  await assert.rejects(
    writeRoadAadt(path, () => ({ light: 100, heavy: 5, moto: 1, sourceId: STAMP_ID }) as never),
    /invalid match/,
  )
})

test('zero, unknown and wrong-layer source ids are rejected', async () => {
  const zero = writeRoadsFixture('malformed-zero-id.arrow', [0])
  await assert.rejects(writeRoadAadt(zero, () => payload(0)), /invalid match/)
  const unknown = writeRoadsFixture('malformed-unknown-id.arrow', [0])
  await assert.rejects(writeRoadAadt(unknown, () => payload(65000)), /not a registered roads source/)
  const railway = writeRoadsFixture('malformed-rail-id.arrow', [0])
  await assert.rejects(writeRoadAadt(railway, () => payload(110)), /not a registered roads source/)
})

test('retract disowns owned rows before coverage without touching another source', async () => {
  const path = writeRoadsFixture('retract.arrow', [7, 2, 7])
  await writeRoadAadt(path, (_row, index) => ({ ...payload(index === 2 ? 12 : STAMP_ID), light: 111 }))
  const result = await writeRoadAadt(
    path, () => null, undefined, MAJOR_CLASSES,
    { sourceId: STAMP_ID, when: row => osmRoadClassRank(row.roadClass) > 4 },
  )
  assert.equal(result.retracted, 1)
  const table = tableFromIPC(bytes(path))
  assert.deepEqual([...Array(3)].map((_, i) => table.getChild('source_id')!.get(i)), [0, STAMP_ID, 12])
  assert.deepEqual([...Array(3)].map((_, i) => table.getChild('aadt_light')!.get(i)), [0, 111, 111])
})

test('a retracted row can be reclaimed in the same pass', async () => {
  const path = writeRoadsFixture('retract-reclaim.arrow', [2])
  await writeRoadAadt(path, () => ({ ...payload(), light: 111 }))
  const result = await writeRoadAadt(
    path, () => ({ light: 500, medium: 10, heavy: 5, moto: 2, sourceId: STAMP_ID }),
    undefined, MAJOR_CLASSES, { sourceId: STAMP_ID, when: () => true },
  )
  assert.deepEqual({ retracted: result.retracted, matched: result.matched }, { retracted: 1, matched: 1 })
  assert.equal(tableFromIPC(bytes(path)).getChild('aadt_light')!.get(0), 500)
})

test('baked ownership admits domestic/global rows and refuses foreign or unknown rows', async () => {
  const countries = [iso2Code('CZ'), iso2Code('US'), 0]
  const foreign = writeRoadsFixture('foreign.arrow', [0, 0, 0], { countryCodes: countries })
  const before = bytes(foreign)
  const us = await writeRoadAadt(foreign, () => payload(21))
  assert.deepEqual({ matched: us.matched, skippedForeign: us.skippedForeign }, { matched: 1, skippedForeign: 2 })
  const table = tableFromIPC(bytes(foreign))
  assert.deepEqual([...Array(3)].map((_, i) => table.getChild('source_id')!.get(i)), [0, 21, 0])
  assert.notDeepEqual(bytes(foreign), before)

  const domestic = writeRoadsFixture('domestic.arrow', [0])
  assert.equal((await writeRoadAadt(domestic, () => payload(20))).matched, 1)
  const global = writeRoadsFixture('global.arrow', [5])
  assert.equal((await writeRoadAadt(global, () => payload(11))).matched, 1)
})

test('a national rerun retracts its stale foreign stamp even when its matcher still claims the row', async () => {
  const path = writeRoadsFixture('foreign-retract.arrow', [1], {
    countryCodes: [iso2Code('RU')],
    sourceIds: [20],
  })
  const result = await writeRoadAadt(
    path,
    () => payload(20),
    undefined,
    MAJOR_CLASSES,
    { sourceId: 20, when: () => false },
  )
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, skippedForeign: result.skippedForeign },
    { matched: 0, retracted: 1, skippedForeign: 1 },
  )
  const table = tableFromIPC(bytes(path))
  assert.equal(table.getChild('source_id')!.get(0), 0)
  assert.equal(table.getChild('aadt_light')!.get(0), 0)
})

test('national enrichment fails closed without the complete admin bake', async () => {
  for (const [name, options] of [
    ['no-country-contract.arrow', { omitCountryContract: true }],
    ['no-country-column.arrow', { omitCountryColumn: true }],
  ] as const) {
    const path = writeRoadsFixture(name, [0], options)
    const before = bytes(path)
    await assert.rejects(writeRoadAadt(path, () => payload(20)), /country_baked_v1|country_iso/)
    assert.deepEqual(bytes(path), before)
  }
})

test('an exact accepted rerun reports its match but remains byte-identical', async () => {
  const path = writeRoadsFixture('idempotent.arrow', [2])
  const first = await writeRoadAadt(path, () => payload())
  assert.equal(first.updated, true)
  const before = bytes(path)
  const second = await writeRoadAadt(path, () => payload())
  assert.deepEqual({ matched: second.matched, updated: second.updated }, { matched: 1, updated: false })
  assert.deepEqual(bytes(path), before)
})

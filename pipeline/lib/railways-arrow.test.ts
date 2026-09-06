/** Safety and idempotence contracts for the z9 railway traffic writer. */

import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { test } from 'node:test'
import { tableFromIPC } from 'apache-arrow'
import { iso2Code } from './prepared-grid.js'
import { railwayBytes, writeRailwaysFixture } from './rail-test-fixture.js'
import { writeRailParallelDivisor, writeRailwayTraffic } from './railways-arrow.js'

const CD_SOURCE_ID = 9181
const MA_SOURCE_ID = 9505

test('service, baked-country and priority gates protect non-eligible railway rows', async () => {
  const path = writeRailwaysFixture('writer-gates.arrow', [
    { latitude: -5.82, longitude: 13.45, country: 'CD' },
    { latitude: -5.82, longitude: 13.45, country: 'CD', service: 2, passenger: 11, freight: 12 },
    { latitude: -5.82, longitude: 13.45, country: 'DZ', passenger: 21, freight: 22 },
    { latitude: -5.82, longitude: 13.45, country: 'CD', sourceId: 110, passenger: 31, freight: 32 },
  ], { includeTraffic: true })
  const originalFieldOrder = tableFromIPC(railwayBytes(path)).schema.fields.map(field => field.name)

  const result = await writeRailwayTraffic(path, () => ({
    passenger: 2,
    freight: 4,
    sourceId: CD_SOURCE_ID,
  }))
  assert.deepEqual(result, {
    rows: 4,
    matched: 1,
    updated: true,
    skippedService: 1,
    skippedForeign: 1,
    skippedPriority: 1,
    retracted: 0,
  })
  const table = tableFromIPC(railwayBytes(path))
  assert.deepEqual(table.schema.fields.map(field => field.name), originalFieldOrder)
  assert.deepEqual([...Array(4)].map((_, index) => table.getChild('source_id')!.get(index)), [CD_SOURCE_ID, 0, 0, 110])
  assert.deepEqual([...Array(4)].map((_, index) => table.getChild('trains_passenger')!.get(index)), [2, 11, 21, 31])
  assert.equal(table.schema.metadata.get('railways_contract'), 'country_baked_v1')
  assert.equal(table.schema.metadata.get('qm_batch_bboxes'), '[[0,0,1,1]]')
})

test('Moroccan national ownership admits EH and rejects an unrelated baked country', async () => {
  const path = writeRailwaysFixture('morocco-territory.arrow', [
    { latitude: 24, longitude: -13, country: 'EH' },
    { latitude: 24, longitude: -13, country: 'DZ' },
  ])
  const result = await writeRailwayTraffic(path, () => ({ passenger: 1, freight: 2, sourceId: MA_SOURCE_ID }))
  assert.deepEqual({ matched: result.matched, skippedForeign: result.skippedForeign }, { matched: 1, skippedForeign: 1 })
  const table = tableFromIPC(railwayBytes(path))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [MA_SOURCE_ID, 0])
  assert.equal(table.getChild('country_iso')!.get(0), iso2Code('EH'))
})

test('invalid counts, source ids and source schemas fail before replacing the Arrow file', async () => {
  for (const [name, match, error] of [
    ['fractional', { passenger: 1.5, freight: 2, sourceId: CD_SOURCE_ID }, /invalid match/],
    ['negative', { passenger: 1, freight: -1, sourceId: CD_SOURCE_ID }, /invalid match/],
    ['zero', { passenger: 0, freight: 0, sourceId: CD_SOURCE_ID }, /all-zero traffic/],
    ['unknown-source', { passenger: 1, freight: 1, sourceId: 65_000 }, /registered railways source/],
    ['road-source', { passenger: 1, freight: 1, sourceId: 20 }, /registered railways source/],
  ] as const) {
    const path = writeRailwaysFixture(`invalid-${name}.arrow`, [{ latitude: 0, longitude: 20, country: 'CD' }])
    const before = railwayBytes(path)
    await assert.rejects(writeRailwayTraffic(path, () => match), error)
    assert.deepEqual(railwayBytes(path), before)
  }

  for (const [name, options, error] of [
    ['rail-type', { omitRailType: true }, /rail_type/],
    ['contract', { omitContract: true }, /railways Arrow contract/],
    ['country', { omitCountry: true }, /country_iso/],
  ] as const) {
    const path = writeRailwaysFixture(`invalid-${name}.arrow`, [{ latitude: 0, longitude: 20 }], options)
    const before = railwayBytes(path)
    await assert.rejects(writeRailwayTraffic(path, () => ({ passenger: 1, freight: 1, sourceId: CD_SOURCE_ID })), error)
    assert.deepEqual(railwayBytes(path), before)
  }
})

test('missing railway source and an exact rerun never create or replace bytes', async () => {
  const missing = `${writeRailwaysFixture('existing-neighbor.arrow', [])}.missing`
  await assert.rejects(
    writeRailwayTraffic(missing, () => ({ passenger: 1, freight: 1, sourceId: CD_SOURCE_ID })),
    /ENOENT/,
  )
  assert.equal(existsSync(missing), false)

  const path = writeRailwaysFixture('writer-idempotent.arrow', [{
    latitude: -5.82,
    longitude: 13.45,
    country: 'CD',
    sourceId: CD_SOURCE_ID,
    passenger: 2,
    freight: 4,
  }], { includeTraffic: true })
  const before = railwayBytes(path)
  const result = await writeRailwayTraffic(path, () => ({ passenger: 2, freight: 4, sourceId: CD_SOURCE_ID }))
  assert.deepEqual({ matched: result.matched, updated: result.updated }, { matched: 1, updated: false })
  assert.deepEqual(railwayBytes(path), before)
})

test('counts, provenance, divisor and retract change as one atomic payload', async () => {
  const path = writeRailwaysFixture('writer-retract.arrow', [
    {
      latitude: -5.82, longitude: 13.45, country: 'CD',
      sourceId: CD_SOURCE_ID, passenger: 9, freight: 4, divisor: 3,
    },
    {
      latitude: -5.83, longitude: 13.46, country: 'CD',
      sourceId: CD_SOURCE_ID, passenger: 7, freight: 2, divisor: 2,
    },
  ], { includeTraffic: true, includeDivisor: true })

  const result = await writeRailwayTraffic(
    path,
    (_row, index) => index === 0
      ? { passenger: 8, freight: 2, sourceId: CD_SOURCE_ID, divisor: 2 }
      : null,
    undefined,
    { retract: { sourceIds: [CD_SOURCE_ID], when: () => true } },
  )
  assert.deepEqual(
    { matched: result.matched, retracted: result.retracted, updated: result.updated },
    { matched: 1, retracted: 2, updated: true },
  )
  const table = tableFromIPC(railwayBytes(path))
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('trains_passenger')!.get(index)), [8, 0])
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('trains_freight')!.get(index)), [2, 0])
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('source_id')!.get(index)), [CD_SOURCE_ID, 0])
  assert.deepEqual([...Array(2)].map((_, index) => table.getChild('parallel_divisor')!.get(index)), [2, 1])
})

test('an omitted traffic divisor preserves the stored value and invalid explicit values write nothing', async () => {
  const path = writeRailwaysFixture('writer-divisor-idempotent.arrow', [{
    latitude: -5.82, longitude: 13.45, country: 'CD',
    sourceId: CD_SOURCE_ID, passenger: 8, freight: 2, divisor: 2,
  }], { includeTraffic: true, includeDivisor: true })
  const before = railwayBytes(path)
  const result = await writeRailwayTraffic(path, () => ({
    passenger: 8, freight: 2, sourceId: CD_SOURCE_ID, divisor: 2,
  }))
  assert.equal(result.updated, false)
  assert.deepEqual(railwayBytes(path), before)

  await assert.rejects(
    writeRailwayTraffic(path, () => ({
      passenger: 8, freight: 2, sourceId: CD_SOURCE_ID, divisor: 0,
    })),
    /invalid match/,
  )
  assert.deepEqual(railwayBytes(path), before)

  const preservedPath = writeRailwaysFixture('writer-divisor-preserved.arrow', [{
    latitude: -5.82, longitude: 13.45, country: 'CD',
    sourceId: CD_SOURCE_ID, passenger: 8, freight: 2, divisor: 3,
  }], { includeTraffic: true, includeDivisor: true })
  await writeRailwayTraffic(preservedPath, () => ({
    passenger: 7, freight: 1, sourceId: CD_SOURCE_ID,
  }))
  assert.equal(tableFromIPC(railwayBytes(preservedPath)).getChild('parallel_divisor')!.get(0), 3)
})

test('policy-free divisor writes only its column and exact reruns preserve bytes', async () => {
  const path = writeRailwaysFixture('divisor-only.arrow', [
    {
      latitude: -5.82, longitude: 13.45, country: 'CD', sourceId: CD_SOURCE_ID,
      passenger: 8, freight: 2, divisor: 1,
    },
    {
      latitude: -5.83, longitude: 13.46, country: 'CD', sourceId: CD_SOURCE_ID,
      passenger: 5, freight: 3, divisor: 3,
    },
  ], { includeTraffic: true, includeDivisor: true })
  const before = tableFromIPC(railwayBytes(path))
  const fieldOrder = before.schema.fields.map(field => field.name)
  const result = await writeRailParallelDivisor(path, index => index === 0 ? 2 : null)
  assert.deepEqual(result, { rows: 2, changedRows: 1, updated: true })

  const after = tableFromIPC(railwayBytes(path))
  assert.deepEqual(after.schema.fields.map(field => field.name), fieldOrder)
  assert.deepEqual([...Array(2)].map((_, index) => after.getChild('parallel_divisor')!.get(index)), [2, 3])
  assert.deepEqual([...Array(2)].map((_, index) => after.getChild('trains_passenger')!.get(index)), [8, 5])
  assert.deepEqual([...Array(2)].map((_, index) => after.getChild('trains_freight')!.get(index)), [2, 3])
  assert.deepEqual([...Array(2)].map((_, index) => after.getChild('source_id')!.get(index)), [CD_SOURCE_ID, CD_SOURCE_ID])
  assert.equal(after.schema.metadata.get('railways_contract'), 'country_baked_v1')
  assert.equal(after.schema.metadata.get('qm_batch_bboxes'), '[[0,0,1,1]]')

  const exact = railwayBytes(path)
  assert.deepEqual(await writeRailParallelDivisor(path, index => index === 0 ? 2 : null), {
    rows: 2, changedRows: 0, updated: false,
  })
  assert.deepEqual(railwayBytes(path), exact)
})

test('divisor-only invalid and all-one decisions never create or replace bytes', async () => {
  const path = writeRailwaysFixture('divisor-only-invalid.arrow', [
    { latitude: -5.82, longitude: 13.45, country: 'CD' },
  ])
  const before = railwayBytes(path)
  assert.deepEqual(await writeRailParallelDivisor(path, () => 1), {
    rows: 1, changedRows: 0, updated: false,
  })
  assert.deepEqual(railwayBytes(path), before)
  assert.equal(tableFromIPC(railwayBytes(path)).getChild('parallel_divisor'), null)

  for (const invalid of [-1, 1.5, 256]) {
    await assert.rejects(writeRailParallelDivisor(path, () => invalid), /invalid divisor/)
    assert.deepEqual(railwayBytes(path), before)
  }
  const missing = `${path}.missing`
  await assert.rejects(writeRailParallelDivisor(missing, () => 2), /ENOENT/)
  assert.equal(existsSync(missing), false)
})

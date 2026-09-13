/** Safety and sidecar contracts for the z9 railway traffic writer. */

import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { test } from 'node:test'
import { listRailIntervals } from './rail-traffic-store.js'
import { RAIL_TEST_DIRECTORY, railwayBytes, writePreparedRailwaySquare, writeRailwaysFixture } from './rail-test-fixture.js'
import { writeSyntheticRailTopology } from './transport-test-fixture.js'
import { writeRailwayTraffic } from './railways-arrow.js'

const CD_SOURCE_ID = 9181
const MA_SOURCE_ID = 9505
const SQUARE = 'z9/276/173'

function preparedPath(name: string) {
  return join(RAIL_TEST_DIRECTORY, name)
}

test('service and baked-country gates skip rows; zero with unknown is omitted', async () => {
  const prepared = preparedPath('writer-gates')
  const path = writePreparedRailwaySquare(prepared, SQUARE, 'writer-gates.arrow', [
    { latitude: -5.82, longitude: 13.45, country: 'CD' },
    { latitude: -5.82, longitude: 13.45, country: 'CD', service: 2, passenger: 11, freight: 12 },
    { latitude: -5.82, longitude: 13.45, country: 'DZ', passenger: 21, freight: 22 },
  ], { includeTraffic: true })
  writeSyntheticRailTopology(prepared, [SQUARE])
  const before = railwayBytes(path)

  const result = await writeRailwayTraffic(path, (row) => {
    if (row.existingPassenger === 11) return { passenger: 0, freight: 0, sourceId: CD_SOURCE_ID }
    return { passenger: 2.5, freight: 0, sourceId: CD_SOURCE_ID, passengerStatus: 'estimated' }
  }, undefined, { allowedCountryIsos: ['CD'], countryIso: 'CD' })
  assert.equal(result.matched, 1)
  assert.equal(result.skippedService, 1)
  assert.equal(result.skippedForeign, 1)
  assert.deepEqual(railwayBytes(path), before)
  const rows = listRailIntervals(prepared, SQUARE)
  assert.equal(rows.length, 1)
  assert.equal(rows[0].passenger, 2.5)
  assert.equal(rows[0].freightStatus, 0)
})

test('Moroccan national ownership admits EH and rejects an unrelated baked country', async () => {
  const prepared = preparedPath('morocco')
  const path = writePreparedRailwaySquare(prepared, SQUARE, 'morocco.arrow', [
    { latitude: 24, longitude: -13, country: 'EH' },
    { latitude: 24, longitude: -13, country: 'DZ' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE])
  const result = await writeRailwayTraffic(path, () => ({ passenger: 1, freight: 2, sourceId: MA_SOURCE_ID }), undefined, {
    allowedCountryIsos: ['MA', 'EH'], countryIso: 'MA',
  })
  assert.deepEqual({ matched: result.matched, skippedForeign: result.skippedForeign }, { matched: 1, skippedForeign: 1 })
  assert.equal(listRailIntervals(prepared, SQUARE).length, 1)
})

test('invalid counts and source ids fail before replacing the sidecar', async () => {
  for (const [name, match, error] of [
    ['negative', { passenger: 1, freight: -1, sourceId: CD_SOURCE_ID }, /invalid match/],
    ['unknown-source', { passenger: 1, freight: 1, sourceId: 65_000 }, /registered railways source/],
    ['road-source', { passenger: 1, freight: 1, sourceId: 20 }, /registered railways source/],
  ] as const) {
    const prepared = preparedPath(`invalid-${name}`)
    const path = writePreparedRailwaySquare(prepared, SQUARE, `invalid-${name}.arrow`, [{ latitude: 0, longitude: 20, country: 'CD' }])
    writeSyntheticRailTopology(prepared, [SQUARE])
    await assert.rejects(writeRailwayTraffic(path, () => match, undefined, { countryIso: 'CD' }), error)
    assert.equal(listRailIntervals(prepared).length, 0)
  }
})

test('missing railway source never creates a sidecar row', async () => {
  const missing = `${writeRailwaysFixture('existing-neighbor.arrow', [])}.missing`
  await assert.rejects(
    writeRailwayTraffic(missing, () => ({ passenger: 1, freight: 1, sourceId: CD_SOURCE_ID }), undefined, { countryIso: 'CD' }),
    /ENOENT|not z9/,
  )
  assert.equal(existsSync(missing), false)
})

test('estimated zero freight is stored when status is explicit', async () => {
  const prepared = preparedPath('zero-ok')
  const path = writePreparedRailwaySquare(prepared, SQUARE, 'zero-ok.arrow', [
    { latitude: -5.82, longitude: 13.45, country: 'CD' },
  ])
  writeSyntheticRailTopology(prepared, [SQUARE])
  await writeRailwayTraffic(path, () => ({
    passenger: 0, freight: 0, sourceId: CD_SOURCE_ID,
    passengerStatus: 'estimated', freightStatus: 'estimated',
  }), undefined, { countryIso: 'CD' })
  const rows = listRailIntervals(prepared, SQUARE)
  assert.equal(rows.length, 1)
  assert.equal(rows[0].passenger, 0)
  assert.equal(rows[0].passengerStatus, 2)
})

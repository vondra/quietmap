/** Measured-zero and derived speed-taper contracts for the road writer. */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { tableFromIPC } from 'apache-arrow'
import { bytes, writeRoadsFixture } from './road-test-fixture.js'
import { writeRoadAadt } from './roads-arrow.js'

const MEASURED_ID = 10
const TAPER_ID = 9862

test('measured all-zero AADT aborts while a baseline speed-only write is legal', async () => {
  const measured = writeRoadsFixture('zero-measured.arrow', [2])
  const before = bytes(measured)
  await assert.rejects(
    writeRoadAadt(measured, () => ({ light: 0, medium: 0, heavy: 0, moto: 0, sourceId: MEASURED_ID })),
    /all-zero AADT from measured source/,
  )
  assert.deepEqual(bytes(measured), before)

  const baseline = writeRoadsFixture('zero-taper.arrow', [4], { speeds: [0] })
  await writeRoadAadt(baseline, () =>
    ({ light: 0, medium: 0, heavy: 0, moto: 0, sourceId: TAPER_ID, speedTaper: 61 }))
  const table = tableFromIPC(bytes(baseline))
  assert.equal(table.getChild('source_id')!.get(0), TAPER_ID)
  assert.equal(table.getChild('speed_taper')!.get(0), 61)
})

test('speed taper gets its own column and never modifies the OSM speed tag', async () => {
  const path = writeRoadsFixture('taper-write.arrow', [4, 4], { speeds: [0, 90] })
  await writeRoadAadt(path, (_row, index) => index === 0
    ? { light: 500, medium: 10, heavy: 20, moto: 5, sourceId: TAPER_ID, speedTaper: 73 }
    : null)
  const table = tableFromIPC(bytes(path))
  assert.deepEqual([table.getChild('speed_taper')!.get(0), table.getChild('speed_taper')!.get(1)], [73, 0])
  assert.deepEqual([table.getChild('speed_limit')!.get(0), table.getChild('speed_limit')!.get(1)], [0, 90])

  const plain = writeRoadsFixture('taper-no-column.arrow', [2])
  await writeRoadAadt(plain, () => ({ light: 700, medium: 1, heavy: 2, moto: 3, sourceId: MEASURED_ID }))
  assert.equal(tableFromIPC(bytes(plain)).getChild('speed_taper'), null)

  for (const speedTaper of [0, 255, 70.5]) {
    await assert.rejects(
      writeRoadAadt(path, () => ({ light: 1, medium: 0, heavy: 0, moto: 0, sourceId: TAPER_ID, speedTaper })),
      /invalid speedTaper/,
    )
  }
})

test('overwrite and retract clear a taper unless the accepted write restates it', async () => {
  const path = writeRoadsFixture('taper-clear.arrow', [4, 4], { speeds: [0, 0] })
  await writeRoadAadt(path, (_row, index) => index === 0
    ? { light: 500, medium: 10, heavy: 20, moto: 5, sourceId: TAPER_ID, speedTaper: 73 }
    : { light: 0, medium: 0, heavy: 0, moto: 0, sourceId: TAPER_ID, speedTaper: 61 })
  await writeRoadAadt(
    path,
    (_row, index) => index === 0
      ? { light: 450, medium: 9, heavy: 18, moto: 4, sourceId: TAPER_ID, speedTaper: 68 }
      : null,
    undefined, undefined, { sourceIds: [TAPER_ID], when: (_row, index) => index !== 0 },
  )
  let table = tableFromIPC(bytes(path))
  assert.deepEqual([table.getChild('speed_taper')!.get(0), table.getChild('speed_taper')!.get(1)], [68, 0])
  assert.equal(table.getChild('source_id')!.get(1), 0)

  await writeRoadAadt(path, (_row, index) => index === 0
    ? { light: 9000, medium: 200, heavy: 300, moto: 20, sourceId: MEASURED_ID }
    : null)
  table = tableFromIPC(bytes(path))
  assert.equal(table.getChild('source_id')!.get(0), MEASURED_ID)
  assert.equal(table.getChild('speed_taper')!.get(0), 0)
})

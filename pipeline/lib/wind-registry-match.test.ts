/**
 * Guard rails for the shared wind-registry matcher: a registry value may only
 * fill a missing spec, and the searched cell block must follow from the radius
 * rather than a fixed 3x3 ring.
 *
 * Run: `cd pipeline && npx tsx --test lib/wind-registry-match.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { haversineM } from './spatial.js'
import { buildRegistryGrid, findNearestRegistryRecord, fillMissingTurbineSpecs } from './wind-registry-match.js'

test('a registry zero never erases a spec the row already carries', () => {
  // Swedish node 10695841749 lost its measured 45 kW to a register record with
  // no power, and the engine then used its 2000 kW default (+7 dB).
  const hub = new Float32Array([80, 0])
  const power = new Float32Array([0, 45])

  assert.equal(fillMissingTurbineSpecs(hub, power, 0, 0, 1500), true)
  assert.deepEqual([hub[0], power[0]], [80, 1500])

  assert.equal(fillMissingTurbineSpecs(hub, power, 1, 90, 0), true)
  assert.deepEqual([hub[1], power[1]], [90, 45])
})

test('a match that fills nothing reports no change, so its hex is not rewritten', () => {
  const hub = new Float32Array([80])
  const power = new Float32Array([45])
  assert.equal(fillMissingTurbineSpecs(hub, power, 0, 100, 2000), false)
  assert.deepEqual([hub[0], power[0]], [80, 45])
})

test('NaN counts as missing — these Arrow columns carry no null bitmap', () => {
  const hub = new Float32Array([NaN])
  const power = new Float32Array([NaN])
  assert.equal(fillMissingTurbineSpecs(hub, power, 0, 90, 2300), true)
  assert.deepEqual([hub[0], power[0]], [90, 2300])
})

test('a record two grid cells east is found inside a 500 m radius at 70 deg north', () => {
  // 0.01 deg of longitude is 380 m at 70 deg, so a fixed 3x3 ring could not
  // reach this pair; the cells are 1000 and 1002.
  const grid = buildRegistryGrid([{ lat: 70, lon: 10.021 }])
  assert.equal(Math.round(haversineM(70, 10.009, 70, 10.021)), 456)
  assert.deepEqual(findNearestRegistryRecord(grid, 70, 10.009, 500), { lat: 70, lon: 10.021 })
})

test('the nearest record wins and anything beyond the radius is null', () => {
  const near = { lat: 55.001, lon: 12 }
  const far = { lat: 55.0015, lon: 12 }
  const grid = buildRegistryGrid([far, near])
  assert.equal(findNearestRegistryRecord(grid, 55, 12, 200), near)
  assert.equal(findNearestRegistryRecord(grid, 55, 12, 50), null)
})

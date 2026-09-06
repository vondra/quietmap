/** z9 row-key, boundary, retract and idempotence tests for the graph-walk adapter. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { writeRailwaysFixture } from './rail-test-fixture.js'
import { enrichZ9RailwaysByGraphWalk } from './rail-walk-enrich.js'

const TEMP = mkdtempSync(join(tmpdir(), 'rail-walk-z9-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))

function squareDirectory(prepared: string, latitude: number, longitude: number): string {
  const x = Math.floor((longitude + 180) / 360 * 512)
  const radians = latitude * Math.PI / 180
  const y = Math.floor((1 - Math.asinh(Math.tan(radians)) / Math.PI) / 2 * 512)
  return join(prepared, 'z9', String(x), String(y))
}

function values(path: string, column: string): unknown[] {
  const table = tableFromIPC(readFileSync(path))
  return [...Array(table.numRows)].map((_, index) => table.getChild(column)!.get(index))
}

test('one pair walks across a z9 boundary, reruns byte-identically and retracts atomically', async () => {
  const prepared = join(TEMP, 'prepared')
  const latitude = 50
  const boundary = 14.0625
  const westDirectory = squareDirectory(prepared, latitude, boundary - 0.001)
  const eastDirectory = squareDirectory(prepared, latitude, boundary + 0.001)
  mkdirSync(westDirectory, { recursive: true })
  mkdirSync(eastDirectory, { recursive: true })

  const west = join(westDirectory, 'railways.arrow')
  const east = join(eastDirectory, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('walk-west.arrow', [{
    latitude,
    longitude: boundary - 0.001,
    endLatitude: latitude,
    endLongitude: boundary,
    lengthMetres: 72,
    country: 'DE',
  }]), west)
  copyFileSync(writeRailwaysFixture('walk-east.arrow', [{
    latitude,
    longitude: boundary,
    endLatitude: latitude,
    endLongitude: boundary + 0.001,
    lengthMetres: 72,
    country: 'DE',
  }]), east)

  const options = {
    preparedDirectory: prepared,
    bbox: [49.99, 14.05, 50.01, 14.08] as const,
    pairs: [{
      fromLat: latitude,
      fromLon: boundary - 0.001,
      toLat: latitude,
      toLon: boundary + 0.001,
      pax: 8,
      frt: 0,
    }],
    sourceId: 100,
    countryIso: 'DE',
  }
  const first = await enrichZ9RailwaysByGraphWalk(options)
  assert.deepEqual(
    { squares: first.squares, walked: first.pairsWalked, stamped: first.walkStamped },
    { squares: 2, walked: 1, stamped: 2 },
  )
  assert.deepEqual(values(west, 'trains_passenger'), [8])
  assert.deepEqual(values(east, 'trains_passenger'), [8])
  assert.deepEqual(values(west, 'source_id'), [100])

  const beforeWest = readFileSync(west)
  const beforeEast = readFileSync(east)
  await enrichZ9RailwaysByGraphWalk(options)
  assert.deepEqual(readFileSync(west), beforeWest)
  assert.deepEqual(readFileSync(east), beforeEast)

  const retract = await enrichZ9RailwaysByGraphWalk({ ...options, pairs: [] })
  assert.equal(retract.retracted, 2)
  assert.deepEqual(values(west, 'trains_passenger'), [0])
  assert.deepEqual(values(east, 'source_id'), [0])
})

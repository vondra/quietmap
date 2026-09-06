/** Failed graph evidence protects existing own counts while foreign national ownership remains independent. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { writeRailwaysFixture } from './rail-test-fixture.js'
import { enrichZ9RailwaysByGraphWalk } from './rail-walk-enrich.js'
import { SOURCE_ID_DK_NATIONAL_RAILWAY } from './source-ids.generated.js'

const TEMP = mkdtempSync(join(tmpdir(), 'rail-quarantine-'))
after(() => rmSync(TEMP, { recursive: true, force: true }))

test('disconnected station pairs cannot retract existing own counts around either endpoint', async () => {
  const square = join(TEMP, 'z9', '273', '160')
  mkdirSync(square, { recursive: true })
  const path = join(square, 'railways.arrow')
  copyFileSync(writeRailwaysFixture('quarantined-own.arrow', [
    { latitude: 55.67, longitude: 12.57, endLatitude: 55.671, endLongitude: 12.57, country: 'DK', sourceId: SOURCE_ID_DK_NATIONAL_RAILWAY, passenger: 70, divisor: 2 },
    { latitude: 55.72, longitude: 12.62, endLatitude: 55.721, endLongitude: 12.62, country: 'DK', sourceId: SOURCE_ID_DK_NATIONAL_RAILWAY, passenger: 60, divisor: 3 },
  ], { includeTraffic: true, includeDivisor: true }), path)
  const before = readFileSync(path)
  const result = await enrichZ9RailwaysByGraphWalk({
    preparedDirectory: TEMP,
    bbox: [55, 12, 56, 13],
    countryIso: 'DK', sourceId: SOURCE_ID_DK_NATIONAL_RAILWAY,
    pairs: [{ fromLat: 55.67, fromLon: 12.57, toLat: 55.721, toLon: 12.62, pax: 8, frt: 0 }],
  })
  assert.equal(result.failures.disconnected, 1)
  assert.equal(result.failedPairs[0].reason, 'disconnected')
  assert.equal(result.retracted, 0)
  assert.deepEqual([...tableFromIPC(readFileSync(path)).getChild('trains_passenger')!], [70, 60])
  assert.deepEqual(readFileSync(path), before)
})

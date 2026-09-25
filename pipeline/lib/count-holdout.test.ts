/** Holdout rule v1 squares and the single writer's switch that withholds measured counts inside them. */

import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { tableFromIPC } from 'apache-arrow'
import { EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT, isCountHoldoutSquare, withholdsCountPoint, withholdsCountLine, withholdsCountGeometry } from './count-holdout.js'
import { trainingCountLines } from './pinned-road-lines.js'
import { writeRoadsFixture } from './road-test-fixture.js'
import { writeRoadAadt } from './roads-arrow.js'
import { SOURCE_ID_EU_CITY_TRAFFIC, SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './sources.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'count-holdout-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))

test('holdout rule v1 matches the worked examples of the brief', () => {
  assert.equal(isCountHoldoutSquare(259, 191), true) // Barcelona
  for (const [x, y] of [[276, 173], [247, 165], [259, 176], [250, 193], [268, 179], [252, 165]]) {
    assert.equal(isCountHoldoutSquare(x, y), false, `${x}/${y}`)
  }
})

test('under the switch a holdout square takes no measured count, but still takes derived flow', async () => {
  const flow = (sourceId: number) => ({ sourceId, countBasis: 'both-directions' as const, observationId: 'o',
    light: 900, medium: 10, heavy: 80, moto: 10 })
  for (const [square, withheld] of [['259/191', true], ['276/173', false]] as const) {
    const path = join(DIRECTORY, 'z9', square, 'roads.arrow')
    mkdirSync(join(path, '..'), { recursive: true })
    copyFileSync(writeRoadsFixture(`holdout-${withheld}.arrow`, [2, 2]), path)
    process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT] = '1'
    try {
      await writeRoadAadt(path, (_row, index) => flow(index === 0 ? SOURCE_ID_EU_CITY_TRAFFIC : SOURCE_ID_ROAD_CONTINUITY_HEURISTIC))
    } finally {
      delete process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT]
    }
    assert.deepEqual([...tableFromIPC(readFileSync(path)).getChild('source_id')!],
      [withheld ? 0 : SOURCE_ID_EU_CITY_TRAFFIC, SOURCE_ID_ROAD_CONTINUITY_HEURISTIC], square)
  }
})

test('source locations reserve edge interiors, multipart identities and dateline-equivalent points', () => {
  const a = [-7.3828125, 57.891497352710324] as const // training 245/154
  const b = [-5.2734375, 57.891497352710324] as const // training 248/154
  const reserved = [-6.6796875, 57.891497352710324] as const // holdout 246/154
  assert.equal(withholdsCountLine([a, b]), false, 'normal enrichment keeps every observation')
  process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT] = '1'
  try {
    assert.equal(withholdsCountPoint(a[1], a[0]), false)
    assert.equal(withholdsCountPoint(reserved[1], reserved[0]), true)
    assert.equal(withholdsCountLine([a, b]), true, 'the endpoints alone miss two reserved squares')
    assert.equal(withholdsCountGeometry([[a, a], [b, b]]), false, 'disconnected parts have no invented connector')
    assert.equal(withholdsCountPoint(0, 180), withholdsCountPoint(0, -180))
    const part = (observationId: string, coordinates: readonly (readonly [number, number])[]) =>
      ({ observationId, coordinates, relativePath: 'counts.geojson', properties: {} })
    const parts = [part('same', [a, a]), part('same', [reserved, reserved]), part('other', [b, b])]
    assert.deepEqual(trainingCountLines(parts).map(p => p.observationId), ['other'], 'a count cannot escape through its other part')
    assert.throws(() => withholdsCountPoint(NaN, 0), /invalid count location/)
  } finally {
    delete process.env[EXCLUDE_HOLDOUT_COUNTS_ENVIRONMENT]
  }
})

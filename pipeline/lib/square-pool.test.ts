import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { policySourceId } from '../enrich-roads-national-policy.js'
import { NATIONAL_ROAD_POLICIES } from './road-national-policies/index.js'
import { iso2Code } from './prepared-grid.js'
import { writeRoadsFixture } from './road-test-fixture.js'
import { parseShard, shardSquares, workerCount } from './square-pool.js'

test('shards partition squares without overlap and reject bad shapes', () => {
  const squares = ['z9/0/1', 'z9/0/2', 'z9/1/0', 'z9/1/1', 'z9/2/2']
  const parts = [0, 1, 2].map(index => shardSquares(squares, parseShard(`${index}/3`)))
  assert.deepEqual(parts, [['z9/0/1', 'z9/1/1'], ['z9/0/2', 'z9/2/2'], ['z9/1/0']])
  assert.deepEqual(shardSquares(squares, parseShard(undefined)), squares)
  assert.throws(() => parseShard('3/3'))
  assert.throws(() => parseShard('a/b'))
  assert.equal(workerCount({ QM_ROAD_WORKERS: '1' }), 1)
  assert.ok(workerCount({ QM_ROAD_WORKERS: '3' }) >= 1)
  assert.ok(workerCount({ QM_ROAD_WORKERS: '3' }) <= 3)
  assert.equal(workerCount({ QM_ROAD_WORKERS: '3' }, Number.MAX_SAFE_INTEGER), 1)
  assert.ok(workerCount({}) >= 1)
  assert.throws(() => workerCount({ QM_ROAD_WORKERS: '0' }))
})

test('a fanned-out road loader writes the serial bytes and prints the serial receipt', () => {
  const directory = mkdtempSync(join(tmpdir(), 'road-square-shards-'))
  after(() => rmSync(directory, { recursive: true, force: true }))
  const sourceId = policySourceId(NATIONAL_ROAD_POLICIES.get('CD')!)
  const squares = [['z9/276/261', 14.6], ['z9/277/261', 15.322], ['z9/278/261', 15.9]] as const
  const run = (scope: string, workers: string) => {
    for (const [square, longitude] of squares) {
      mkdirSync(join(directory, scope, square), { recursive: true })
      copyFileSync(writeRoadsFixture(`shards-${scope}-${longitude}.arrow`, [1, 7, 1], {
        origin: [longitude, -4.325], sourceIds: [0, sourceId, sourceId],
        countryCodes: [iso2Code('CD'), iso2Code('CD'), iso2Code('CG')],
      }), join(directory, scope, square, 'roads.arrow'))
    }
    const receipt = execFileSync(process.execPath, ['--import', 'tsx',
      resolve(import.meta.dirname, '../enrich-roads-national-policy.ts'), '--country', 'CD',
      '--prepared-dir', join(directory, scope)], { env: { ...process.env, QM_ROAD_WORKERS: workers }, encoding: 'utf8' })
    return { receipt: JSON.parse(receipt), bytes: squares.map(([square]) => readFileSync(join(directory, scope, square, 'roads.arrow'))) }
  }
  const serial = run('serial', '1')
  const sharded = run('sharded', '2')
  assert.deepEqual(serial.receipt, { country: 'CD', sourceId, rows: 9, matched: 3, retracted: 6,
    skipped: 3, skippedForeign: 3, squares: 3, squaresUpdated: 3 })
  assert.deepEqual(sharded.receipt, serial.receipt)
  assert.deepEqual(sharded.bytes, serial.bytes)
})

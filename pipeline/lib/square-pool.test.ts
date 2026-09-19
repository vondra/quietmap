import assert from 'node:assert/strict'
import { after, test } from 'node:test'
import { spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { policySourceId } from '../enrich-roads-national-policy.js'
import { NATIONAL_ROAD_POLICIES } from './road-national-policies/index.js'
import { iso2Code } from './prepared-grid.js'
import { writeRoadsFixture } from './road-test-fixture.js'
import { parseShard, shardSquares, workerCount } from './square-pool.js'

test('a square has one owner whatever list it appears in, and bad shard shapes are rejected', () => {
  const squares = ['z9/0/1', 'z9/0/2', 'z9/1/0', 'z9/1/1', 'z9/2/2']
  const parts = [0, 1, 2].map(index => shardSquares(squares, parseShard(`${index}/3`)))
  assert.deepEqual(parts, [['z9/1/1', 'z9/2/2'], ['z9/0/1'], ['z9/0/2', 'z9/1/0']])
  assert.deepEqual([0, 1, 2].map(index => shardSquares(squares.slice(1), parseShard(`${index}/3`))),
    [['z9/1/1', 'z9/2/2'], [], ['z9/0/2', 'z9/1/0']])
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

test('a fanned-out road loader writes the serial bytes and counters, tolerates an empty shard, and fails without a receipt when a shard fails', () => {
  const directory = mkdtempSync(join(tmpdir(), 'road-square-shards-'))
  after(() => rmSync(directory, { recursive: true, force: true }))
  const sourceId = policySourceId(NATIONAL_ROAD_POLICIES.get('CD')!)
  // Shard 0 owns two squares, shard 1 none, shard 2 two.
  const squares = [['z9/276/261', 14.6, 3], ['z9/279/261', 16.5, 4],
    ['z9/277/261', 15.322, 5], ['z9/278/262', 15.9, 6]] as const
  assert.deepEqual([0, 1, 2].map(index => shardSquares(squares.map(([square]) => square), { index, count: 3 }).length), [2, 0, 2])
  const fixtures = squares.map(([, longitude, rows]) => writeRoadsFixture(`shards-${longitude}.arrow`, Array<number>(rows).fill(1), {
    origin: [longitude, -4.325], sourceIds: Array.from({ length: rows }, (_, row) => (row % 2 ? sourceId : 0)),
    countryCodes: Array.from({ length: rows }, (_, row) => iso2Code(row % 3 ? 'CD' : 'CG')),
  }))
  const run = (scope: string, workers: string, corruptSquare?: string) => {
    squares.forEach(([square], index) => {
      mkdirSync(join(directory, scope, square), { recursive: true })
      copyFileSync(fixtures[index], join(directory, scope, square, 'roads.arrow'))
    })
    if (corruptSquare) writeFileSync(join(directory, scope, corruptSquare, 'roads.arrow'), 'not arrow')
    const child = spawnSync(process.execPath, ['--import', 'tsx',
      resolve(import.meta.dirname, '../enrich-roads-national-policy.ts'), '--country', 'CD',
      '--prepared-dir', join(directory, scope)], { env: { ...process.env, QM_ROAD_WORKERS: workers }, encoding: 'utf8' })
    return { ...child, bytes: squares.map(([square]) => readFileSync(join(directory, scope, square, 'roads.arrow'))) }
  }
  const serial = run('serial', '1')
  const sharded = run('sharded', '3')
  const { shards: serialShards, ...serialCounters } = JSON.parse(serial.stdout)
  const { shards, ...shardedCounters } = JSON.parse(sharded.stdout)
  assert.deepEqual([serialShards, shards], [1, 3])
  assert.equal(serialCounters.squares, 4)
  assert.ok(serialCounters.matched > 0 && serialCounters.retracted > 0 && serialCounters.skippedForeign > 0)
  assert.deepEqual(shardedCounters, serialCounters)
  assert.deepEqual(sharded.bytes, serial.bytes)

  const failed = run('failed', '3', 'z9/278/262')
  assert.notEqual(failed.status, 0)
  assert.equal(failed.stdout, '')
  assert.match(failed.stderr, /shard 2\/3 exited 1/)
})

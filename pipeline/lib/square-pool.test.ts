import assert from 'node:assert/strict'
import { test } from 'node:test'
import { parseShard, shardSquares, workerCount } from './square-pool.js'

test('shards partition squares without overlap and reject bad shapes', () => {
  const squares = ['z9/0/1', 'z9/0/2', 'z9/1/0', 'z9/1/1', 'z9/2/2']
  const parts = [0, 1, 2].map(index => shardSquares(squares, parseShard(`${index}/3`)))
  assert.deepEqual(parts, [['z9/0/1', 'z9/1/1'], ['z9/0/2', 'z9/2/2'], ['z9/1/0']])
  assert.deepEqual(shardSquares(squares, parseShard(undefined)), squares)
  assert.throws(() => parseShard('3/3'))
  assert.throws(() => parseShard('a/b'))
  assert.equal(workerCount({}), 1)
  assert.equal(workerCount({ QM_ROAD_WORKERS: '16' }), 16)
  assert.throws(() => workerCount({ QM_ROAD_WORKERS: '0' }))
})

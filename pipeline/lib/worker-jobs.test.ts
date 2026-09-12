import assert from 'node:assert/strict'
import { test } from 'node:test'
import { fitJobs } from './worker-jobs.js'

test('memory caps requested workers and keeps a lower manual cap', () => {
  const worker = 4 * 2 ** 30
  assert.equal(fitJobs(20, worker, 60 * 2 ** 30), 15)
  assert.equal(fitJobs(8, worker, 60 * 2 ** 30), 8)
  assert.equal(fitJobs(20, worker, 3 * 2 ** 30), 1)
})

test('rejects a non-positive request', () => {
  assert.throws(() => fitJobs(0, 4 * 2 ** 30, 60 * 2 ** 30), /--jobs must be >= 1/)
})

//! Route-level tests for the containment-only building hover lookup.

import assert from 'node:assert/strict'
import test from 'node:test'
import Fastify from 'fastify'
import { noiseOnflyV2Routes } from './noise-onfly-v2.js'
import { DISABLED_PUBLISHED_LINE_MODEL } from '../published-line-model.js'

test('building-at validates, passes through null/object JSON, and maps worker errors to 500', async (t) => {
  let resultJson = 'null'
  let workerError: Error | null = null
  const calls: Array<[number, number]> = []
  const app = Fastify({ logger: false })
  await noiseOnflyV2Routes(app, DISABLED_PUBLISHED_LINE_MODEL, {
    queryBuildingAt: async (lat, lng) => {
      calls.push([lat, lng])
      if (workerError) throw workerError
      return resultJson
    },
  })
  t.after(async () => app.close())

  const noBuilding = await app.inject('/api/building-at?lat=49.7910&lng=14.1963')
  assert.equal(noBuilding.statusCode, 200)
  assert.match(noBuilding.headers['content-type'] ?? '', /^application\/json/)
  assert.equal(noBuilding.payload, 'null')
  assert.deepEqual(noBuilding.json(), null)

  resultJson = '{"height_m":3,"building_type":"house"}'
  const building = await app.inject('/api/building-at?lat=49.7910&lng=14.1963')
  assert.equal(building.statusCode, 200)
  assert.deepEqual(building.json(), { height_m: 3, building_type: 'house' })
  assert.deepEqual(calls, [[49.791, 14.1963], [49.791, 14.1963]])

  workerError = new Error('worker failed')
  const failure = await app.inject('/api/building-at?lat=49.7910&lng=14.1963')
  assert.equal(failure.statusCode, 500)
  assert.deepEqual(failure.json(), { error: 'worker failed' })
})

//! Route-level tests for the building hover lookup and popup parameter validation.

import assert from 'node:assert/strict'
import test from 'node:test'
import Fastify from 'fastify'
import { noiseOnflyV2Routes } from './noise-onfly-v2.js'

test('building-at validates, passes through null/object JSON, and maps worker errors to 500', async (t) => {
  let resultJson = 'null'
  let workerError: Error | null = null
  const calls: Array<[number, number]> = []
  const app = Fastify({ logger: false })
  await noiseOnflyV2Routes(app, {
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

test('popup rejects a receiver height outside the engine floor and the public ceiling', async (t) => {
  const app = Fastify({ logger: false })
  await noiseOnflyV2Routes(app)
  t.after(async () => app.close())
  for (const query of ['receiver_height_m=0.4', 'receiver_height_m=101', 'receiver_height_m=x',
    'receiver_height_m=1.2&detail=all']) {
    const response = await app.inject(`/api/noise-onfly-v2?lat=50.0755&lng=14.4378&${query}`)
    assert.equal(response.statusCode, 400, query)
    assert.match(response.json().error, /receiver_height_m/)
  }
})

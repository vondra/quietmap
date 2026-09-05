/** Building-height route registration and publication never restore acoustic heatmaps. */

import assert from 'node:assert/strict'
import test from 'node:test'
import Fastify from 'fastify'
import { buildApp } from '../app.js'
import { NoiseOnflySupervisor } from '../engine/noise-onfly-supervisor.js'
import { rasterTileRoutes } from './raster-tiles.js'

test('the real app registers building heights without acoustic raster routes', async (t) => {
  const query = t.mock.method(NoiseOnflySupervisor.prototype, 'queryObstacleFootprints',
    async function (this: NoiseOnflySupervisor, ...bounds: number[]) {
      assert.ok(this instanceof NoiseOnflySupervisor)
      assert.deepEqual(bounds, [49.653404588437894, 14.23828125, 49.66762782262193, 14.26025390625])
      return '[]'
    })
  const app = await buildApp({
    logger: false,
    enableClusterRoutes: false,
    importOpsModule: async () => null,
  })
  t.after(async () => app.close())
  // Invalid zoom exercises registration without starting a native data query.
  const building = await app.inject('/api/raster/building/9/0/0.png')
  assert.equal(building.statusCode, 400)
  assert.equal(building.body, 'Invalid zoom')
  const png = await app.inject('/api/raster/building/14/8840/5580.png')
  assert.equal(png.statusCode, 200)
  assert.equal(png.headers['content-type'], 'image/png')
  assert.equal(query.mock.callCount(), 1)
  const acoustic = await app.inject('/api/raster/roads/14/8840/5580.png')
  assert.equal(acoustic.statusCode, 404)
})

test('building PNG cache keeps only successful tiles and remains local to its provider', async (t) => {
  const app = Fastify()
  let calls = 0
  let failure = true
  await app.register(rasterTileRoutes, {
    queryObstacleFootprints: async () => {
      calls++
      if (failure) throw new Error('injected footprint failure')
      return '[]'
    },
  })
  t.after(async () => app.close())
  const url = '/api/raster/building/14/8840/5580.png'
  const failed = await app.inject(url)
  assert.equal(failed.statusCode, 500)
  assert.equal(failed.headers['cache-control'], undefined)
  failure = false
  const success = await app.inject(url)
  assert.equal(success.statusCode, 200)
  assert.equal(success.headers['content-type'], 'image/png')
  assert.equal(success.headers['cache-control'], 'public, max-age=3600')
  assert.deepEqual(success.rawPayload.subarray(0, 8), Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
  assert.deepEqual((await app.inject(url)).rawPayload, success.rawPayload)
  assert.equal(calls, 2)
  for (const suffix of ['NaN/0/0', '17/0/0', '14/-1/0', '14/16384/0', '14/0/16384', '14/0.5/0']) {
    assert.equal((await app.inject(`/api/raster/building/${suffix}.png`)).statusCode, 400)
  }
  assert.equal(calls, 2)

  const other = Fastify()
  await other.register(rasterTileRoutes, {
    queryObstacleFootprints: async () => { throw new Error('different provider') },
  })
  t.after(async () => other.close())
  assert.equal((await other.inject(url)).statusCode, 500)

  for (let x = 0; x < 500; x++) {
    assert.equal((await app.inject(`/api/raster/building/14/${x}/0.png`)).statusCode, 200)
  }
  assert.equal(calls, 502)
  await app.inject(url)
  assert.equal(calls, 503, 'the oldest tile is evicted after 500 newer keys')
})

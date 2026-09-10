import assert from 'node:assert/strict'
import test from 'node:test'
import Fastify from 'fastify'
import { noisePreviewRoutes } from './noise-preview.js'

test('missing or failed corners remain null, and a preview retains its explicit partial outdoor contract', async (t) => {
  const app = Fastify({ logger: false })
  let result = 'null'
  let fail = false
  const points: number[][] = []
  await noisePreviewRoutes(app, async (lat, lng) => {
    points.push([lat, lng])
    if (fail) throw new Error('stored generation differs')
    return result
  })
  t.after(() => app.close())
  const url = '/api/noise-preview?lat=50&lng=14'
  assert.equal((await app.inject(url)).json(), null)
  result = JSON.stringify({ status: 'provisional', receiver: 'outdoor', accuracy: 'unmeasured',
    center: [50, 14], layers: [{ layer: 'road', period_power: [100, 100, 100], lden_db: 26.4 }] })
  assert.deepEqual((await app.inject(url)).json(), JSON.parse(result))
  fail = true
  const fallback = await app.inject(url)
  assert.equal(fallback.statusCode, 200)
  assert.equal(fallback.json(), null)
  for (const query of ['lat=50x&lng=14', 'lat=&lng=14', 'lat=91&lng=14', 'lat=50&lng=Infinity']) {
    assert.equal((await app.inject(`/api/noise-preview?${query}`)).statusCode, 400)
  }
  assert.deepEqual(points, [[50, 14], [50, 14], [50, 14]])
})

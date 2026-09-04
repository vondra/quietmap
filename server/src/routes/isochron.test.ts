import assert from 'node:assert/strict'
import { createServer, type Server } from 'node:http'
import test from 'node:test'
import { isochronRoutes } from './isochron.js'
import { buildApp } from '../app.js'

const ready = async () => ({ ready: true as const, failed: [], errors: {} })

function valhallaStub(bodyHandler?: (body: string) => [number, unknown]) {
  return createServer((req, res) => {
    let body = ''
    req.on('data', (chunk) => { body += chunk })
    req.on('end', () => {
      const [status, payload] = bodyHandler
        ? bodyHandler(body)
        : [200, {
            features: [{
              properties: { contour: 15 },
              geometry: { type: 'Polygon', coordinates: [[[14.5, 49.9], [14.51, 49.9], [14.51, 49.91], [14.5, 49.9]]] },
            }],
          }]
      res.writeHead(status, { 'content-type': 'application/json' })
      res.end(JSON.stringify(payload))
    })
  })
}

async function withStub(t: test.TestContext, handler?: (body: string) => [number, unknown]) {
  const stub = valhallaStub(handler)
  await new Promise<void>((resolve) => stub.listen(0, '127.0.0.1', resolve))
  t.after(() => stub.close())
  process.env.VALHALLA_URL = `http://127.0.0.1:${(stub.address() as { port: number }).port}`
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => {
    app.close()
    delete process.env.VALHALLA_URL
  })
  return app
}

test('isochron returns the largest contour polygon with the request stamped in properties', async (t) => {
  const app = await withStub(t, (body) => {
    const parsed = JSON.parse(body)
    assert.equal(parsed.locations[0].lat, 49.9)
    assert.equal(parsed.locations[0].lon, 14.5)
    assert.equal(parsed.costing, 'pedestrian')
    return [200, {
      features: [{
        properties: { contour: 15 },
        geometry: { type: 'Polygon', coordinates: [[[14.5, 49.9], [14.51, 49.9], [14.51, 49.91], [14.5, 49.9]]] },
      }],
    }]
  })
  const res = await app.inject('/api/isochron?lat=49.9&lng=14.5&time=15&modes=walk')
  assert.equal(res.statusCode, 200)
  const feature = res.json()
  assert.equal(feature.geometry.type, 'Polygon')
  assert.deepEqual(feature.properties.modes, ['walk'])
  assert.equal(feature.properties.time, 15)
})

test('isochron 400s on missing and invalid input', async (t) => {
  const app = await withStub(t)
  assert.equal((await app.inject('/api/isochron?lat=49.9&lng=14.5&time=15')).statusCode, 400)
  assert.equal((await app.inject('/api/isochron?lat=abc&lng=14.5&time=15&modes=walk')).statusCode, 400)
  assert.equal((await app.inject('/api/isochron?lat=49.9&lng=14.5&time=15&modes=rocket')).statusCode, 400)
})

test('isochron 502s when the router backend fails', async (t) => {
  const app = await withStub(t, () => [500, { error: 'boom' }])
  const res = await app.inject('/api/isochron?lat=49.9&lng=14.5&time=15&modes=walk')
  assert.equal(res.statusCode, 502)
})

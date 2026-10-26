// The reachable area against a stub router: one polygon a mode, the car's first; only the point and
// the minutes sent, under our name; a bad request (a repeated or an empty field too) refused before
// the router is asked; a mode the router fails left out, and 502 when it fails them all.
import assert from 'node:assert/strict'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import test from 'node:test'
import Fastify from 'fastify'
import { isochronRoutes } from './isochron.ts'

async function router(t: test.TestContext, answer: (body: { costing: string }) => [number, object]) {
  const asked: { costing: string, locations: unknown, contours: unknown, client: unknown }[] = []
  const stub = createServer((request: IncomingMessage, response: ServerResponse) => {
    let text = ''
    request.on('data', chunk => { text += chunk })
    request.on('end', () => {
      const body = { ...JSON.parse(text), client: request.headers['x-client-id'] }
      asked.push(body)
      const [status, json] = answer(body)
      response.writeHead(status, { 'content-type': 'application/json' }).end(JSON.stringify(json))
    })
  })
  await new Promise<void>(resolve => stub.listen(0, '127.0.0.1', resolve))
  const app = Fastify()
  await app.register(isochronRoutes, { valhallaUrl: `http://127.0.0.1:${(stub.address() as AddressInfo).port}` })
  t.after(async () => {
    await app.close()
    stub.close()
  })
  return { app, asked }
}

const polygon = (costing: string) => ({ features: [{ geometry: { type: 'Polygon', coordinates: [[[0, 0], [1, 0], [0, 1], [0, 0]]] }, properties: { costing } }] })

test('the area of each mode asked, the car first, from the point and the minutes alone', async (t) => {
  const { app, asked } = await router(t, body => [200, polygon(body.costing)])
  const answer = await app.inject('/api/isochron?lat=50.1&lng=374.4&minutes=30&modes=walk,car')
  assert.equal(answer.statusCode, 200)
  assert.deepEqual(answer.json().features.map((feature: { properties: { mode: string } }) => feature.properties.mode), ['car', 'walk'])
  assert.deepEqual(asked.map(body => body.costing).sort(), ['auto', 'pedestrian'])
  assert.deepEqual(asked[0].locations, [{ lat: 50.1, lon: 14.399999999999977 }])
  assert.deepEqual(asked[0].contours, [{ time: 30 }])
  assert.equal(asked[0].client, 'quietmap.org')
})

test('a bad request is refused before the router is asked; a failing router answers 502', async (t) => {
  const { app, asked } = await router(t, () => [500, {}])
  for (const query of [
    'lat=91&lng=0&minutes=5&modes=walk', 'lat=0&lng=0&minutes=0&modes=walk', 'lat=0&lng=0&minutes=121&modes=car',
    'lat=0&lng=0&minutes=5&modes=bike', 'lat=&lng=&minutes=5&modes=walk', 'lat=0&lng=0&minutes=5&modes=walk&modes=car',
  ]) {
    assert.equal((await app.inject(`/api/isochron?${query}`)).statusCode, 400, query)
  }
  assert.equal(asked.length, 0)
  assert.equal((await app.inject('/api/isochron?lat=0&lng=0&minutes=5&modes=walk')).statusCode, 502)
})

test('a mode the router fails is left out, the other kept', async (t) => {
  const { app } = await router(t, body => body.costing === 'auto' ? [504, {}] : [200, polygon(body.costing)])
  const answer = await app.inject('/api/isochron?lat=50&lng=14&minutes=120&modes=walk,car')
  assert.equal(answer.statusCode, 200)
  assert.deepEqual(answer.json().features.map((feature: { properties: { mode: string } }) => feature.properties.mode), ['walk'])
})

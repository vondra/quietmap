import assert from 'node:assert/strict'
import test from 'node:test'
import Fastify from 'fastify'
import { healthRoutes } from './health.js'

test('liveness is independent from readiness and health aliases readiness', async (t) => {
  const app = Fastify()
  await healthRoutes(app, async () => ({
    ready: false,
    failed: ['engine'],
    errors: { engine: '/secret/path/libsource_reader.so failed to load' },
  }))
  t.after(async () => app.close())

  const live = await app.inject('/api/live')
  assert.equal(live.statusCode, 200)
  assert.deepEqual(live.json(), { status: 'ok' })

  for (const url of ['/api/ready', '/api/health']) {
    const response = await app.inject(url)
    assert.equal(response.statusCode, 503)
    assert.deepEqual(response.json(), { status: 'not_ready', failed: ['engine'] })
    assert.doesNotMatch(response.body, /secret|libsource_reader/)
    assert.equal(response.headers['cache-control'], 'no-store')
  }
})

test('ready response stays compatible with the legacy health body', async (t) => {
  const app = Fastify()
  await healthRoutes(app, async () => ({ ready: true, failed: [], errors: {} }))
  t.after(async () => app.close())

  assert.deepEqual((await app.inject('/api/ready')).json(), { status: 'ok' })
  assert.deepEqual((await app.inject('/api/health')).json(), { status: 'ok' })
})

test('unexpected checker failures are sanitized as not ready', async (t) => {
  const app = Fastify()
  await healthRoutes(app, async () => { throw new Error('/secret/runtime/path') })
  t.after(async () => app.close())

  const response = await app.inject('/api/ready')
  assert.equal(response.statusCode, 503)
  assert.deepEqual(response.json(), {
    status: 'not_ready',
    failed: ['engine', 'frontend', 'prepared-data'],
  })
  assert.doesNotMatch(response.body, /secret|runtime\/path/)
})

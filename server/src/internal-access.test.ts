import assert from 'node:assert/strict'
import test from 'node:test'
import Fastify from 'fastify'
import { requireLocalPeer } from './internal-access.js'

test('local-peer guard: allows a loopback socket (Caddy/TUI), blocks a direct remote peer', async (t) => {
  const app = Fastify({ trustProxy: ['127.0.0.1', '::1'] })
  t.after(async () => app.close())
  let handlerCalls = 0
  app.get('/a/thing', { onRequest: requireLocalPeer }, async () => { handlerCalls++; return { private: true } })

  // Caddy proxies from loopback and forwards the real client via X-Forwarded-For: the
  // SOCKET peer is loopback, so the guard ALLOWS it (basic_auth already ran at the edge).
  // A request.ip check would see the public XFF and wrongly reject this authed request.
  const viaCaddy = await app.inject({
    method: 'GET', url: '/a/thing', remoteAddress: '127.0.0.1',
    headers: { 'x-forwarded-for': '203.0.113.20' },
  })
  assert.equal(viaCaddy.statusCode, 200)
  assert.equal(handlerCalls, 1)

  // A DIRECT hit on the public port (no Caddy): the socket peer is the real remote → 404,
  // so the raw port can't bypass the edge auth.
  const direct = await app.inject({ method: 'GET', url: '/a/thing', remoteAddress: '203.0.113.20' })
  assert.equal(direct.statusCode, 404)
  assert.equal(handlerCalls, 1)
})

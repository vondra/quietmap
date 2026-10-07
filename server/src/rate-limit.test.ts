// Per-client rate limits: the expensive routes are throttled per IPv4 address or IPv6 /64, local
// callers are not, and tiles never are.
import assert from 'node:assert/strict'
import { tmpdir } from 'node:os'
import test from 'node:test'
import { buildApp, type AppConfig } from './app.ts'
import { rateLimitClientKey } from './rate-limit.ts'

// Every burst request takes a handler fast path (invalid parameters, a short query, a missing
// archive), so no popup starts and no geocoder call leaves: the assertions exercise only the
// limiter in front of the handlers.
const CONFIG: AppConfig = {
  popupBin: '/nonexistent/qm-popup',
  preparedDir: '/nonexistent',
  years: ['2026'],
  popupConcurrency: 1,
  tilesDir: tmpdir(),
  noIndex: false,
  photonUrl: 'http://127.0.0.1:9',
}

async function app(t: test.TestContext) {
  const built = await buildApp(CONFIG)
  t.after(() => built.close())
  return built
}

const BOGUS_POPUP = '/api/popup?lat=bogus&lon=bogus'

test('rate-limit bucket key: IPv4 full address, IPv6 collapsed to /64', () => {
  assert.equal(rateLimitClientKey('203.0.113.7'), '203.0.113.7')
  assert.equal(rateLimitClientKey('::ffff:203.0.113.7'), '203.0.113.7')
  assert.equal(
    rateLimitClientKey('2001:db8:cafe:1:aaaa:bbbb:cccc:dddd'),
    '2001:db8:cafe:1::/64',
  )
  assert.equal(rateLimitClientKey('2001:db8::1'), '2001:db8:0:0::/64')
  assert.equal(rateLimitClientKey('::1'), '0:0:0:0::/64')
  assert.equal(rateLimitClientKey('fe80::1%eth0'), 'fe80:0:0:0::/64')
})

test('the popup route returns 429 on the 6th request within a second', async (t) => {
  const server = await app(t)
  const statuses: number[] = []
  for (let i = 0; i < 6; i++) {
    const response = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '203.0.113.10' })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses, [400, 400, 400, 400, 400, 429])

  const limited = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '203.0.113.10' })
  assert.equal(limited.statusCode, 429)
  assert.match(limited.json().message, /rate limit/i)

  // A different IPv4 client has its own bucket and is not affected.
  const otherClient = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '203.0.113.11' })
  assert.equal(otherClient.statusCode, 400)
})

test('geocode proxies are rate-limited too', async (t) => {
  const server = await app(t)
  const statuses: number[] = []
  for (let i = 0; i < 6; i++) {
    // q shorter than 2 chars answers [] without calling the geocoder.
    const response = await server.inject({ method: 'GET', url: '/api/search?q=a', remoteAddress: '203.0.113.20' })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses, [200, 200, 200, 200, 200, 429])

  // /api/search and /api/reverse each carry their own per-route counter.
  const reverse = await server.inject({ method: 'GET', url: '/api/reverse?lat=999&lon=999', remoteAddress: '203.0.113.21' })
  assert.equal(reverse.statusCode, 200)
})

test('two clients inside one IPv6 /64 share a bucket; a different /64 does not', async (t) => {
  const server = await app(t)
  for (let i = 0; i < 5; i++) {
    const response = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '2001:db8:1:2::aaaa' })
    assert.equal(response.statusCode, 400)
  }
  // Different interface ID, same /64 prefix — the exhausted bucket applies.
  const sameSlash64 = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '2001:db8:1:2:9999::bbbb' })
  assert.equal(sameSlash64.statusCode, 429)
  // Neighbouring /64 is a separate client.
  const otherSlash64 = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '2001:db8:1:3::cccc' })
  assert.equal(otherSlash64.statusCode, 400)
})

test('local unproxied callers bypass the limit; proxied clients do not', async (t) => {
  const server = await app(t)
  // A benchmark hitting localhost directly must never see 429.
  for (let i = 0; i < 8; i++) {
    const response = await server.inject({ method: 'GET', url: BOGUS_POPUP, remoteAddress: '127.0.0.1' })
    assert.equal(response.statusCode, 400)
  }
  // The same loopback socket carrying X-Forwarded-For is the reverse proxy passing on a public
  // visitor — the forwarded client IP is the bucket and IS limited.
  const statuses: number[] = []
  for (let i = 0; i < 6; i++) {
    const response = await server.inject({
      method: 'GET',
      url: BOGUS_POPUP,
      remoteAddress: '127.0.0.1',
      headers: { 'x-forwarded-for': '203.0.113.40' },
    })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses, [400, 400, 400, 400, 400, 429])
})

test('tile routes are never rate-limited (the map bursts dozens per pan)', async (t) => {
  const server = await app(t)
  for (let i = 0; i < 12; i++) {
    const tile = await server.inject({ method: 'GET', url: '/api/tiles/b1/road/4/8/5.bin', remoteAddress: '203.0.113.30' })
    assert.notEqual(tile.statusCode, 429)
    const manifest = await server.inject({ method: 'GET', url: '/api/tiles-manifest', remoteAddress: '203.0.113.30' })
    assert.notEqual(manifest.statusCode, 429)
  }
})

test('a host marked noindex says so on every response', async (t) => {
  const server = await buildApp({ ...CONFIG, noIndex: true })
  t.after(() => server.close())
  const response = await server.inject('/api/search?q=a')
  assert.equal(response.headers['x-robots-tag'], 'noindex, nofollow, noarchive')
  const plain = await app(t)
  assert.equal((await plain.inject('/api/search?q=a')).headers['x-robots-tag'], undefined)
})

import assert from 'node:assert/strict'
import test from 'node:test'
import { buildApp } from './app.js'
import { rateLimitClientKey } from './rate-limit.js'

const ready = async () => ({ ready: true as const, failed: [], errors: {} })

// All burst requests use handler fast paths (invalid/short params) so no
// engine worker spawns and no external Photon call fires — the assertions
// exercise only the rate limiter in front of the handlers.

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

test('popup compute route returns 429 on the 6th request within a second', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())

  const statuses: number[] = []
  for (let i = 0; i < 6; i++) {
    const response = await app.inject({
      method: 'GET',
      url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
      remoteAddress: '203.0.113.10',
    })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses, [400, 400, 400, 400, 400, 429])

  const limited = await app.inject({
    method: 'GET',
    url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
    remoteAddress: '203.0.113.10',
  })
  assert.equal(limited.statusCode, 429)
  assert.match(limited.json().message, /rate limit/i)

  // A different IPv4 client has its own bucket and is not affected.
  const otherClient = await app.inject({
    method: 'GET',
    url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
    remoteAddress: '203.0.113.11',
  })
  assert.equal(otherClient.statusCode, 400)
})

test('building hover lookup allows normal bursts but remains rate-limited', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())

  const statuses: number[] = []
  for (let i = 0; i < 21; i++) {
    const response = await app.inject({
      method: 'GET',
      url: '/api/building-at?lat=bogus&lng=bogus',
      remoteAddress: '203.0.113.12',
    })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses.slice(0, 20), Array(20).fill(400))
  assert.equal(statuses[20], 429)
})

test('geocode proxies are rate-limited too', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())

  const statuses: number[] = []
  for (let i = 0; i < 6; i++) {
    // q shorter than 2 chars answers [] without calling Photon.
    const response = await app.inject({
      method: 'GET',
      url: '/api/search?q=a',
      remoteAddress: '203.0.113.20',
    })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses, [200, 200, 200, 200, 200, 429])

  // /api/search and /api/reverse each carry their own per-route counter.
  const reverse = await app.inject({
    method: 'GET',
    url: '/api/reverse?lat=999&lon=999',
    remoteAddress: '203.0.113.21',
  })
  assert.equal(reverse.statusCode, 200)
})

test('two clients inside one IPv6 /64 share a bucket; a different /64 does not', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())

  for (let i = 0; i < 5; i++) {
    const response = await app.inject({
      method: 'GET',
      url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
      remoteAddress: '2001:db8:1:2::aaaa',
    })
    assert.equal(response.statusCode, 400)
  }

  // Different interface ID, same /64 prefix — the exhausted bucket applies.
  const sameSlash64 = await app.inject({
    method: 'GET',
    url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
    remoteAddress: '2001:db8:1:2:9999::bbbb',
  })
  assert.equal(sameSlash64.statusCode, 429)

  // Neighbouring /64 is a separate client.
  const otherSlash64 = await app.inject({
    method: 'GET',
    url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
    remoteAddress: '2001:db8:1:3::cccc',
  })
  assert.equal(otherSlash64.statusCode, 400)
})

test('local unproxied callers bypass the limit; Caddy-forwarded clients do not', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())

  // check-popup fires 115 popup queries at 4-concurrent against localhost —
  // a loopback socket with no forwarding header must never see 429.
  for (let i = 0; i < 8; i++) {
    const response = await app.inject({
      method: 'GET',
      url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
      remoteAddress: '127.0.0.1',
    })
    assert.equal(response.statusCode, 400)
  }

  // The same loopback socket carrying X-Forwarded-For is Caddy proxying a
  // public visitor — the forwarded client IP is the bucket and IS limited.
  const statuses: number[] = []
  for (let i = 0; i < 6; i++) {
    const response = await app.inject({
      method: 'GET',
      url: '/api/noise-onfly-v2?lat=bogus&lng=bogus',
      remoteAddress: '127.0.0.1',
      headers: { 'x-forwarded-for': '203.0.113.40' },
    })
    statuses.push(response.statusCode)
  }
  assert.deepEqual(statuses, [400, 400, 400, 400, 400, 429])
})

test('tile routes are never rate-limited (the map bursts dozens per pan)', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())

  for (let i = 0; i < 12; i++) {
    const response = await app.inject({
      method: 'GET',
      url: '/api/tiles/b1/road/4/8/5.bin',
      remoteAddress: '203.0.113.30',
    })
    assert.notEqual(response.statusCode, 429)
  }

  const manifest = []
  for (let i = 0; i < 12; i++) {
    manifest.push(await app.inject({
      method: 'GET',
      url: '/api/tiles-manifest',
      remoteAddress: '203.0.113.30',
    }))
  }
  assert.ok(manifest.every((r) => r.statusCode !== 429))
})

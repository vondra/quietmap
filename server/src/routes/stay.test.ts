import assert from 'node:assert/strict'
import test, { mock } from 'node:test'
import Fastify from 'fastify'
import { stayRoutes, snap, pickPrecision, slim, resetUpstreamWindowForTests } from './stay.js'

test('snap keeps grid-boundary values in place', () => {
  // Regression: floor(50.05/0.05) is 1000.999… without the epsilon, snapping
  // a whole cell too far and doubling the requested box (grid is 0.01 now,
  // same failure class).
  assert.equal(snap(50.05, false), '50.05')
  assert.equal(snap(50.05, true), '50.05')
  assert.equal(snap(50.063, false), '50.06')
  assert.equal(snap(50.063, true), '50.07')
  assert.equal(snap(-0.025, false), '-0.03')
  assert.equal(snap(-0.025, true), '-0.02')
})

test('pickPrecision fits the H3 grid to the page budget', () => {
  // Central-Prague-sized box (~40 km²): r9 would need ~380 cells, r8 fits (54).
  assert.equal(pickPrecision(50.05, 14.35, 50.10, 14.45), 8)
  // City-overview box (~700 km²): r7 → ~136 cells ≤ 300-cell budget.
  assert.equal(pickPrecision(50.0, 14.2, 50.25, 14.55), 7)
  // Tiny box: finest configured resolution wins.
  assert.equal(pickPrecision(50.05, 14.35, 50.055, 14.355), 10)
  // Largest allowed box at the equator: r3 keeps it under budget.
  assert.equal(pickPrecision(-6, 0, 6, 12), 3)
})

const SAMPLE = {
  id: '71440222.0000',
  url: 'https://www.stay22.com/allez/roam/usds_71440222.0000?aid=stay22',
  suppliers: {
    booking: { id: '8612595', link: 'https://www.stay22.com/allez/booking/8612595', price: { total: 341 } },
    expedia: { id: 'x', link: 'https://www.stay22.com/allez/expedia/x', price: { total: 299 } },
  },
  name: 'Garden apartment',
  location: { coordinates: { lat: 50.081664, lng: 14.456728 } },
  rating: { value: 8.9, hotelStars: 3, count: 57 },
  capacity: { guests: 3, bedrooms: 1, beds: 2, bathrooms: 1 },
  policies: { instantBook: true, freeCancellation: true },
  media: { thumbnail: 'https://q-xx.bstatic.com/photo.jpg' },
}

test('slim maps a Stay22 result and picks the cheapest supplier price', () => {
  const s = slim(SAMPLE, 2)
  assert.ok(s)
  assert.equal(s.id, '71440222.0000')
  assert.equal(s.lat, 50.081664)
  assert.equal(s.price?.total, 299)
  assert.equal(s.price?.perNight, 150)
  assert.equal(s.rating.stars, 3)
  assert.equal(s.freeCancellation, true)
})

test('slim drops results without coordinates and survives missing fields', () => {
  assert.equal(slim({ name: 'x', url: 'https://x.example/a' }, 2), null)
  const bare = slim({ id: 1, name: 'x', url: 'https://x.example/a', location: { coordinates: { lat: 1, lng: 2 } } }, 2)
  assert.ok(bare)
  assert.equal(bare.price, null)
  assert.equal(bare.rating.value, null)
})

test('slim refuses non-https URLs and non-numeric numbers from upstream', () => {
  // A poisoned javascript: link would execute on click; a stray "8.9" string
  // would throw in the card's toFixed.
  assert.equal(slim({ ...SAMPLE, url: 'javascript:alert(1)' }, 2), null)
  const t = slim({
    ...SAMPLE,
    media: { thumbnail: 'http://insecure.example/p.jpg' },
    rating: { value: '8.9', count: 57, hotelStars: 3 },
    suppliers: { booking: { price: { total: '341' } } },
  }, 2)
  assert.ok(t)
  assert.equal(t.thumbnail, null)
  assert.equal(t.rating.value, null)
  assert.equal(t.rating.count, 57)
  assert.equal(t.price, null)
})

test('GET /api/stay validates the bbox before any upstream call', async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async () => {
    throw new Error('must not be called')
  })
  t.after(() => fetchMock.mock.restore())

  const app = Fastify()
  await app.register(stayRoutes)
  t.after(async () => app.close())

  for (const qs of [
    'swlat=x&swlng=14&nelat=51&nelng=15',            // non-numeric
    'swlat=51&swlng=14&nelat=50&nelng=15',           // inverted
    'swlat=50&swlng=14&nelat=63&nelng=15',           // span over cap
  ]) {
    const response = await app.inject(`/api/stay?${qs}`)
    assert.equal(response.statusCode, 400, qs)
  }
  assert.equal(fetchMock.mock.callCount(), 0)
})

const mk = (i: number) => ({
  ...SAMPLE,
  id: `h${i}`,
  location: { coordinates: { lat: 48.21 + i * 1e-4, lng: 17.21 } },
})

test('GET /api/stay pages through a dense flat-mode bucket', async (t) => {
  resetUpstreamWindowForTests()
  const pages = [
    { results: Array.from({ length: 100 }, (_, i) => mk(i)), meta: { total: 150 } },
    { results: Array.from({ length: 50 }, (_, i) => mk(100 + i)), meta: { total: 150 } },
  ]
  const urls: string[] = []
  const fetchMock = mock.method(globalThis, 'fetch', async (url: any) => {
    urls.push(String(url))
    return new Response(JSON.stringify(pages[urls.length - 1]), { status: 200 })
  })
  t.after(() => fetchMock.mock.restore())

  const app = Fastify()
  await app.register(stayRoutes)
  t.after(async () => app.close())

  // Snapped span 0.02 deg — street scale, so flat mode with pagination.
  const response = await app.inject('/api/stay?swlat=48.20&swlng=17.20&nelat=48.22&nelng=17.22')
  assert.equal(response.statusCode, 200)
  assert.equal(response.json().listings.length, 150)
  assert.equal(urls.length, 2)
  assert.ok(!urls[0].includes('cluster='), 'flat mode must not cluster')
  assert.ok(urls[1].includes('page=2'))
})

test('GET /api/stay falls back to uniform sampling when flat mode truncates', async (t) => {
  resetUpstreamWindowForTests()
  // Page 1 reveals total=400 > the 300 budget → flat paging must stop
  // immediately (paging on would burn the window the clustered refetch
  // needs) and the biased flat cut is replaced by cluster=top sampling.
  const flatPage = { results: Array.from({ length: 100 }, (_, i) => mk(i)), meta: { total: 400 } }
  const clusterSet = { results: Array.from({ length: 80 }, (_, i) => mk(1000 + i)), meta: { total: 80 } }
  const urls: string[] = []
  const fetchMock = mock.method(globalThis, 'fetch', async (url: any) => {
    urls.push(String(url))
    return new Response(JSON.stringify(urls.length <= 1 ? flatPage : clusterSet), { status: 200 })
  })
  t.after(() => fetchMock.mock.restore())

  const app = Fastify()
  await app.register(stayRoutes)
  t.after(async () => app.close())

  const response = await app.inject('/api/stay?swlat=48.30&swlng=17.30&nelat=48.32&nelng=17.32')
  assert.equal(response.statusCode, 200)
  assert.equal(urls.length, 2, 'flat aborts after page 1, one clustered set')
  assert.ok(!urls[0].includes('cluster='), 'starts flat')
  assert.ok(urls[1].includes('cluster=top'), 'refetches clustered')
  assert.equal(response.json().listings.length, 80)
  assert.equal(response.headers['cache-control'], 'public, max-age=300')
})

test('GET /api/stay forwards owner filters and prices per real night', async (t) => {
  resetUpstreamWindowForTests()
  const day = (o: number) => new Date(Date.now() + o * 86_400_000).toISOString().slice(0, 10)
  const urls: string[] = []
  const fetchMock = mock.method(globalThis, 'fetch', async (url: any) => {
    urls.push(String(url))
    return new Response(JSON.stringify({ results: [SAMPLE], meta: { total: 1 } }), { status: 200 })
  })
  t.after(() => fetchMock.mock.restore())

  const app = Fastify()
  await app.register(stayRoutes)
  t.after(async () => app.close())

  const qs = `swlat=48.40&swlng=17.40&nelat=48.42&nelng=17.42&checkin=${day(10)}&checkout=${day(13)}&adults=3&max=120&minstars=4&minrating=8`
  const response = await app.inject(`/api/stay?${qs}`)
  assert.equal(response.statusCode, 200)
  const upstream = urls[0]
  for (const frag of [`checkin=${day(10)}`, `checkout=${day(13)}`, 'adults=3', 'max=120', 'minstarrating=4', 'minguestrating=8']) {
    assert.ok(upstream.includes(frag), `upstream missing ${frag}`)
  }
  const listing = response.json().listings[0]
  assert.equal(response.json().meta.nights, 3)
  assert.equal(listing.price.perNight, Math.round(299 / 3))

  // Nonsense dates fall back to the defaults instead of erroring.
  const bad = await app.inject('/api/stay?swlat=48.44&swlng=17.44&nelat=48.46&nelng=17.46&checkin=2020-01-01&checkout=2020-01-05')
  assert.equal(bad.statusCode, 200)
  assert.equal(bad.json().meta.nights, 2)
})

test('GET /api/stay serves the second hit from cache', async (t) => {
  resetUpstreamWindowForTests()
  const upstream = { results: [SAMPLE] }
  const fetchMock = mock.method(globalThis, 'fetch', async () =>
    new Response(JSON.stringify(upstream), { status: 200 }))
  t.after(() => fetchMock.mock.restore())

  const app = Fastify()
  await app.register(stayRoutes)
  t.after(async () => app.close())

  // Distinct bbox from other tests — the route cache is module-level.
  const qs = 'swlat=48.10&swlng=17.10&nelat=48.15&nelng=17.15'
  const first = await app.inject(`/api/stay?${qs}`)
  assert.equal(first.statusCode, 200)
  assert.equal(first.json().listings.length, 1)
  assert.equal(first.json().listings[0].price.total, 299)

  const second = await app.inject(`/api/stay?${qs}`)
  assert.equal(second.statusCode, 200)
  assert.equal(fetchMock.mock.callCount(), 1)
})

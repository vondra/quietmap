// The places-to-stay route against a mocked Stay22: the box it asks for (an over-wide view's middle),
// each kind asked apart in two lists and every page read, one listing per place at its cheapest
// price and with https links only, queries refused before Stay22 is asked (impossible dates too),
// Stay22's refusal passed on, a failure or a malformed success never kept, one kind's failure beside
// the other's places, an answer's lifetime kept and ended, the key's and a visitor's minute, at most
// four searches at once, Stay22's busy minute waited out (a minute at most), and a server without a
// Stay22 account that answers 503.
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test, { mock } from 'node:test'
import Fastify from 'fastify'
import { readConfig } from '../config.ts'
import { clusterPrecision, retryAfterSeconds, slimListing } from '../stay22.ts'
import { snapBox, stayRoutes } from './stay.ts'

async function app(t: test.TestContext, stay22: { aid: string; apiKey: string } | null = { aid: 'test-aid', apiKey: 'test-key' }) {
  const built = Fastify()
  await built.register(stayRoutes, { stay22 })
  t.after(() => built.close())
  return built
}

const SAMPLE = {
  id: '71440222.0000',
  url: 'https://www.stay22.com/allez/roam/usds_71440222.0000?aid=test-aid',
  suppliers: {
    booking: { link: 'https://www.stay22.com/allez/booking/8612595', price: { total: 341 } },
    expedia: { link: 'https://www.stay22.com/allez/expedia/x', price: { total: 299 } },
  },
  name: 'Garden apartment',
  location: { coordinates: { lat: 50.081664, lng: 14.456728 } },
  rating: { value: 8.9, hotelStars: 3, count: 57 },
  capacity: { guests: 3, bedrooms: 1, beds: 2, bathrooms: 1 },
  policies: { instantBook: true, freeCancellation: true },
  media: { thumbnail: 'https://q-xx.bstatic.com/photo.jpg' },
}
const place = (id: string) => ({ ...SAMPLE, id })

/** One page of Stay22's answer. */
const page = (results: object[], total = results.length) =>
  new Response(JSON.stringify({ meta: { total }, results }), { status: 200 })

/** The day `days` from today (UTC). */
const day = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString().slice(0, 10)
const STAY = `checkin=${day(30)}&checkout=${day(33)}`
const VIEW = 'swlat=50.0712&swlng=14.4105&nelat=50.0838&nelng=14.4318'

test('a view is answered for its box moved out to a grid of 1, 2 or 5 times a power of ten', () => {
  assert.deepEqual(snapBox({ swlat: 50.0712, swlng: 14.4105, nelat: 50.0838, nelng: 14.4318 }),
    { swlat: 50.07, swlng: 14.41, nelat: 50.085, nelng: 14.435 })
  // An edge on the grid stays (floor(50.05 / 0.005) is not 10009).
  assert.deepEqual(snapBox({ swlat: 50.05, swlng: 14.4, nelat: 50.06, nelng: 14.42 }),
    { swlat: 50.05, swlng: 14.4, nelat: 50.06, nelng: 14.42 })
  assert.deepEqual(snapBox({ swlat: -33.9512, swlng: 18.3811, nelat: -33.8811, nelng: 18.4602 }),
    { swlat: -33.96, swlng: 18.38, nelat: -33.88, nelng: 18.47 })
  // A phone's street: a grid of 0.002°, not 0.0005° (each pan would be a new search).
  assert.deepEqual(snapBox({ swlat: 50.077391, swlng: 14.424241, nelat: 50.085609, nelng: 14.430159 }),
    { swlat: 50.076, swlng: 14.424, nelat: 50.086, nelng: 14.432 })
  assert.deepEqual(snapBox({ swlat: 49.1, swlng: 13.2, nelat: 50.9, nelng: 16.7 }),
    { swlat: 49, swlng: 13, nelat: 51, nelng: 17 })
})

test('the one-place-per-cell list takes the finest H3 resolution whose cells fit a list', () => {
  // Central Prague, 4.8 km²: resolution 10 would be 317 cells, 9 is 45.
  assert.equal(clusterPrecision({ swlat: 50.07, swlng: 14.41, nelat: 50.09, nelng: 14.44 }), 9)
  // Prague, 637 km²: resolution 7 is 123 cells.
  assert.equal(clusterPrecision({ swlat: 49.95, swlng: 14.25, nelat: 50.15, nelng: 14.65 }), 7)
  // The widest box at the equator: resolution 3 is 256 cells.
  assert.equal(clusterPrecision({ swlat: -8, swlng: 0, nelat: 8, nelng: 16 }), 3)
})

test('a result keeps its cheapest supplier, https links and numbers only', () => {
  assert.deepEqual(slimListing(SAMPLE), {
    id: '71440222.0000',
    name: 'Garden apartment',
    lat: 50.081664,
    lng: 14.456728,
    total: 299,
    stars: 3,
    score: 8.9,
    reviews: 57,
    guests: 3,
    bedrooms: 1,
    freeCancellation: true,
    thumbnail: 'https://q-xx.bstatic.com/photo.jpg',
    url: SAMPLE.url,
  })
  // A javascript: link would run on a click; a string number would break the page's arithmetic.
  assert.equal(slimListing({ ...SAMPLE, url: 'javascript:alert(1)' }), null)
  const loose = slimListing({
    ...SAMPLE,
    media: { thumbnail: 'http://insecure.example/p.jpg' },
    rating: { value: '8.9', count: 57 },
    suppliers: { booking: { price: { total: '341' } } },
  })
  assert.equal(loose?.thumbnail, null)
  assert.equal(loose?.score, null)
  assert.equal(loose?.stars, null)
  assert.equal(loose?.total, null)
  assert.equal(slimListing({ ...SAMPLE, id: undefined }), null)
  assert.equal(slimListing({ ...SAMPLE, location: {} }), null)
})

test('a query that names no box, no stay or an unknown filter is refused before Stay22 is asked', async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async () => { throw new Error('must not be called') })
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  for (const query of [
    STAY,
    `swlat=x&swlng=14&nelat=51&nelng=15&${STAY}`,
    `swlat=51&swlng=14&nelat=50&nelng=15&${STAY}`,
    `swlat=50&swlng=179&nelat=51&nelng=181&${STAY}`,
    VIEW,
    `${VIEW}&checkin=${day(30)}&checkout=5.11.2026`,
    // Stay22 refuses a check-in before its today, and a check-out not after the check-in.
    `${VIEW}&checkin=${day(-1)}&checkout=${day(1)}`,
    `${VIEW}&checkin=${day(3)}&checkout=${day(3)}`,
    // An impossible date: Date.parse reads 31 November as 1 December, a stay of no night.
    `${VIEW}&checkin=${new Date().getUTCFullYear() + 1}-11-31&checkout=${new Date().getUTCFullYear() + 1}-12-01`,
    `${VIEW}&${STAY}&type=villa`,
    `${VIEW}&${STAY}&adults=0`,
    `${VIEW}&${STAY}&minstars=6`,
    `${VIEW}&${STAY}&minscore=11`,
  ]) {
    const response = await server.inject(`/api/stay?${query}`)
    assert.equal(response.statusCode, 400, query)
    assert.equal(typeof response.json().error, 'string')
  }
  assert.equal(fetchMock.mock.callCount(), 0)
})

test('each kind is asked apart in its ranked and its per-cell list; a place is answered once', async (t) => {
  const asked: URL[] = []
  const fetchMock = mock.method(globalThis, 'fetch', async (url: string, init: RequestInit) => {
    assert.deepEqual(init.headers, { 'X-API-KEY': 'test-key' })
    const params = new URL(url).searchParams
    asked.push(new URL(url))
    const perCell = params.get('cluster') === 'top'
    if (params.get('type') === 'hotel') return page(perCell ? [place('h2'), place('h3')] : [place('h1'), place('h2')])
    return page([place('r1')])
  })
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)

  const response = await server.inject(`/api/stay?${VIEW}&${STAY}&adults=3&minstars=4&minscore=8`)
  assert.equal(response.statusCode, 200)
  assert.equal(response.headers['cache-control'], 'no-store')
  const answer = response.json()
  assert.equal(answer.failure, null)
  assert.ok(answer.expiresIn > 55 * 60 - 5 && answer.expiresIn <= 55 * 60)
  assert.deepEqual(answer.listings.map((listing: { id: string }) => listing.id), ['h1', 'h2', 'h3', 'r1'])
  assert.equal(answer.nights, 3)
  assert.equal(answer.currency, 'EUR')
  assert.equal(answer.listings[0].total, 299)

  const box = { swlat: 50.07, swlng: 14.41, nelat: 50.085, nelng: 14.435 }
  assert.deepEqual(asked.map(url => [url.searchParams.get('type'), url.searchParams.get('cluster')]),
    [['hotel', null], ['hotel', 'top'], ['rental', null], ['rental', 'top']])
  for (const url of asked) {
    assert.equal(`${url.origin}${url.pathname}`, 'https://api.stay22.com/v2/accommodations')
    const params = Object.fromEntries(url.searchParams)
    assert.deepEqual({ ...params, type: undefined, cluster: undefined, precision: undefined }, {
      swlat: '50.07', swlng: '14.41', nelat: '50.085', nelng: '14.435',
      checkin: day(30), checkout: day(33), currency: 'EUR', pageSize: '100', aid: 'test-aid',
      adults: '3', minstarrating: '4', minguestrating: '8', page: '1',
      type: undefined, cluster: undefined, precision: undefined,
    })
    if (params.cluster) assert.equal(params.precision, String(clusterPrecision(box)))
  }

  // The same box is answered from memory, for the same view or one moved inside it.
  const again = await server.inject(`/api/stay?${VIEW}&${STAY}&adults=3&minstars=4&minscore=8`)
  const moved = await server.inject(`/api/stay?swlat=50.0715&swlng=14.4111&nelat=50.0841&nelng=14.4322&${STAY}&adults=3&minstars=4&minscore=8`)
  assert.deepEqual(again.json().listings, answer.listings)
  assert.deepEqual(moved.json().listings, answer.listings)
  assert.equal(fetchMock.mock.callCount(), 4)
})

test('a list longer than a page is read to its third page', async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async (url: string) => {
    const params = new URL(url).searchParams
    if (params.get('cluster') === 'top') return page([])
    const number = Number(params.get('page'))
    const ids = Array.from({ length: number < 3 ? 100 : 50 }, (_, at) => place(`h${number}-${at}`))
    return page(ids, 250)
  })
  t.after(() => fetchMock.mock.restore())
  const response = await (await app(t)).inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)
  assert.equal(response.statusCode, 200)
  assert.equal(response.json().listings.length, 250)
  const pages = fetchMock.mock.calls.map(call => new URL(call.arguments[0] as string).searchParams)
    .filter(params => params.get('cluster') == null).map(params => params.get('page')).sort()
  assert.deepEqual(pages, ['1', '2', '3'])
  assert.equal(fetchMock.mock.callCount(), 4)
})

test("Stay22's refusal reaches the visitor; a failed search is answered 502 and not kept", async (t) => {
  let answer = () => new Response(JSON.stringify({ message: 'querystring/checkin Must be today or in the future' }), { status: 400 })
  const fetchMock = mock.method(globalThis, 'fetch', async () => answer())
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)

  const refused = await server.inject(`/api/stay?${VIEW}&${STAY}`)
  assert.equal(refused.statusCode, 400)
  assert.deepEqual(refused.json(), { error: 'Stay22: querystring/checkin Must be today or in the future' })

  answer = () => new Response('busy', { status: 503 })
  const failed = await server.inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)
  assert.equal(failed.statusCode, 502)
  answer = () => page([SAMPLE])
  const retried = await server.inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)
  assert.equal(retried.statusCode, 200)
  assert.equal(retried.json().listings.length, 1)
})

test('after a 429 no search goes to Stay22 until its Retry-After has passed', async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async () =>
    new Response(JSON.stringify({ code: 'RATE_LIMIT_EXCEEDED' }), { status: 429, headers: { 'retry-after': '7' } }))
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  const busy = await server.inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)
  assert.equal(busy.statusCode, 503)
  assert.equal(busy.headers['retry-after'], '7')
  assert.match(busy.json().error, /asked too often just now; places to stay again in 7 s/)
  const calls = fetchMock.mock.callCount()
  // Another place within the minute is answered at once, without asking.
  const elsewhere = await server.inject(`/api/stay?swlat=48.85&swlng=2.33&nelat=48.86&nelng=2.35&${STAY}`)
  assert.equal(elsewhere.statusCode, 503)
  assert.equal(fetchMock.mock.callCount(), calls)
})

test('a view wider than 16° is searched for its middle 16°', async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async () => page([]))
  t.after(() => fetchMock.mock.restore())
  const response = await (await app(t)).inject(`/api/stay?swlat=40&swlng=14&nelat=57&nelng=15&${STAY}&type=hotel`)
  assert.equal(response.statusCode, 200)
  // The middle 40.5-56.5 moved out to a grid of 2°.
  const params = new URL(fetchMock.mock.calls[0].arguments[0] as string).searchParams
  assert.deepEqual([params.get('swlat'), params.get('nelat'), params.get('swlng'), params.get('nelng')], ['40', '58', '14', '16'])
})

test("Stay22's 429 stops every list of the search: no page is sent after it", async (t) => {
  const sent: string[] = []
  const fetchMock = mock.method(globalThis, 'fetch', async (url: string, init: RequestInit) => {
    const params = new URL(url).searchParams
    const list = `${params.get('type')} ${params.get('cluster') ?? 'ranked'} ${params.get('page')}`
    if (params.get('type') === 'rental') {
      sent.push(list)
      return new Response('{}', { status: 429, headers: { 'retry-after': '9' } })
    }
    // The hotels' first page, of 250, comes back after the rentals were refused.
    await new Promise(resolve => setTimeout(resolve, 20))
    if (init.signal?.aborted) throw new DOMException('aborted', 'AbortError')
    sent.push(list)
    return page(Array.from({ length: 100 }, (_, at) => place(`h${at}`)), 250)
  })
  t.after(() => fetchMock.mock.restore())
  const response = await (await app(t)).inject(`/api/stay?${VIEW}&${STAY}`)
  assert.equal(response.statusCode, 503)
  assert.equal(response.headers['retry-after'], '9')
  await new Promise(resolve => setTimeout(resolve, 50))
  assert.deepEqual(sent.sort(), ['rental ranked 1', 'rental top 1'])
})

test('a Retry-After is believed up to a minute: none, a negative or an absurd one waits the minute', () => {
  assert.equal(retryAfterSeconds('7'), 7)
  assert.equal(retryAfterSeconds('999999999'), 60)
  assert.equal(retryAfterSeconds('-1'), 60)
  assert.equal(retryAfterSeconds(null), 60)
  assert.equal(retryAfterSeconds('Wed, 21 Oct 2026 07:28:00 GMT'), 60)
})

test('a success without results is a failure, never a kept "no rooms"', async (t) => {
  let answer = () => new Response(JSON.stringify({ message: 'upstream temporarily unavailable' }), { status: 200 })
  const fetchMock = mock.method(globalThis, 'fetch', async () => answer())
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  assert.equal((await server.inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)).statusCode, 502)
  answer = () => page([SAMPLE])
  const retried = await server.inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)
  assert.equal(retried.statusCode, 200)
  assert.equal(retried.json().listings.length, 1)
})

test("one kind's failure keeps the other kind's places, with the failure beside them, and is not kept", async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async (url: string) =>
    (new URL(url).searchParams.get('type') === 'rental' ? new Response('busy', { status: 503 }) : page([place('h1')])))
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  const response = await server.inject(`/api/stay?${VIEW}&${STAY}`)
  assert.equal(response.statusCode, 200)
  assert.deepEqual(response.json().listings.map((listing: { id: string }) => listing.id), ['h1'])
  assert.equal(response.json().failure, 'Stay22 did not answer; no places to stay for now.')
  await server.inject(`/api/stay?${VIEW}&${STAY}`)
  assert.equal(fetchMock.mock.callCount(), 8)
})

test("a visitor spends a third of the key's minute; three visitors spend the key's 150", async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async () => page([place('h1')]))
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  // 14 distinct views from one address: a search of both kinds is 4 calls, so 12 searches spend 48
  // of the visitor's 50, the 13th gets two lists (answered with the failure), the 14th none.
  const spendAll = async (visitor: number) => {
    const statuses: number[] = []
    for (let at = 0; at < 14; at += 1) {
      const view = `swlat=${10 + at}&swlng=${visitor}&nelat=${10.01 + at}&nelng=${visitor + 0.01}`
      const response = await server.inject({ url: `/api/stay?${view}&${STAY}`, remoteAddress: `203.0.113.${visitor}` })
      statuses.push(response.statusCode === 200 && response.json().failure ? 206 : response.statusCode)
    }
    return statuses
  }
  assert.deepEqual(await spendAll(1), [...Array(12).fill(200), 206, 503])
  assert.equal(fetchMock.mock.callCount(), 50)
  await spendAll(2)
  await spendAll(3)
  assert.equal(fetchMock.mock.callCount(), 150)
  const fourth = await server.inject({ url: `/api/stay?swlat=30&swlng=10&nelat=30.01&nelng=10.01&${STAY}`, remoteAddress: '203.0.113.4' })
  assert.equal(fourth.statusCode, 503)
  assert.equal(fetchMock.mock.callCount(), 150)
})

test('at most four searches run at once; the fifth waits a second', async (t) => {
  let release!: () => void
  const held = new Promise<void>(resolve => { release = resolve })
  const fetchMock = mock.method(globalThis, 'fetch', async () => {
    await held
    return page([place('h1')])
  })
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  const view = (at: number) => `/api/stay?swlat=${10 + at}&swlng=10&nelat=${10.01 + at}&nelng=10.01&${STAY}&type=hotel`
  const running = [0, 1, 2, 3].map(at => server.inject(view(at)))
  await new Promise(resolve => setTimeout(resolve, 20))
  const fifth = await server.inject(view(4))
  assert.equal(fifth.statusCode, 503)
  assert.equal(fifth.headers['retry-after'], '1')
  release()
  assert.deepEqual((await Promise.all(running)).map(response => response.statusCode), [200, 200, 200, 200])
})

test("an answer lives 55 minutes from when it was asked, kept answers too, and then leaves memory", async (t) => {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'], now: Date.now() })
  const fetchMock = mock.method(globalThis, 'fetch', async () => page([SAMPLE]))
  t.after(() => fetchMock.mock.restore())
  const server = await app(t)
  const ask = async () => (await server.inject(`/api/stay?${VIEW}&${STAY}&type=hotel`)).json()
  assert.equal((await ask()).expiresIn, 55 * 60)
  t.mock.timers.tick(10 * 60_000)
  assert.equal((await ask()).expiresIn, 45 * 60)
  assert.equal(fetchMock.mock.callCount(), 2)
  t.mock.timers.tick(45 * 60_000)
  assert.equal((await ask()).expiresIn, 55 * 60)
  assert.equal(fetchMock.mock.callCount(), 4)
})

test('without a Stay22 account the route answers 503, and nothing is asked', async (t) => {
  const fetchMock = mock.method(globalThis, 'fetch', async () => { throw new Error('must not be called') })
  t.after(() => fetchMock.mock.restore())
  const response = await (await app(t, null)).inject(`/api/stay?${VIEW}&${STAY}`)
  assert.equal(response.statusCode, 503)
  assert.deepEqual(response.json(), { error: 'Places to stay are not set up on this server.' })
  assert.equal(fetchMock.mock.callCount(), 0)
})

test('the Stay22 account is both its values or neither', async (t) => {
  const prepared = await mkdtemp(join(tmpdir(), 'qm-stay-config-'))
  t.after(() => rm(prepared, { recursive: true, force: true }))
  await mkdir(join(prepared, '2026'))
  const env = {
    PORT: '8000',
    QM_POPUP_BIN: process.execPath,
    QM_RASTER_BIN: process.execPath,
    QM_PREPARED_DIR: prepared,
    QM_YEARS: '2026',
    QM_TILES_DIR: prepared,
  }
  assert.equal(readConfig(env).stay22, null)
  assert.throws(() => readConfig({ ...env, STAY22_AID: 'aid' }), /STAY22_API_KEY is not set/)
  assert.throws(() => readConfig({ ...env, STAY22_API_KEY: 'key' }), /STAY22_AID is not set/)
  assert.deepEqual(readConfig({ ...env, STAY22_AID: 'aid', STAY22_API_KEY: 'key' }).stay22, { aid: 'aid', apiKey: 'key' })
})

// Places to stay: the search a visitor starts from (Stay22's today, which west of UTC can be the
// visitor's tomorrow) and moved on past midnight, dates that keep the stay's length, a view's box
// seen straight down, the query for a view (two boxes across the antimeridian), prices, the pins
// kept while the map moves and until they expire, and the place a feature carries.
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  MAX_PINS, defaultStaySearch, firstCheckin, formatPrice, freshSearch, stayFeatures, stayOfFeature, stayRequests, viewFootprint,
  withCheckin, withCheckout, withStays,
} from '../src/lib/stays.ts'

/** `now` read in the time zone `zone`. */
function inZone(zone, run) {
  const saved = process.env.TZ
  process.env.TZ = zone
  try {
    return run()
  } finally {
    if (saved === undefined) delete process.env.TZ
    else process.env.TZ = saved
  }
}

test('a search starts today for two nights and two guests; today is never before Stay22\'s', () => {
  inZone('Europe/Prague', () => {
    assert.deepEqual(defaultStaySearch(new Date('2026-10-08T21:30:00Z')),
      { kind: 'all', checkin: '2026-10-08', checkout: '2026-10-10', adults: 2, minStars: null, minScore: null })
  })
  // 21:00 in Los Angeles is already the next day in UTC, the day Stay22 starts from.
  inZone('America/Los_Angeles', () => assert.equal(firstCheckin(new Date('2026-10-09T04:00:00Z')), '2026-10-09'))
  // In Auckland the visitor's today is ahead of UTC: theirs counts.
  inZone('Pacific/Auckland', () => assert.equal(firstCheckin(new Date('2026-10-08T20:00:00Z')), '2026-10-09'))
})

test('a new check-in keeps the stay\'s length; a check-out stays at least a night after it', () => {
  const search = { kind: 'all', checkin: '2026-10-08', checkout: '2026-10-11', adults: 2, minStars: null, minScore: null }
  assert.deepEqual(withCheckin(search, '2026-12-30'), { ...search, checkin: '2026-12-30', checkout: '2027-01-02' })
  assert.deepEqual(withCheckout(search, '2026-10-20'), { ...search, checkout: '2026-10-20' })
  assert.deepEqual(withCheckout(search, '2026-10-08'), { ...search, checkout: '2026-10-09' })
})

test('past midnight a search moves to the first check-in Stay22 takes, its length kept; else it stays itself', () => {
  const search = { kind: 'all', checkin: '2026-10-08', checkout: '2026-10-10', adults: 2, minStars: null, minScore: null }
  inZone('Europe/Prague', () => {
    assert.equal(freshSearch(search, new Date('2026-10-08T21:00:00Z')), search)
    assert.deepEqual(freshSearch(search, new Date('2026-10-09T00:30:00Z')), { ...search, checkin: '2026-10-09', checkout: '2026-10-11' })
  })
})

test('a view is its canvas seen straight down, turned by the bearing, in the world copy -180..180', () => {
  const near = (box) => Object.fromEntries(Object.entries(box).map(([key, value]) => [key, +value.toFixed(6)]))
  // What MapLibre's bounds were for this view, flat and north up (Wenceslas Square, 1400 x 900).
  assert.deepEqual(near(viewFootprint({ lng: 14.4272, lat: 50.0815 }, 15.5, 0, 1400, 900)),
    { west: 14.416579, south: 50.077118, east: 14.437821, north: 50.085881, zoom: 15.5 })
  // Turned a quarter, the canvas's height runs east-west.
  const turned = viewFootprint({ lng: 14.4272, lat: 50.0815 }, 15.5, 90, 1400, 900)
  assert.ok(Math.abs(turned.east - turned.west - (14.437821 - 14.416579) * 900 / 1400) < 1e-6)
  const copy = viewFootprint({ lng: 374.4272, lat: 50.0815 }, 15.5, 0, 1400, 900)
  assert.equal(+copy.west.toFixed(6), 14.416579)
})

test('a view asks for its box; across the antimeridian for a box on each side, none of it left out', () => {
  const search = { kind: 'hotel', checkin: '2026-10-08', checkout: '2026-10-10', adults: 3, minStars: 4, minScore: 8 }
  const queries = (view) => stayRequests(view, search).map(request => Object.fromEntries(new URL(request, 'http://localhost').searchParams))
  assert.deepEqual(queries({ west: 14.4105123, south: 50.0712, east: 14.4318, north: 50.0838 }), [{
    swlat: '50.0712', swlng: '14.410512', nelat: '50.0838', nelng: '14.4318',
    checkin: '2026-10-08', checkout: '2026-10-10', adults: '3', type: 'hotel', minstars: '4', minscore: '8',
  }])
  const sides = (view) => queries(view).map(query => [query.swlng, query.nelng])
  assert.deepEqual(sides({ west: 179.5, south: -18, east: 180.6, north: -17 }), [['179.5', '180'], ['-180', '-179.4']])
  assert.deepEqual(sides({ west: -180.6, south: -18, east: -179.5, north: -17 }), [['179.4', '180'], ['-180', '-179.5']])
  assert.equal(new URL(stayRequests({ west: 1, south: 1, east: 2, north: 2 }, { ...search, kind: 'all', minStars: null, minScore: null })[0], 'http://localhost').search,
    '?swlat=1&swlng=1&nelat=2&nelng=2&checkin=2026-10-08&checkout=2026-10-10&adults=3')
})

const stay = (id, total, reviews = 0, expires = 2_000) => ({
  id, name: id, lat: 50, lng: 14, total, stars: null, score: null, reviews, guests: null, bedrooms: null,
  freeCancellation: false, thumbnail: null, url: 'https://example.com', nights: 2, currency: 'EUR', expires,
})

test('prices print in whole units; a pin labels its price of a night, none without a price, and carries its place', () => {
  assert.equal(formatPrice(1234.4, 'EUR'), '€1,234')
  const { features } = stayFeatures([stay('a', 299, 57), stay('b', null)])
  assert.deepEqual(features.map(({ properties: { price, reviews } }) => ({ price, reviews })), [
    { price: '€150', reviews: 57 },
    { price: '', reviews: 0 },
  ])
  assert.deepEqual(features[0].geometry.coordinates, [14, 50])
  assert.deepEqual(stayOfFeature(features[1]), stay('b', null))
})

test('an answer adds its pins to the earlier ones, a place once at its newest price, none past its expiry', () => {
  const pins = withStays([stay('a', 100), stay('b', 200)], [stay('b', 250), stay('c', 300)], 1_000)
  assert.deepEqual(pins.map(pin => [pin.id, pin.total]), [['a', 100], ['b', 250], ['c', 300]])
  const later = withStays(pins, [stay('d', 400, 0, 3_000)], 2_500)
  assert.deepEqual(later.map(pin => pin.id), ['d'])
  const many = withStays([], Array.from({ length: MAX_PINS + 5 }, (_, at) => stay(`p${at}`, at)), 1_000)
  assert.equal(many.length, MAX_PINS)
  assert.equal(many[0].id, 'p5')
})

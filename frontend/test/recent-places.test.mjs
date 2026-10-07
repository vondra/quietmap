// The recent places: newest first, one entry per place, at most five, names that come back late,
// and what storage held read defensively.
import assert from 'node:assert/strict'
import test from 'node:test'

import { RECENT_PLACES_MAX, samePlace, withName, withPlace, withoutPlace } from '../src/lib/recent-places.ts'

const place = (lat, lng, sone = 4) => ({ lat, lng, place: null, sone, lden: 43 })

test('a place opened again moves to the front with its new numbers, not a second time', () => {
  const list = withPlace(withPlace([], place(50, 14, 3)), place(49, 15))
  const again = withPlace(list, place(50.0001, 14.0001, 5))
  assert.equal(again.length, 2)
  assert.deepEqual(again[0], place(50.0001, 14.0001, 5))
})

test('points 30 m apart are separate places; at most five are kept, the oldest go', () => {
  assert.equal(samePlace({ lat: 50, lng: 14 }, { lat: 50.0003, lng: 14 }), false)
  assert.equal(samePlace({ lat: 50, lng: 14 }, { lat: 50.0002, lng: 14 }), true)
  let list = []
  for (let k = 0; k < 10; k++) list = withPlace(list, place(40 + k, 10))
  assert.equal(list.length, RECENT_PLACES_MAX)
  assert.equal(list[0].lat, 49)
  assert.equal(list.at(-1).lat, 45)
})

test('a name that comes back after the next click fills its own place only, and is kept on reopening', () => {
  const list = withPlace(withPlace([], place(50, 14)), place(49, 15))
  const named = withName(list, { lat: 50.0001, lng: 14 }, 'Bedřichov')
  assert.deepEqual(named.map(p => p.place), [null, 'Bedřichov'])
  assert.deepEqual(withName(named, { lat: 50, lng: 14 }, 'Elsewhere').map(p => p.place), [null, 'Bedřichov'])
  assert.equal(withPlace(named, place(50, 14, 6))[0].place, 'Bedřichov')
})

test('a removed place leaves the others in their order', () => {
  const list = [place(1, 1), place(2, 2), place(3, 3)]
  assert.deepEqual(withoutPlace(list, { lat: 2, lng: 2 }).map(p => p.lat), [1, 3])
})

// The places' rules: the recent places newest first, a place opened again in its tab, one entry per
// place, at most five, names that come back late; the names the visitor gives places, the later of
// two kept; what storage held read defensively.
import assert from 'node:assert/strict'
import test from 'node:test'

import { loadPlaceTitles } from '../src/lib/browser-places.ts'
import {
  PLACE_TITLES_MAX, RECENT_PLACES_MAX, mergedTitles, placeLabel, samePlace, titleAt, withName, withPlace, withTitle,
  withoutPlace,
} from '../src/lib/place-rules.ts'

const place = (lat, lng, sone = 4) => ({ lat, lng, place: null, sone, lden: 43 })

test('a place opened again keeps its tab with its new numbers, a silent answer too, not a second time', () => {
  const list = withPlace(withPlace([], place(50, 14, 3)), place(49, 15))
  const again = withPlace(list, place(50.0001, 14.0001, 5))
  assert.equal(again.length, 2)
  assert.equal(again[0].lat, 49)
  assert.deepEqual(again[1], place(50.0001, 14.0001, 5))
  assert.equal(withPlace(again, { lat: 50, lng: 14, place: null, sone: null, lden: null })[1].sone, null)
})

test('a place opened between two kept 40 m apart takes the first one\'s tab and leaves no second', () => {
  const list = withPlace(withPlace([], place(50, 14)), place(50.00036, 14))
  const between = withPlace(list, place(50.00018, 14))
  assert.deepEqual(between.map(p => p.lat), [50.00018])
  assert.deepEqual(withPlace(between, place(50, 14)).map(p => p.lat), [50])
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
  assert.equal(withPlace(named, place(50, 14, 6))[1].place, 'Bedřichov')
})

test('a removed place leaves the others in their order', () => {
  const list = [place(1, 1), place(2, 2), place(3, 3)]
  assert.deepEqual(withoutPlace(list, { lat: 2, lng: 2 }).map(p => p.lat), [1, 3])
})

test('a name stays where it was given: the nearest within 30 m answers, two spots 40 m apart keep two', () => {
  const door = { lat: 50, lng: 14 }
  const gate = { lat: 50.00036, lng: 14 }
  let titles = withTitle(withTitle([], door, 'Door', 1), gate, 'Gate', 2)
  // Between them, 20 m from each: the nearest is renamed, the other kept.
  titles = withTitle(titles, { lat: 50.00017, lng: 14 }, 'Front door', 3)
  assert.deepEqual(titles.map(t => t.title).sort(), ['Front door', 'Gate'])
  assert.equal(titleAt(titles, door), 'Front door')
  // Renamed again and again from 20 m north, it stays at the door.
  for (let k = 0; k < 3; k++) titles = withTitle(titles, { lat: 50.00018, lng: 14 }, `Door ${k}`, 4 + k)
  assert.equal(titleAt(titles, door), 'Door 2')
  assert.deepEqual(titles.find(t => t.title === 'Door 2'), { lat: 50, lng: 14, title: 'Door 2', updatedAt: 6 })
  // Emptied, only its own name goes.
  assert.deepEqual(withTitle(titles, door, null, 7).map(t => t.title), ['Gate'])
  assert.equal(titleAt(titles, { lat: 50.001, lng: 14 }), null)
  let many = []
  for (let k = 0; k <= PLACE_TITLES_MAX; k++) many = withTitle(many, { lat: k, lng: 0 }, `place ${k}`, k)
  assert.equal(many.length, PLACE_TITLES_MAX)
  assert.equal(titleAt(many, { lat: 0, lng: 0 }), null)
})

test('of two names for one place the later stays, on either side of the antimeridian', () => {
  const east = { lat: -16.5, lng: 179.99995 }
  const west = { lat: -16.5, lng: -179.99995 }
  const named = withTitle([], east, 'Taveuni', 20)
  assert.equal(titleAt(named, west), 'Taveuni')
  // An older rename (another device's, sent late) changes nothing; a newer one renames.
  assert.equal(withTitle(named, west, 'Old', 10), named)
  assert.equal(withTitle(named, west, null, 10), named)
  assert.deepEqual(withTitle(named, west, 'New', 30), [{ ...east, title: 'New', updatedAt: 30 }])
})

test("a browser's names join an account's: the later name of one place kept, the account's on a tie", () => {
  const account = [{ lat: 50, lng: 14, title: 'Home', updatedAt: 5 }, { lat: 49, lng: 15, title: 'Cabin', updatedAt: 0 }]
  const browser = [
    { lat: 50.0001, lng: 14, title: 'Flat', updatedAt: 9 },
    { lat: 49, lng: 15.0001, title: 'Hut', updatedAt: 0 },
    { lat: 48, lng: 16, title: 'Lake', updatedAt: 0 },
  ]
  const merged = mergedTitles(account, browser)
  assert.equal(titleAt(merged, { lat: 50, lng: 14 }), 'Flat')
  assert.equal(titleAt(merged, { lat: 49, lng: 15 }), 'Cabin')
  assert.equal(titleAt(merged, { lat: 48, lng: 16 }), 'Lake')
  assert.equal(merged.length, 3)
  // The account's point stays where its name was first given.
  assert.deepEqual(merged.find(t => t.title === 'Flat'), { lat: 50, lng: 14, title: 'Flat', updatedAt: 9 })
  // With the most names kept, the newest stay: an older name for a kept place does not come back,
  // and the oldest name goes.
  const full = Array.from({ length: PLACE_TITLES_MAX }, (_, k) => ({ lat: k * 0.01, lng: 0, title: `p${k}`, updatedAt: 100 }))
  const last = full.at(-1)
  const capped = mergedTitles(full, [{ lat: 80, lng: 0, title: 'old', updatedAt: 1 }, { ...last, title: 'stale', updatedAt: 2 }])
  assert.equal(capped.length, PLACE_TITLES_MAX)
  assert.equal(titleAt(capped, last), 'p199')
  assert.deepEqual(capped.filter(t => t.title === 'stale' || t.title === 'old'), [])
})

test('without the browser storage the names are none to read, not an empty list', () => {
  assert.equal(loadPlaceTitles(), null)
})

test('a geocoded name is proposed by its first part', () => {
  assert.equal(placeLabel('Tijarafe, Santa Cruz de Tenerife, Spain'), 'Tijarafe')
  assert.equal(placeLabel(null), null)
  assert.equal(placeLabel(' , Spain'), null)
})

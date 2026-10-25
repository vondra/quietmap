// The recent places: newest first, one entry per place, at most five, names that come back late,
// and what storage held read defensively; the names the visitor gives places.
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  PLACE_TITLES_MAX, RECENT_PLACES_MAX, loadPlaceTitles, placeLabel, samePlace, titleAt, withName, withPlace, withTitle,
  withoutPlace,
} from '../src/lib/recent-places.ts'

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

test('a name stays where it was given: the nearest within 30 m answers, two spots 40 m apart keep two', () => {
  const door = { lat: 50, lng: 14 }
  const gate = { lat: 50.00036, lng: 14 }
  let titles = withTitle(withTitle([], door, 'Door'), gate, 'Gate')
  // Between them, 20 m from each: the nearest is renamed, the other kept.
  titles = withTitle(titles, { lat: 50.00017, lng: 14 }, 'Front door')
  assert.deepEqual(titles.map(t => t.title).sort(), ['Front door', 'Gate'])
  assert.equal(titleAt(titles, door), 'Front door')
  // Renamed again and again from 20 m north, it stays at the door.
  for (let k = 0; k < 3; k++) titles = withTitle(titles, { lat: 50.00018, lng: 14 }, `Door ${k}`)
  assert.equal(titleAt(titles, door), 'Door 2')
  assert.deepEqual(titles.find(t => t.title === 'Door 2'), { lat: 50, lng: 14, title: 'Door 2' })
  // Emptied, only its own name goes.
  assert.deepEqual(withTitle(titles, door, null).map(t => t.title), ['Gate'])
  assert.equal(titleAt(titles, { lat: 50.001, lng: 14 }), null)
  let many = []
  for (let k = 0; k <= PLACE_TITLES_MAX; k++) many = withTitle(many, { lat: k, lng: 0 }, `place ${k}`)
  assert.equal(many.length, PLACE_TITLES_MAX)
  assert.equal(titleAt(many, { lat: 0, lng: 0 }), null)
})

test('without the browser storage the names are none to read, not an empty list', () => {
  assert.equal(loadPlaceTitles(), null)
})

test('a geocoded name is proposed by its first part', () => {
  assert.equal(placeLabel('Tijarafe, Santa Cruz de Tenerife, Spain'), 'Tijarafe')
  assert.equal(placeLabel(null), null)
  assert.equal(placeLabel(' , Spain'), null)
})

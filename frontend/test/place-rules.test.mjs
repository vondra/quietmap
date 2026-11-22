// The places' rules: the history of the places opened, the one opened last first, one entry per
// place, at most fifty, names that come back late; the names the visitor gives places, the later of
// two kept; what storage held read defensively.
import assert from 'node:assert/strict'
import test from 'node:test'

import { loadPlaceTitles, loadRecentPlaces } from '../src/lib/browser-places.ts'
import {
  PLACE_TITLES_MAX, RECENT_PLACES_MAX, historyOrder, mergedTitles, placeLabel, samePlace, titleAt, withAnswer, withName,
  withOpened, withTitle, withoutPlace,
} from '../src/lib/place-rules.ts'

const answer = (lat, lng, sone = 4) => ({ lat, lng, place: null, sone, lden: 43 })
/** The places of `list`, by latitude, in their order. */
const lats = list => list.map(p => p.lat)
/** The name given the place at `at`, or null. */
const nameAt = (titles, at) => titleAt(titles, at)?.title ?? null

test('the history: the place opened last first, then the one opened before, however often each was', () => {
  let list = []
  // A opened three times, B twice, C and D once, D after C, B last.
  for (const [lat, at] of [[1, 1], [1, 2], [2, 3], [3, 4], [1, 5], [4, 6], [2, 7]]) list = withOpened(list, { lat, lng: 0 }, at)
  // D, opened once just before B, stands above A, opened thrice (owner 2026-10-30: a place searched
  // once was gone from the list's top after the next click).
  assert.deepEqual(lats(list), [2, 4, 1, 3])
  assert.deepEqual(lats(withOpened(list, { lat: 3.0000001, lng: 0 }, 8)), [3, 2, 4, 1])
  // Places kept from before the history kept times keep their order.
  const old = [1, 2, 3].map(lat => ({ ...answer(lat, 0), openedAt: 0 }))
  assert.deepEqual(lats(historyOrder(old)), [1, 2, 3])
})

test("an answer fills its place's numbers, and a name it has none, where it stands; a place forgotten stays so", () => {
  const list = withOpened(withOpened([], { lat: 50, lng: 14 }, 1), { lat: 49, lng: 15 }, 2)
  const answered = withAnswer(list, { ...answer(50.0001, 14.0001, 5), place: 'Kytín' })
  assert.deepEqual(lats(answered), [49, 50])
  assert.deepEqual(answered[1], { lat: 50, lng: 14, place: 'Kytín', sone: 5, lden: 43, openedAt: 1 })
  // A silent answer replaces the numbers; the name known stays, a search's or the geocoder's.
  const silent = withAnswer(answered, { lat: 50, lng: 14, place: 'Elsewhere', sone: null, lden: null })
  assert.deepEqual([silent[1].place, silent[1].sone], ['Kytín', null])
  assert.equal(withAnswer(answered, { ...answer(50, 14, 5), place: 'Kytín' }), answered)
  assert.equal(withAnswer(answered, answer(48, 16)), answered)
  // Opened from a search, a place keeps the result's name over the geocoder's.
  const searched = withOpened([], { lat: 48, lng: 16 }, 3, 'Baker Street 221B, London')
  assert.equal(withAnswer(searched, { ...answer(48, 16), place: 'Marylebone' })[0].place, 'Baker Street 221B, London')
  // Places kept from before the history kept times keep their order through an answer.
  const old = [1, 2, 3].map(lat => ({ ...answer(lat, 0), openedAt: 0 }))
  assert.deepEqual(lats(withAnswer(old, answer(3, 0, 9))), [1, 2, 3])
})

test('points 30 m apart are separate places, one entry a place where first opened; at most fifty, the one opened first goes', () => {
  assert.equal(samePlace({ lat: 50, lng: 14 }, { lat: 50.0003, lng: 14 }), false)
  assert.equal(samePlace({ lat: 50, lng: 14 }, { lat: 50.0002, lng: 14 }), true)
  // Opened between two kept 40 m apart, two places, a point opens the first in the history again,
  // which stays where it was first opened: moved, it would take in the other.
  const two = withOpened(withOpened([], { lat: 50, lng: 14 }, 1), { lat: 50.00036, lng: 14 }, 2)
  const between = withOpened(two, { lat: 50.00018, lng: 14 }, 3)
  assert.deepEqual(between.map(p => [p.lat, p.openedAt]), [[50.00036, 3], [50, 1]])
  let list = withOpened(withOpened([], { lat: 0, lng: 0 }, 1), { lat: 0, lng: 0 }, 2)
  for (let k = 1; k <= 60; k++) list = withOpened(list, { lat: k, lng: 0 }, 2 + k)
  assert.equal(list.length, RECENT_PLACES_MAX)
  // Kept: the fifty opened last, the last first.
  assert.deepEqual(lats(list).slice(0, 2), [60, 59])
  assert.equal(lats(list).at(-1), 11)
})

test('a name that comes back after the next click fills its own place only, and is kept on reopening', () => {
  const list = withOpened(withOpened([], { lat: 50, lng: 14 }, 1), { lat: 49, lng: 15 }, 2)
  const named = withName(list, { lat: 50.0001, lng: 14 }, 'Bedřichov')
  assert.deepEqual(named.map(p => p.place), [null, 'Bedřichov'])
  assert.deepEqual(withName(named, { lat: 50, lng: 14 }, 'Elsewhere').map(p => p.place), [null, 'Bedřichov'])
  assert.equal(withOpened(named, { lat: 50, lng: 14 }, 3)[0].place, 'Bedřichov')
})

test('places a browser kept before the history kept times read as opened at no known time, a count dropped', (t) => {
  const items = new Map([['quietmap.recent-places', JSON.stringify([{ lat: 50, lng: 14, place: 'Kytín', sone: 0.4, lden: 33, count: 3 }])]])
  globalThis.localStorage = { getItem: key => items.get(key) ?? null }
  t.after(() => delete globalThis.localStorage)
  assert.deepEqual(loadRecentPlaces(), [{ lat: 50, lng: 14, place: 'Kytín', sone: 0.4, lden: 33, openedAt: 0 }])
  // A list kept under the order by how often reads in the history's order.
  items.set('quietmap.recent-places', JSON.stringify([{ ...answer(1, 0), openedAt: 100 }, { ...answer(2, 0), openedAt: 300 }, { ...answer(3, 0), openedAt: 200 }]))
  assert.deepEqual(lats(loadRecentPlaces()), [2, 3, 1])
})

test("a forgotten place leaves the others in the history's order", () => {
  const list = [{ ...answer(1, 0), openedAt: 300 }, { ...answer(3, 0), openedAt: 200 }, { ...answer(2, 0), openedAt: 100 }]
  assert.deepEqual(lats(withoutPlace(list, { lat: 1, lng: 0 })), [3, 2])
})

test('a name stays where it was given: the nearest within 30 m answers, two spots 40 m apart keep two', () => {
  const door = { lat: 50, lng: 14 }
  const gate = { lat: 50.00036, lng: 14 }
  let titles = withTitle(withTitle([], door, 'Door', 1), gate, 'Gate', 2)
  // Between them, 20 m from each: the nearest is renamed, the other kept.
  titles = withTitle(titles, { lat: 50.00017, lng: 14 }, 'Front door', 3)
  assert.deepEqual(titles.map(t => t.title).sort(), ['Front door', 'Gate'])
  assert.equal(nameAt(titles, door), 'Front door')
  // Renamed again and again from 20 m north, it stays at the door.
  for (let k = 0; k < 3; k++) titles = withTitle(titles, { lat: 50.00018, lng: 14 }, `Door ${k}`, 4 + k)
  assert.equal(nameAt(titles, door), 'Door 2')
  // The history opens a named place at its name's point, the door, from wherever its entry stands.
  assert.deepEqual(titleAt(titles, { lat: 50.00018, lng: 14 }), { lat: 50, lng: 14, title: 'Door 2', updatedAt: 6 })
  // Emptied, only its own name goes.
  assert.deepEqual(withTitle(titles, door, null, 7).map(t => t.title), ['Gate'])
  assert.equal(nameAt(titles, { lat: 50.001, lng: 14 }), null)
  let many = []
  for (let k = 0; k <= PLACE_TITLES_MAX; k++) many = withTitle(many, { lat: k, lng: 0 }, `place ${k}`, k)
  assert.equal(many.length, PLACE_TITLES_MAX)
  assert.equal(nameAt(many, { lat: 0, lng: 0 }), null)
})

test('of two names for one place the later stays, on either side of the antimeridian', () => {
  const east = { lat: -16.5, lng: 179.99995 }
  const west = { lat: -16.5, lng: -179.99995 }
  const named = withTitle([], east, 'Taveuni', 20)
  assert.equal(nameAt(named, west), 'Taveuni')
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
  assert.equal(nameAt(merged, { lat: 50, lng: 14 }), 'Flat')
  assert.equal(nameAt(merged, { lat: 49, lng: 15 }), 'Cabin')
  assert.equal(nameAt(merged, { lat: 48, lng: 16 }), 'Lake')
  assert.equal(merged.length, 3)
  // The account's point stays where its name was first given.
  assert.deepEqual(merged.find(t => t.title === 'Flat'), { lat: 50, lng: 14, title: 'Flat', updatedAt: 9 })
  // With the most names kept, the newest stay: an older name for a kept place does not come back,
  // and the oldest name goes.
  const full = Array.from({ length: PLACE_TITLES_MAX }, (_, k) => ({ lat: k * 0.01, lng: 0, title: `p${k}`, updatedAt: 100 }))
  const last = full.at(-1)
  const capped = mergedTitles(full, [{ lat: 80, lng: 0, title: 'old', updatedAt: 1 }, { ...last, title: 'stale', updatedAt: 2 }])
  assert.equal(capped.length, PLACE_TITLES_MAX)
  assert.equal(nameAt(capped, last), 'p199')
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

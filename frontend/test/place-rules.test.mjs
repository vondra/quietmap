// The places' rules: the history of the places opened, the one opened last first, one entry per
// place, at most fifty, names that come back late; the names the visitor gives places, the later of
// two kept; what storage held read defensively.
import assert from 'node:assert/strict'
import test from 'node:test'

import { loadPlaceTitles, loadRecentPlaces } from '../src/lib/browser-places.ts'
import {
  PLACE_TITLES_MAX, RECENT_PLACES_MAX, bindPlaceTitles, historyOrder, historyPoint, levelsAt, mergedTitles, placeLabel, samePlace, titleAt, titlePosition, withAnswer, withName,
  withOpened, withTitleAt, withoutPlace,
} from '../src/lib/place-rules.ts'

const answer = (lat, lng, sone = 4) => ({ lat, lng, place: null, sone, lden: 43 })
/** The places of `list`, by latitude, in their order. */
const lats = list => list.map(p => p.lat)
/** The name given the place at `at`, or null. */
const nameAt = (titles, at) => titleAt(titles, at)?.title ?? null
const withTitle = (titles, point, name, time, history = []) => {
  const {at, anchor} = titlePosition(titles, history, point)
  return withTitleAt(titles, at, name, time, anchor)
}

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
  const across = answer(50.0001, 14.0001, 8)
  assert.equal(withAnswer(list, across, []), list)
  const answered = withAnswer(list, { ...answer(50, 14, 5), place: 'Kytín' }, [])
  assert.deepEqual(lats(answered), [49, 50])
  assert.deepEqual(answered[1], { lat: 50, lng: 14, place: 'Kytín', levels: { lat: 50, lng: 14, sone: 5, lden: 43 }, openedAt: 1 })
  const titles = withTitle([], across, 'Across the road', 3, answered)
  assert.equal(levelsAt(answered[1], across), null)
  const named = withAnswer(answered, across, titles)
  assert.equal(levelsAt(named[1], across).sone, 8)
  assert.equal(withAnswer(named, answer(50, 14, 5), titles), named)
  // A silent answer replaces the numbers; the name known stays, a search's or the geocoder's.
  const silent = withAnswer(answered, { lat: 50, lng: 14, place: 'Elsewhere', sone: null, lden: null }, [])
  assert.deepEqual([silent[1].place, silent[1].levels.sone], ['Kytín', null])
  assert.equal(withAnswer(answered, { ...answer(50, 14, 5), place: 'Kytín' }, []), answered)
  assert.equal(withAnswer(answered, answer(48, 16), []), answered)
  // Opened from a search, a place keeps the result's name over the geocoder's.
  const searched = withOpened([], { lat: 48, lng: 16 }, 3, 'Baker Street 221B, London')
  assert.equal(withAnswer(searched, { ...answer(48, 16), place: 'Marylebone' }, [])[0].place, 'Baker Street 221B, London')
  // Places kept from before the history kept times keep their order through an answer.
  const old = [1, 2, 3].map(lat => ({ ...answer(lat, 0), openedAt: 0 }))
  assert.deepEqual(lats(withAnswer(old, answer(3, 0, 9), [])), [1, 2, 3])
})

test('a name between two history positions has one owner however the account orders the rows', () => {
  const a = { lat: -0.0002, lng: 14 }, b = { lat: 0.0002, lng: 14 }, midway = answer(0, 14, 6)
  const entries = withOpened(withOpened([], a, 1), b, 2)
  const titles = withTitleAt([], midway, 'Middle', 3)
  for (const list of [entries, [...entries].reverse()]) {
    const updated = withAnswer(list, midway, titles)
    const targets = historyOrder(updated).map(p => historyPoint(updated, titles, p))
    assert.deepEqual(targets.map(p => p.lat), [b.lat, midway.lat])
    assert.deepEqual(historyOrder(updated).map(p => levelsAt(p, midway)?.sone), [undefined, 6])
    assert.equal(withOpened(updated, midway, 4)[0].lat, a.lat)
    assert.equal(titleAt(titles, b, list), null)
    const namedBoth = withTitleAt(titles, b, 'Northern receiver', 5)
    assert.deepEqual(mergedTitles([], namedBoth, list), bindPlaceTitles(namedBoth, list))
    assert.deepEqual(mergedTitles(titles, namedBoth), namedBoth)
  }
})

test('points 30 m apart are separate places, one entry a place where first opened; at most fifty, the one opened first goes', () => {
  assert.equal(samePlace({ lat: 50, lng: 14 }, { lat: 50.0003, lng: 14 }), false)
  assert.equal(samePlace({ lat: 50, lng: 14 }, { lat: 50.0002, lng: 14 }), true)
  // An intervening click reopens the closer row and leaves its original position intact.
  const two = withOpened(withOpened([], { lat: 50, lng: 14 }, 1), { lat: 50.00036, lng: 14 }, 2)
  const between = withOpened(two, { lat: 50.00019, lng: 14 }, 3)
  assert.deepEqual(between.map(p => [p.lat, p.openedAt]), [[50.00036, 3], [50, 1]])
  let list = withOpened(withOpened([], { lat: 0, lng: 0 }, 1), { lat: 0, lng: 0 }, 2)
  for (let k = 1; k <= 60; k++) list = withOpened(list, { lat: k, lng: 0 }, 2 + k)
  assert.equal(list.length, RECENT_PLACES_MAX)
  // Kept: the fifty opened last, the last first.
  assert.deepEqual(lats(list).slice(0, 2), [60, 59])
  assert.equal(lats(list).at(-1), 11)
})

test('adding or forgetting another history row cannot move a saved receiver and its result', () => {
  const a = {lat: 50.00505, lng: 21.959486}, b = {lat: 50.0054, lng: 21.959947}
  const c = {lat: 50.005264, lng: 21.959733}
  const entries = withOpened(withOpened([], a, 1), c, 2)
  const titles = withTitle([], c, 'P25', 3, entries)
  const named = withAnswer(entries, {...c, place: null, sone: 10, lden: 70.1}, titles)
  const three = withAnswer(withOpened(named, b, 4, null, titles), {...b, place: null, sone: 7, lden: 66}, titles)
  const view = (rows, names) => rows.map(row => {
    const target = historyPoint(rows, names, row)
    return [target.lat, levelsAt(row, target)?.lden, target.title]
  })
  assert.deepEqual(view(three,titles), [[b.lat,66,undefined],[c.lat,70.1,'P25']])
  const forgotten = withoutPlace(three, three[1])
  assert.deepEqual(view(forgotten,titles), [[b.lat,66,undefined]])
  assert.equal(withOpened(forgotten,c,5,null,titles)[0].lat, a.lat)
  const legacy = bindPlaceTitles([{...c,title:'P25',updatedAt:3}], entries)
  assert.deepEqual(view(three,legacy),view(three,titles))
})

test('two doors on opposite sides of a first click can each keep a name, including after forgetting a row', () => {
  const point = metres => ({ lat: metres / 111320, lng: 14 })
  const first = point(0), east = point(20), west = point(-20)
  let rows = withOpened([], first, 1)
  rows = withOpened(rows, east, 2)
  let names = withTitle([], east, 'East door', 3, rows)
  rows = withOpened(rows, west, 4, null, names)
  names = withTitle(names, west, 'West door', 5, rows)
  assert.deepEqual(rows.map(row => historyPoint(rows, names, row).title), ['West door', 'East door'])
  rows = withoutPlace(rows, rows[1])
  assert.equal(rows.length, 1)
  rows = withOpened(rows, east, 6, null, names)
  assert.deepEqual(rows.map(row => historyPoint(rows, names, row).title), ['East door', 'West door'])
  assert.deepEqual(names.map(t => [t.title, t.lat]), [['West door', west.lat], ['East door', east.lat]])
})

test('imported names follow corresponding retained account rows even when the first clicks differ', () => {
  const point = metres => ({ lat: metres / 111320, lng: 14 })
  const rows = withOpened(withOpened([], point(2), 1), point(43), 2)
  const incoming = [{...point(0),title:'Door',updatedAt:1,anchor:point(0)},
                    {...point(22),title:'Gate',updatedAt:2,anchor:point(41)}]
  const names = mergedTitles([], incoming, rows)
  assert.deepEqual(rows.map(row => historyPoint(rows, names, row).title), ['Gate','Door'])
  const onlyGate = rows.slice(0, 1)
  const both = mergedTitles(names.filter(n => n.title === 'Gate'), [incoming[0]], onlyGate)
  assert.equal(both.length, 2)
  assert.equal(historyPoint(onlyGate, both, onlyGate[0]).title, 'Gate')
})

test('import matches the named receiver even if two devices first opened it from opposite doors', () => {
  const a = {lat:50.00505,lng:21.959486}, b = {lat:50.0054,lng:21.959947}
  const mic = {lat:50.005264,lng:21.959733}, phone = {lat:50.0052568,lng:21.9597247}
  const rows = withOpened(withOpened([], a, 1), b, 2)
  const neighbour = {...a,title:'Neighbour',anchor:a,updatedAt:1}
  const account = [neighbour,{...mic,title:'Mic on desktop',anchor:b,updatedAt:2}]
  const label = {...phone,title:'Mic on phone',anchor:a,updatedAt:3}
  const names = mergedTitles(account, [label], rows)
  assert.equal(names.length, 2)
  assert.deepEqual(names.find(n => n.title === 'Neighbour'),neighbour)
  assert.deepEqual(names.find(n => n.title === 'Mic on phone'),{...mic,title:'Mic on phone',anchor:b,updatedAt:3})
  // Without the other phone's corresponding named receiver, the independent door outlives its row.
  assert.equal(mergedTitles([neighbour], [{...mic,title:'Mic',anchor:b,updatedAt:3}], []).length, 2)
})

test('an earlier name imported near an occupied row keeps the later account name and both distinct first clicks', () => {
  const point = metres => ({lat:metres/111320,lng:14})
  const saved = (at,anchor,title,updatedAt) => ({...point(at),anchor:point(anchor),title,updatedAt})
  const rows = [0,20].map(at => ({...point(at),place:null,levels:null,openedAt:1}))
  const account = [saved(-4,0,'Door on account',3),saved(39,20,'Gate',1)]
  const names = mergedTitles(account,[saved(0,16,'Earlier door',2)],rows)
  assert.deepEqual(rows.map(row=>historyPoint(rows,names,row).title),['Door on account','Gate'])
  assert.equal(names.length,2)
  assert.deepEqual(bindPlaceTitles([saved(20,0,'East',1),saved(-20,-20,'West',2)].map(({anchor,...t})=>t),
    [point(0),point(-20)]).map(t=>t.anchor),[point(0),point(-20)])
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
  assert.deepEqual(loadRecentPlaces(), [{ lat: 50, lng: 14, place: 'Kytín', levels: null, openedAt: 0 }])
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
  assert.deepEqual(titleAt(titles, { lat: 50.00018, lng: 14 }), { ...door, anchor: door, title: 'Door 2', updatedAt: 6 })
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
  assert.deepEqual(withTitle(named, west, 'New', 30), [{ ...east, anchor: east, title: 'New', updatedAt: 30 }])
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

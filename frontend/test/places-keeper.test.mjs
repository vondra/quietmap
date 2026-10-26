// Where the places live, against a fake server and a fake browser storage: a change made before the
// server answers is held, then kept by whoever keeps the places; a read of the account keeps the
// changes not yet sent; a sign-out comes after the changes, and nothing brings the account back; a
// page still showing an account another tab replaced drops its change and shows the new account; a
// map not reached at the start leaves the places this browser's; a place opened while another change
// was on its way counts once.
import assert from 'node:assert/strict'
import test from 'node:test'

import { withTitle } from '../src/lib/place-rules.ts'
import { createPlacesKeeper } from '../src/lib/places-keeper.ts'

const HOME = { lat: 50, lng: 14, place: 'Home', sone: 2, lden: 44, count: 1, openedAt: 1 }
const CAFE = { lat: 49, lng: 15, place: 'Cafe', sone: 9, lden: 60, count: 1, openedAt: 2 }
const NEW = { lat: 48, lng: 16, place: 'New', sone: 1, lden: 38 }

/** This browser's storage, as localStorage. */
function browserStorage(t) {
  const items = new Map()
  globalThis.localStorage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => void items.set(key, String(value)) }
  t.after(() => delete globalThis.localStorage)
  return {
    titles: () => JSON.parse(items.get('quietmap.place-titles') ?? '[]').map(title => title.title),
    recent: () => JSON.parse(items.get('quietmap.recent-places') ?? '[]').map(place => place.place),
  }
}

/** The map's server: accounts by address, the address this browser is signed in as, and a gate
 *  that holds the answers to /api/me until opened. */
function server(t, signedIn) {
  const state = { signedIn, accounts: { a: { titles: [], recent: [HOME] }, b: { titles: [], recent: [] } }, requests: [], reachable: true }
  let open = () => {}
  let openTitles = () => {}
  state.gate = Promise.resolve()
  state.titleGate = Promise.resolve()
  state.hold = () => { state.gate = new Promise(resolve => { open = resolve }) }
  state.open = () => open()
  state.holdTitles = () => { state.titleGate = new Promise(resolve => { openTitles = resolve }) }
  state.openTitles = () => openTitles()
  globalThis.fetch = async (url, init = {}) => {
    const method = init.method ?? 'GET'
    state.requests.push(`${method} ${url}`)
    if (url === '/api/me' && method === 'GET') await state.gate
    if (!state.reachable) throw new TypeError('fetch failed')
    const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
    const account = state.accounts[state.signedIn]
    if (url === '/api/auth/signout') {
      state.signedIn = null
      return new Response(null, { status: 204 })
    }
    if (!account) return json({ error: 'Not signed in' }, 401)
    if (url === '/api/me' && method === 'GET') return json({ email: state.signedIn, ...account })
    if (init.headers['X-Quietmap-Account'] !== state.signedIn) return json({ error: 'Signed in as another account' }, 409)
    const body = JSON.parse(init.body)
    if (url === '/api/places/title') {
      await state.titleGate
      account.titles = withTitle(account.titles, body, body.title, body.updatedAt)
      return json({ titles: account.titles })
    }
    if (url === '/api/places/recent') {
      account.recent = body.recent
      return new Response(null, { status: 204 })
    }
    throw new Error(`unexpected ${method} ${url}`)
  }
  t.after(() => delete globalThis.fetch)
  return state
}

const settled = keeper => keeper.refresh()

test('a change made before the server answers is held, then kept by whoever keeps the places', async (t) => {
  for (const signedIn of ['a', null]) {
    const storage = browserStorage(t)
    const state = server(t, signedIn)
    state.hold()
    const keeper = createPlacesKeeper()
    const read = keeper.refresh()
    keeper.rename({ lat: 50, lng: 14 }, 'Cabin')
    assert.deepEqual(keeper.shown().titles.map(title => title.title), ['Cabin'])
    assert.deepEqual(storage.titles(), [])
    state.open()
    await read
    await settled(keeper)
    assert.deepEqual(keeper.shown().titles.map(title => title.title), ['Cabin'])
    if (signedIn) {
      assert.deepEqual(state.accounts.a.titles.map(title => title.title), ['Cabin'])
      assert.deepEqual(storage.titles(), [])
    } else {
      assert.deepEqual(storage.titles(), ['Cabin'])
    }
  }
})

test("a read of the account keeps the changes not yet sent, and another device's place", async (t) => {
  browserStorage(t)
  const state = server(t, 'a')
  const keeper = createPlacesKeeper()
  await keeper.refresh()
  // Another device opened the café; this one opens a new place while the account is read again.
  state.accounts.a.recent = [CAFE, HOME]
  const read = keeper.refresh()
  keeper.open(NEW, NEW.place)
  await read
  await settled(keeper)
  assert.deepEqual(keeper.shown().recent.map(place => place.place), ['New', 'Cafe', 'Home'])
  assert.deepEqual(state.accounts.a.recent.map(place => place.place), ['New', 'Cafe', 'Home'])
})

test('a sign-out comes after the changes made, and no read brings the account back', async (t) => {
  const storage = browserStorage(t)
  const state = server(t, 'a')
  const keeper = createPlacesKeeper()
  await keeper.refresh()
  keeper.open(NEW, NEW.place)
  const read = keeper.refresh()
  assert.equal(await keeper.signOut(), true)
  await read
  await settled(keeper)
  assert.deepEqual(state.accounts.a.recent.map(place => place.place), ['New', 'Home'])
  assert.equal(keeper.shown().account, null)
  assert.deepEqual(keeper.shown().recent, [])
  assert.deepEqual(storage.recent(), [])
})

test('a page still showing an account another tab replaced drops its change and shows the new account', async (t) => {
  browserStorage(t)
  const state = server(t, 'a')
  const keeper = createPlacesKeeper()
  await keeper.refresh()
  state.signedIn = 'b'
  keeper.rename({ lat: 50, lng: 14 }, 'Home')
  await settled(keeper)
  await settled(keeper)
  assert.deepEqual(keeper.shown().account, { email: 'b' })
  assert.deepEqual(keeper.shown().titles, [])
  assert.deepEqual(state.accounts.b.titles, [])
  assert.deepEqual(state.accounts.a.titles, [])
})

test("a map not reached at the start leaves the places this browser's", async (t) => {
  const storage = browserStorage(t)
  const state = server(t, 'a')
  state.reachable = false
  const keeper = createPlacesKeeper()
  await keeper.refresh()
  keeper.open(NEW, NEW.place)
  assert.equal(keeper.shown().account, null)
  assert.deepEqual(storage.recent(), ['New'])
  assert.equal(state.requests.some(request => request.startsWith('PUT')), false)
})

test('a place opened while a name was on its way, sent with the list, counts once', async (t) => {
  browserStorage(t)
  const state = server(t, 'a')
  state.hold()
  const keeper = createPlacesKeeper()
  const read = keeper.refresh()
  keeper.open(CAFE, 'Cafe')
  keeper.rename(CAFE, 'Corner')
  state.holdTitles()
  state.open()
  await read
  while (!state.requests.includes('PUT /api/places/title')) await new Promise(resolve => setTimeout(resolve, 1))
  // The name is on its way; a read of the account waits behind it, and another place opens meanwhile:
  // the list sent after the name carries it, and the read must not count it again.
  const again = keeper.refresh()
  keeper.open(NEW, 'New')
  state.openTitles()
  await again
  await settled(keeper)
  const counts = list => list.map(place => [place.place, place.count])
  assert.deepEqual(counts(state.accounts.a.recent), [['New', 1], ['Cafe', 1], ['Home', 1]])
  assert.deepEqual(counts(keeper.shown().recent), [['New', 1], ['Cafe', 1], ['Home', 1]])
})

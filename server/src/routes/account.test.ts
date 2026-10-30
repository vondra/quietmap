// The sign-in and a signed-in visitor's places against a real SQLite file and a fake mail: a link
// asked only from the map's own pages for a plain address, the same answer for any address, three
// links an address an hour, fifty mails a day and ten requests a client an hour, a mail that failed
// neither working nor counting; a link checked without being used, used once within its 15 minutes;
// the session's cookie, renewed at most daily and ended after 180 days unused; a change refused for
// a page that names another account; names under the 30 m rule and the later time, recent places
// replaced one per place with their times, a database from before the times or from the counts read
// without counts, a longitude from a repeated world wrapped, a browser's places added at the visitor's word;
// addresses and sessions forgotten when they end; everything deleted with the account; what is sent
// checked; a server without accounts that answers 503, and the four values configured all or none.
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import test from 'node:test'
import Fastify from 'fastify'
import rateLimit from '@fastify/rate-limit'
import { readConfig } from '../config.ts'
import { AccountStore, HOUR_MS, LINK_MS, SESSION_MS } from '../accounts/store.ts'
import type { SendLink } from '../accounts/mail.ts'
import { MAILS_A_DAY, accountRoutes } from './account.ts'

const ORIGIN = 'https://map.test'
const DAY_MS = 86_400_000

async function server(t: test.TestContext, { limited = false, failing = () => false }: { limited?: boolean, failing?: () => boolean } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'qm-accounts-'))
  const database = join(directory, 'accounts.sqlite')
  const store = new AccountStore(database)
  const mails: { to: string, url: string }[] = []
  const sendLink: SendLink = async (to, url) => {
    if (failing()) throw new Error('Resend answered 500')
    mails.push({ to, url })
  }
  const app = Fastify()
  if (limited) await app.register(rateLimit, { global: false })
  await app.register(accountRoutes, { accounts: { store, origin: ORIGIN, sendLink } })
  t.after(async () => {
    await app.close()
    store.close()
    await rm(directory, { recursive: true, force: true })
  })
  const send = (method: 'GET' | 'POST' | 'PUT' | 'DELETE', url: string, payload?: object, headers: Record<string, string> = {}) =>
    app.inject({ method, url, payload, headers: { origin: ORIGIN, ...headers } })
  /** A signed-in visitor's headers for `email`: the session's cookie and the account the page names. */
  const signIn = async (email: string) => {
    assert.equal((await send('POST', '/api/auth/email', { email })).statusCode, 204)
    const token = new URL(mails.at(-1)!.url).hash.slice(1)
    const used = await send('POST', '/api/auth/link/use', { token })
    assert.equal(used.statusCode, 204)
    return { cookie: String(used.headers['set-cookie']).split(';')[0], 'x-quietmap-account': email }
  }
  return { mails, send, signIn, store, database }
}

test('a link is mailed only for a request from the map, checked without being used, and used once', async (t) => {
  const { mails, send } = await server(t)
  assert.equal((await send('POST', '/api/auth/email', { email: 'a@b.cz' }, { origin: 'https://evil.test' })).statusCode, 403)
  // Not an address, or one with a Cyrillic а that shows as gmail.com.
  for (const email of ['not an address', 'owner@gm\u0430il.com', 'owner\u200b@gmail.com']) {
    assert.equal((await send('POST', '/api/auth/email', { email })).statusCode, 400, email)
  }
  assert.equal(mails.length, 0)

  const asked = await send('POST', '/api/auth/email', { email: ' Ondra@Example.org ' })
  assert.equal(asked.statusCode, 204)
  assert.equal(asked.headers['cache-control'], 'no-store')
  const url = new URL(mails[0].url)
  assert.equal(mails[0].to, 'ondra@example.org')
  assert.equal(`${url.origin}${url.pathname}`, `${ORIGIN}/signin`)
  // The token is after `#`: a browser never sends it in a request line.
  const token = url.hash.slice(1)
  assert.equal(token.length, 43)

  // A mail scanner's look, or the page's, uses nothing.
  for (let k = 0; k < 2; k++) assert.deepEqual((await send('POST', '/api/auth/link', { token })).json(), { email: 'ondra@example.org' })
  assert.equal((await send('POST', '/api/auth/link/use', { token }, { origin: 'https://evil.test' })).statusCode, 403)
  const used = await send('POST', '/api/auth/link/use', { token })
  assert.equal(used.statusCode, 204)
  assert.match(String(used.headers['set-cookie']), /^__Host-qm_session=[\w-]{43}; Path=\/; Secure; HttpOnly; SameSite=Lax; Max-Age=15552000$/)
  assert.equal((await send('POST', '/api/auth/link/use', { token })).statusCode, 404)
  assert.equal((await send('POST', '/api/auth/link', { token })).statusCode, 404)

  const cookie = String(used.headers['set-cookie']).split(';')[0]
  const me = await send('GET', '/api/me', undefined, { cookie })
  assert.deepEqual(me.json(), { email: 'ondra@example.org', titles: [], recent: [] })
  assert.equal(me.headers['cache-control'], 'no-store')
  assert.equal((await send('GET', '/api/me')).statusCode, 401)
})

test('a link ends 15 minutes after it is mailed; an address gets three an hour, the server fifty a day', async (t) => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.parse('2026-10-25T12:00:00Z') })
  const { mails, send } = await server(t)
  await send('POST', '/api/auth/email', { email: 'late@example.org' })
  t.mock.timers.tick(LINK_MS)
  assert.equal((await send('POST', '/api/auth/link/use', { token: new URL(mails[0].url).hash.slice(1) })).statusCode, 404)

  // Three links of the hour mailed; the fourth is answered alike, and not sent.
  for (let k = 0; k < 3; k++) assert.equal((await send('POST', '/api/auth/email', { email: 'many@example.org' })).statusCode, 204)
  assert.equal(mails.filter(mail => mail.to === 'many@example.org').length, 3)
  t.mock.timers.tick(3_600_000)
  await send('POST', '/api/auth/email', { email: 'many@example.org' })
  assert.equal(mails.filter(mail => mail.to === 'many@example.org').length, 4)

  for (let k = mails.length; k < MAILS_A_DAY; k++) await send('POST', '/api/auth/email', { email: `visitor${k}@example.org` })
  assert.equal(mails.length, MAILS_A_DAY)
  const refused = await send('POST', '/api/auth/email', { email: 'one-more@example.org' })
  assert.equal(refused.statusCode, 503)
  assert.ok(Number(refused.headers['retry-after']) > 0)
  t.mock.timers.tick(DAY_MS)
  assert.equal((await send('POST', '/api/auth/email', { email: 'one-more@example.org' })).statusCode, 204)
  assert.equal(mails.length, MAILS_A_DAY + 1)
})

test('a session is renewed at most daily, its cookie sent anew, and ends 180 days unused or when signed out', async (t) => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.parse('2026-10-25T12:00:00Z') })
  const { send, signIn } = await server(t)
  const headers = await signIn('owner@example.org')
  assert.equal((await send('GET', '/api/me', undefined, headers)).headers['set-cookie'], undefined)
  t.mock.timers.tick(DAY_MS + 1)
  assert.match(String((await send('GET', '/api/me', undefined, headers)).headers['set-cookie']), /Max-Age=15552000$/)
  t.mock.timers.tick(SESSION_MS - 1)
  assert.equal((await send('GET', '/api/me', undefined, headers)).statusCode, 200)
  t.mock.timers.tick(SESSION_MS)
  assert.equal((await send('GET', '/api/me', undefined, headers)).statusCode, 401)

  const again = await signIn('owner@example.org')
  const out = await send('POST', '/api/auth/signout', {}, again)
  assert.equal(out.statusCode, 204)
  assert.match(String(out.headers['set-cookie']), /^__Host-qm_session=; .*Max-Age=0$/)
  assert.equal((await send('GET', '/api/me', undefined, again)).statusCode, 401)
})

test("an account's names follow the 30 m rule and the later time; its recent places are replaced", async (t) => {
  const { send, signIn } = await server(t)
  const headers = await signIn('owner@example.org')
  const name = (lat: number, title: string | null, updatedAt: number) =>
    send('PUT', '/api/places/title', { lat, lng: 14, title, updatedAt }, headers)
  await name(50, 'Door', 10)
  // 20 m north and later: the same place renamed, kept at the door.
  assert.deepEqual((await name(50.00018, 'Front door', 20)).json(), { titles: [{ lat: 50, lng: 14, title: 'Front door', updatedAt: 20 }] })
  // Another device's older rename, sent late, changes nothing; a time from the future is the server's now.
  assert.deepEqual((await name(50, 'Old', 15)).json().titles.map((t: { title: string }) => t.title), ['Front door'])
  const future = (await name(50, 'Home', Date.now() + DAY_MS)).json().titles[0]
  assert.ok(future.updatedAt <= Date.now())
  assert.deepEqual((await name(50, null, Date.now())).json(), { titles: [] })

  const recent = [
    { lat: 1, lng: 2, place: 'Tijarafe, Spain', sone: 0.3, lden: 31, openedAt: 20 },
    { lat: 3, lng: 4, place: null, sone: null, lden: null, openedAt: 10 },
  ]
  assert.equal((await send('PUT', '/api/places/recent', { recent }, headers)).statusCode, 204)
  assert.deepEqual((await send('GET', '/api/me', undefined, headers)).json().recent, recent)
  // A list kept under the order by how often reads in the history's order.
  await send('PUT', '/api/places/recent', { recent: [...recent].reverse() }, headers)
  assert.deepEqual((await send('GET', '/api/me', undefined, headers)).json().recent, recent)
  // A time from the future is the server's now; a count a browser still sends is not kept.
  await send('PUT', '/api/places/recent', { recent: [{ lat: 1, lng: 2, count: 3, openedAt: Date.now() + DAY_MS }] }, headers)
  const [kept] = (await send('GET', '/api/me', undefined, headers)).json().recent
  assert.equal(kept.count, undefined)
  assert.ok(kept.openedAt <= Date.now())
  // Another account sees none of it.
  assert.deepEqual((await send('GET', '/api/me', undefined, await signIn('other@example.org'))).json(), { email: 'other@example.org', titles: [], recent: [] })
})

test("a browser's places join the account at the visitor's word: names merged, recent places only into none", async (t) => {
  const { send, signIn } = await server(t)
  const headers = await signIn('owner@example.org')
  await send('PUT', '/api/places/title', { lat: 50, lng: 14, title: 'Home', updatedAt: 5 }, headers)
  const browser = {
    titles: [{ lat: 50.0001, lng: 14, title: 'Flat', updatedAt: 9 }, { lat: 48, lng: 16, title: 'Lake' }],
    recent: [{ lat: 48, lng: 16, place: null, sone: 1.1, lden: 38, openedAt: 7 }],
  }
  const merged = (await send('POST', '/api/places/import', browser, headers)).json()
  assert.deepEqual(merged.titles, [{ lat: 50, lng: 14, title: 'Flat', updatedAt: 9 }, { lat: 48, lng: 16, title: 'Lake', updatedAt: 0 }])
  assert.deepEqual(merged.recent, browser.recent)
  const second = (await send('POST', '/api/places/import', { titles: [], recent: [{ lat: 1, lng: 1 }] }, headers)).json()
  assert.deepEqual(second.recent, browser.recent)
})

test('deleting the account deletes its sessions, its places and its links not yet used, overwritten in the file', async (t) => {
  const { mails, send, signIn, database } = await server(t)
  const headers = await signIn('owner@example.org')
  await send('PUT', '/api/places/title', { lat: 50, lng: 14, title: 'Home', updatedAt: 5 }, headers)
  await send('POST', '/api/auth/email', { email: 'owner@example.org' })
  const pending = new URL(mails.at(-1)!.url).hash.slice(1)
  assert.equal((await send('DELETE', '/api/me', undefined, { ...headers, origin: 'https://evil.test' })).statusCode, 403)
  const deleted = await send('DELETE', '/api/me', undefined, headers)
  assert.equal(deleted.statusCode, 204)
  assert.match(String(deleted.headers['set-cookie']), /Max-Age=0$/)
  assert.equal((await send('GET', '/api/me', undefined, headers)).statusCode, 401)
  assert.equal((await send('POST', '/api/auth/link/use', { token: pending })).statusCode, 404)
  for (const file of [database, `${database}-wal`, `${database}-journal`]) {
    assert.equal((await readFile(file).catch(() => Buffer.alloc(0))).includes('owner@example.org'), false, file)
  }
  assert.deepEqual((await send('GET', '/api/me', undefined, await signIn('owner@example.org'))).json().titles, [])
})

test('a mail that could not be sent neither works nor counts against the address', async (t) => {
  let failing = true
  const { mails, send } = await server(t, { failing: () => failing })
  for (let k = 0; k < 3; k++) assert.equal((await send('POST', '/api/auth/email', { email: 'owner@example.org' })).statusCode, 502)
  failing = false
  for (let k = 0; k < 3; k++) assert.equal((await send('POST', '/api/auth/email', { email: 'owner@example.org' })).statusCode, 204)
  assert.equal(mails.length, 3)
})

test('a client asks for ten links an hour', async (t) => {
  const { send } = await server(t, { limited: true })
  for (let k = 0; k < 10; k++) assert.equal((await send('POST', '/api/auth/email', { email: `v${k}@example.org` })).statusCode, 204)
  assert.equal((await send('POST', '/api/auth/email', { email: 'v10@example.org' })).statusCode, 429)
})

test('a change from a page that names another account is refused, as another tab signed in anew', async (t) => {
  const { send, signIn } = await server(t)
  const first = await signIn('first@example.org')
  const second = await signIn('second@example.org')
  // The first page, still showing its account, with the browser's cookie now the second's.
  const stale = { ...second, 'x-quietmap-account': first['x-quietmap-account'] }
  assert.equal((await send('PUT', '/api/places/title', { lat: 50, lng: 14, title: 'Home', updatedAt: 5 }, stale)).statusCode, 409)
  assert.equal((await send('DELETE', '/api/me', undefined, stale)).statusCode, 409)
  assert.equal((await send('PUT', '/api/places/title', { lat: 50, lng: 14, title: 'Home', updatedAt: 5 })).statusCode, 401)
  assert.deepEqual((await send('GET', '/api/me', undefined, second)).json().titles, [])
})

test('a longitude from a repeated world is wrapped, and the recent places are one a place', async (t) => {
  const { send, signIn } = await server(t)
  const headers = await signIn('owner@example.org')
  const named = (await send('PUT', '/api/places/title', { lat: -16.5, lng: 181, title: 'Taveuni', updatedAt: 5 }, headers)).json()
  assert.deepEqual(named.titles, [{ lat: -16.5, lng: -179, title: 'Taveuni', updatedAt: 5 }])
  const twice = [{ lat: 50, lng: 14, place: 'A' }, { lat: 50.0001, lng: 14, place: 'B' }, { lat: 49, lng: 374 }]
  assert.equal((await send('PUT', '/api/places/recent', { recent: twice }, headers)).statusCode, 204)
  assert.deepEqual((await send('GET', '/api/me', undefined, headers)).json().recent, [
    { lat: 50, lng: 14, place: 'A', sone: null, lden: null, openedAt: 0 },
    { lat: 49, lng: 14, place: null, sone: null, lden: null, openedAt: 0 },
  ])
})

test('a database from before the recent places had times, half migrated, or from when they were counted, reads them without counts', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'qm-accounts-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  for (const columns of [[], ['count INTEGER NOT NULL DEFAULT 1'], ['count INTEGER NOT NULL DEFAULT 1', 'opened_at INTEGER NOT NULL DEFAULT 0']]) {
    const database = join(directory, `accounts-${columns.length}.sqlite`)
    const old = new DatabaseSync(database)
    old.exec(`CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL);
      CREATE TABLE recent_places (user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
        position INTEGER NOT NULL, lat REAL NOT NULL, lng REAL NOT NULL, place TEXT, sone REAL, lden REAL,
        PRIMARY KEY (user_id, position));
      INSERT INTO users VALUES (1, 'owner@example.org', 0);
      INSERT INTO recent_places VALUES (1, 0, 50, 14, 'Kytín', 0.4, 33);`)
    for (const column of columns) old.exec(`ALTER TABLE recent_places ADD COLUMN ${column}`)
    old.close()
    const store = new AccountStore(database)
    // Rows come with no prototype: compared as plain objects.
    assert.deepEqual(store.places(1).recent.map(place => ({ ...place })), [{ lat: 50, lng: 14, place: 'Kytín', sone: 0.4, lden: 33, openedAt: 0 }])
    store.close()
    const kept = new DatabaseSync(database)
    assert.deepEqual(kept.prepare('PRAGMA table_info(recent_places)').all().map(column => column.name),
      ['user_id', 'position', 'lat', 'lng', 'place', 'sone', 'lden', 'opened_at'])
    kept.close()
  }
})

test('an address that asked for a link is forgotten after the hour, a session once it ends', async (t) => {
  t.mock.timers.enable({ apis: ['Date'], now: Date.parse('2026-10-25T12:00:00Z') })
  const { send, signIn, store, database } = await server(t)
  await signIn('owner@example.org')
  await send('POST', '/api/auth/email', { email: 'never@example.org' })
  const rows = () => {
    const db = new DatabaseSync(database, { readOnly: true })
    const counts = [db.prepare('SELECT count(*) AS n FROM login_links').get(), db.prepare('SELECT count(*) AS n FROM sessions').get()]
    db.close()
    return counts.map(row => (row as { n: number }).n)
  }
  store.forgetEnded(Date.now() + HOUR_MS - 1)
  assert.deepEqual(rows(), [1, 1])
  store.forgetEnded(Date.now() + HOUR_MS)
  assert.deepEqual(rows(), [0, 1])
  store.forgetEnded(Date.now() + SESSION_MS)
  assert.deepEqual(rows(), [0, 0])
})

test('what is sent is checked: coordinates, names, the most places, and a change without the session', async (t) => {
  const { send, signIn } = await server(t)
  const headers = await signIn('owner@example.org')
  for (const sent of [{ lat: 91, lng: 0, title: 'x' }, { lat: 0, lng: 0, title: 'x'.repeat(81) }, { lat: 0, lng: 0, title: '  ' }, { lat: 0, lng: 0, title: 5 }]) {
    assert.equal((await send('PUT', '/api/places/title', sent, headers)).statusCode, 400, JSON.stringify(sent))
  }
  const tooMany = Array.from({ length: 51 }, (_, k) => ({ lat: k, lng: 0 }))
  assert.equal((await send('PUT', '/api/places/recent', { recent: tooMany }, headers)).statusCode, 400)
  assert.equal((await send('PUT', '/api/places/recent', { recent: 'all' }, headers)).statusCode, 400)
  assert.equal((await send('POST', '/api/places/import', { titles: Array(201).fill({ lat: 0, lng: 0, title: 'x' }), recent: [] }, headers)).statusCode, 400)
  assert.equal((await send('PUT', '/api/places/title', { lat: 0, lng: 0, title: 'x' })).statusCode, 401)
})

test('a server without the accounts answers every sign-in route 503', async (t) => {
  const app = Fastify()
  await app.register(accountRoutes, { accounts: null })
  t.after(() => app.close())
  for (const [method, url] of [['POST', '/api/auth/email'], ['POST', '/api/auth/link'], ['GET', '/api/me'], ['PUT', '/api/places/title']] as const) {
    assert.equal((await app.inject({ method, url, payload: {}, headers: { origin: ORIGIN } })).statusCode, 503, url)
  }
})

test('the sign-in is its four values or none, its origin an https origin and its key file readable', async (t) => {
  const prepared = await mkdtemp(join(tmpdir(), 'qm-accounts-config-'))
  t.after(() => rm(prepared, { recursive: true, force: true }))
  await mkdir(join(prepared, '2026'))
  await writeFile(join(prepared, 'key'), 'secret')
  const env = {
    PORT: '8000',
    QM_POPUP_BIN: process.execPath,
    QM_RASTER_BIN: process.execPath,
    QM_PREPARED_DIR: prepared,
    QM_YEARS: '2026',
    QM_TILES_DIR: prepared,
  }
  const accounts = {
    QM_ACCOUNTS_DB: join(prepared, 'accounts.sqlite'),
    QM_PUBLIC_ORIGIN: 'https://quietmap.org',
    QM_MAIL_KEY_FILE: join(prepared, 'key'),
    QM_MAIL_FROM: 'Quiet Map <sign-in@quietmap.org>',
  }
  assert.equal(readConfig(env).accounts, null)
  assert.deepEqual(readConfig({ ...env, ...accounts }).accounts, {
    database: accounts.QM_ACCOUNTS_DB, origin: 'https://quietmap.org', mailKeyFile: accounts.QM_MAIL_KEY_FILE, mailFrom: accounts.QM_MAIL_FROM,
  })
  assert.throws(() => readConfig({ ...env, ...accounts, QM_MAIL_FROM: '' }), /QM_MAIL_FROM is not set/)
  for (const origin of ['https://quietmap.org/', 'http://quietmap.org', 'quietmap.org']) {
    assert.throws(() => readConfig({ ...env, ...accounts, QM_PUBLIC_ORIGIN: origin }), /QM_PUBLIC_ORIGIN/, origin)
  }
  assert.throws(() => readConfig({ ...env, ...accounts, QM_MAIL_KEY_FILE: join(prepared, 'missing') }), /ENOENT/)
})

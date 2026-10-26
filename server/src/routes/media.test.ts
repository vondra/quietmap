// A signed-in visitor's recordings and photos against a real SQLite file and media directory: one
// uploaded under its id, listed within 30 m of its place, read whole and in ranges, deleted with its
// file; a retry of the same bytes answered with the item kept, other bytes under its id refused;
// another account's items out of reach; an upload refused before its body is read when signed out,
// from another site or for another account; a wrong body, type, point, time or id refused, a
// half-minute recording at 48 kHz taken; a photo kept without its EXIF; an account's 200 items and
// the disk's free space; everything deleted with the account.
import assert from 'node:assert/strict'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import test from 'node:test'
import Fastify from 'fastify'
import compress from '@fastify/compress'
import { pcm16, wavHeader } from '../../../frontend/src/lib/wav.ts'
import { AccountStore, MEDIA_ITEMS_MAX } from '../accounts/store.ts'
import { accountRoutes } from './account.ts'
import { mediaRoutes } from './media.ts'

const ORIGIN = 'https://map.test'
const PLACE = { lat: 51.5074, lng: -0.1278 }
const TAKEN_AT = Date.parse('2026-10-25T18:00:00Z')

const wav = (seconds: number, level = 0.25, rate = 8_000) => {
  const pcm = pcm16(new Float32Array(seconds * rate).fill(level))
  return Buffer.concat([Buffer.from(wavHeader(pcm.byteLength, rate)), Buffer.from(pcm.buffer)])
}
const PHOTO = readFileSync(new URL('../media/photo-with-gps.jpg', import.meta.url))
const id = (k: number) => k.toString(16).padStart(32, '0')
const about = (fields: object = {}) =>
  encodeURIComponent(JSON.stringify({ ...PLACE, takenAt: TAKEN_AT, details: { microphone: 'Jiří’s iPhone Microphone' }, ...fields }))

/** The routes on a fresh database, the disk deemed to have room unless `diskFreeMinBytes` says. */
async function server(t: test.TestContext, { diskFreeMinBytes = 0 } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'qm-media-'))
  const database = join(directory, 'accounts.sqlite')
  const store = new AccountStore(database, { diskFreeMinBytes })
  const app = Fastify()
  await app.register(compress)
  const accounts = { store, origin: ORIGIN, sendLink: async () => {} }
  await app.register(accountRoutes, { accounts })
  await app.register(mediaRoutes, { accounts })
  t.after(async () => {
    await app.close()
    store.close()
    await rm(directory, { recursive: true, force: true })
  })
  /** A signed-in visitor's headers for `email`: the session's cookie and the account the page names. */
  const signIn = (email: string) => {
    const session = store.useLink(store.requestLink(email, Date.now())!, Date.now())
    return { origin: ORIGIN, cookie: `__Host-qm_session=${session}`, 'x-quietmap-account': email }
  }
  const upload = (headers: Record<string, string>, k: number, body: Buffer | Readable, type = 'audio/wav', header = about()) =>
    app.inject({ method: 'PUT', url: `/api/media/${id(k)}`, payload: body, headers: { ...headers, 'content-type': type, 'x-quietmap-media': header } })
  return { app, store, signIn, upload, media: `${database}-media` }
}

test('a recording uploaded under its id is listed at its place, read whole and in ranges, and deleted with its file', async (t) => {
  const { app, signIn, upload, media } = await server(t)
  const owner = signIn('owner@example.org')
  const recording = wav(2)
  const saved = await upload(owner, 1, recording)
  assert.equal(saved.statusCode, 200)
  const item = { id: id(1), kind: 'sound', ...PLACE, takenAt: TAKEN_AT, bytes: recording.length, durationS: 2 }
  assert.deepEqual(saved.json(), item)

  const near = (lat: number) => app.inject({ url: `/api/media?lat=${lat}&lng=${PLACE.lng}`, headers: { cookie: owner.cookie } })
  assert.deepEqual((await near(PLACE.lat + 0.0001)).json(), { items: [item] })
  assert.deepEqual((await near(PLACE.lat + 0.001)).json(), { items: [] })

  const read = (headers: Record<string, string> = {}, method: 'GET' | 'HEAD' = 'GET') =>
    app.inject({ method, url: `/api/media/${id(1)}`, headers: { cookie: owner.cookie, 'accept-encoding': 'gzip', ...headers } })
  const whole = await read()
  assert.equal(whole.statusCode, 200)
  assert.deepEqual(whole.rawPayload, recording)
  assert.equal(whole.headers['content-type'], 'audio/wav')
  assert.equal(whole.headers['content-encoding'], undefined)
  assert.equal(whole.headers['x-content-type-options'], 'nosniff')
  assert.equal(whole.headers['cache-control'], 'no-store')
  assert.equal(whole.headers['accept-ranges'], 'bytes')
  // iOS asks for two bytes before it plays.
  const probe = await read({ range: 'bytes=0-1' })
  assert.equal(probe.statusCode, 206)
  assert.equal(probe.headers['content-range'], `bytes 0-1/${recording.length}`)
  assert.equal(probe.payload, 'RI')
  const past = await read({ range: `bytes=${recording.length + 10}-` })
  assert.equal(past.statusCode, 416)
  assert.equal(past.headers['content-range'], `bytes */${recording.length}`)
  assert.equal((await read({}, 'HEAD')).headers['content-length'], String(recording.length))

  assert.equal((await app.inject({ method: 'DELETE', url: `/api/media/${id(1)}`, headers: { ...owner, origin: 'https://evil.test' } })).statusCode, 403)
  assert.equal((await app.inject({ method: 'DELETE', url: `/api/media/${id(1)}`, headers: owner })).statusCode, 204)
  assert.equal((await read()).statusCode, 404)
  assert.deepEqual(readdirSync(join(media, '1')), [])
})

test('a retry of the same bytes answers the item kept; other bytes under its id are refused, from any account', async (t) => {
  const { app, signIn, upload } = await server(t)
  const owner = signIn('owner@example.org')
  const first = await upload(owner, 7, wav(1))
  const again = await upload(owner, 7, wav(1))
  assert.equal(again.statusCode, 200)
  assert.deepEqual(again.json(), first.json())
  assert.equal((await upload(owner, 7, wav(1, 0.5))).statusCode, 409)
  assert.equal((await upload(signIn('other@example.org'), 7, wav(1))).statusCode, 409)
  const listed = await app.inject({ url: `/api/media?lat=${PLACE.lat}&lng=${PLACE.lng}`, headers: { cookie: owner.cookie } })
  assert.equal(listed.json().items.length, 1)
})

test("another account's items are neither listed, read nor deleted", async (t) => {
  const { app, signIn, upload } = await server(t)
  assert.equal((await upload(signIn('owner@example.org'), 1, wav(1))).statusCode, 200)
  const other = signIn('other@example.org')
  assert.deepEqual((await app.inject({ url: `/api/media?lat=${PLACE.lat}&lng=${PLACE.lng}`, headers: { cookie: other.cookie } })).json(), { items: [] })
  assert.equal((await app.inject({ url: `/api/media/${id(1)}`, headers: { cookie: other.cookie } })).statusCode, 404)
  assert.equal((await app.inject({ method: 'DELETE', url: `/api/media/${id(1)}`, headers: other })).statusCode, 404)
  assert.equal((await app.inject({ url: `/api/media/${id(1)}` })).statusCode, 401)
})

test('an upload is refused before its body is read when signed out, from another site or for another account', async (t) => {
  const { signIn, upload } = await server(t)
  const owner = signIn('owner@example.org')
  for (const [status, headers] of [
    [401, { origin: ORIGIN }], [403, { ...owner, origin: 'https://evil.test' }], [409, { ...owner, 'x-quietmap-account': 'other@example.org' }],
  ] as const) {
    let read = false
    const body = new Readable({
      read() {
        read = true
        this.push(wav(1))
        this.push(null)
      },
    })
    assert.equal((await upload(headers, 1, body)).statusCode, status)
    assert.equal(read, false, `${status} after reading the body`)
  }
})

test('a wrong body, type, point, time, details or id is refused', async (t) => {
  const { app, signIn, upload } = await server(t)
  const owner = signIn('owner@example.org')
  const stereo = wav(1)
  stereo.writeUInt16LE(2, 22)
  assert.equal((await upload(owner, 1, stereo)).statusCode, 400)
  assert.equal((await upload(owner, 1, wav(1), 'audio/mpeg')).statusCode, 415)
  assert.equal((await upload(owner, 1, Buffer.alloc(17 * 1024 ** 2), 'image/jpeg')).statusCode, 413)
  assert.equal((await upload(owner, 1, Buffer.from('<svg/>'), 'image/jpeg')).statusCode, 400)
  assert.equal((await upload(owner, 1, Buffer.from('{}'), 'application/json')).statusCode, 415)
  for (const header of [
    'not json', about({ lat: 91 }), about({ takenAt: 'yesterday' }), about({ takenAt: TAKEN_AT * 2 }), about({ details: 'text' }),
    about({ details: { note: 'x'.repeat(5_000) } }),
  ]) {
    assert.equal((await upload(owner, 1, wav(1), 'audio/wav', header)).statusCode, 400, header)
  }
  for (const wrongId of ['XYZ', '0'.repeat(31), `${'0'.repeat(32)}.wav`]) {
    const answer = await app.inject({ method: 'PUT', url: `/api/media/${wrongId}`, payload: wav(1), headers: { ...owner, 'content-type': 'audio/wav', 'x-quietmap-media': about() } })
    assert.equal(answer.statusCode, 400, wrongId)
  }
  // Half a minute at 48 kHz, 2.88 MB: over Fastify's default body of 1 MiB.
  assert.equal((await upload(owner, 1, wav(30, 0.25, 48_000))).statusCode, 200)
})

test('a photo is kept without its EXIF, a longitude from a repeated world wrapped', async (t) => {
  const { app, signIn, upload } = await server(t)
  const owner = signIn('owner@example.org')
  const saved = await upload(owner, 2, PHOTO, 'image/jpeg', about({ lng: PLACE.lng + 360 }))
  assert.equal(saved.statusCode, 200)
  assert.ok(Math.abs(saved.json().lng - PLACE.lng) < 1e-9)
  assert.equal(saved.json().durationS, null)
  const read = await app.inject({ url: `/api/media/${id(2)}`, headers: { cookie: owner.cookie } })
  assert.equal(read.headers['content-type'], 'image/jpeg')
  for (const gone of ['Exif', 'PhoneMaker', 'a comment']) assert.equal(read.rawPayload.indexOf(gone), -1, gone)
  assert.equal(read.rawPayload.subarray(-2).toString('hex'), 'ffd9')
})

test(`an account keeps at most ${MEDIA_ITEMS_MAX} items; deleting the account deletes them with their files`, async (t) => {
  const { app, store, signIn, upload, media } = await server(t)
  const owner = signIn('owner@example.org')
  for (let k = 0; k < MEDIA_ITEMS_MAX; k++) assert.equal((await upload(owner, k, wav(0.01))).statusCode, 200)
  assert.equal((await upload(owner, MEDIA_ITEMS_MAX, wav(0.01))).statusCode, 507)
  assert.equal((await upload(signIn('other@example.org'), MEDIA_ITEMS_MAX, wav(0.01))).statusCode, 200)

  assert.equal((await app.inject({ method: 'DELETE', url: '/api/me', headers: owner })).statusCode, 204)
  assert.equal(existsSync(join(media, '1')), false)
  assert.equal(store.mediaFile(1, id(0)), null)
  assert.equal(existsSync(join(media, '2')), true)
})

test('an upload waits while the disk keeping it has less than its floor free', async (t) => {
  const { signIn, upload } = await server(t, { diskFreeMinBytes: Number.MAX_SAFE_INTEGER })
  const refused = await upload(signIn('owner@example.org'), 1, wav(1))
  assert.equal(refused.statusCode, 507)
  assert.match(refused.json().error, /no room/)
})

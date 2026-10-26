// Recordings and photos of a place in a real browser with Chromium's fake microphone, the server
// mocked: on a phone the open place's row under the search field carries the microphone, which says how
// to record and starts at its OK; the take runs until Stop and goes to the account as the WAV the
// server takes, under an id of its own, with the place and how it was recorded; one the server did not
// keep plays from the page, is sent again under the same id, then listed, and a tap plays it from the
// server. A photo is drawn again as a
// JPEG without the camera's EXIF and kept. Signed out, or on a desktop, the place has no such buttons.
import { readFileSync } from 'node:fs'
import { expect, test, type Page, type Route } from '@playwright/test'
import { PHONE, POINT, canvasCenter, installHermeticMap, mapUrl, openPoint } from './support'

test.use({ ...PHONE, permissions: ['microphone'] })

const ACCOUNT = 'owner@example.org'
const PHOTO = readFileSync(new URL('../../server/src/media/photo-with-gps.jpg', import.meta.url))
const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) })

interface Sent { id: string, type: string, account: string, about: { lat: number, lng: number, takenAt: number, details: Record<string, unknown> }, body: Buffer }

/** A signed-in map whose server keeps what is sent, refusing the first upload when `refuseFirst`. */
async function signedInMap(page: Page, { refuseFirst = false } = {}) {
  await installHermeticMap(page, POINT)
  await page.route('**/api/me', route => json(route, { email: ACCOUNT, titles: [], recent: [] }))
  const kept: object[] = []
  const sent: Sent[] = []
  await page.route('**/api/media?**', route => json(route, { items: kept }))
  await page.route(/\/api\/media\/[0-9a-f]{32}$/, route => {
    const request = route.request()
    const id = new URL(request.url()).pathname.split('/').pop()!
    if (request.method() === 'GET') {
      const stored = sent.find(each => each.id === id)
      return stored ? route.fulfill({ status: 200, contentType: stored.type, body: stored.body }) : route.fulfill({ status: 404 })
    }
    if (request.method() !== 'PUT') return route.fulfill({ status: 404 })
    const headers = request.headers()
    const about = JSON.parse(decodeURIComponent(headers['x-quietmap-media']))
    const body = request.postDataBuffer()!
    sent.push({ id, type: headers['content-type'], account: headers['x-quietmap-account'], about, body })
    if (refuseFirst && sent.length === 1) return json(route, { error: 'The map has no room for it now. Try again later.' }, 507)
    const sound = headers['content-type'] === 'audio/wav'
    const item = {
      id, kind: sound ? 'sound' : 'photo', lat: about.lat, lng: about.lng, takenAt: about.takenAt, bytes: body.length,
      durationS: sound ? body.readUInt32LE(40) / 2 / body.readUInt32LE(24) : null,
    }
    kept.push(item)
    return json(route, item)
  })
  return { sent }
}

test('a recording at the open place goes to the account as a WAV; one not kept is sent again under its id', async ({ page }) => {
  const { sent } = await signedInMap(page, { refuseFirst: true })
  const row = await openPoint(page)
  const media = page.getByTestId('place-media')
  await row.getByRole('button', { name: 'Record the sound here' }).tap()
  // How to record, in two sentences; nothing is recorded before its OK, and Cancel takes it away.
  await expect(media).toContainText('until you press Stop')
  await media.getByRole('button', { name: 'Cancel' }).tap()
  await expect(media).toHaveCount(0)
  await row.getByRole('button', { name: 'Record the sound here' }).tap()
  await media.getByRole('button', { name: 'OK, record' }).tap()
  await expect(media.getByTestId('recording-time')).toHaveText('0:01')
  await media.getByRole('button', { name: 'Stop' }).tap()

  // A tap plays it, in the tap: not kept, from the page's copy, which plays on once it is kept; then,
  // closed and tapped again, from the server.
  const playing = () => media.locator('audio').evaluate((audio: HTMLAudioElement) =>
    `${audio.src.startsWith('blob:') ? 'page' : new URL(audio.src).pathname.split('/')[2]} ${audio.played.length > 0 ? 'played' : 'silent'} ${audio.duration >= 1}`)
  await media.getByRole('button', { name: 'Not kept' }).tap()
  await expect.poll(playing).toBe('page played true')
  await media.getByRole('button', { name: 'Send again' }).tap()
  await expect(media.getByTestId('media-item')).toHaveCount(1)
  await expect(media.getByText('Saving…')).toHaveCount(0)
  await media.getByTestId('media-item').tap()
  await expect(media.locator('audio')).toBeHidden()
  await media.getByTestId('media-item').tap()
  await expect.poll(playing).toBe('media played true')
  expect(sent).toHaveLength(2)
  const [first, again] = sent
  expect(again.id).toBe(first.id)
  expect(again.body.equals(first.body)).toBe(true)

  const wav = again.body
  const rate = wav.readUInt32LE(24)
  expect([wav.toString('latin1', 0, 4), wav.toString('latin1', 8, 16), wav.toString('latin1', 36, 40)]).toEqual(['RIFF', 'WAVEfmt ', 'data'])
  expect([wav.readUInt16LE(20), wav.readUInt16LE(22), wav.readUInt16LE(34)]).toEqual([1, 1, 16])
  expect(wav.readUInt32LE(40)).toBe(wav.length - 44)
  expect(wav.readUInt32LE(40) / 2 / rate).toBeGreaterThanOrEqual(1)
  // The fake microphone's beep: not silence.
  const samples = new Int16Array(wav.buffer.slice(wav.byteOffset + 44, wav.byteOffset + wav.length))
  expect(samples.reduce((loudest, sample) => Math.max(loudest, Math.abs(sample)), 0)).toBeGreaterThan(1000)

  expect(again.type).toBe('audio/wav')
  expect(again.account).toBe(ACCOUNT)
  expect(again.about.lat).toBeCloseTo(POINT.lat, 6)
  expect(again.about.lng).toBeCloseTo(POINT.lng, 6)
  expect(again.about.details).toMatchObject({
    recorder: 1, sampleRate: rate, ended: 'stopped',
    asked: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1 },
  })
  expect(again.about.details.settings).not.toHaveProperty('deviceId')
  expect(typeof again.about.details.microphone).toBe('string')
})

test('a photo is drawn again as a JPEG without the EXIF that held its position, and kept at the place', async ({ page }) => {
  const { sent } = await signedInMap(page)
  const row = await openPoint(page)
  await row.getByTestId('photo-input').setInputFiles({ name: 'place.jpg', mimeType: 'image/jpeg', buffer: PHOTO })
  await expect(page.getByTestId('place-media').getByTestId('media-item')).toHaveCount(1)
  const [photo] = sent
  expect(photo.type).toBe('image/jpeg')
  expect(photo.body.subarray(0, 2).toString('hex')).toBe('ffd8')
  expect(photo.body.indexOf('Exif')).toBe(-1)
  expect(photo.about.details).toMatchObject({ width: 16, height: 12 })
})

test('signed out, a place has no recording or photo buttons', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.route('**/api/me', route => json(route, { error: 'Not signed in' }, 401))
  const row = await openPoint(page)
  await expect(row).toBeVisible()
  await expect(row.getByRole('button', { name: 'Record the sound here' })).toHaveCount(0)
  await expect(row.getByTestId('photo-input')).toHaveCount(0)
})

test('on a desktop a signed-in visitor has no microphone or camera, and the popup starts with the loudness', async ({ browser }) => {
  // A desktop: a mouse, no touch (the file's phone settings reach a new page too).
  const page = await browser.newPage({ viewport: { width: 1280, height: 720 }, isMobile: false, hasTouch: false })
  await signedInMap(page)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  const row = page.getByTestId('open-place')
  await expect(row).toBeVisible()
  await expect(row.getByRole('button', { name: 'Record the sound here' })).toBeHidden()
  await expect(page.getByTestId('detail-popup-skeleton').filter({ visible: true })).toBeVisible()
  await page.close()
})

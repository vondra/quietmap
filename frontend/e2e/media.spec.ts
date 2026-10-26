// Recordings and photos of a place in a real browser with Chromium's fake microphone, the server
// mocked: a signed-in visitor records at the open place and stops, and the take goes to the account
// as the WAV the server takes, under an id of its own, with the place and how it was recorded; one
// the server did not keep is sent again under the same id and then listed. A photo is drawn again as a
// JPEG without the camera's EXIF and kept. Signed out, the place has no such buttons.
import { readFileSync } from 'node:fs'
import { expect, test, type Page, type Route } from '@playwright/test'
import { PHONE, POINT, installHermeticMap, openPoint } from './support'

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
    if (request.method() !== 'PUT') return route.fulfill({ status: 404 })
    const headers = request.headers()
    const about = JSON.parse(decodeURIComponent(headers['x-quietmap-media']))
    const body = request.postDataBuffer()!
    const id = new URL(request.url()).pathname.split('/').pop()!
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
  await openPoint(page)
  const sheet = page.getByTestId('mobile-detail-sheet')
  await sheet.getByRole('button', { name: 'Record the sound here' }).tap()
  await expect(sheet.getByTestId('recording-time')).toContainText('0:01 / 0:30')
  await sheet.getByRole('button', { name: 'Stop' }).tap()

  await sheet.getByRole('button', { name: 'Not kept' }).tap()
  await sheet.getByRole('button', { name: 'Send again' }).tap()
  await expect(sheet.getByTestId('media-item')).toHaveCount(1)
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
  await openPoint(page)
  const sheet = page.getByTestId('mobile-detail-sheet')
  await sheet.getByTestId('photo-input').setInputFiles({ name: 'place.jpg', mimeType: 'image/jpeg', buffer: PHOTO })
  await expect(sheet.getByTestId('media-item')).toHaveCount(1)
  const [photo] = sent
  expect(photo.type).toBe('image/jpeg')
  expect(photo.body.subarray(0, 2).toString('hex')).toBe('ffd8')
  expect(photo.body.indexOf('Exif')).toBe(-1)
  expect(photo.about.details).toMatchObject({ width: 16, height: 12 })
})

test('signed out, a place has no recording or photo buttons', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.route('**/api/me', route => json(route, { error: 'Not signed in' }, 401))
  await expect(await openPoint(page)).toBeVisible()
  const sheet = page.getByTestId('mobile-detail-sheet')
  await expect(sheet.getByRole('button', { name: 'Record the sound here' })).toHaveCount(0)
  await expect(sheet.getByTestId('photo-input')).toHaveCount(0)
})

// Places to stay in a real browser, without a backend: the switch and its search in the panel, what a
// view asks for (from today for two nights, two guests, then the kind and stars chosen; a box each
// side of the antimeridian), the pin on the map, a pin that opens its place above the popup of its
// point (in a repeated world copy too), on a desktop and on a phone, a place gone with its price,
// and a view that asks again once Stay22 takes searches again.
import { expect, test, type Page } from '@playwright/test'
import { popupUpdate } from './answers'
import {
  PHONE,
  POINT,
  SOURCE_DB,
  afterPaint,
  canvasCenter,
  installHermeticMap,
  mapUrl,
  pngCenterPixel,
  popupRequests,
  sendPopupLine,
} from './support'

const PLACE = {
  id: 'h1', name: 'Fixture hotel', lat: POINT.lat, lng: POINT.lng, total: 300, stars: 4, score: 8.6, reviews: 1234,
  guests: 2, bedrooms: 1, freeCancellation: true, thumbnail: null, url: 'https://www.stay22.com/allez/roam/h1',
}

/** An answer of the one place, whose price lives `expiresIn` seconds more. */
const answer = (expiresIn = 3300) => JSON.stringify({ listings: [PLACE], nights: 2, currency: 'EUR', expiresIn, failure: null })

/** `/api/stay` answers the one place (the first answer living `firstExpiresIn` seconds), and records
 *  what every view asked; the labels' glyphs are answered empty, so nothing leaves the page. */
async function installStays(page: Page, firstExpiresIn?: number): Promise<URLSearchParams[]> {
  const asked: URLSearchParams[] = []
  await page.route('**/api/stay?**', route => {
    asked.push(new URL(route.request().url()).searchParams)
    return route.fulfill({ status: 200, contentType: 'application/json', body: answer(asked.length === 1 ? firstExpiresIn : undefined) })
  })
  await page.route('**/fonts/**', route => route.fulfill({ status: 200, body: '' }))
  return asked
}

/** The first check-in Stay22 takes, in the page's clock: the later of its local and its UTC day. */
const firstCheckin = (page: Page) => page.evaluate(() => {
  const now = new Date()
  const local = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`
  const utc = now.toISOString().slice(0, 10)
  return local > utc ? local : utc
})

/** The place's dot, slate with a white ring, at the map's centre. */
async function expectPinAtCentre(page: Page) {
  const { canvas } = await canvasCenter(page)
  await expect.poll(async () => {
    await afterPaint(page)
    return (await pngCenterPixel(page, await canvas.screenshot())).slice(0, 3)
  }).toEqual([0x33, 0x41, 0x55])
}

test('desktop: the switch shows the places for today and two nights; a pin opens its place above its popup', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const asked = await installStays(page)
  await page.goto(mapUrl(POINT))
  const toggle = page.getByRole('button', { name: 'Places to stay' }).filter({ visible: true })
  await expect(toggle).toHaveAttribute('aria-pressed', 'false')
  await toggle.click()
  await expect(toggle).toHaveAttribute('aria-pressed', 'true')
  await expect(page).toHaveURL(/[#&]stay=1(&|$)/)

  await expect.poll(() => asked.length).toBeGreaterThan(0)
  const checkin = await firstCheckin(page)
  const first = asked[0]
  expect(first.get('checkin')).toBe(checkin)
  expect(new Date(Date.parse(first.get('checkout')!) - Date.parse(checkin)).getTime()).toBe(2 * 86_400_000)
  expect(first.get('adults')).toBe('2')
  expect([first.get('type'), first.get('minstars'), first.get('minscore')]).toEqual([null, null, null])
  expect(Number(first.get('swlat'))).toBeLessThan(POINT.lat)
  expect(Number(first.get('nelng'))).toBeGreaterThan(POINT.lng)
  await expect(page.getByTestId('stay-checkin').filter({ visible: true })).toHaveValue(checkin)
  await expectPinAtCentre(page)

  // The pin opens the popup at the place's point, the place above the answer.
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toEqual([{ lat: PLACE.lat, lng: PLACE.lng }])
  const header = page.locator('[data-testid="stay-header"]:visible')
  await expect(header).toContainText('Fixture hotel')
  await expect(header).toContainText('★★★★')
  await expect(header).toContainText('8.6 · 1,234 reviews')
  await expect(header.getByTestId('stay-price')).toHaveText('€150 a night · €300 for 2 nights')
  await expect(header).toContainText('sleeps 2 · 1 bedroom · free cancellation')
  const offer = header.getByRole('link', { name: 'View offer' })
  await expect(offer).toHaveAttribute('href', PLACE.url)
  await expect(offer).toHaveAttribute('target', '_blank')
  await expect(offer).toHaveAttribute('rel', /sponsored/)
  await sendPopupLine(page, popupUpdate(1, false, PLACE.lat, PLACE.lng, SOURCE_DB))
  await expect(page.locator('[data-testid="lden"]:visible')).toHaveText(`${SOURCE_DB.toFixed(1)} dB Lden`)

  // A click 10 px beside the dot is the popup of that point, without the place.
  await page.mouse.click(x + 10, y)
  await expect.poll(async () => (await popupRequests(page)).length).toBe(2)
  expect((await popupRequests(page))[1].lng).toBeGreaterThan(PLACE.lng)
  await expect(header).toHaveCount(0)

  // A narrower search asks again: hotels of four stars or more.
  await page.locator('button[aria-label="Close"]:visible').click()
  const before = asked.length
  await page.getByRole('radio', { name: 'Hotels' }).filter({ visible: true }).click()
  await page.getByRole('radio', { name: '★★★★', exact: true }).filter({ visible: true }).click()
  await expect.poll(() => asked.slice(before).some(params => params.get('type') === 'hotel' && params.get('minstars') === '4')).toBe(true)
})

test('desktop: a full Stay22 minute is said, and the view asks again after the seconds named', async ({ page }) => {
  await installHermeticMap(page, POINT)
  let asked = 0
  await page.route('**/api/stay?**', route => {
    asked += 1
    return asked === 1
      ? route.fulfill({ status: 503, headers: { 'retry-after': '1' }, contentType: 'application/json',
        body: JSON.stringify({ error: 'Stay22 takes no more searches this minute; places to stay again in 1 s.' }) })
      : route.fulfill({ status: 200, contentType: 'application/json', body: answer() })
  })
  await page.route('**/fonts/**', route => route.fulfill({ status: 200, body: '' }))
  await page.goto(`${mapUrl(POINT)}&stay=1`)
  await expect(page.getByTestId('stay-note')).toHaveText(/no more searches this minute/)
  await expectPinAtCentre(page)
  await expect(page.getByTestId('stay-note')).toHaveCount(0)
  expect(asked).toBe(2)
})

test('desktop: a pin opens its place in a repeated world copy too', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await installStays(page)
  await page.goto(`${mapUrl({ ...POINT, lng: POINT.lng + 360 })}&stay=1`)
  await expectPinAtCentre(page)
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toEqual([{ lat: PLACE.lat, lng: PLACE.lng }])
  await expect(page.locator('[data-testid="stay-header"]:visible')).toContainText('Fixture hotel')
})

test('desktop: a view across the antimeridian asks for a box on each side', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const asked = await installStays(page)
  await page.goto('/#lat=-17.7&lng=179.99&z=12&bm=terrain&ro=road&stay=1')
  await expect.poll(() => asked.length).toBe(2)
  const sides = asked.map(params => [Number(params.get('swlng')), Number(params.get('nelng'))]).sort((a, b) => a[0] - b[0])
  expect(sides[0][0]).toBe(-180)
  expect(sides[0][1]).toBeGreaterThan(-180)
  expect(sides[1][0]).toBeLessThan(180)
  expect(sides[1][1]).toBe(180)
})

test('desktop: a place and its pin go when their price expires, and the view asks again', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const asked = await installStays(page, 4)
  await page.goto(`${mapUrl(POINT)}&stay=1`)
  await expectPinAtCentre(page)
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  const header = page.locator('[data-testid="stay-header"]:visible')
  await expect(header).toContainText('Fixture hotel')
  await expect(header).toHaveCount(0)
  await expect.poll(() => asked.length).toBeGreaterThan(1)
})

test.describe('mobile', () => {
  test.use(PHONE)

  test('a link with the places shows them; a tapped pin opens the sheet with its place', async ({ page }) => {
    await installHermeticMap(page, POINT)
    const asked = await installStays(page)
    await page.goto(`${mapUrl(POINT)}&stay=1`)
    await expect.poll(() => asked.length).toBeGreaterThan(0)
    await expectPinAtCentre(page)
    const { x, y } = await canvasCenter(page)
    await page.touchscreen.tap(x, y)
    await expect.poll(() => popupRequests(page)).toEqual([{ lat: PLACE.lat, lng: PLACE.lng }])
    const sheet = page.getByTestId('mobile-detail-sheet')
    await expect(sheet.getByTestId('stay-header')).toContainText('Fixture hotel')
    await expect(sheet.getByTestId('stay-price')).toHaveText('€150 a night · €300 for 2 nights')
  })
})

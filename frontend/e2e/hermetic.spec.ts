// The visitor's path in a real browser, without a backend: heatmap hover, the streamed popup (its
// refinement, errors and aborts, the loudest flights), building clicks, layer switches, search, and
// the phone sheet.
import { expect, test } from '@playwright/test'
import { FIXTURE_FLIGHTS, popupUpdate, withAircraft } from './answers'
import {
  FIXTURE_DB,
  PHONE,
  POINT,
  SOURCE_DB,
  TILE_Z,
  abortedPopupRequests,
  afterPaint,
  canvasCenter,
  endPopup,
  installHermeticMap,
  mapUrl,
  pngCenterPixel,
  popupRequests,
  sendPopupLine,
} from './support'

const badge = (page: import('@playwright/test').Page) => page.locator('[data-testid="noise-badge"]:visible')

test('desktop: hover reads the painted cell, the popup redraws on every streamed update', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const expectedTiles = ['road', 'rail'].map(source =>
    `/api/tiles/b1/${source}/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`)
  const tilesLoaded = expectedTiles.map(path => page.waitForResponse(response =>
    new URL(response.url()).pathname === path && response.status() === 200,
  ))
  // Overzoom one exact z12 receiver so its single audible cell spans multiple
  // screen pixels: a one-cell spatial shift still misses the canvas centre,
  // and the linear texture filter dilutes the centre pixel far less.
  await page.goto(mapUrl(POINT, 'road,rail', TILE_Z + 2))
  await Promise.all(tilesLoaded)
  const { canvas, x, y } = await canvasCenter(page)
  await page.mouse.move(x, y)
  await expect(page.getByTestId('heatmap-hover')).toHaveText(`Lden: ${(SOURCE_DB + 10 * Math.log10(2)).toFixed(1)} dB`)
  let combinedPixel: number[] = []
  await expect.poll(async () => {
    await afterPaint(page)
    combinedPixel = await pngCenterPixel(page, await canvas.screenshot())
    return Math.min(combinedPixel[0] - combinedPixel[1], combinedPixel[0] - combinedPixel[2]) > 35
  }).toBe(true)

  await page.mouse.click(x, y)
  await expect(page.locator('[data-testid="detail-popup-skeleton"]:visible')).toBeVisible()
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  const [query] = await popupRequests(page)
  expect(Math.abs(query.lat - POINT.lat)).toBeLessThan(1e-6)
  expect(Math.abs(query.lng - POINT.lng)).toBeLessThan(1e-6)

  // The first ring's answer is shown at once, marked as still being refined.
  await sendPopupLine(page, withAircraft(popupUpdate(1, true, POINT.lat, POINT.lng, SOURCE_DB), FIXTURE_FLIGHTS.slice(0, 1)))
  await expect(badge(page)).toHaveText(`${SOURCE_DB.toFixed(1)} dB`)
  await expect(page.locator('[data-testid="popup-refining"]:visible')).toBeVisible()
  await expect(page.locator('[data-testid="annoyance"]:visible')).toHaveCount(0)
  await expect(page.getByRole('button', { name: /Fixture street/ }).filter({ visible: true })).toBeVisible()
  // An opened contributor stays open while later updates redraw the popup.
  await page.getByRole('button', { name: /Fixture street/ }).filter({ visible: true }).click()
  await expect(page.getByText('9.6k/day').filter({ visible: true })).toBeVisible()
  // The aircraft layer is one row, ranked below the louder street; it opens on its loudest flights.
  const popup = page.locator('[data-testid="detail-popup"]:visible')
  await expect(popup.getByRole('button')).toHaveText([/^Fixture street/, /^Aircraft/])
  await popup.getByRole('button', { name: /^Aircraft/ }).click()
  const flights = popup.getByRole('table', { name: 'Loudest flights' })
  await expect(flights.locator('tbody tr')).toHaveCount(1)

  await sendPopupLine(page, withAircraft(popupUpdate(2, false, POINT.lat, POINT.lng, FIXTURE_DB), FIXTURE_FLIGHTS))
  await endPopup(page)
  await expect(badge(page)).toHaveText(`${FIXTURE_DB.toFixed(1)} dB`)
  await expect(page.locator('[data-testid="popup-refining"]:visible')).toHaveCount(0)
  // The final answer says how many of 100 residents would be highly annoyed.
  await expect(page.locator('[data-testid="annoyance"]:visible')).toHaveText('7 of 100 people would be highly annoyed here')
  await expect(page.getByText('9.6k/day').filter({ visible: true })).toBeVisible()
  await expect(flights.locator('tbody tr')).toHaveCount(2)
  await expect(flights.locator('tbody tr').nth(0).locator('td')).toHaveText(['70', '0.44', '0.26', '09-02 D', /^Airbus A320\b/])
  await expect(flights.locator('tbody tr').nth(1).locator('td'))
    .toHaveText(['69', '1.31', '0.61', '09-01 N', /^Aérospatiale AS355 Écureuil 2\b/])
  // A long type name wraps: the table fits the card, nothing clipped or scrolled.
  await flights.scrollIntoViewIfNeeded()
  await expect(flights).toBeInViewport({ ratio: 1 })
  const trace = flights.getByRole('link', { name: /^Airbus A320\b/ })
  await expect(trace).toHaveAttribute('href', 'https://adsb.lol/?icao=4b0a1c&showTrace=2025-09-02')
  await expect(trace).toHaveAttribute('target', '_blank')
  await expect(trace).toHaveAttribute('rel', 'noopener noreferrer')

  await page.locator('button[aria-label="Close"]:visible').click()
  await expect(page.locator('[data-testid="detail-popup"]:visible')).toHaveCount(0)
  const road = page.getByTestId('layer-road').filter({ visible: true })
  const rail = page.getByTestId('layer-rail').filter({ visible: true })
  await expect(road).toHaveAttribute('aria-pressed', 'true')
  await expect(rail).toHaveAttribute('aria-pressed', 'true')
  await rail.click()
  await expect(rail).toHaveAttribute('aria-pressed', 'false')
  await page.mouse.move(x, y)
  await expect(page.getByTestId('heatmap-hover')).toHaveText(`Lden: ${SOURCE_DB.toFixed(1)} dB`)
  await road.click()
  await expect(road).toHaveAttribute('aria-pressed', 'false')
  await expect(page.getByTestId('heatmap-hover')).toHaveCount(0)
  await expect.poll(async () => {
    await afterPaint(page)
    const emptyPixel = await pngCenterPixel(page, await canvas.screenshot())
    return Math.max(...emptyPixel.slice(0, 3))
  }).toBeLessThanOrEqual(2)
})

test('desktop: an error line replaces the partial answer, a new click aborts the old request', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)

  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  await sendPopupLine(page, popupUpdate(1, true, POINT.lat, POINT.lng, SOURCE_DB))
  await expect(badge(page)).toHaveText(`${SOURCE_DB.toFixed(1)} dB`)
  await sendPopupLine(page, { error: 'The noise computation failed at this point.' })
  await endPopup(page)
  await expect(page.locator('[data-testid="detail-popup-error"]:visible'))
    .toHaveText('The noise computation failed at this point.')
  await expect(badge(page)).toHaveCount(0)

  // A click elsewhere while the answer is still streaming aborts that request.
  await page.mouse.click(x - 40, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(2)
  await sendPopupLine(page, popupUpdate(1, true, POINT.lat, POINT.lng, SOURCE_DB))
  await expect(badge(page)).toHaveText(`${SOURCE_DB.toFixed(1)} dB`)
  await page.mouse.click(x + 40, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(3)
  await expect.poll(() => abortedPopupRequests(page)).toBe(1)
  await expect(page.locator('[data-testid="detail-popup-skeleton"]:visible')).toBeVisible()
})

// Inside a building the popup shows the level at the building's loudest façade receiver, and
// lists answer fields it has no words for.
test('desktop: a point inside a building reads as its noisiest façade', async ({ page }) => {
  await installHermeticMap(page, POINT, FIXTURE_DB)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  const building = {
    id: '011400ad000668eb',
    height_m: 29,
    facade: { bearing_deg: 140, index: 2, receiver: [POINT.lat, POINT.lng + 0.0001] as [number, number] },
    facade_receivers: 12,
    storeys: 8,
  }
  await sendPopupLine(page, popupUpdate(1, false, POINT.lat, POINT.lng, FIXTURE_DB, building))
  await expect(badge(page)).toHaveText(`${FIXTURE_DB.toFixed(1)} dB`)
  const exposure = page.locator('[data-testid="building-exposure"]:visible')
  await expect(exposure).toContainText('Noisiest façade of this building — faces SE')
  await expect(exposure).toContainText('1 of 12 façade points')
  await exposure.hover()
  await expect(page.getByRole('tooltip')).toContainText('storeys')
})

test('search: picking a result flies the map there and opens its popup', async ({ page }) => {
  const target = { display_name: 'Ruzyně Airport', secondary: 'Prague, CZ', lat: 50.1, lon: 14.26 }
  await installHermeticMap(page, POINT)
  await page.route('**/api/search?**', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify([target]),
  }))
  await page.goto(mapUrl(POINT))
  await page.getByRole('searchbox').fill('Ruzyně')
  const option = page.getByRole('option', { name: new RegExp(target.display_name) })
  await expect(option).toBeVisible()
  await option.click()
  // The hash carries four decimals; anchor the complete values.
  await expect(page).toHaveURL(/lat=50\.1000&lng=14\.2600&/)
  await expect.poll(() => popupRequests(page)).toEqual([{ lat: target.lat, lng: target.lon }])
})

test.describe('mobile', () => {
  test.use(PHONE)

  test('map tap opens the real mobile sheet and layer controls', async ({ page }) => {
    await installHermeticMap(page, POINT)
    const tilePath = `/api/tiles/b1/road/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`
    const tileLoaded = page.waitForResponse(response =>
      new URL(response.url()).pathname === tilePath && response.status() === 200,
    )
    await page.goto(mapUrl(POINT))
    await tileLoaded
    const { x, y } = await canvasCenter(page)
    await page.touchscreen.tap(x, y)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    const sheet = page.getByTestId('mobile-detail-sheet')
    await expect(sheet).toBeVisible()
    await expect(sheet.getByTestId('detail-popup-skeleton')).toBeVisible()

    await sendPopupLine(page, withAircraft(popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB), FIXTURE_FLIGHTS))
    await expect(sheet.getByTestId('noise-badge')).toHaveText(`${SOURCE_DB.toFixed(1)} dB`)
    // The loudest flights fit the phone: the whole table is on screen, nothing clipped or scrolled.
    await sheet.getByRole('button', { name: /^Aircraft/ }).tap()
    const flights = sheet.getByRole('table', { name: 'Loudest flights' })
    await expect(flights.locator('tbody tr')).toHaveCount(2)
    await expect(flights).toBeInViewport({ ratio: 1 })

    // A detail sheet deliberately covers the layers button. Start a clean map
    // view instead of force-clicking through it (which no user can do). A
    // changed, ignored query string forces a new document; a hash-only goto
    // would keep the existing React instance and its open sheet.
    await page.goto(`/?e2e=layers${mapUrl(POINT).slice(1)}`)
    await canvasCenter(page)
    await page.getByRole('button', { name: 'Toggle layers panel' }).tap()
    const panel = page.locator('[data-testid="layers-panel"]:visible')
    await expect(panel).toBeVisible()
    const road = panel.getByTestId('layer-road')
    await expect(road).toHaveAttribute('aria-pressed', 'true')
    await road.tap()
    await expect(road).toHaveAttribute('aria-pressed', 'false')
  })
})

// A phone's way through the map in a real browser, without a backend: the popup's sheet, the
// control row with the credits' ⓘ and the layers panel, the locate button, and a place's name.
import { expect, test } from '@playwright/test'
import { FIXTURE_FLIGHTS, popupUpdate, withAircraft } from './answers'
import {
  PHONE,
  POINT,
  SOURCE_DB,
  TILE_Z,
  afterPaint,
  canvasCenter,
  installHermeticMap,
  mapUrl,
  popupRequests,
  sendPopupLine,
} from './support'

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
  await expect(sheet.getByTestId('lden')).toHaveText(`${SOURCE_DB.toFixed(1)} dB Lden`)
  // The sheet would cover the tapped point (the map's centre): the map moves it up, the view's
  // centre south of it. A finger leaves no hover readout beside the popup.
  const viewLat = () => page.evaluate(() => Number(new URLSearchParams(location.hash.slice(1)).get('lat')))
  await expect.poll(viewLat).toBeLessThan(POINT.lat)
  await expect(page.getByTestId('heatmap-hover')).toHaveCount(0)
  // The loudest flights fit the phone: the whole table is on screen, nothing clipped or scrolled.
  await sheet.getByRole('button', { name: /^Aircraft/ }).tap()
  const flights = sheet.getByRole('table', { name: 'Loudest flights' })
  await expect(flights.locator('tbody tr')).toHaveCount(2)
  await expect(flights).toBeInViewport({ ratio: 1 })
  // The sheet closes from its corner as well as by a swipe.
  await sheet.getByRole('button', { name: 'Close' }).tap()
  await expect(sheet).toHaveCount(0)

  // A detail sheet deliberately covers the layers button. Start a clean map
  // view instead of force-clicking through it (which no user can do). A
  // changed, ignored query string forces a new document; a hash-only goto
  // would keep the existing React instance and its open sheet.
  const reopenedTileLoaded = page.waitForResponse(response =>
    new URL(response.url()).pathname === tilePath && response.status() === 200,
  )
  await page.goto(`/?e2e=layers${mapUrl(POINT).slice(1)}`)
  await reopenedTileLoaded
  await afterPaint(page)
  // The credits stay open while the map loads and paints, and fold at the first touch of the map
  // into their ⓘ, a button of the control row beside the layers button; it opens them again,
  // the About first.
  const credits = page.locator('.attrib-about')
  await expect(credits).toBeVisible()
  await page.locator('.maplibregl-canvas-container').dispatchEvent('pointerdown')
  await expect(credits).toBeHidden()
  const info = page.locator('.maplibregl-ctrl-attrib-button')
  const layersButton = page.getByRole('button', { name: 'Toggle layers panel' })
  const bottom = async (button: typeof info) => {
    const box = await button.boundingBox()
    return box ? box.y + box.height : NaN
  }
  expect(await bottom(info)).toBeCloseTo(await bottom(layersButton), 0)
  await info.tap()
  await expect(credits).toBeVisible()
  await expect(credits).toHaveAttribute('href', '/about')
  await info.tap()
  await layersButton.tap()
  const panel = page.locator('[data-testid="layers-panel"]:visible')
  await expect(panel).toBeVisible()
  // Each switch turns its own layer over.
  const road = panel.getByTestId('layer-road')
  const rail = panel.getByTestId('layer-railway')
  await expect(road).toHaveAttribute('aria-pressed', 'true')
  await expect(rail).toHaveAttribute('aria-pressed', 'false')
  await rail.tap()
  await expect(rail).toHaveAttribute('aria-pressed', 'true')
  await expect(road).toHaveAttribute('aria-pressed', 'true')
  await road.tap()
  await expect(road).toHaveAttribute('aria-pressed', 'false')
  await expect(rail).toHaveAttribute('aria-pressed', 'true')
  // A data layer on shows its legend: the renderer's colours and what they stand for.
  await page.route('**/api/raster/**', route => route.fulfill({ status: 204 }))
  await page.getByRole('button', { name: 'Advanced' }).filter({ visible: true }).tap()
  await page.getByTestId('data-layer-traffic').filter({ visible: true }).tap()
  const legend = page.getByTestId('legend-traffic').filter({ visible: true })
  await expect(legend).toContainText('15,000 a day')
  await expect(legend).toContainText('estimated by class')
})

test('the locate button opens the popup where the visitor stands, again from a map moved away', async ({ page, context }) => {
  await context.grantPermissions(['geolocation'])
  await context.setGeolocation({ latitude: POINT.lat, longitude: POINT.lng, accuracy: 10 })
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT, 'road', TILE_Z - 2))
  await canvasCenter(page)
  await page.getByRole('button', { name: 'Show my location' }).tap()
  const here = { lat: POINT.lat, lng: POINT.lng }
  await expect.poll(() => popupRequests(page)).toEqual([here])
  const sheet = page.getByTestId('mobile-detail-sheet')
  await expect(sheet).toBeVisible()
  // The sheet moved the map off the visitor, which stops following them: a press takes the map
  // back to the last position found and opens its popup again.
  await sheet.getByRole('button', { name: 'Close' }).tap()
  await page.getByRole('button', { name: 'Show my location' }).tap()
  await expect.poll(() => popupRequests(page)).toEqual([here, here])
  await expect(sheet).toBeVisible()
})

test('a place takes the name the visitor gives it in the sheet, and keeps it on the next visit', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const tilePath = `/api/tiles/b1/road/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`
  const openAtPoint = async (url: string) => {
    const tileLoaded = page.waitForResponse(response =>
      new URL(response.url()).pathname === tilePath && response.status() === 200,
    )
    await page.goto(url)
    await tileLoaded
    const { x, y } = await canvasCenter(page)
    await page.touchscreen.tap(x, y)
    return page.getByTestId('mobile-detail-sheet').getByTestId('place-title')
  }
  let title = await openAtPoint(mapUrl(POINT))
  // The geocoded name is proposed in the field, which a phone does not zoom into.
  await expect(title).toHaveAttribute('placeholder', 'E2E fixture')
  await expect(title).toHaveValue('')
  expect(await title.evaluate(field => getComputedStyle(field).fontSize)).toBe('16px')
  // A tap edits it; Escape leaves it as it was, Enter keeps what was typed.
  await title.tap()
  await expect(title).toBeFocused()
  await page.keyboard.type('Discarded')
  await page.keyboard.press('Escape')
  await expect(title).toHaveValue('')
  await title.tap()
  await page.keyboard.type('Above the house')
  await page.keyboard.press('Enter')
  await expect(title).not.toBeFocused()
  await expect(title).toHaveValue('Above the house')
  // A new document (a changed, ignored query string) and the same point: the name is kept.
  title = await openAtPoint(`/?e2e=title${mapUrl(POINT).slice(1)}`)
  await expect(title).toHaveValue('Above the house')
  // Emptied, the name gives the proposal back.
  await title.tap()
  await title.fill('')
  await page.keyboard.press('Enter')
  await expect(title).toHaveValue('')
  await expect(title).toHaveAttribute('placeholder', 'E2E fixture')
})

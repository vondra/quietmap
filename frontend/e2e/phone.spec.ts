// A phone's way through the map in a real browser, without a backend: the popup's sheet, the base
// map's credits, the control row with the ⓘ to the About pages and the layers panel, the locate
// button, and the places' tabs with their names.
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
  openPoint,
  openTab,
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
  // The base map's credits stay in sight with the sheet open: under the search bar, above the sheet.
  const credits = page.locator('.maplibregl-ctrl-attrib')
  const box = async (element: typeof credits) => (await element.boundingBox())!
  await expect(credits).toContainText('OpenStreetMap contributors')
  expect((await box(credits)).y + (await box(credits)).height).toBeLessThanOrEqual((await box(sheet)).y)
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
  // The credits stay after a touch of the map; the ⓘ, a button of the row beside the layers button,
  // leads to the About pages; the layers' sheet leaves the credits in sight too.
  await page.locator('.maplibregl-canvas-container').dispatchEvent('pointerdown')
  await expect(credits).toBeInViewport({ ratio: 1 })
  const info = page.getByRole('link', { name: 'About quietmap.org' })
  await expect(info).toHaveAttribute('href', '/about')
  const layersButton = page.getByRole('button', { name: 'Toggle layers panel' })
  expect((await box(info)).y + (await box(info)).height).toBeCloseTo((await box(layersButton)).y + (await box(layersButton)).height, 0)
  await layersButton.tap()
  const panel = page.locator('[data-testid="layers-panel"]:visible')
  await expect(panel).toBeVisible()
  expect((await box(credits)).y + (await box(credits)).height).toBeLessThanOrEqual((await box(page.getByRole('button', { name: 'Drag to close' }))).y)
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

test('places are tabs: a new one first, an old one opened where it stands, each named in its tab and kept', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const tilePath = `/api/tiles/b1/road/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`
  const sheet = page.getByTestId('mobile-detail-sheet')
  const tabs = sheet.getByTestId('place-tab')
  const field = sheet.getByTestId('place-title')
  const tapAt = async (dx: number, dy: number) => {
    const { x, y } = await canvasCenter(page)
    await page.touchscreen.tap(x + dx, y + dy)
  }
  const openAtPoint = async (url: string) => {
    const tileLoaded = page.waitForResponse(response =>
      new URL(response.url()).pathname === tilePath && response.status() === 200,
    )
    await page.goto(url)
    await tileLoaded
    await tapAt(0, 0)
  }
  await openAtPoint(mapUrl(POINT))
  // The tab shows the geocoded name; a tap on it writes a name, in a field a phone does not zoom into.
  await expect(tabs).toHaveCount(1)
  await expect(openTab(page)).toContainText('E2E fixture')
  await openTab(page).getByTitle('Rename this place').tap()
  await expect(field).toBeFocused()
  await expect(field).toHaveValue('E2E fixture')
  expect(await field.evaluate(input => getComputedStyle(input).fontSize)).toBe('16px')
  // Escape leaves it as it was, Enter keeps what was typed.
  await page.keyboard.type('Discarded')
  await page.keyboard.press('Escape')
  await expect(field).toHaveCount(0)
  await expect(openTab(page)).toContainText('E2E fixture')
  await openTab(page).getByTitle('Rename this place').tap()
  await page.keyboard.type('Above the house')
  await page.keyboard.press('Enter')
  await expect(field).toHaveCount(0)
  await expect(openTab(page)).toContainText('Above the house')

  // Answered, a place stays a tab; a place opened next comes first, one tab and one name each.
  await sendPopupLine(page, popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB))
  await tapAt(0, -120)
  await expect.poll(() => popupRequests(page)).toHaveLength(2)
  const next = (await popupRequests(page))[1]
  await sendPopupLine(page, popupUpdate(1, false, next.lat, next.lng, SOURCE_DB))
  await expect(tabs).toHaveCount(2)
  await expect(tabs.nth(0).locator('[aria-current="page"]')).toHaveCount(1)
  await expect(tabs.nth(1)).toContainText('Above the house')
  await expect(sheet.getByTitle('Rename this place')).toHaveCount(1)
  // The named place's tab opens it again at once, where its tab stands.
  await tabs.nth(1).getByRole('button', { name: /Above the house/ }).tap()
  await expect(tabs.nth(1).locator('[aria-current="page"]')).toHaveCount(1)
  await expect.poll(() => popupRequests(page)).toHaveLength(3)
  await expect(tabs).toHaveCount(2)

  // A new document (a changed, ignored query string) and the same point: the name is kept.
  await openAtPoint(`/?e2e=title${mapUrl(POINT).slice(1)}`)
  await expect(openTab(page)).toContainText('Above the house')
  // Emptied, the name gives the geocoded one back.
  await openTab(page).getByTitle('Rename this place').tap()
  await field.fill('')
  await page.keyboard.press('Enter')
  await expect(openTab(page)).toContainText('E2E fixture')
})

test('a name left as it was is not kept, though the geocoded name came while it was being written', async ({ page }) => {
  await installHermeticMap(page, POINT)
  let release = () => {}
  const late = new Promise<void>(resolve => { release = resolve })
  await page.route('**/api/reverse?**', async route => {
    await late
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ place: 'Late name' }) })
  })
  await openPoint(page)
  await openTab(page).getByTitle('Rename this place').tap()
  const field = page.getByTestId('mobile-detail-sheet').getByTestId('place-title')
  await expect(field).toHaveValue(/^[\d.]+, [\d.]+$/)
  release()
  await expect(field).toHaveAttribute('placeholder', 'Late name')
  await page.keyboard.press('Enter')
  await expect(openTab(page)).toContainText('Late name')
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem('quietmap.place-titles') ?? '[]'))).toEqual([])
})

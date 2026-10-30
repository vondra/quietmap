// A phone's way through the map in a real browser, without a backend: the popup's sheet, the base
// map's credits, the control row with the ⓘ to the About pages and the layers panel, the locate
// button, the open place under the search field and the history of places opened in its list.
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

test('the open place stands under the search field, renamed there; the search lists the places opened, the last first', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const tilePath = `/api/tiles/b1/road/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`
  const row = page.getByTestId('open-place')
  const field = row.getByTestId('place-title')
  const options = page.getByRole('option')
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
  const rename = async (name: string) => {
    await field.tap()
    await field.fill(name)
    await page.keyboard.press('Enter')
    await expect(field).toHaveValue(name)
  }
  await openAtPoint(mapUrl(POINT))
  // The geocoded name stands under the search field as the field that renames it, in a size a phone
  // does not zoom into: a tap puts the caret into the name as written, to add to it. Escape leaves it
  // as it was, Enter keeps what was typed.
  await expect(field).toHaveValue('E2E fixture')
  expect(await field.evaluate(input => getComputedStyle(input).fontSize)).toBe('16px')
  await field.tap()
  await expect(field).toBeFocused()
  await page.keyboard.press('End')
  await page.keyboard.type(' discarded')
  await expect(field).toHaveValue('E2E fixture discarded')
  await page.keyboard.press('Escape')
  await expect(field).not.toBeFocused()
  await expect(field).toHaveValue('E2E fixture')
  await rename('Above the house')

  // A second place, named too, then the first again from the list (by the keys, flown in to the
  // search's zoom), then a third by a tap: the third first (opened last), the first, the second.
  await tapAt(0, -200)
  await expect.poll(() => popupRequests(page)).toHaveLength(2)
  await rename('Bus stop')
  await page.getByRole('combobox').tap()
  await expect(options).toHaveCount(2)
  await expect(options.nth(0)).toContainText('Bus stop')
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('Enter')
  await expect.poll(() => popupRequests(page)).toHaveLength(3)
  await expect(options).toHaveCount(0)
  await expect(field).toHaveValue('Above the house')
  await expect(page).toHaveURL(/z=14\.00/)
  await tapAt(-60, -220)
  await expect.poll(() => popupRequests(page)).toHaveLength(4)
  const search = page.getByRole('combobox')
  await search.tap()
  await expect(options).toHaveCount(3)
  await expect(options.nth(0)).toContainText('E2E fixture')
  await expect(options.nth(1)).toContainText('Above the house')
  await expect(options.nth(2)).toContainText('Bus stop')
  // A tap on the map (between the list and the sheet) while the list is open only closes it.
  await tapAt(0, -100)
  await expect(options).toHaveCount(0)
  expect(await popupRequests(page)).toHaveLength(4)
  // Typed, the list names the places of the history it matches, from one letter.
  await search.fill('b')
  await expect(options).toHaveCount(2)
  await search.fill('bus')
  await expect(options).toHaveCount(1)
  // A tap on a row opens its place, its name the field again; the place opened once just before stands
  // second, above the one opened twice (owner 2026-10-30: a place searched once was gone from the top
  // after the next click).
  await options.nth(0).getByRole('button', { name: /^Bus stop/ }).tap()
  await expect.poll(() => popupRequests(page)).toHaveLength(5)
  await expect(field).toHaveValue('Bus stop')
  await search.tap()
  await expect(options).toHaveCount(3)
  await expect(options.nth(0)).toContainText('Bus stop')
  await expect(options.nth(1)).toContainText('E2E fixture')
  await expect(options.nth(2)).toContainText('Above the house')
  // Escape closes the list and brings the open place's row back.
  await page.keyboard.press('Escape')
  await expect(options).toHaveCount(0)
  await expect(field).toHaveValue('Bus stop')
  // From the field the keys forget a place (Shift+Delete).
  await search.fill('')
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('Shift+Delete')
  await expect(options).toHaveCount(2)
  await expect(options.nth(1)).toContainText('E2E fixture')

  // A new document (a changed, ignored query string) and the same point: the name is kept; emptied, it
  // gives the geocoded one back.
  await openAtPoint(`/?e2e=title${mapUrl(POINT).slice(1)}`)
  await expect(field).toHaveValue('Above the house')
  await field.tap()
  await field.fill('')
  await page.keyboard.press('Enter')
  await expect(field).toHaveValue('E2E fixture')
})

test('a name left as it was is not kept, though the geocoded name came while it was being written', async ({ page }) => {
  await installHermeticMap(page, POINT)
  let release = () => {}
  const late = new Promise<void>(resolve => { release = resolve })
  await page.route('**/api/reverse?**', async route => {
    await late
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ place: 'Late name' }) })
  })
  const row = await openPoint(page)
  const field = row.getByTestId('place-title')
  await field.tap()
  await expect(field).toHaveValue(/^[\d.]+, [\d.]+$/)
  release()
  await expect(field).toHaveAttribute('placeholder', 'Late name')
  await page.keyboard.press('Enter')
  await expect(field).toHaveValue('Late name')
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem('quietmap.place-titles') ?? '[]'))).toEqual([])
})

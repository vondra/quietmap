// The visitor's path in a real browser, without a backend: heatmap hover, the streamed popup (its
// refinement, errors and aborts, the loudest flights, an opened row's segments), the detailed
// calculation, layer switches, search and the locate button; a phone's way in `phone.spec.ts`.
import { expect, test } from '@playwright/test'
import { combineLayerCells } from '../src/lib/hm3-combine'
import { HM3_BYTE_SONE } from '../src/lib/hm3-decoder'
import { fmtSone } from '../src/utils/formatters'
import { computedPiece, FIXTURE_FLIGHTS, pieceRun, popupUpdate, roadContributor, withAircraft } from './answers'
import {
  FIXTURE_DB,
  POINT,
  RAILWAY_SONE,
  ROAD_SONE,
  SOURCE_DB,
  TILE_Z,
  TOTAL_SONE,
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
/** A one-cell grid at `sone`, as the fixture's tiles carry it. */
const soneCells = (sone: number) => Uint8Array.of(Math.round(80 + 20 * Math.log2(sone)))
const ALL_LAYERS = 'road,railway,industrial,building,ship,aircraft'
const lden = (page: import('@playwright/test').Page) => page.locator('[data-testid="lden"]:visible')

test('desktop: hover reads the painted cell, the popup redraws on every streamed update', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const totalTile = `/api/tiles/b1/total/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`
  const tileLoaded = page.waitForResponse(response =>
    new URL(response.url()).pathname === totalTile && response.status() === 200,
  )
  // Every layer on shows the total. Overzoom one exact z12 receiver so its single audible cell
  // spans multiple screen pixels: a one-cell spatial shift still misses the canvas centre, and the
  // linear texture filter dilutes the centre pixel far less.
  await page.goto(mapUrl(POINT, ALL_LAYERS, TILE_Z + 2))
  await tileLoaded
  const { canvas, x, y } = await canvasCenter(page)
  await page.mouse.move(x, y)
  await expect(page.getByTestId('heatmap-hover')).toHaveText(`${TOTAL_SONE} sone`)
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
  await expect(lden(page)).toHaveText(`${SOURCE_DB.toFixed(1)} dB Lden`)
  await expect(page.locator('[data-testid="popup-refining"]:visible')).toBeVisible()
  // The loudness comes with the final answer only.
  await expect(badge(page)).toHaveText('… sone')
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
  await expect(lden(page)).toHaveText(`${FIXTURE_DB.toFixed(1)} dB Lden`)
  await expect(page.locator('[data-testid="popup-refining"]:visible')).toHaveCount(0)
  // The final answer says how loud the place sounds over the whole day, and each row's share of it.
  await expect(badge(page)).toHaveText('15 sone Nden')
  await expect(popup.locator('[data-testid="row-share"]')).toHaveText(['75 %', '25 %'])
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
  // Each switch turns its layer over: the railway off leaves the road (the only other layer the
  // fixture publishes), the road off and the railway on the railway alone, both with every other
  // layer off their combination, every layer on the total again.
  const layer = (id: string) => page.getByTestId(`layer-${id}`).filter({ visible: true })
  const [road, rail] = [layer('road'), layer('railway')]
  const hover = page.getByTestId('heatmap-hover')
  await expect(road).toHaveAttribute('aria-pressed', 'true')
  await expect(rail).toHaveAttribute('aria-pressed', 'true')
  await rail.click()
  await expect(rail).toHaveAttribute('aria-pressed', 'false')
  await expect(road).toHaveAttribute('aria-pressed', 'true')
  await page.mouse.move(x, y)
  await expect(hover).toHaveText(`${ROAD_SONE.toFixed(1)} sone`)
  await road.click()
  await rail.click()
  await expect(road).toHaveAttribute('aria-pressed', 'false')
  await page.mouse.move(x + 1, y)
  await expect(hover).toHaveText(`${RAILWAY_SONE.toFixed(1)} sone`)
  await road.click()
  for (const id of ['industrial', 'building', 'ship', 'aircraft']) await layer(id).click()
  await page.mouse.move(x, y)
  await expect(hover).toHaveText(`${fmtSone(HM3_BYTE_SONE[combineLayerCells([soneCells(ROAD_SONE), soneCells(RAILWAY_SONE)])[0]])} sone`)
  for (const id of ['industrial', 'building', 'ship', 'aircraft']) await layer(id).click()
  await page.mouse.move(x + 1, y)
  await expect(hover).toHaveText(`${TOTAL_SONE} sone`)
})

test('desktop: an error line replaces the partial answer, a new click aborts the old request', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)

  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  await sendPopupLine(page, popupUpdate(1, true, POINT.lat, POINT.lng, SOURCE_DB))
  await expect(lden(page)).toHaveText(`${SOURCE_DB.toFixed(1)} dB Lden`)
  await sendPopupLine(page, { error: 'The noise computation failed at this point.' })
  await endPopup(page)
  await expect(page.locator('[data-testid="detail-popup-error"]:visible'))
    .toHaveText('The noise computation failed at this point.')
  await expect(badge(page)).toHaveCount(0)

  // A click elsewhere while the answer is still streaming aborts that request.
  await page.mouse.click(x - 40, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(2)
  await sendPopupLine(page, popupUpdate(1, true, POINT.lat, POINT.lng, SOURCE_DB))
  await expect(lden(page)).toHaveText(`${SOURCE_DB.toFixed(1)} dB Lden`)
  await page.mouse.click(x + 40, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(3)
  await expect.poll(() => abortedPopupRequests(page)).toBe(1)
  await expect(page.locator('[data-testid="detail-popup-skeleton"]:visible')).toBeVisible()
})

// Inside a building the popup shows the level at the building's loudest façade receiver; where
// that receiver is reads in the detailed calculation, not above the sources, with the levels
// exceeded in each period. The weather's hover leads to the About section that tells how to read
// its tables, opened on that section.
test('desktop: the detailed calculation tells the façade receiver, the percentile levels and how to read the weather', async ({ page }) => {
  await installHermeticMap(page, POINT)
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
  const weather = {
    favourable_percent: [0, 1, 2].map(period => Array.from({ length: 16 }, (_, sector) => 20 + 10 * period + sector)),
    alpha_db_per_km: [0.11, 0.37, 1.01, 2.06, 3.9, 9.42, 30, 103.48],
  }
  await sendPopupLine(page, { ...popupUpdate(1, false, POINT.lat, POINT.lng, FIXTURE_DB, building), weather })
  await expect(lden(page)).toHaveText(`${FIXTURE_DB.toFixed(1)} dB Lden`)
  await expect(page.locator('[data-testid="building-exposure"]')).toHaveCount(0)
  await page.locator('[data-testid="calculation-toggle"]:visible').click()
  const calculation = page.locator('[data-testid="calculation"]:visible')
  await expect(calculation).toContainText(/Façade\s*facing SE, loudest of 12 façade points/)
  const percentiles = calculation.getByRole('table').filter({ hasText: 'Percentile levels' })
  await expect(percentiles.locator('tbody tr').nth(0).locator('td')).toHaveText(['Day', '67.0', '66.0', '61.0', '57.0'])
  await expect(percentiles.locator('tbody tr').nth(2).locator('td')).toHaveText(['Night', '60.0', '59.0', '54.0', '48.0'])
  // The calculation computes nothing again.
  expect(await popupRequests(page)).toHaveLength(1)
  await calculation.getByText('Weather', { exact: true }).hover()
  const howToRead = page.getByRole('tooltip').getByRole('link', { name: 'How to read it' })
  await expect(howToRead).toHaveAttribute('href', '/about/noise-at-a-point#the-weather')
  const [about] = await Promise.all([page.context().waitForEvent('page'), howToRead.click()])
  await expect(about.getByRole('heading', { name: 'The weather', level: 2 })).toBeInViewport()
})

// A section link's fragment is the visitor's text: one that no escape decodes names no section.
test('an About page opened at a broken section link shows', async ({ page }) => {
  const errors: Error[] = []
  page.on('pageerror', error => errors.push(error))
  await page.goto('/about/noise-at-a-point#%')
  const weather = page.getByRole('heading', { name: 'The weather', level: 2 })
  await expect(weather).toBeVisible()
  // The page seeks the section once it shows: the throw there blanked it a moment later.
  await afterPaint(page)
  await expect(weather).toBeVisible()
  expect(errors).toEqual([])
})

// An opened row computes its segments when asked: the same click again with the row's parts, whose
// answer never replaces the click's own. How all of its sound arrives in calm air, its loudest
// segments, and an opened one's sound path, rays and terrain profile.
test('desktop: an opened row computes its segments, how they arrive and their rays', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  await sendPopupLine(page, popupUpdate(1, false, POINT.lat, POINT.lng, FIXTURE_DB))
  await endPopup(page)
  const popup = page.locator('[data-testid="detail-popup"]:visible')
  await popup.getByRole('button', { name: /^Fixture street/ }).click()
  await popup.getByRole('button', { name: /^Segments/ }).click()
  await expect.poll(async () => (await popupRequests(page))[1])
    .toEqual({ lat: POINT.lat, lng: POINT.lng, source: '00000000000000aa' })
  const segments = popup.getByTestId('segments')
  await expect(segments).toContainText('computing…')
  const street = roadContributor(FIXTURE_DB)
  await sendPopupLine(page, {
    ...popupUpdate(2, false, POINT.lat, POINT.lng, FIXTURE_DB + 5),
    arrival: {
      pieces: 3,
      edges: [{ edge: 'open', share: 0.75, screening_db: 0 }, { edge: 'buildings', share: 0.25, screening_db: -12.3 }],
    },
    pieces: [computedPiece(street, 61.2, 12), computedPiece(street, 55.4, 40)],
  })
  await endPopup(page)
  // The run's own levels are not the popup's.
  await expect(lden(page)).toHaveText(`${FIXTURE_DB.toFixed(1)} dB Lden`)
  await expect(popup.getByRole('button', { name: /^Segments/ })).toHaveText('Segments (3)')
  const arrival = segments.getByRole('table').filter({ hasText: 'In calm air' })
  await expect(arrival.locator('tbody tr')).toHaveText([/Line of sight\s*75 %\s*0\.0/, /Over buildings or walls\s*25 %\s*−12\.3/])
  // The two loudest of three, each by its direction and distance with the Lden it delivers.
  await expect(segments).toContainText('2 loudest')
  const listed = segments.getByRole('button', { name: /●/ })
  await expect(listed).toHaveText([/●\s*E\s*12 m\s*61\.2/, /●\s*E\s*40 m\s*55\.4/])
  await listed.nth(1).click()
  // Opened, a segment is computed again on its own, every ray with the terrain under it.
  await expect.poll(async () => (await popupRequests(page))[2])
    .toEqual({ lat: POINT.lat, lng: POINT.lng, source: '00000000000000aa', piece: 1 })
  const segment = segments.getByTestId('segment')
  await expect(segment).toContainText(/Length\s*45 m/)
  await expect(segment).toContainText('Sound path, dB Lden')
  // Its four rays, the southernmost behind the building: 20 dB taken in calm air, 8.5 downwind.
  const rays = segment.getByTestId('rays').getByRole('button')
  await expect(rays).toHaveCount(4)
  await expect(rays.nth(0)).toHaveText(/^●\s*1\s*−20\.0\s*−8\.5\s*−1\.5\s*\+0\.3\s*24\.9$/)
  await rays.nth(0).click()
  const ray = segment.getByTestId('ray')
  await expect(ray).toContainText(/Length\s*22 m/)
  await expect(ray).toContainText('computing…')
  await sendPopupLine(page, pieceRun(popupUpdate(3, false, POINT.lat, POINT.lng, FIXTURE_DB + 5), computedPiece(street, 55.4, 40)))
  await endPopup(page)
  await expect(ray).toContainText(/Angle\s*17\.2°/)
  await expect(ray).toContainText(/Terrain profile\s*350 → 354 m a\.s\.l\./)
  await expect(ray.getByRole('img')).toBeVisible()
  await expect(ray).toContainText('building')
  await expect(ray).not.toContainText('barrier')
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
  await page.getByRole('combobox').fill('Ruzyně')
  const option = page.getByRole('option', { name: new RegExp(target.display_name) })
  await expect(option).toBeVisible()
  // A press anywhere in the row opens its place, its empty end too.
  await option.click({ position: { x: (await option.boundingBox())!.width - 8, y: 24 } })
  // The hash carries four decimals; anchor the complete values.
  await expect(page).toHaveURL(/lat=50\.1000&lng=14\.2600&/)
  await expect.poll(() => popupRequests(page)).toEqual([{ lat: target.lat, lng: target.lon }])
  // The place keeps the result's name, not the geocoder's for its point.
  await expect(page.getByTestId('open-place').getByTestId('place-title')).toHaveValue(target.display_name)
})

test('desktop: a press of the locate button after the visitor has moved opens the popup at the new place', async ({ page, context }) => {
  await context.grantPermissions(['geolocation'])
  await context.setGeolocation({ latitude: POINT.lat, longitude: POINT.lng, accuracy: 10 })
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT, 'road', TILE_Z - 2))
  await canvasCenter(page)
  const locate = page.locator('button.maplibregl-ctrl-geolocate')
  await locate.click()
  const first = { lat: POINT.lat, lng: POINT.lng }
  await expect.poll(() => popupRequests(page)).toEqual([first])
  // Off while following, then on again where the visitor has walked to: the popup opens at the
  // new position, not at the last one before the control finds it.
  await locate.click()
  const walked = { lat: POINT.lat + 0.01, lng: POINT.lng + 0.01 }
  await context.setGeolocation({ latitude: walked.lat, longitude: walked.lng, accuracy: 10 })
  await locate.click()
  await expect.poll(() => popupRequests(page)).toEqual([first, walked])
})

test('desktop: the layers and the sign-in are two tabs of the corner card, its form one a browser fills', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.route('**/api/me', route => route.fulfill({ status: 401, contentType: 'application/json', body: '{"error":"Not signed in"}' }))
  await page.goto(mapUrl(POINT))
  const layers = page.getByRole('tab', { name: 'Layers' })
  const signIn = page.getByRole('tab', { name: 'Sign in' })
  await expect(layers).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByTestId('layer-road').filter({ visible: true })).toBeVisible()
  await signIn.click()
  await expect(signIn).toHaveAttribute('aria-selected', 'true')
  // What a browser's autofill reads: a form, a labelled e-mail field named and marked for it, a submit.
  const email = page.getByLabel('Sign in with your e-mail')
  await expect(email).toHaveAttribute('type', 'email')
  await expect(email).toHaveAttribute('name', 'email')
  await expect(email).toHaveAttribute('autocomplete', 'email')
  await expect(page.locator('form[name="sign-in"] button[type="submit"]')).toHaveText('Send me a link')
  await expect(email).not.toBeFocused()
  // The arrow keys move between the tabs, the open one the tab stop.
  await page.keyboard.press('ArrowLeft')
  await expect(layers).toBeFocused()
  await expect(layers).toHaveAttribute('aria-selected', 'true')
  await expect(signIn).toHaveAttribute('tabindex', '-1')
  await page.keyboard.press('ArrowRight')
  await expect(signIn).toBeFocused()
  await expect(email).toBeVisible()
  // A click on the map opens a popup, which folds the card to its tabs.
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect(email).toHaveCount(0)
  await expect(signIn).toBeVisible()
})

test("desktop: the reachable area is drawn from the open place by the button at the search field's end; a new one drops the one on its way", async ({ page }) => {
  await installHermeticMap(page, POINT)
  const asked: string[] = []
  await page.route('**/api/isochron?**', async route => {
    asked.push(new URL(route.request().url()).search)
    // The first answer comes late, after the second was asked.
    if (asked.length === 1) await new Promise(resolve => setTimeout(resolve, 1500))
    const ring = [[POINT.lng - 0.01, POINT.lat - 0.01], [POINT.lng + 0.01, POINT.lat - 0.01], [POINT.lng, POINT.lat + 0.01], [POINT.lng - 0.01, POINT.lat - 0.01]]
    const modes = new URL(route.request().url()).searchParams.get('modes')!.split(',')
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({
      type: 'FeatureCollection', features: modes.map(mode => ({ type: 'Feature', geometry: { type: 'Polygon', coordinates: [ring] }, properties: { mode, minutes: 60 } })),
    }) }).catch(() => {})
  })
  const dropped = page.waitForEvent('requestfailed', request => request.url().includes('/api/isochron?'))
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  const button = page.getByRole('button', { name: 'Reachable area' })
  const panel = page.getByTestId('open-place').getByTestId('isochron-panel')
  await button.click()
  await panel.getByRole('button', { name: 'Car' }).click()
  await panel.getByRole('button', { name: 'Show area' }).click()
  await expect.poll(() => asked).toHaveLength(1)
  const query = new URLSearchParams(asked[0])
  expect([query.get('minutes'), query.get('modes')]).toEqual(['60', 'walk'])
  expect(Number(query.get('lat'))).toBeCloseTo(POINT.lat, 6)
  // Folded and opened again while the first is on its way, the panel asks anew.
  await button.click()
  await button.click()
  await panel.getByRole('spinbutton', { name: 'Minutes' }).fill('5')
  await panel.getByRole('button', { name: 'Show area' }).click()
  expect((await dropped).failure()?.errorText).toContain('ABORTED')
  expect(new URLSearchParams(asked[1]).get('minutes')).toBe('5')
  await expect(panel.getByRole('button', { name: 'Show area' })).toBeEnabled()
  await expect(panel.getByRole('alert')).toHaveCount(0)
})

test('the dark scheme follows the system: the controls darken, the base map stays light', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' })
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const search = page.getByRole('combobox')
  await expect(search).toBeVisible()
  const background = () => search.evaluate(input => getComputedStyle(input).backgroundColor)
  expect(await background()).toBe('oklch(0.145 0 0)')
  await page.emulateMedia({ colorScheme: 'light' })
  expect(await background()).toBe('oklch(1 0 0)')
})

// The visitor's path in a real browser, without a backend: heatmap hover, the streamed popup (its
// refinement, errors and aborts, the loudest flights, an opened row's segments), the detailed
// calculation, layer switches, search, and the phone sheet.
import { expect, test } from '@playwright/test'
import { combineLayerCells } from '../src/lib/hm3-combine'
import { HM3_BYTE_SONE } from '../src/lib/hm3-decoder'
import { fmtSone } from '../src/utils/formatters'
import { computedPiece, FIXTURE_FLIGHTS, pieceRun, popupUpdate, roadContributor, withAircraft } from './answers'
import {
  FIXTURE_DB,
  PHONE,
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
// exceeded in each period.
test('desktop: the detailed calculation tells the façade receiver and the percentile levels', async ({ page }) => {
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
  await sendPopupLine(page, popupUpdate(1, false, POINT.lat, POINT.lng, FIXTURE_DB, building))
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
    await page.goto(`/?e2e=layers${mapUrl(POINT).slice(1)}`)
    await canvasCenter(page)
    // The credits' ⓘ in the control row opens them with the way to the About pages first.
    await page.locator('.maplibregl-ctrl-attrib-button').tap()
    await expect(page.locator('.attrib-about')).toBeVisible()
    await expect(page.locator('.attrib-about')).toHaveAttribute('href', '/about')
    await page.locator('.maplibregl-ctrl-attrib-button').tap()
    await page.getByRole('button', { name: 'Toggle layers panel' }).tap()
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
})

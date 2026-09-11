import { expect, test } from '@playwright/test'
import { afterPaint, canvasCenter, deferred, hm3PixelCenter, installHermeticMap, mapUrl, pngCenterPixel } from './support'

// Visitor-path coverage (owner ask 2026-07-20): search → fly-to, Reachable-in isochrone,
// desktop layer toggles, and map zoom — the flows a first-time visitor actually touches,
// hermetic so they run in CI without a live backend or real tiles.
const POINT = hm3PixelCenter(49.8486, 14.1639)
const TARGET = { display_name: 'Ruzyně Airport', secondary: 'Prague, CZ', lat: 50.1, lon: 14.26 }
// The mocked reachable area sits north of the searched point, so fitting the
// map to it moves the shared view somewhere the search fly-to never goes.
const AREA_CENTER = { lat: 50.16, lon: 14.26 }

async function mockSearch(page: import('@playwright/test').Page) {
  await page.route('**/api/search?**', async route => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([TARGET]),
    })
  })
}

test('search: picking a result flies the map to it', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await mockSearch(page)
  await page.goto(mapUrl(POINT))
  await page.getByRole('searchbox').fill('Ruzyně')
  const option = page.getByRole('option', { name: new RegExp(TARGET.display_name) })
  await expect(option).toBeVisible()
  await option.click()
  // The hash carries four decimals; anchor the complete values.
  await expect(page).toHaveURL(/lat=50\.1000&lng=14\.2600&/)
})

test('isochron: search → panel → Show area calls the API and flies to the polygon', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await mockSearch(page)
  const isochronRequested = deferred()
  let apiQuery: Record<string, string> = {}
  await page.route('**/api/isochron?**', async route => {
    const url = new URL(route.request().url())
    apiQuery = Object.fromEntries(url.searchParams.entries())
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        type: 'Feature',
        properties: { contour: 15, modes: ['walk'], time: 15 },
        geometry: {
          type: 'Polygon',
          coordinates: [[
            [AREA_CENTER.lon - 0.01, AREA_CENTER.lat - 0.01],
            [AREA_CENTER.lon + 0.01, AREA_CENTER.lat - 0.01],
            [AREA_CENTER.lon + 0.01, AREA_CENTER.lat + 0.01],
            [AREA_CENTER.lon - 0.01, AREA_CENTER.lat + 0.01],
            [AREA_CENTER.lon - 0.01, AREA_CENTER.lat - 0.01],
          ]],
        },
      }),
    })
    isochronRequested.resolve()
  })

  await page.goto(mapUrl(POINT))
  await page.getByRole('searchbox').fill('Ruzyně')
  await page.getByRole('option', { name: new RegExp(TARGET.display_name) }).click()
  await page.getByRole('button', { name: 'Toggle isochron' }).click()
  await expect(page.getByText('Reachable in')).toBeVisible()
  await page.getByRole('button', { name: 'Show area' }).click()
  await isochronRequested.promise
  expect(Number(apiQuery.lat)).toBeCloseTo(TARGET.lat, 6)
  expect(Number(apiQuery.lng)).toBeCloseTo(TARGET.lon, 6)
  expect(apiQuery.time).toBe('60')
  expect(apiQuery.modes).toBe('walk,car')
  // fitBounds to the drawn polygon moves the shared view to its centre, and
  // the blue area fill now covers the canvas centre over the black basemap.
  await expect(page).toHaveURL(new RegExp(`lat=${AREA_CENTER.lat}`))
  const { canvas } = await canvasCenter(page)
  await expect.poll(async () => {
    await afterPaint(page)
    const [r, , b] = await pngCenterPixel(page, await canvas.screenshot())
    return b - r
  }).toBeGreaterThan(10)
})

test('isochron: Show area names a downed router instead of leaving the map blank', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await mockSearch(page)
  await page.route('**/api/isochron?**', async route => {
    await route.fulfill({
      status: 502,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'Valhalla walk: 502' }),
    })
  })
  await page.goto(mapUrl(POINT))
  await page.getByRole('searchbox').fill('Ruzyně')
  await page.getByRole('option', { name: new RegExp(TARGET.display_name) }).click()
  await page.getByRole('button', { name: 'Toggle isochron' }).click()
  await page.getByRole('button', { name: 'Show area' }).click()
  await expect(page.getByRole('alert')).toHaveText('Could not draw the area. The routing service is down.')
})

test('isochron: Reachable in, min and Walk stay on one row', async ({ page }) => {
  // Safari's number spinners plus flex-wrap + ml-auto used to put "Reachable in"
  // on one line and "min Walk Car" on the next. Default 1280 lets the panel sit
  // at search-bar max-w-md; a 390 px run hid the radar toggle on the second pass.
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  await page.getByRole('button', { name: 'Toggle isochron' }).click()
  const label = page.getByText('Reachable in')
  const minutes = page.getByText('min', { exact: true })
  const walk = page.getByTestId('isochron-walk')
  await expect(label).toBeVisible()
  const labelBox = await label.boundingBox()
  const minBox = await minutes.boundingBox()
  const walkBox = await walk.boundingBox()
  expect(labelBox && minBox && walkBox).toBeTruthy()
  expect(Math.abs(labelBox!.y - minBox!.y), 'min must sit on the Reachable-in row').toBeLessThan(6)
  expect(Math.abs(labelBox!.y - walkBox!.y), 'Walk must sit on the Reachable-in row').toBeLessThan(6)
})

test('narrow desktop: the layers card does not swallow the isochron toggle', async ({ page }) => {
  // 1024x768 — iPad landscape, small laptops, a half-screen window. Below ~1096 px
  // the centred search bar slides under the 320 px card column on its right.
  await page.setViewportSize({ width: 1024, height: 768 })
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  await page.getByRole('button', { name: 'Toggle isochron' }).click()
  await expect(page.getByText('Reachable in')).toBeVisible()
})

test('desktop layers panel: toggling a layer rewrites the overlay URL state', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT, 'road,rail'))
  // Desktop shows the control card (with the layers panel) directly — no toggle button
  // (that's the mobile sheet).
  const panel = page.locator('[data-testid="layers-panel"]:visible')
  await expect(panel).toBeVisible()
  const rail = panel.getByTestId('layer-rail')
  await expect(rail).toHaveAttribute('aria-pressed', 'true')
  await rail.click()
  await expect(rail).toHaveAttribute('aria-pressed', 'false')
  await expect(page).toHaveURL(/ro=road(?!,rail)/)
})

test('map zoom: double-click zooms in and updates the shared URL zoom', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.dblclick(x, y)
  await expect(page).toHaveURL(/z=13/)
})

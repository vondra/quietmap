import { expect, test } from '@playwright/test'
import {
  FIXTURE_DB,
  SOURCE_DB,
  TILE_Z,
  deferred,
  hm3PixelCenter,
  installHermeticMap,
  mapUrl,
  popupFixture,
} from './support'

// Visitor-path coverage (owner ask 2026-07-20): search → fly-to, Reachable-in isochrone,
// desktop layer toggles, and map zoom — the flows a first-time visitor actually touches,
// hermetic so they run in CI without a live backend or real tiles.
const POINT = hm3PixelCenter(49.8486, 14.1639)
const TARGET = { display_name: 'Ruzyně Airport', secondary: 'Prague, CZ', lat: 50.1, lon: 14.26 }

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
  await expect(page).toHaveURL(new RegExp(`lat=${TARGET.lat}.*lng=${TARGET.lon}|lng=${TARGET.lon}.*lat=${TARGET.lat}`))
})

test('isochron: search → panel → Show area calls the API and flies to the polygon', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await mockSearch(page)
  const isochronRequested = deferred()
  let apiQuery: Record<string, string> = {}
  await page.route('**/api/isochron?**', async route => {
    const url = new URL(route.request().url())
    apiQuery = Object.fromEntries(url.searchParams.entries())
    isochronRequested.resolve()
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        type: 'Feature',
        properties: { contour: 15, modes: ['walk'], time: 15 },
        geometry: {
          type: 'Polygon',
          coordinates: [[
            [TARGET.lon - 0.01, TARGET.lat - 0.01],
            [TARGET.lon + 0.01, TARGET.lat - 0.01],
            [TARGET.lon + 0.01, TARGET.lat + 0.01],
            [TARGET.lon - 0.01, TARGET.lat + 0.01],
            [TARGET.lon - 0.01, TARGET.lat - 0.01],
          ]],
        },
      }),
    })
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
  // fitBounds to the polygon around the target moves the shared view state there.
  await expect(page).toHaveURL(new RegExp(`lat=${TARGET.lat}`))
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
  const noise = deferred()
  await page.route('**/api/noise-onfly-v2?**', async route => {
    noise.resolve()
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(popupFixture(POINT.lat, POINT.lng, SOURCE_DB)),
    })
  })
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
  const canvas = page.locator('canvas').first()
  await expect(canvas).toBeVisible()
  const box = await canvas.boundingBox()
  if (!box) throw new Error('no canvas box')
  await page.mouse.dblclick(box.x + box.width / 2, box.y + box.height / 2)
  await expect(page).toHaveURL(/z=13/)
})

import { devices, expect, test } from '@playwright/test'
import {
  FIXTURE_DB,
  SOURCE_DB,
  TILE_Z,
  afterPaint,
  canvasCenter,
  deferred,
  hm3PixelCenter,
  installHermeticMap,
  mapUrl,
  pngCenterPixel,
  popupFixture,
} from './support'

const POINT = hm3PixelCenter(49.8486, 14.1639)

test('desktop: rendered HM3 hover and clicked popup agree', async ({ page }) => {
  await installHermeticMap(page, POINT)
  const releaseNoise = deferred()
  const noiseRequested = deferred()
  let query: { lat: number; lng: number } | null = null
  await page.route('**/api/noise-onfly-v2?**', async route => {
    const url = new URL(route.request().url())
    query = { lat: Number(url.searchParams.get('lat')), lng: Number(url.searchParams.get('lng')) }
    noiseRequested.resolve()
    await releaseNoise.promise
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(popupFixture(query.lat, query.lng, FIXTURE_DB, {
        road: SOURCE_DB,
        railway: SOURCE_DB,
      })),
    })
  })

  const expectedTiles = ['road', 'rail'].map(source =>
    `/api/tiles/b1/${source}/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`)
  const tilesLoaded = expectedTiles.map(path => page.waitForResponse(response =>
    new URL(response.url()).pathname === path && response.status() === 200,
  ))
  // Overzoom one exact z12 receiver so its single audible cell spans multiple
  // screen pixels. A one-cell spatial shift still misses the canvas centre,
  // while production's linear texture filter no longer dilutes the assertion.
  await page.goto(mapUrl(POINT, 'road,rail', TILE_Z + 2))
  await Promise.all(tilesLoaded)
  const { canvas, x, y } = await canvasCenter(page)
  await page.mouse.move(x, y)
  await expect(page.getByTestId('heatmap-hover')).toHaveText(`Lden: ${FIXTURE_DB.toFixed(1)} dB`)
  let combinedPixel: number[] = []
  await expect.poll(async () => {
    await afterPaint(page)
    combinedPixel = await pngCenterPixel(page, await canvas.screenshot())
    return Math.min(combinedPixel[0] - combinedPixel[1], combinedPixel[0] - combinedPixel[2]) > 35
  }).toBe(true)

  await page.mouse.click(x, y)
  await noiseRequested.promise
  await expect(page.locator('[data-testid="detail-popup-skeleton"]:visible')).toBeVisible()
  expect(query).not.toBeNull()
  expect(Math.abs(query!.lat - POINT.lat)).toBeLessThan(1e-6)
  expect(Math.abs(query!.lng - POINT.lng)).toBeLessThan(1e-6)

  releaseNoise.resolve()
  await expect(page.locator('[data-testid="noise-badge"]:visible'))
    .toHaveText(`${FIXTURE_DB.toFixed(1)} dB`)

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
  let roadPixel: number[] = []
  await expect.poll(async () => {
    await afterPaint(page)
    roadPixel = await pngCenterPixel(page, await canvas.screenshot())
    const redDominance = roadPixel[0] - roadPixel[1]
    // Weninger palette (2026-07-17): more energy reads REDDER (R−G grows with dB), and the
    // bilinear upsample dilutes both pixels ~equally, so the old fixed RGB distance (>15) and
    // >35 dominance no longer fit. Combined 60+60≈63 dB must beat road-only 60 dB on R−G.
    return redDominance > 25 && (combinedPixel[0] - combinedPixel[1]) > redDominance
  }).toBe(true)

  await road.click()
  await expect(road).toHaveAttribute('aria-pressed', 'false')
  await expect(page.getByTestId('heatmap-hover')).toHaveCount(0)
  await expect.poll(async () => {
    await afterPaint(page)
    const emptyPixel = await pngCenterPixel(page, await canvas.screenshot())
    return Math.max(...emptyPixel.slice(0, 3))
  }).toBeLessThanOrEqual(2)
})

// Owner decision 2026-09-02: inside a building every level the popup publishes
// is the indoor estimate, so the popup must also SAY that, and name the facade
// level and the envelope step it took.
test('desktop: an enclosed point reads as indoors, naming the facade level and the step', async ({ page }) => {
  const ENVELOPE_DELTA_DB = 20
  const INDOOR_DB = FIXTURE_DB - ENVELOPE_DELTA_DB
  // The map paints the indoor value at an enclosed pixel, so the fixture does
  // too: this test is the popup half of that one agreed number.
  await installHermeticMap(page, POINT, INDOOR_DB)
  await page.route('**/api/noise-onfly-v2?**', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify(popupFixture(POINT.lat, POINT.lng, INDOOR_DB, { road: INDOOR_DB }, {
      envelope_class: 'default',
      envelope_delta_db: ENVELOPE_DELTA_DB,
      facade_lden: FIXTURE_DB,
      indoor_lden_tilted: FIXTURE_DB - 15,
    })),
  }))

  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)

  await expect(page.locator('[data-testid="noise-badge"]:visible'))
    .toHaveText(`${INDOOR_DB.toFixed(1)} dB`)
  const notice = page.locator('[data-testid="indoor-calculation"]:visible')
  await expect(notice).toContainText('Inside a building')
  await expect(notice).toContainText(`${FIXTURE_DB.toFixed(1)} dB fa\u00e7ade`)
  await expect(notice).toContainText(`${ENVELOPE_DELTA_DB} dB`)
})

test.describe('mobile', () => {
  test.use({
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 1,
    userAgent: devices['Pixel 5'].userAgent,
    isMobile: true,
    hasTouch: true,
  })

  test('map tap opens the real mobile sheet and layer controls', async ({ page }) => {
    await installHermeticMap(page, POINT)
    const releaseNoise = deferred()
    const noiseRequested = deferred()
    let query: { lat: number; lng: number } | null = null
    await page.route('**/api/noise-onfly-v2?**', async route => {
      const url = new URL(route.request().url())
      const lat = Number(url.searchParams.get('lat'))
      const lng = Number(url.searchParams.get('lng'))
      query = { lat, lng }
      noiseRequested.resolve()
      await releaseNoise.promise
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(popupFixture(lat, lng, SOURCE_DB)),
      })
    })

    const tilePath = `/api/tiles/b1/road/${TILE_Z}/${POINT.tx}/${POINT.ty}.bin`
    const tileLoaded = page.waitForResponse(response =>
      new URL(response.url()).pathname === tilePath && response.status() === 200,
    )
    await page.goto(mapUrl(POINT))
    await tileLoaded
    const { x, y } = await canvasCenter(page)
    await page.touchscreen.tap(x, y)
    await noiseRequested.promise
    expect(query).not.toBeNull()
    expect(Math.abs(query!.lat - POINT.lat)).toBeLessThan(1e-6)
    expect(Math.abs(query!.lng - POINT.lng)).toBeLessThan(1e-6)
    const sheet = page.getByTestId('mobile-detail-sheet')
    await expect(sheet).toBeVisible()
    await expect(sheet.getByTestId('detail-popup-skeleton')).toBeVisible()

    releaseNoise.resolve()
    await expect(sheet.getByTestId('noise-badge')).toHaveText(`${SOURCE_DB.toFixed(1)} dB`)

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

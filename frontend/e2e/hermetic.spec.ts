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
import { screeningFanPopupFixture } from './screening-fan-fixture'
import type { NoiseComputeData } from '../src/types/noise'

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
  // screen pixels: a one-cell spatial shift still misses the canvas centre,
  // and the linear texture filter dilutes the centre pixel far less.
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

// Inside a building the map and the popup show the level at the building's
// noisiest façade receiver, and the popup says which façade that is.
test('desktop: a point inside a building reads as its noisiest façade', async ({ page }) => {
  await installHermeticMap(page, POINT, FIXTURE_DB)
  const noisiestFacade: NoiseComputeData['building_exposure'] = {
    receiver: [POINT.lat, POINT.lng + 0.0001], facade_bearing_deg: 140, facade_points: 12,
  }
  const noExposedFacade: NoiseComputeData['building_exposure'] = { receiver: null, facade_bearing_deg: null, facade_points: 0 }
  let exposed = true
  await page.route('**/api/noise-onfly-v2?**', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify(exposed
      ? popupFixture(POINT.lat, POINT.lng, FIXTURE_DB, { road: FIXTURE_DB }, noisiestFacade)
      : { ...popupFixture(POINT.lat, POINT.lng, FIXTURE_DB, {}, noExposedFacade), total_lden: null, total_lden_free: null }),
  }))

  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect(page.locator('[data-testid="noise-badge"]:visible')).toHaveText(`${FIXTURE_DB.toFixed(1)} dB`)
  const exposure = page.locator('[data-testid="building-exposure"]:visible')
  await expect(exposure).toContainText('Noisiest façade of this building — faces SE')
  await expect(exposure).toContainText('1 of 12 façade points')

  await page.locator('button[aria-label="Close"]:visible').click()
  exposed = false
  await page.mouse.click(x, y)
  await expect(page.locator('[data-testid="detail-popup"]:visible'))
    .toContainText('Not assessed — this building has no exposed façade')
})

test('desktop: segment screening fan shows angular intervals and the characteristic ray', async ({ page }) => {
  await installHermeticMap(page, POINT, 58.5)
  await page.route('**/api/noise-onfly-v2?**', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify(screeningFanPopupFixture(POINT.lat, POINT.lng)),
  }))

  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect(page.locator('[data-testid="noise-badge"]:visible')).toHaveText('58.5 dB')

  await page.getByRole('button', { name: 'Segments (1)' }).click()
  await page.getByRole('button', { name: /Fan fixture road/ }).click()
  await expect(page.getByText('Screening fan', { exact: true })).toBeVisible()

  const fanValue = page.getByText('5 int. · 22 % blocked', { exact: true })
  await expect(fanValue).toBeVisible()
  // Hover, not click: a click first fires mouseenter (opens) and then toggles the
  // tooltip closed once React has committed that open state — a CI-timing race.
  await fanValue.hover()
  const tooltip = page.getByRole('tooltip')
  await expect(tooltip).toContainText('Arc quadrature · span 50.0° · 22.0 % blocked.')
  await expect(tooltip).toContainText(
    '-3.0–3.0° · blocked · building 8.0 m · ΔL -10.4 dB · characteristic point',
  )
  await expect(tooltip).toContainText('-8.0–-3.0° · blocked · barrier 3.5 m · terrain -1.3 dB · ΔL -7.2 dB')

  await page.keyboard.press('Escape')
  await page.getByText('building 8.0\u00A0m', { exact: true }).hover()
  await expect(page.getByRole('tooltip')).toContainText(
    'evaluates one exact source-point ray per interval, then energy-averages',
  )

  // The map fan legend shows exactly the slice causes present in the fixture:
  // open, building/barrier, and building + terrain (the barrier slice also
  // carries 1.3 dB of terrain on its own ray).
  await page.keyboard.press('Escape')
  await expect(page.getByText('Map:', { exact: true })).toBeVisible()
  await expect(page.getByText('building / barrier')).toBeVisible()
  await expect(page.getByText('building + terrain')).toBeVisible()

  // Layout regression: nothing inside the popup may overflow horizontally —
  // a 4 px bleed once made vertical scrolling pan the card sideways.
  // Allowlist: the kind-filter strip scrolls by design, truncated names
  // ellipsis by design.
  const overflow = await page.evaluate(() => {
    const bad: string[] = []
    for (const el of document.querySelectorAll('[data-testid="detail-popup"] *')) {
      const h = el as HTMLElement
      if (h.scrollWidth > h.clientWidth + 2) {
        const cls = typeof h.className === 'string' ? h.className : ''
        if (cls.includes('overflow-x-auto') || cls.includes('truncate')) continue
        bad.push(`${el.tagName}.${cls.slice(0, 50)}:${(h.textContent ?? '').trim().slice(0, 40)}`)
      }
    }
    return bad
  })
  expect(overflow).toEqual([])
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

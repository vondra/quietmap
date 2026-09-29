import { test, expect, type Page } from '@playwright/test'
import { afterPaint, deferred, hm3PixelCenter, installHermeticMap, mapUrl, pngCenterPixel, popupFixture } from './support'

for (const viewport of [
  { width: 1280, height: 900 },
  { width: 1280, height: 500 },
  { width: 390, height: 844 },
]) {
  test(`opening stay filters reveals options without crossing the scrollbar at ${viewport.width}×${viewport.height}`, async ({ page }) => {
    await page.setViewportSize(viewport)
    const point = hm3PixelCenter(50.0755, 14.4378)
    await installHermeticMap(page, point)
    await page.route('**/api/stay?**', route => route.fulfill({
      status: 200, contentType: 'application/json', body: JSON.stringify({ listings: [], meta: { nights: 2 } }),
    }))
    await page.goto(mapUrl(point))
    if (viewport.width < 768) {
      await page.getByRole('button', { name: 'Toggle layers panel' }).click()
    }
    await page.getByRole('button', { name: 'Places to stay', exact: true }).filter({ visible: true }).click()
    const rating = page.getByTestId('stay-rating').filter({ visible: true })
    await expect(rating).toBeVisible()
    const layout = await rating.evaluate(element => {
      let scroller = element.parentElement!
      while (getComputedStyle(scroller).overflowY !== 'auto') scroller = scroller.parentElement!
      const bounds = scroller.getBoundingClientRect()
      const padding = parseFloat(getComputedStyle(scroller).paddingRight)
      const controls = [...scroller.querySelectorAll<HTMLElement>('button, input')]
      const stayControls = controls.filter(control => control.dataset.testid?.startsWith('stay-'))
      const ratingBounds = element.getBoundingClientRect()
      return {
        optionsVisible: stayControls.every(control => {
          const rect = control.getBoundingClientRect()
          return rect.top >= bounds.top && rect.bottom <= bounds.bottom
        }) && ratingBounds.bottom <= bounds.bottom,
        controlsClearOfScrollbar: controls.every(control =>
          control.getBoundingClientRect().right <= bounds.left + scroller.clientWidth - padding + 1),
        horizontalOverflow: scroller.scrollWidth - scroller.clientWidth,
        pageScroll: document.documentElement.scrollTop,
      }
    })
    expect(layout).toEqual({
      optionsVisible: true,
      controlsClearOfScrollbar: true,
      horizontalOverflow: 0,
      pageScroll: 0,
    })
    await page.getByTestId('stay-adults-plus').filter({ visible: true }).click()
    await expect(page.getByTestId('stay-adults').filter({ visible: true })).toHaveText('3')
  })
}

const STAY_POINT = hm3PixelCenter(50.0755, 14.4378)
const SECOND_STAY = { ...STAY_POINT, lng: STAY_POINT.lng + 360 / (512 * 2 ** 12) * 90 }
const stayListing = (name: string, point = STAY_POINT) => ({
  id: name, name, lat: point.lat, lng: point.lng, thumbnail: null,
  rating: { value: 8.5, count: 100, stars: 4 },
  capacity: { guests: 2, bedrooms: 1 }, freeCancellation: true,
  price: { total: 200, perNight: 100 }, url: 'https://example.com/stay',
})

async function openStayMap(page: Page, second = false) {
  await page.route('**/api/stay?**', route => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({
      listings: [stayListing('First hotel'), ...(second ? [stayListing('Second hotel', SECOND_STAY)] : [])],
      meta: { nights: 2 },
    }),
  }))
  await page.goto(`${mapUrl(STAY_POINT, '')}&stay=1`)
  // The grey dot proves deck has painted the selectable pin, not only fetched it.
  await expect.poll(async () => {
    await afterPaint(page)
    return (await pngCenterPixel(page, await page.screenshot())).slice(0, 3)
  }).toEqual([148, 163, 184])
}

test('accommodation pins stay neutral and read no heatmap cells', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 })
  await installHermeticMap(page, STAY_POINT)
  await page.route('**/api/tiles-manifest', route => route.fulfill({ json: {
    build: 'b1', zoom: 12, layers: { total: { build: 'b1', file: 'total.b1.pmtiles' } },
  } }))
  let heatmapTileRequests = 0
  await page.route('**/api/tiles/b1/total/**', route => {
    heatmapTileRequests++
    const bytes = Buffer.alloc(6 + 512 * 512, 50) // 25 dB cells
    bytes.set([72, 77, 51, 32, 4, 1])
    return route.fulfill({ contentType: 'application/octet-stream', body: bytes })
  })
  await openStayMap(page) // Asserts the rendered pin is neutral grey.
  expect(heatmapTileRequests).toBe(0)
})

for (const width of [1280, 390]) {
  test(`stay card shows the exact point level from one shared request at width ${width}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 })
    let requests = 0
    await installHermeticMap(page, STAY_POINT)
    await page.route('**/api/noise-onfly-v2?**', route => {
      requests++
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(
        popupFixture(STAY_POINT.lat, STAY_POINT.lng, 45, { road: 45 }),
      ) })
    })
    await openStayMap(page)
    await page.mouse.click(width / 2, 450)
    const card = page.getByRole('heading', { name: 'First hotel', exact: true }).filter({ visible: true }).locator('..').locator('..')
    await expect(card.getByRole('status')).toHaveText('Outdoor noise: 45.0 dB Lden')
    await afterPaint(page)
    expect(requests).toBe(1)
    expect(await page.getByRole('heading', { name: 'First hotel', exact: true, includeHidden: true }).count()).toBe(2)
    const artifact = process.env.STAYS_SCREENSHOTS
    if (artifact) await page.screenshot({ path: `${artifact}/card-${testInfo.project.name}-${width}.png` })
  })
}

test('switching hotels discards the previous pending noise result', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 })
  const firstRequested = deferred()
  const releaseFirst = deferred()
  let requests = 0
  await installHermeticMap(page, STAY_POINT)
  await page.route('**/api/noise-onfly-v2?**', async route => {
    requests++
    const first = Math.abs(Number(new URL(route.request().url()).searchParams.get('lng')) - STAY_POINT.lng) < 1e-8
    if (first) {
      firstRequested.resolve()
      await releaseFirst.promise
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(
      popupFixture(STAY_POINT.lat, first ? STAY_POINT.lng : SECOND_STAY.lng, first ? 70 : 45),
    ) }).catch(() => {}) // The first fetch is deliberately aborted when the selection changes.
  })
  await openStayMap(page, true)
  await page.mouse.click(640, 450)
  await firstRequested.promise
  await expect(page.getByRole('status').filter({ visible: true })).toHaveText('Outdoor noise: Calculating…')
  await page.mouse.click(730, 450)
  await expect(page.getByRole('heading', { name: 'Second hotel', exact: true }).filter({ visible: true })).toBeVisible()
  await expect(page.getByRole('status').filter({ visible: true })).toHaveText('Outdoor noise: 45.0 dB Lden')
  releaseFirst.resolve()
  await afterPaint(page)
  await expect(page.getByRole('status').filter({ visible: true })).toHaveText('Outdoor noise: 45.0 dB Lden')
  expect(requests).toBe(2)
})

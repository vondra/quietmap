// A visitor narrows stays by quote and mapped total, opens one beyond reach and clears the area.
import { expect, test, type Page } from '@playwright/test'
import { HM3_VERSION, TILE_PX, paethPrediction } from '../src/lib/hm3-decoder'
import { PHONE, POINT, TILE_Z, afterPaint, canvasCenter, hm3PixelCenter, installHermeticMap, mapUrl, popupRequests } from './support'

const spots = [
  { name: 'Quiet affordable stay', dx: -0.004, dy: 0, total: 200, byte: 60, lden: 70 },
  { name: 'Loud stay', dx: 0, dy: 0, total: 200, byte: 140, lden: 120 },
  { name: 'Above the price limit', dx: 0.004, dy: 0, total: 300.01, byte: 60, lden: 70 },
  { name: 'No mapped noise', dx: -0.004, dy: -0.003, total: 200, byte: 255, lden: 255 },
  { name: 'Outside reach', dx: 0, dy: -0.003, total: 200, byte: 60, lden: 70 },
  { name: 'No quoted price', dx: 0.004, dy: -0.003, total: null, byte: 60, lden: 70 },
].map((p, i) => ({ ...p, ...hm3PixelCenter(POINT.lat + p.dy, POINT.lng + p.dx), id: `h${i}` }))
const listings = spots.map(p => ({
  ...p, stars: 4, score: 8.6, reviews: 100, guests: 2, bedrooms: 1,
  freeCancellation: true, thumbnail: null, url: `https://www.stay22.com/allez/roam/${p.id}`,
}))

function tile(z: number, tx: number, ty: number, lden: boolean, missing: boolean, uniform = false): Buffer {
  const out = Buffer.alloc(6 + TILE_PX * TILE_PX, 255)
  out.write(lden ? 'HMD ' : 'HM3 ', 0)
  out[4] = lden ? 1 : HM3_VERSION
  out[5] = 0
  // Coarse cells deliberately look quiet: a place must be judged at the native zoom.
  if (z < TILE_Z || uniform) out.fill(60, 6)
  if (z === TILE_Z && !missing && !uniform) for (const p of spots) if (p.tx === tx && p.ty === ty) out[6 + p.py * TILE_PX + p.px] = lden ? p.lden : p.byte
  for (let at = TILE_PX * TILE_PX - 1; at >= 0; at--) {
    out[6 + at] -= paethPrediction(at % TILE_PX ? out[6 + at - 1] : 0,
      at >= TILE_PX ? out[6 + at - TILE_PX] : 0, at >= TILE_PX && at % TILE_PX ? out[6 + at - TILE_PX - 1] : 0)
  }
  return out
}

async function fixtures(page: Page, offers = listings, uniformNoise = false) {
  await installHermeticMap(page, POINT)
  await page.route('**/api/me', route => route.fulfill({ status: 503, body: '{}' }))
  await page.route('**/fonts/**', route => route.fulfill({ status: 200, body: '' }))
  let queries = 0
  await page.route('**/api/stay?**', route => {
    queries++
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ listings: offers, nights: 2, currency: 'EUR', expiresIn: 3600, failure: null }) })
  })
  let missingLden = false, failingNoise = false, nativeReads = 0
  await page.route('**/api/tiles/b1/**/*.bin', route => {
    const [, source, z, x, y] = new URL(route.request().url()).pathname.match(/\/b1\/([^/]+)\/(\d+)\/(\d+)\/(\d+)\.bin$/)!
    if (+z === TILE_Z && source === 'total') {
      nativeReads++
      if (failingNoise) return route.fulfill({ status: 503, body: '' })
    }
    return route.fulfill({ status: 200, contentType: 'application/octet-stream', body: tile(+z, +x, +y, source.startsWith('lden-'), missingLden && source.startsWith('lden-'), uniformNoise) })
  })
  const ring = [[POINT.lng - 0.009, POINT.lat - 0.001], [POINT.lng + 0.009, POINT.lat - 0.001],
    [POINT.lng + 0.009, POINT.lat + 0.005], [POINT.lng - 0.009, POINT.lat + 0.005], [POINT.lng - 0.009, POINT.lat - 0.001]]
  await page.route('**/api/isochron?**', route => route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({
    type: 'FeatureCollection', features: [{ type: 'Feature', geometry: { type: 'Polygon', coordinates: [ring] }, properties: { mode: 'walk' } }],
  }) }))
  return { queries: () => queries, nativeReads: () => nativeReads, missingLden: () => { missingLden = true }, failNoise: (fail: boolean) => { failingNoise = fail } }
}

/** Project from the visitor's current URL, without reaching into MapLibre or React state. */
async function position(page: Page, at: { lat: number; lng: number }) {
  const hash = new URLSearchParams(new URL(page.url()).hash.slice(1))
  const [lat, lng, zoom] = ['lat', 'lng', 'z'].map(k => Number(hash.get(k)))
  const y = (v: number) => Math.log(Math.tan(Math.PI / 4 + v * Math.PI / 360))
  const { x: cx, y: cy, canvas } = await canvasCenter(page)
  const scale = 512 * 2 ** zoom
  return { x: cx + (at.lng - lng) / 360 * scale, y: cy + (y(lat) - y(at.lat)) / (2 * Math.PI) * scale, canvas }
}

async function visiblePins(page: Page) {
  const pixels = await Promise.all(spots.map(async at => {
    const { x, y } = await position(page, at)
    return { x: Math.round(x), y: Math.round(y) }
  }))
  await afterPaint(page)
  const shot = await page.locator('canvas.maplibregl-canvas').screenshot()
  return page.evaluate(async ({ image, pixels }) => {
    const img = new Image(); img.src = image; await img.decode()
    const c = document.createElement('canvas'); c.width = img.width; c.height = img.height
    const ctx = c.getContext('2d')!; ctx.drawImage(img, 0, 0)
    return pixels.map(({ x, y }) => {
      const rgb = [...ctx.getImageData(x, y, 1, 1).data].slice(0, 3)
      // Only the reachable fill tints the slate core; the quiet wash stays below the pin.
      return rgb[0] >= 35 && rgb[0] <= 65 && rgb[1] >= 45 && rgb[1] <= 85 && rgb[2] >= 60 && rgb[2] <= 135
    })
  }, { image: `data:image/png;base64,${shot.toString('base64')}`, pixels })
}

async function layers(page: Page, phone: boolean) {
  if (phone) await page.getByRole('button', { name: 'Toggle layers panel', exact: true }).click()
  else await page.getByRole('tab', { name: 'Layers', exact: true }).click()
}
async function closeLayers(page: Page, phone: boolean) {
  if (phone) await page.getByRole('button', { name: 'Drag to close', exact: true }).click()
  else await page.getByRole('button', { name: 'Fold', exact: true }).click()
}

for (const phone of [false, true]) test.describe(phone ? 'phone' : 'desktop', () => {
  if (phone) test.use(PHONE)
  test('price and quiet filter stays; a stay beyond the reachable area remains bookable', async ({ page }) => {
    test.setTimeout(45_000)
    await fixtures(page)
    await page.goto(`${mapUrl(POINT, 'road', TILE_Z - 1)}&stay=1`)
    await expect.poll(() => visiblePins(page)).toEqual([true, true, true, true, true, true])
    if (phone) await layers(page, phone)
    const price = page.getByRole('spinbutton', { name: 'Maximum price per night in euros' }).filter({ visible: true })
    await price.fill('150')
    await page.getByRole('button', { name: 'Quiet zones', exact: true }).filter({ visible: true }).click()
    await page.screenshot({ path: test.info().outputPath(`${phone ? 'phone' : 'desktop'}-filters.png`) })
    await closeLayers(page, phone)
    await expect.poll(() => visiblePins(page)).toEqual([true, false, false, false, true, false])

    // A distinct origin: draw a five-minute walk, then explore a stay beyond it.
    const origin = await position(page, { lat: POINT.lat + 0.0015, lng: POINT.lng })
    await page.mouse.click(origin.x, origin.y)
    await page.getByRole('button', { name: 'Reachable area', exact: true }).click()
    const panel = page.getByTestId('isochron-panel').filter({ visible: true })
    await panel.getByRole('button', { name: 'Car', exact: true }).click()
    await panel.getByRole('spinbutton', { name: 'Minutes', exact: true }).fill('5')
    const fitted = page.waitForResponse(response => new URL(response.url()).pathname === '/api/stay')
    await panel.getByRole('button', { name: 'Show area', exact: true }).click()
    await fitted
    // The shared URL is debounced; preserve the chosen filters before navigating through it.
    await expect(page).toHaveURL(url => new URLSearchParams(url.hash.slice(1)).get('qc') === '1')
    await page.getByRole('button', { name: 'Reachable area', exact: true }).click()
    // Explore beyond the fitted area, as a visitor panning along the countryside would.
    await page.evaluate(({ lat, lng, zoom }) => {
      const params = new URLSearchParams(location.hash.slice(1))
      params.set('lat', String(lat)); params.set('lng', String(lng)); params.set('z', String(zoom))
      location.hash = params.toString()
    }, { lat: POINT.lat, lng: POINT.lng, zoom: TILE_Z })
    await expect.poll(() => visiblePins(page)).toEqual([true, false, false, false, true, false])
    const at = await position(page, spots[4])
    await page.mouse.click(at.x, at.y)
    await expect(page.locator('[data-testid="stay-header"]:visible')).toContainText(spots[4].name)
    expect((await popupRequests(page)).at(-1)).toEqual({ lat: spots[4].lat, lng: spots[4].lng })
    await page.screenshot({ path: test.info().outputPath(`${phone ? 'phone' : 'desktop'}-opened-stay.png`) })
    await page.locator('button[aria-label="Close"]:visible').click()
    await expect.poll(() => visiblePins(page)).toEqual([true, false, false, false, true, false])
    await layers(page, phone)
    const reach = page.getByTestId('stay-reach').filter({ visible: true })
    await expect(reach).toContainText('5 min from')
    await expect(price).toHaveValue('150')
    await reach.getByRole('button', { name: 'Clear reachable area' }).click()
    await closeLayers(page, phone)
    // Return to the wider view: the fitted area can put the outside stay off screen on desktop.
    await page.evaluate(({ lat, lng, zoom }) => {
      const params = new URLSearchParams(location.hash.slice(1))
      params.set('lat', String(lat)); params.set('lng', String(lng)); params.set('z', String(zoom))
      location.hash = params.toString()
    }, { lat: POINT.lat, lng: POINT.lng, zoom: TILE_Z })
    await expect.poll(() => visiblePins(page)).toEqual([true, false, false, false, true, false])
    await page.screenshot({ path: test.info().outputPath(`${phone ? 'phone' : 'desktop'}-quiet-stay.png`) })
  })
})

test('local filter changes reuse quotes; switching to missing Lden cannot keep quiet Nden matches', async ({ page }) => {
  const fixture = await fixtures(page)
  await page.goto(`${mapUrl(POINT, 'road', TILE_Z - 1)}&stay=1`)
  await expect.poll(() => visiblePins(page)).toEqual([true, true, true, true, true, true])
  const before = fixture.queries()
  await page.getByRole('spinbutton', { name: 'Maximum price per night in euros' }).filter({ visible: true }).fill('150')
  await page.getByRole('button', { name: 'Quiet zones', exact: true }).filter({ visible: true }).click()
  await expect.poll(() => visiblePins(page)).toEqual([true, false, false, false, true, false])
  expect(fixture.queries()).toBe(before)
  fixture.missingLden()
  await page.getByText('Advanced', { exact: true }).click()
  await page.getByRole('radio', { name: 'Lden dB', exact: true }).click()
  await expect.poll(() => visiblePins(page)).toEqual([false, false, false, false, false, false])
  await expect(page.getByTestId('stay-note')).toHaveText('No matching places; some have no mapped noise')
  expect(fixture.queries()).toBe(before)
})


test('a wide search keeps point readings beyond the tile cache when a price is edited', async ({ page }) => {
  const offers = Array.from({ length: 72 }, (_, i) => ({ ...listings[0], id: `wide-${i}`,
    ...hm3PixelCenter(POINT.lat + (Math.floor(i / 9) - 3) * 0.08, POINT.lng + (i % 9 - 4) * 0.12) }))
  const fixture = await fixtures(page, offers, true)
  await page.goto(`${mapUrl(POINT, 'road', 8)}&stay=1&qc=1`)
  await expect.poll(fixture.nativeReads).toBe(72)
  await expect(page.getByTestId('stay-note')).toHaveCount(0)
  const price = page.getByRole('spinbutton', { name: 'Maximum price per night in euros' }).filter({ visible: true })
  await price.fill('1')
  await expect(page.getByTestId('stay-note')).toHaveText('No places found with these filters')
  await price.fill('150.5')
  await expect(page.getByTestId('stay-note')).toHaveCount(0)
  expect(fixture.nativeReads()).toBe(72)
  expect(fixture.queries()).toBe(1)
})

test('failed noise reads say so and recover while the visitor leaves the map still', async ({ page }) => {
  await page.clock.install()
  const fixture = await fixtures(page)
  fixture.failNoise(true)
  await page.goto(`${mapUrl(POINT, 'road', TILE_Z - 1)}&stay=1&qc=1`)
  await expect(page.getByTestId('stay-note')).toHaveText('Noise could not be loaded. Retrying…')
  fixture.failNoise(false)
  await page.clock.fastForward(31_000)
  await expect.poll(() => visiblePins(page)).toEqual([true, false, true, false, true, true])
  await expect(page.getByTestId('stay-note')).toHaveCount(0)
})

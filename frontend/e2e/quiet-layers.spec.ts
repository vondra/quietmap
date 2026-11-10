// Quiet areas preserve data colours and map labels across map styles and tilted views.
import { expect, test, type Page } from '@playwright/test'
import { HM3_VERSION, TILE_PX, paethPrediction } from '../src/lib/hm3-decoder'
import { PHONE, POINT, afterPaint, canvasCenter, installHermeticMap, mapUrl, pngCenterPixel } from './support'

const TRAFFIC = [40, 80, 200, 255]
const LABEL = [210, 30, 40, 255]

function uniformTile(lden: boolean, byte: number): Buffer {
  const tile = Buffer.alloc(6 + TILE_PX * TILE_PX, byte)
  tile.write(lden ? 'HMD ' : 'HM3 ', 0)
  tile[4] = lden ? 1 : HM3_VERSION
  tile[5] = 0
  for (let at = TILE_PX * TILE_PX - 1; at >= 0; at--) {
    tile[6 + at] -= paethPrediction(at % TILE_PX ? tile[6 + at - 1] : 0,
      at >= TILE_PX ? tile[6 + at - TILE_PX] : 0,
      at >= TILE_PX && at % TILE_PX ? tile[6 + at - TILE_PX - 1] : 0)
  }
  return tile
}

async function colourPng(page: Page, rgba: number[]): Promise<Buffer> {
  const base64 = await page.evaluate(rgba => {
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 32
    const ctx = canvas.getContext('2d')!
    ctx.fillStyle = `rgba(${rgba.slice(0, 3).join(',')},${rgba[3] / 255})`
    ctx.fillRect(0, 0, 32, 32)
    return canvas.toDataURL().split(',')[1]
  }, rgba)
  return Buffer.from(base64, 'base64')
}

async function fixtures(page: Page) {
  await installHermeticMap(page, POINT)
  const [traffic, label, black] = await Promise.all([TRAFFIC, LABEL, [0, 0, 0, 255]].map(rgba => colourPng(page, rgba)))
  await page.route('**/api/me', route => route.fulfill({ status: 401, body: '{}' }))
  await page.route('**/api/raster/traffic/**', route => route.fulfill({ contentType: 'image/png', body: traffic }))
  await page.route('**/api/tiles/b1/**/*.bin', route => {
    const source = new URL(route.request().url()).pathname.split('/')[4]
    const lden = source.startsWith('lden-')
    // Quiet total: 0.5 sone / 30 dB; the selected road layer has a visible colour.
    return route.fulfill({ contentType: 'application/octet-stream', body: uniformTile(lden, source.endsWith('total') ? 60 : 100) })
  })
  await page.route('**/positron-gl-style/style.json', route => route.fulfill({ json: {
    version: 8,
    sprite: 'https://map-fixture.invalid/labels',
    sources: { labels: { type: 'geojson', data: { type: 'FeatureCollection', features: [
      { type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: [POINT.lng, POINT.lat] } },
    ] } } },
    layers: [
      { id: 'paper', type: 'background', paint: { 'background-color': '#000000' } },
      { id: 'place-label', type: 'symbol', source: 'labels', layout: { 'icon-image': 'label' } },
    ],
  } }))
  await page.route(/https:\/\/map-fixture\.invalid\/labels(?:@2x)?\.json/, route => route.fulfill({ json: {
    label: { x: 0, y: 0, width: 32, height: 32, pixelRatio: 1 },
  } }))
  await page.route(/https:\/\/map-fixture\.invalid\/labels(?:@2x)?\.png/, route => route.fulfill({ contentType: 'image/png', body: label }))
  await page.route('**/World_Imagery/MapServer/tile/**', route => route.fulfill({ contentType: 'image/png', body: black }))
}

async function centrePixel(page: Page) {
  await afterPaint(page)
  return pngCenterPixel(page, await page.locator('canvas.maplibregl-canvas').screenshot())
}

async function controls(page: Page, phone: boolean, action: () => Promise<unknown>) {
  if (phone) await page.getByRole('button', { name: 'Toggle layers panel', exact: true }).click()
  else await page.getByRole('tab', { name: 'Layers', exact: true }).click()
  const advanced = page.getByRole('button', { name: 'Advanced', exact: true }).filter({ visible: true })
  if (await advanced.getAttribute('aria-expanded') === 'false') await advanced.click()
  await action()
  const close = page.getByRole('button', { name: phone ? 'Drag to close' : 'Fold', exact: true })
  await close.click()
  await expect(close).toBeHidden()
}

async function touchGesture(page: Page, start: { x: number; y: number }[], end: { x: number; y: number }[]) {
  const touch = await page.context().newCDPSession(page)
  for (let step = 0; step <= 12; step++) {
    await touch.send('Input.dispatchTouchEvent', {
      type: step ? 'touchMove' : 'touchStart',
      touchPoints: start.map((point, id) => ({ id,
        x: point.x + (end[id].x - point.x) * step / 12,
        y: point.y + (end[id].y - point.y) * step / 12,
      })),
    })
    await afterPaint(page)
  }
  await touch.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await touch.detach()
}

async function expectGreen(page: Page) {
  await expect.poll(async () => {
    const [r, g, b] = await centrePixel(page)
    return g > r + 40 && g > b + 30
  }).toBe(true)
}

for (const phone of [false, true]) test.describe(phone ? 'phone' : 'desktop', () => {
  if (phone) test.use({ ...PHONE, deviceScaleFactor: 2 })
  test('quiet zones preserve data colours and labels through toggles and basemap changes', async ({ page }) => {
    test.setTimeout(60_000)
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    await fixtures(page)
    await page.goto(mapUrl(POINT, 'road', 14))
    await canvasCenter(page)
    if (phone) await page.getByRole('button', { name: 'Toggle layers panel', exact: true }).click()
    await page.getByRole('button', { name: 'Quiet zones', exact: true }).filter({ visible: true }).click()
    await page.getByRole('button', { name: phone ? 'Drag to close' : 'Fold', exact: true }).click()
    await expectGreen(page)

    await controls(page, phone, async () => {
      await page.getByTestId('data-layer-traffic').filter({ visible: true }).click()
    })
    await expect.poll(() => centrePixel(page)).toEqual(TRAFFIC)

    // Removing and adding the data again must restore the same order.
    await controls(page, phone, () => page.getByTestId('data-layer-traffic').filter({ visible: true }).click())
    await expectGreen(page)
    await controls(page, phone, () => page.getByTestId('data-layer-traffic').filter({ visible: true }).click())
    await expect.poll(() => centrePixel(page)).toEqual(TRAFFIC)
    // A shared link starts with both layers enabled, before the map has chosen its anchor.
    await expect(page).toHaveURL(/data=traffic/)
    await page.reload()
    await expect.poll(() => centrePixel(page)).toEqual(TRAFFIC)

    for (const name of ['Standard', 'Satellite', 'Terrain']) {
      await page.getByRole('button', { name, exact: true }).filter({ visible: true }).click()
      await expect.poll(() => centrePixel(page)).toEqual(name === 'Terrain' ? TRAFFIC : LABEL)
    }

    // Quiet zones also work with every noise layer off, in both quantities.
    await controls(page, phone, async () => {
      await page.getByTestId('data-layer-traffic').filter({ visible: true }).click()
      await page.getByTestId('layer-road').filter({ visible: true }).click()
    })
    await expectGreen(page)
    for (const metric of ['Sone', 'Lden dB']) {
      await controls(page, phone, async () => {
        await page.getByRole('radio', { name: metric, exact: true }).filter({ visible: true }).click()
        await page.getByTestId('quiet-threshold').filter({ visible: true }).fill('0')
        await expect(page).toHaveURL(new RegExp(`qt=${metric === 'Sone' ? '0.25' : '20'}(?:&|$)`))
      })
      await expect.poll(() => centrePixel(page)).toEqual([0, 0, 0, 255])
      await controls(page, phone, async () => {
        const slider = page.getByTestId('quiet-threshold').filter({ visible: true })
        await slider.fill((await slider.getAttribute('max'))!)
        await expect(page).toHaveURL(new RegExp(`qt=${metric === 'Sone' ? '4' : '55'}(?:&|$)`))
      })
      await expectGreen(page)
    }
    await controls(page, phone, async () => {
      await page.getByRole('radio', { name: 'Sone', exact: true }).filter({ visible: true }).click()
      await page.getByTestId('layer-road').filter({ visible: true }).click()
    })
    await expectGreen(page)
    // Tilt with two fingers or a right drag, then cover both composite and tile rasters.
    const centre = await canvasCenter(page)
    if (phone) await touchGesture(page,
      [-60, 60].map(dx => ({ x: centre.x + dx, y: centre.y })),
      [-60, 60].map(dx => ({ x: centre.x + dx, y: centre.y - 120 })))
    else {
      await page.mouse.move(centre.x, centre.y)
      await page.mouse.down({ button: 'right' })
      await page.mouse.move(centre.x, centre.y - 120, { steps: 12 })
      await page.mouse.up({ button: 'right' })
    }
    await page.mouse.move(0, 0) // Keep the hover tooltip outside the sampled map area.
    for (const zoom of [14, 10]) {
      if (zoom === 10) {
        if (phone) {
          await touchGesture(page,
            [-120, 120].map(dx => ({ x: centre.x + dx, y: centre.y })),
            [-7.5, 7.5].map(dx => ({ x: centre.x + dx, y: centre.y })))
        } else for (let step = 0; step < 4; step++) {
          await page.getByRole('button', { name: 'Zoom out', exact: true }).click()
          await expect(page).toHaveURL(url => Number(new URLSearchParams(url.hash.slice(1)).get('z')) === 13 - step)
        }
        await expect.poll(() => Number(new URLSearchParams(new URL(page.url()).hash.slice(1)).get('z'))).toBeLessThan(11)
      }
      await expect.poll(async () => {
        const png = await page.locator('canvas.maplibregl-canvas').screenshot()
        return page.evaluate(async data => {
          const image = new Image(); image.src = data; await image.decode()
          const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height
          const ctx = canvas.getContext('2d')!; ctx.drawImage(image, 0, 0)
          const pixels = ctx.getImageData(image.width * 0.3, image.height * 0.3, image.width * 0.4, image.height * 0.4).data
          for (let at = 0; at < pixels.length; at += 4) if (pixels[at + 1] <= pixels[at] + 40 || pixels[at + 1] <= pixels[at + 2] + 30) return false
          return true
        }, `data:image/png;base64,${png.toString('base64')}`)
      }).toBe(true)
    }
    await page.screenshot({ path: test.info().outputPath('quiet-underlays.png') })
    expect(errors).toEqual([])
  })
})

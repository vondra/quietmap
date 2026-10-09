// Hermetic browser fixtures: a one-cell HM3 world, a black basemap, a streamed popup the test writes
// line by line (the answers are in answers.ts), and the painted canvas read back.
import { devices, expect, type Page } from '@playwright/test'
import { TILE_PX } from '../src/lib/hm3-decoder'

export const FIXTURE_DB = 63
export const SOURCE_DB = 60
/** The hermetic world's published zoom (a z12 world, while the served heatmap is z13: the
 *  frontend must take its tile ceiling from the manifest). */
export const TILE_Z = 12

const BLACK_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
  'base64',
)

export type PixelCenter = {
  lat: number
  lng: number
  tx: number
  ty: number
  px: number
  py: number
}

/** Web Mercator y of a latitude: 0 at the world's north edge, 1 at its south edge. */
function mercatorY(lat: number): number {
  const latRad = lat * Math.PI / 180
  return (1 - Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI) / 2
}

/** Snap a geographic point to the exact HM3 receiver lattice at tile zoom `z`. */
export function hm3PixelCenter(lat: number, lng: number, z = TILE_Z): PixelCenter {
  const worldPixels = 2 ** z * TILE_PX
  const gx = Math.floor((lng + 180) / 360 * worldPixels)
  const gy = Math.floor(mercatorY(lat) * worldPixels)
  const x = (gx + 0.5) / worldPixels
  const y = (gy + 0.5) / worldPixels
  return {
    lat: Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180 / Math.PI,
    lng: x * 360 - 180,
    tx: Math.floor(gx / TILE_PX),
    ty: Math.floor(gy / TILE_PX),
    px: gx % TILE_PX,
    py: gy % TILE_PX,
  }
}

/** The clicked point of the hermetic world, the map's centre. */
export const POINT = hm3PixelCenter(49.8486, 14.1639)

/** A phone: the popup is the bottom sheet over the lower half of the map. */
export const PHONE = {
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 1,
  userAgent: devices['Pixel 5'].userAgent,
  isMobile: true,
  hasTouch: true,
}

export function mapUrl(point: PixelCenter, layers = 'road', zoom = TILE_Z): string {
  return `/#lat=${point.lat}&lng=${point.lng}&z=${zoom}&bm=terrain&ro=${layers}`
}

/** A tile without levels but at most one audible receiver at the expected pixel. */
function hm3Tile(point?: PixelCenter, db?: number): Buffer {
  const tile = Buffer.alloc(6 + TILE_PX * TILE_PX, 255)
  tile.write('HM3 ', 0, 'ascii')
  tile[4] = 3
  tile[5] = 1
  if (point && db != null) tile[6 + point.py * TILE_PX + point.px] = Math.round(db * 2)
  return tile
}

/** Replace the third-party basemap with a stable opaque-black tile. */
export async function mockTerrainBasemap(page: Page): Promise<void> {
  await page.route(/https:\/\/[abc]\.tile\.opentopomap\.org\/.*/, route => route.fulfill({
    status: 200,
    contentType: 'image/png',
    body: BLACK_PNG,
  }))
}

type PopupSeam = {
  requests: string[]
  /** Requests aborted while their stream was still open. */
  aborted: number
  streams: { controller: ReadableStreamDefaultController<Uint8Array>; open: boolean }[]
}

/**
 * `/api/popup` answered in the page: every request gets a stream the test writes with
 * `sendPopupLine` and closes with `endPopup`. A route fulfilment would deliver the whole body at
 * once; this seam shows each streamed update the way the server sends it.
 */
async function installPopupSeam(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const seam: PopupSeam = { requests: [], aborted: 0, streams: [] }
    ;(window as unknown as { __popup: PopupSeam }).__popup = seam
    const realFetch = window.fetch.bind(window)
    window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      if (!url.startsWith('/api/popup')) return realFetch(input, init)
      let controller!: ReadableStreamDefaultController<Uint8Array>
      const body = new ReadableStream<Uint8Array>({ start(c) { controller = c } })
      const stream = { controller, open: true }
      seam.requests.push(url)
      seam.streams.push(stream)
      init?.signal?.addEventListener('abort', () => {
        if (!stream.open) return
        stream.open = false
        seam.aborted += 1
        controller.error(new DOMException('aborted', 'AbortError'))
      })
      return Promise.resolve(new Response(body, { status: 200, headers: { 'content-type': 'application/x-ndjson' } }))
    }
  })
}

/** The points the page requested from `/api/popup`, in order. */
export async function popupRequests(page: Page): Promise<{ lat: number; lng: number }[]> {
  const urls = await page.evaluate(() => (window as unknown as { __popup: PopupSeam }).__popup.requests)
  return urls.map(url => {
    const params = new URL(url, 'http://localhost').searchParams
    return { lat: Number(params.get('lat')), lng: Number(params.get('lon')) }
  })
}

export async function abortedPopupRequests(page: Page): Promise<number> {
  return page.evaluate(() => (window as unknown as { __popup: PopupSeam }).__popup.aborted)
}

/** Write one line into the latest popup request's stream. */
export async function sendPopupLine(page: Page, line: object): Promise<void> {
  await page.evaluate((text) => {
    const { streams } = (window as unknown as { __popup: PopupSeam }).__popup
    streams[streams.length - 1].controller.enqueue(new TextEncoder().encode(`${text}\n`))
  }, JSON.stringify(line))
}

/** End the latest popup request's stream, as the server does after the final or error line. */
export async function endPopup(page: Page): Promise<void> {
  await page.evaluate(() => {
    const { streams } = (window as unknown as { __popup: PopupSeam }).__popup
    const stream = streams[streams.length - 1]
    stream.open = false
    stream.controller.close()
  })
}

/** `paintedDb` is the level the road/rail tiles carry at `point`. */
export async function installHermeticMap(
  page: Page,
  point: PixelCenter,
  paintedDb = SOURCE_DB,
): Promise<void> {
  await mockTerrainBasemap(page)
  await installPopupSeam(page)
  await page.route('**/api/tiles-manifest', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      build: 'b1',
      zoom: TILE_Z,
      layers: {
        road: { build: 'b1', file: 'road.b1.pmtiles' },
        railway: { build: 'b1', file: 'railway.b1.pmtiles' },
      },
    }),
  }))
  await page.route('**/api/tiles/b1/**/*.bin', route => {
    const match = new URL(route.request().url()).pathname
      .match(/^\/api\/tiles\/b1\/([^/]+)\/(\d+)\/(\d+)\/(\d+)\.bin$/)
    const source = match?.[1]
    const atPoint = match != null
      && Number(match[2]) === TILE_Z && Number(match[3]) === point.tx && Number(match[4]) === point.ty
    const level = source === 'road' || source === 'railway' ? paintedDb : undefined
    return route.fulfill({
      status: 200,
      contentType: 'application/octet-stream',
      body: atPoint && level != null ? hm3Tile(point, level) : hm3Tile(),
    })
  })
  await page.route('**/api/reverse?**', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ place: 'E2E fixture' }),
  }))
}

export async function canvasCenter(page: Page): Promise<{
  canvas: ReturnType<Page['locator']>
  x: number
  y: number
}> {
  const canvas = page.locator('canvas.maplibregl-canvas')
  await expect(canvas).toBeVisible()
  const box = await canvas.boundingBox()
  expect(box).not.toBeNull()
  return {
    canvas,
    x: box!.x + box!.width / 2,
    y: box!.y + box!.height / 2,
  }
}

/** Two animation frames: synchronize with the actual WebGL paint, not a timer. */
export async function afterPaint(page: Page): Promise<void> {
  await page.evaluate(() => new Promise<void>(resolve => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  }))
}

/** Decode a locator screenshot in-browser and return the RGBA at each of `at` (its pixels from the
 *  top left), by default at its exact centre. */
async function pngPixels(page: Page, png: Buffer, at?: { x: number; y: number }[]): Promise<number[][]> {
  return page.evaluate(async ({ source, at }) => {
    const image = new Image()
    image.src = source
    await image.decode()
    const canvas = document.createElement('canvas')
    canvas.width = image.width
    canvas.height = image.height
    const context = canvas.getContext('2d')!
    context.drawImage(image, 0, 0)
    return (at ?? [{ x: Math.floor(image.width / 2), y: Math.floor(image.height / 2) }])
      .map(({ x, y }) => [...context.getImageData(x, y, 1, 1).data])
  }, { source: `data:image/png;base64,${png.toString('base64')}`, at })
}

/** Decode a locator screenshot in-browser and return its exact centre RGBA. */
export async function pngCenterPixel(page: Page, png: Buffer): Promise<number[]> {
  return (await pngPixels(page, png))[0]
}

/** The painted map's RGBA at each [lat, lon] of `places`, the map centred on `center` at `zoom`
 *  (MapLibre draws the world 512 px wide at zoom 0). */
export async function mapPixels(
  page: Page,
  center: PixelCenter,
  places: readonly (readonly number[])[],
  zoom = TILE_Z,
): Promise<number[][]> {
  const canvas = page.locator('canvas.maplibregl-canvas')
  const box = (await canvas.boundingBox())!
  const worldPixels = 512 * 2 ** zoom
  const at = places.map(([lat, lng]) => ({
    x: Math.floor(box.width / 2 + (lng - center.lng) / 360 * worldPixels),
    y: Math.floor(box.height / 2 + (mercatorY(lat) - mercatorY(center.lat)) * worldPixels),
  }))
  await afterPaint(page)
  return pngPixels(page, await canvas.screenshot(), at)
}

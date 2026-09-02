import { expect, type Page } from '@playwright/test'

export const FIXTURE_DB = 63
export const SOURCE_DB = 60
export const TILE_Z = 12
export const TILE_PX = 512

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

/** Snap a geographic point to the exact receiver lattice used by z12 HM3. */
export function hm3PixelCenter(lat: number, lng: number): PixelCenter {
  const worldPixels = 2 ** TILE_Z * TILE_PX
  const latRad = lat * Math.PI / 180
  const gx = Math.floor((lng + 180) / 360 * worldPixels)
  const gy = Math.floor(
    (1 - Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI) / 2 * worldPixels,
  )
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

export function mapUrl(point: PixelCenter, overlays = 'road', zoom = TILE_Z): string {
  return `/#lat=${point.lat}&lng=${point.lng}&z=${zoom}&bm=terrain&ro=${overlays}`
}

/** A no-data tile with at most one audible receiver at the expected pixel. */
export function hm3Tile(point?: PixelCenter, db?: number): Buffer {
  const tile = Buffer.alloc(6 + TILE_PX * TILE_PX, 255)
  tile.write('HM3 ', 0, 'ascii')
  tile[4] = 3
  tile[5] = 1
  if (point && db != null) tile[6 + point.py * TILE_PX + point.px] = Math.round(db * 2)
  return tile
}

/** The enclosed-receiver half of the popup payload; omitted outdoors. */
export type IndoorEnvelope = {
  envelope_class: 'residential' | 'commercial' | 'industrial' | 'historic' | 'default'
  envelope_delta_db: number
  facade_lden: number
  indoor_lden_tilted: number
}

export function popupFixture(
  lat: number,
  lng: number,
  db = FIXTURE_DB,
  sourceLevels: Record<string, number> = { road: db },
  indoor?: IndoorEnvelope,
) {
  return {
    ...indoor,
    h3_center: [lat, lng],
    elevation_m: 350,
    total_lden: db,
    total_lden_free: db,
    sources: Object.entries(sourceLevels).map(([source_type, level]) => ({
      source_type, lden: level, lden_free: level, ld: level, le: level, ln: level,
      segment_count: 0, displayed_count: 0,
    })),
    top_contributors: [],
    other_sources_lden: null,
    compute_time_ms: 1,
    segments: [],
    segments_meta: null,
    timings: null,
  }
}

/** Replace the third-party basemap with a stable opaque-black tile. */
export async function mockTerrainBasemap(page: Page): Promise<void> {
  await page.route(/https:\/\/[abc]\.tile\.opentopomap\.org\/.*/, route => route.fulfill({
    status: 200,
    contentType: 'image/png',
    body: BLACK_PNG,
  }))
}

/** `paintedDb` is the level the road/rail tiles carry at `point` — pass the
 *  popup's own level when the test asserts that map and popup agree. */
export async function installHermeticMap(
  page: Page,
  point: PixelCenter,
  paintedDb = SOURCE_DB,
): Promise<void> {
  await mockTerrainBasemap(page)
  await page.route('**/api/tiles-manifest', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      // A z12 world on purpose, while the code's WORLD_BASE_ZOOM is 13: the
      // frontend must take its native tile ceiling from the manifest, never
      // from a compiled-in constant. Every tile assertion below is at TILE_Z.
      build: 'b1',
      zoom: TILE_Z,
      layers: {
        road: { build: 'b1', file: 'road.b1.pmtiles' },
        rail: { build: 'b1', file: 'rail.b1.pmtiles' },
      },
    }),
  }))
  await page.route('**/api/tiles/b1/**/*.bin', route => {
    const match = new URL(route.request().url()).pathname
      .match(/^\/api\/tiles\/b1\/([^/]+)\/(\d+)\/(\d+)\/(\d+)\.bin$/)
    const source = match?.[1]
    const atPoint = match != null
      && Number(match[2]) === TILE_Z && Number(match[3]) === point.tx && Number(match[4]) === point.ty
    const level = source === 'road' || source === 'rail' ? paintedDb : undefined
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

/** Decode a locator screenshot in-browser and return its exact centre RGBA. */
export async function pngCenterPixel(page: Page, png: Buffer): Promise<number[]> {
  return page.evaluate(async (source) => {
    const image = new Image()
    image.src = source
    await image.decode()
    const canvas = document.createElement('canvas')
    canvas.width = image.width
    canvas.height = image.height
    const context = canvas.getContext('2d')!
    context.drawImage(image, 0, 0)
    return [...context.getImageData(Math.floor(image.width / 2), Math.floor(image.height / 2), 1, 1).data]
  }, `data:image/png;base64,${png.toString('base64')}`)
}

export function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>(done => { resolve = done })
  return { promise, resolve }
}

//! Deterministic Playwright session that exercises the visitor's real map click.

import { createHash } from 'node:crypto'
import { chromium } from 'playwright'
import { POPUP_TIMEOUT_MS, REQUEST_INTERVAL_MS } from '../../../server/scripts/popup-parity/endpoint.mjs'

export const BROWSER_VIEWPORT = Object.freeze({ width: 1280, height: 720 })
export const MAP_COORDINATE_EPSILON_DEGREES = 1e-6
const BROWSER_BASEMAP = 'standard'

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function wrappedLongitudeDelta(actual, expected) {
  return ((actual - expected + 540) % 360) - 180
}

export function assertCanvasClickCoordinates(requested, point, label) {
  const latDelta = requested.lat - point.lat
  const lngDelta = wrappedLongitudeDelta(requested.lng, point.lng)
  if (Math.abs(latDelta) >= MAP_COORDINATE_EPSILON_DEGREES
      || Math.abs(lngDelta) >= MAP_COORDINATE_EPSILON_DEGREES) {
    throw new Error(
      `${label}: canvas click requested ${requested.lat},${requested.lng}; `
      + `catalog has ${point.lat},${point.lng}`,
    )
  }
  return { lat_delta: latDelta, wrapped_lng_delta: lngDelta }
}

function mapUrl(origin, point, captureKey) {
  const url = new URL('/', origin)
  url.searchParams.set('popup-parity', captureKey)
  const hash = new URLSearchParams()
  hash.set('lat', String(point.lat))
  hash.set('lng', String(point.lng))
  hash.set('z', '14')
  hash.set('bm', BROWSER_BASEMAP)
  hash.set('ro', '')
  url.hash = hash.toString()
  return url.toString()
}

async function afterPaint(page) {
  await page.evaluate(() => new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(resolve))
  }))
}

async function canvasCenter(page) {
  const canvas = page.locator('canvas.maplibregl-canvas')
  await canvas.waitFor({ state: 'visible', timeout: POPUP_TIMEOUT_MS })
  await afterPaint(page)
  const box = await canvas.boundingBox()
  if (!box) throw new Error('visible MapLibre canvas has no bounding box')
  return {
    x: box.x + box.width / 2,
    y: box.y + box.height / 2,
    width: box.width,
    height: box.height,
  }
}

function parsePopupRequest(response, expectedOrigin, point) {
  const request = response.request()
  const url = new URL(request.url())
  if (url.origin !== expectedOrigin || url.pathname !== '/api/noise-onfly-v2') {
    throw new Error(`popup/${point.id}: response came from unexpected URL ${url}`)
  }
  if (request.method() !== 'GET' || url.searchParams.has('full')) {
    throw new Error(`popup/${point.id}: expected default GET without full`)
  }
  const requested = {
    lat: Number(url.searchParams.get('lat')),
    lng: Number(url.searchParams.get('lng')),
  }
  if (!Number.isFinite(requested.lat) || !Number.isFinite(requested.lng)) {
    throw new Error(`popup/${point.id}: request coordinates are not finite`)
  }
  return { url, requested }
}

export async function openMapBrowserSession(originInput, expectedInstance, options = {}) {
  const origin = new URL(originInput).origin
  const executablePath = options.executablePath ?? process.env.PLAYWRIGHT_CHROME
  const browser = await chromium.launch({
    headless: true,
    ...(executablePath ? { executablePath } : {}),
  })
  const context = await browser.newContext({
    viewport: BROWSER_VIEWPORT,
    deviceScaleFactor: 1,
    locale: 'en-US',
    timezoneId: 'UTC',
    colorScheme: 'light',
    reducedMotion: 'reduce',
  })
  const page = await context.newPage()
  let nextPopupStart = 0
  let captureSequence = 0

  return {
    page,
    metadata: {
      engine: 'chromium',
      version: browser.version(),
      executable_path: executablePath ?? null,
      headless: true,
      viewport: BROWSER_VIEWPORT,
      device_scale_factor: 1,
      locale: 'en-US',
      timezone: 'UTC',
      color_scheme: 'light',
      reduced_motion: 'reduce',
      basemap: BROWSER_BASEMAP,
      network_mocking: false,
      raster_overlays: [],
      zoom: 14,
      click: 'maplibre_canvas_center',
      coordinate_epsilon_degrees: MAP_COORDINATE_EPSILON_DEGREES,
    },
    async click(point, label = String(point.id)) {
      captureSequence += 1
      const navigation = await page.goto(
        mapUrl(origin, point, `${captureSequence}-${label}`),
        { waitUntil: 'domcontentloaded', timeout: POPUP_TIMEOUT_MS },
      )
      if (!navigation?.ok()) {
        throw new Error(`popup/${point.id}: page navigation returned ${navigation?.status() ?? 'no response'}`)
      }
      const canvas = await canvasCenter(page)
      const responsePromise = page.waitForResponse((response) => {
        const url = new URL(response.url())
        return url.origin === origin
          && url.pathname === '/api/noise-onfly-v2'
          && !url.searchParams.has('full')
      }, { timeout: POPUP_TIMEOUT_MS })
      const wait = nextPopupStart - Date.now()
      if (wait > 0) await sleep(wait)
      const started = performance.now()
      nextPopupStart = Math.max(nextPopupStart + REQUEST_INTERVAL_MS, Date.now() + REQUEST_INTERVAL_MS)
      await page.mouse.click(canvas.x, canvas.y)
      const response = await responsePromise
      const { url, requested } = parsePopupRequest(response, origin, point)
      const coordinateDelta = assertCanvasClickCoordinates(requested, point, `popup/${point.id}`)
      const instance = await response.headerValue('x-0db-instance')
      if (!instance || instance !== expectedInstance) {
        throw new Error(`popup/${point.id}: instance changed from ${expectedInstance} to ${instance}`)
      }
      const body = await response.body()
      if (!response.ok()) {
        throw new Error(`popup/${point.id}: HTTP ${response.status()}: ${body.toString('utf8').slice(0, 500)}`)
      }
      const popup = page.locator('[data-testid="detail-popup"]:visible')
      await popup.waitFor({ state: 'visible', timeout: POPUP_TIMEOUT_MS })
      await afterPaint(page)
      const renderedText = await popup.innerText()
      return {
        body,
        payload: JSON.parse(body.toString('utf8')),
        elapsed_ms: performance.now() - started,
        http_status: response.status(),
        content_type: await response.headerValue('content-type') ?? '',
        instance,
        browser: {
          request_url: url.toString(),
          requested_coordinates: [requested.lat, requested.lng],
          catalog_coordinate_delta: coordinateDelta,
          canvas,
          rendered_text: renderedText,
          rendered_text_sha256: createHash('sha256').update(renderedText).digest('hex'),
        },
      }
    },
    async afterPaint() {
      await page.waitForLoadState('networkidle', { timeout: POPUP_TIMEOUT_MS })
      await page.evaluate(() => document.fonts.ready.then(() => undefined))
      await afterPaint(page)
    },
    async close() {
      await context.close()
      await browser.close()
    },
  }
}

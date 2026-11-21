// Sharing a selected point opens the same calculation and map settings in a fresh browser.
import { expect, test, type Page } from '@playwright/test'
import { PHONE, POINT, afterPaint, canvasCenter, endPopup, installHermeticMap, mapPixels, mapUrl, popupRequests, sendPopupLine, swipeDown } from './support'
import { popupUpdate } from './answers'

const badge = (page: Page) => page.getByTestId('noise-badge').filter({ visible: true })

// Inspect the composited page: the canvas alone still contains a point hidden by a panel.
// The fixture has no other marks in the selected point's blue.
async function visibleMarkerPixels(page: Page): Promise<number> {
  const screenshot = await page.screenshot({ scale: 'css' })
  return page.evaluate(async source => {
    const image = new Image()
    image.src = source
    await image.decode()
    const canvas = document.createElement('canvas')
    canvas.width = image.width; canvas.height = image.height
    const context = canvas.getContext('2d')!
    context.drawImage(image, 0, 0)
    const pixels = context.getImageData(0, 0, image.width, image.height).data
    let blue = 0
    for (let at = 0; at < pixels.length; at += 4) {
      if (pixels[at] === 59 && pixels[at + 1] === 130 && pixels[at + 2] === 246) blue++
    }
    return blue
  }, `data:image/png;base64,${screenshot.toString('base64')}`)
}

function answerWithFullSourceList() {
  const answer = popupUpdate(1, false, POINT.lat, POINT.lng, 60)
  answer.top_contributors = Array.from({ length: 8 }, (_, index) => ({
    ...answer.top_contributors[0], id: index.toString(16).padStart(16, '0'), name: `Street ${index}`, share: 1 / 8,
  }))
  return answer
}

test('a desktop shared point stays clear as its answer grows', async ({ page }) => {
  await installHermeticMap(page, POINT)
  // At this camera the point lies below the skeleton, within the eventual result card.
  await page.goto(`/#lat=${POINT.lat - 0.004}&lng=${POINT.lng - 0.02}&z=14&pad=0.166667,0,0,0&d=${POINT.lat},${POINT.lng}&bm=terrain&ro=road&panel=none`)
  await expect(page.getByTestId('detail-popup-skeleton').filter({ visible: true })).toBeVisible()
  await expect.poll(() => visibleMarkerPixels(page)).toBeGreaterThan(60)
  await sendPopupLine(page, answerWithFullSourceList())
  await endPopup(page)
  await expect(badge(page)).toContainText('sone')
  await expect.poll(() => visibleMarkerPixels(page)).toBeGreaterThan(60)
  await page.screenshot({ path: test.info().outputPath('desktop-shared-streaming.png') })
})

test.describe('phone point visibility', () => {
  test.use({ ...PHONE, viewport: { width: 360, height: 640 } })
  test('an offscreen shared point keeps its camera; tapping its clipped pin brings it into view', async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 844 })
    await installHermeticMap(page, POINT)
    const degreesPerPixel = 360 / (512 * 2 ** 14)
    const stay = { id: 'edge', name: 'Edge stay', lat: POINT.lat, lng: POINT.lng - 182 * degreesPerPixel,
      total: 200, stars: 4, score: 8, reviews: 100, guests: 2, bedrooms: 1, freeCancellation: false,
      thumbnail: null, url: 'https://www.stay22.com/allez/roam/edge' }
    await page.route('**/api/stay?**', route => route.fulfill({ json: {
      listings: [stay], nights: 2, currency: 'EUR', expiresIn: 3300, failure: null,
    } }))
    await page.goto(`${mapUrl(POINT, 'road', 14)}&stay=1&panel=none&d=${stay.lat},${stay.lng}`)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    await afterPaint(page)
    const sharedView = new URLSearchParams(new URL(page.url()).hash.slice(1))
    expect(Number(sharedView.get('lat'))).toBe(Number(POINT.lat.toFixed(6)))
    expect(Number(sharedView.get('lng'))).toBe(Number(POINT.lng.toFixed(6)))
    await swipeDown(page.getByTestId('mobile-detail-sheet').getByRole('button', { name: /^(Collapse|Expand)$/ }))
    const insideEdge = [POINT.lat, POINT.lng - 179.5 * degreesPerPixel] as const
    await expect.poll(async () => (await mapPixels(page, POINT, [insideEdge], 14))[0]).toEqual([51, 65, 85, 255])
    await page.touchscreen.tap(1, 422)
    await expect.poll(() => popupRequests(page)).toHaveLength(2)
    const point = (await popupRequests(page))[1]
    expect(point.lng).toBeCloseTo(stay.lng, 6)
    await sendPopupLine(page, { ...answerWithFullSourceList(), center: [point.lat, point.lng] })
    await endPopup(page)
    await expect(badge(page)).toContainText('sone')
    await expect.poll(() => visibleMarkerPixels(page)).toBeGreaterThan(60)
    await page.screenshot({ path: test.info().outputPath('clipped-stay-phone.png') })
  })
  test('a point just below search remains visible when its place row opens', async ({ page }) => {
    await installHermeticMap(page, POINT)
    await page.goto(`${mapUrl(POINT, 'road', 14)}&p=60&panel=none`)
    await canvasCenter(page)
    await page.touchscreen.tap(180, 60)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    const [point] = await popupRequests(page)
    await sendPopupLine(page, { ...answerWithFullSourceList(), center: [point.lat, point.lng] })
    await endPopup(page)
    await expect(badge(page)).toContainText('sone')
    // Perspective makes this marker smaller than the same circle in a flat view.
    await expect.poll(() => visibleMarkerPixels(page)).toBeGreaterThan(40)
    expect(Number(new URLSearchParams(new URL(page.url()).hash.slice(1)).get('p'))).toBe(60)
    await page.screenshot({ path: test.info().outputPath('tilted-phone-point.png') })
  })
})

test('a shared on-screen place clears the phone panels, and rotation lets a new search arrive', async ({ page, browser }) => {
  test.setTimeout(30_000)
  await installHermeticMap(page, POINT)
  await page.route('**/api/search?**', route => route.fulfill({ json: [
    { display_name: 'Shared park', secondary: 'City', lat: POINT.lat, lon: POINT.lng },
  ] }))
  await page.goto(mapUrl(POINT))
  await page.getByRole('combobox').fill('Shared park')
  await page.getByRole('option', { name: /Shared park/ }).click()
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  const answer = answerWithFullSourceList()
  await sendPopupLine(page, answer)
  await endPopup(page)
  const link = page.url()
  const senderState = new URLSearchParams(new URL(link).hash.slice(1))
  const recipient = await browser.newContext({ ...PHONE, viewport: { width: 360, height: 640 }, reducedMotion: 'reduce' })
  try {
    const opened = await recipient.newPage()
    await installHermeticMap(opened, POINT)
    await opened.goto(link)
    const point = { lat: Number(POINT.lat.toFixed(6)), lng: Number(POINT.lng.toFixed(6)) }
    await expect.poll(() => popupRequests(opened)).toEqual([point])
    await sendPopupLine(opened, { ...answer, center: [point.lat, point.lng] })
    await endPopup(opened)
    await expect(badge(opened)).toContainText('sone')
    for (const viewport of [{ width: 360, height: 640 }, { width: 844, height: 390 }, { width: 390, height: 844 }]) {
      await opened.setViewportSize(viewport)
      await expect.poll(() => visibleMarkerPixels(opened)).toBeGreaterThan(60)
      await opened.screenshot({ path: test.info().outputPath(`shared-park-${viewport.width}.png`) })
      const state = new URLSearchParams(new URL(opened.url()).hash.slice(1))
      for (const key of ['d', 'z', 'ro']) expect(state.get(key)).toBe(senderState.get(key))
      expect(await popupRequests(opened)).toEqual([point])
    }
    const next = { lat: point.lat, lng: point.lng + 0.001 }
    await opened.emulateMedia({ reducedMotion: 'no-preference' })
    await opened.route('**/api/search?**', route => route.fulfill({ json: [
      { display_name: 'Next place', secondary: 'City', lat: next.lat, lon: next.lng },
    ] }))
    await opened.getByRole('combobox').fill('Next place')
    await opened.getByRole('option', { name: /Next place/ }).click()
    await afterPaint(opened)
    await opened.setViewportSize({ width: 844, height: 390 })
    await expect.poll(() => popupRequests(opened)).toEqual([point, next])
  } finally { await recipient.close() }
})

for (const phone of [false, true]) test.describe(phone ? 'phone' : 'desktop', () => {
  if (phone) test.use(PHONE)
  test('copying the current address preserves the point, layers and unit for another visitor', async ({ page, browser }) => {
    await installHermeticMap(page, POINT)
    await page.goto(`${mapUrl(POINT)}&m=lden&qc=1&qt=45`)
    const center = await canvasCenter(page)
    await page.mouse.click(center.x - 30, center.y - 80)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    const [point] = await popupRequests(page)
    await sendPopupLine(page, popupUpdate(1, false, point.lat, point.lng, 60))
    await expect(badge(page)).toContainText('60.0 dB')
    // The address is written as the place opens: copied at once, it is already the place's link.
    const link = page.url()
    const state = new URLSearchParams(new URL(link).hash.slice(1))
    expect(state.get('d')).toBe(`${point.lat.toFixed(6)},${point.lng.toFixed(6)}`)
    expect(state.get('m')).toBe('lden')
    expect(state.get('ro')).toBe('road')
    expect(state.get('qc')).toBe('1')
    expect(state.get('qt')).toBe('45')
    expect(state.get('bm')).toBe('terrain')
    expect(Number(state.get('z'))).toBe(12)
    await page.screenshot({ path: test.info().outputPath('shared-place.png') })

    const recipient = await browser.newContext(phone ? PHONE : {})
    try {
      const opened = await recipient.newPage()
      await installHermeticMap(opened, POINT)
      await opened.goto(link)
      const sharedPoint = { lat: Number(point.lat.toFixed(6)), lng: Number(point.lng.toFixed(6)) }
      await expect.poll(() => popupRequests(opened)).toEqual([sharedPoint])
      await sendPopupLine(opened, popupUpdate(1, false, sharedPoint.lat, sharedPoint.lng, 60))
      await expect(badge(opened)).toContainText('60.0 dB')
      await expect(opened.getByTestId('open-place')).toBeVisible()
      const openedState = new URLSearchParams(new URL(opened.url()).hash.slice(1))
      for (const name of ['d', 'm', 'ro', 'qc', 'qt', 'bm', 'z']) expect(openedState.get(name)).toBe(state.get(name))
      await opened.screenshot({ path: test.info().outputPath('received-place.png') })
    } finally {
      await recipient.close()
    }
  })
})

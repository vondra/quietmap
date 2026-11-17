// Sharing a selected point opens the same calculation and map settings in a fresh browser.
import { expect, test, type Page } from '@playwright/test'
import { PHONE, POINT, afterPaint, canvasCenter, endPopup, installHermeticMap, mapPixels, mapUrl, popupRequests, sendPopupLine } from './support'
import { popupUpdate } from './answers'

const shareButton = (page: Page) => page.getByRole('button', { name: 'Share this place', exact: true }).filter({ visible: true })
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
    await page.getByRole('button', { name: 'Close', exact: true }).filter({ visible: true }).click()
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

test('a shared on-screen place clears the phone panels, and rotation lets a new search arrive', async ({ page, context, browser }) => {
  test.setTimeout(30_000)
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await page.addInitScript(() => Object.defineProperty(navigator, 'share', { value: undefined }))
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
  await shareButton(page).click()
  const link = await page.evaluate(() => navigator.clipboard.readText())
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
  test('copying the current address preserves the point, layers and unit for another visitor', async ({ page, context, browser }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    await page.addInitScript(() => Object.defineProperty(navigator, 'share', { value: undefined }))
    await installHermeticMap(page, POINT)
    await page.goto(`/?private-query=do-not-share${mapUrl(POINT).slice(1)}&m=lden&qc=1&qt=45`)
    const center = await canvasCenter(page)
    // Keep the address's pending write asleep while a visitor opens and shares a fresh point.
    await page.clock.install()
    await page.clock.pauseAt(new Date(Date.now() + 1000))
    await page.mouse.click(center.x - 30, center.y - 80)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    const [point] = await popupRequests(page)
    await sendPopupLine(page, popupUpdate(1, false, point.lat, point.lng, 60))
    await expect(badge(page)).toContainText('60.0 dB')
    expect(new URL(page.url()).hash).toContain('d=')
    await shareButton(page).click()
    await expect(page.getByRole('status').filter({ visible: true })).toHaveText('Link copied')
    const link = await page.evaluate(() => navigator.clipboard.readText())
    const url = new URL(link), state = new URLSearchParams(url.hash.slice(1))
    expect(url.search).toBe('')
    expect(url.pathname).toBe('/')
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
      await expect(shareButton(opened)).toBeInViewport({ ratio: 1 })
      const openedState = new URLSearchParams(new URL(opened.url()).hash.slice(1))
      for (const name of ['d', 'm', 'ro', 'qc', 'qt', 'bm', 'z']) expect(openedState.get(name)).toBe(state.get(name))
      await opened.screenshot({ path: test.info().outputPath('received-place.png') })
    } finally {
      await recipient.close()
    }
  })
})

test.describe('native sharing on a phone', () => {
  test.use(PHONE)
  test('uses the device chooser, respects cancellation, and offers a selectable link on failure', async ({ page }) => {
    await page.addInitScript(() => {
      const state = { calls: [] as ShareData[], outcome: 'success', copies: 0 }
      Object.assign(window, { shareProbe: state })
      Object.defineProperty(navigator, 'share', { value: async (data: ShareData) => {
        state.calls.push(data)
        if (state.outcome !== 'success') throw new DOMException('Unavailable', state.outcome)
      } })
      Object.defineProperty(navigator, 'clipboard', { value: { writeText: async () => { state.copies++ } } })
    })
    await installHermeticMap(page, POINT)
    await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng}`)
    await expect(shareButton(page)).toBeVisible()
    await shareButton(page).tap()
    const calls = () => page.evaluate(() => (window as unknown as { shareProbe: { calls: ShareData[] } }).shareProbe.calls)
    await expect.poll(calls).toHaveLength(1)
    expect((await calls())[0].title).toBe('Quiet Map')
    expect(new URL((await calls())[0].url!).hash).toContain(`d=${POINT.lat.toFixed(6)},${POINT.lng.toFixed(6)}`)
    const outcome = (value: string) => page.evaluate(value => {
      (window as unknown as { shareProbe: { outcome: string } }).shareProbe.outcome = value
    }, value)
    await outcome('AbortError')
    await shareButton(page).tap()
    await expect.poll(calls).toHaveLength(2)
    await expect(page.getByRole('status').filter({ visible: true })).toHaveText('')
    await outcome('NotAllowedError')
    await shareButton(page).tap()
    const link = page.getByRole('textbox', { name: 'Copy this link to share the place.' }).filter({ visible: true })
    await expect(link).toHaveValue((await calls())[0].url!)
    await expect(link).toBeInViewport({ ratio: 1 })
    await expect(link).toBeFocused()
    expect(await link.evaluate(input => [(input as HTMLInputElement).selectionStart, (input as HTMLInputElement).selectionEnd])).toEqual([0, (await link.inputValue()).length])
    expect(await page.evaluate(() => (window as unknown as { shareProbe: { copies: number } }).shareProbe.copies)).toBe(0)
    await page.screenshot({ path: test.info().outputPath('manual-share.png') })
    await page.getByRole('button', { name: 'Done', exact: true }).tap()
    await expect(page.getByRole('dialog', { name: 'Share link' })).toHaveCount(0)
    await expect(page.getByTestId('mobile-detail-sheet')).toBeVisible()
  })
})

test('a refused clipboard offers the link without claiming it was copied', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'share', { value: undefined })
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async () => { throw new DOMException('Denied', 'NotAllowedError') } } })
  })
  await installHermeticMap(page, POINT)
  await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng}`)
  await shareButton(page).click()
  await expect(page.getByRole('dialog', { name: 'Share link' })).toContainText('Copy this link')
  const link = page.getByRole('textbox', { name: 'Copy this link to share the place.' }).filter({ visible: true })
  await expect(link).toHaveValue(/#.*d=/)
  await expect(link).toBeInViewport({ ratio: 1 })
  await expect(link).toBeFocused()
  expect(await link.evaluate(input => [(input as HTMLInputElement).selectionStart, (input as HTMLInputElement).selectionEnd])).toEqual([0, (await link.inputValue()).length])
  await page.screenshot({ path: test.info().outputPath('manual-share-desktop.png') })
  // Switching layouts hides the card that opened the modal; the copy control must stay usable.
  for (const viewport of [{ width: 360, height: 780 }, { width: 844, height: 390 }]) {
    await page.setViewportSize(viewport)
    await expect(page.getByRole('dialog', { name: 'Share link' })).toBeVisible()
    await expect(link).toBeInViewport({ ratio: 1 })
  }
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog', { name: 'Share link' })).toHaveCount(0)
  await expect(shareButton(page)).toBeFocused()
})

test('keyboard sharing keeps focus through cancellation and starts only one chooser', async ({ page }) => {
  await page.addInitScript(() => {
    const state = { calls: 0, cancel: () => {} }
    Object.assign(window, { shareProbe: state })
    Object.defineProperty(navigator, 'share', { value: () => {
      state.calls++
      return new Promise<void>((_, reject) => {
        state.cancel = () => reject(new DOMException('Cancelled', 'AbortError'))
      })
    } })
  })
  await installHermeticMap(page, POINT)
  await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng}`)
  await shareButton(page).focus()
  await page.keyboard.press('Enter')
  const calls = () => page.evaluate(() => (window as unknown as { shareProbe: { calls: number } }).shareProbe.calls)
  await expect.poll(calls).toBe(1)
  await expect(shareButton(page)).toBeFocused()
  await page.keyboard.press('Enter')
  expect(await calls()).toBe(1)
  await page.evaluate(() => (window as unknown as { shareProbe: { cancel: () => void } }).shareProbe.cancel())
  await expect(shareButton(page)).toBeFocused()
  await expect(page.getByRole('dialog', { name: 'Share link' })).toHaveCount(0)
})

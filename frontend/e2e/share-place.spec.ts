// Sharing a selected point opens the same calculation and map settings in a fresh browser.
import { expect, test, type Page } from '@playwright/test'
import { PHONE, POINT, canvasCenter, installHermeticMap, mapUrl, popupRequests, sendPopupLine } from './support'
import { popupUpdate } from './answers'

const shareButton = (page: Page) => page.getByRole('button', { name: 'Share this place', exact: true }).filter({ visible: true })
const badge = (page: Page) => page.getByTestId('noise-badge').filter({ visible: true })

for (const phone of [false, true]) test.describe(phone ? 'phone' : 'desktop', () => {
  if (phone) test.use(PHONE)
  test('copying before the address updates preserves the point, layers and unit for another visitor', async ({ page, context, browser }) => {
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
    expect(new URL(page.url()).hash).not.toContain('d=')
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

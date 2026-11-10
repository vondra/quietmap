// Copying the address restores the public view, selections and panels in a fresh browser.
import { expect, test, type Page } from '@playwright/test'
import { popupUpdate } from './answers'
import { PHONE, POINT, canvasCenter, endPopup, installHermeticMap, mapUrl, popupRequests, sendPopupLine } from './support'

const HOTEL = { id: 'shared-hotel', name: 'Shared hotel', lat: POINT.lat, lng: POINT.lng, total: 360,
  stars: 4, score: 8.6, reviews: 120, guests: 3, bedrooms: 1, freeCancellation: true,
  thumbnail: null, url: 'https://www.stay22.com/allez/roam/shared-hotel' }

async function fixtures(page: Page) {
  await installHermeticMap(page, POINT)
  await page.route('**/fonts/**', route => route.fulfill({ status: 200, body: '' }))
  const stays: URLSearchParams[] = [], areas: URLSearchParams[] = []
  await page.route('**/api/stay?**', route => {
    stays.push(new URL(route.request().url()).searchParams)
    return route.fulfill({ json: { listings: [HOTEL], nights: 3, currency: 'EUR', expiresIn: 3600, failure: null } })
  })
  await page.route('**/api/isochron?**', route => {
    areas.push(new URL(route.request().url()).searchParams)
    const x = POINT.lng, y = POINT.lat
    return route.fulfill({ json: { type: 'FeatureCollection', features: [{ type: 'Feature', properties: { mode: 'walk' },
      geometry: { type: 'Polygon', coordinates: [[[x - 0.004, y - 0.004], [x + 0.004, y - 0.004], [x + 0.004, y + 0.004], [x - 0.004, y + 0.004], [x - 0.004, y - 0.004]]] } }] } })
  })
  return { stays, areas }
}
const visible = (page: Page, name: string) => page.getByRole('button', { name, exact: true }).filter({ visible: true })
const popup = (page: Page) => page.getByTestId('detail-popup').filter({ visible: true })
async function answer(page: Page) {
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  const [at] = await popupRequests(page)
  await sendPopupLine(page, popupUpdate(1, false, at.lat, at.lng, 60))
  await endPopup(page)
  await expect(popup(page)).toBeVisible()
}

for (const phone of [false, true]) test.describe(phone ? 'phone address' : 'desktop address', () => {
  if (phone) test.use(PHONE)
  test('the address carries stay search, quiet filters, open controls and the selected stay', async ({ page, browser }) => {
    test.setTimeout(45_000)
    await fixtures(page)
    await page.goto(`${mapUrl(POINT)}&stay=1`)
    if (phone) await visible(page, 'Toggle layers panel').click()
    await page.getByTestId('stay-checkin').filter({ visible: true }).fill('2027-06-10')
    await page.getByTestId('stay-checkout').filter({ visible: true }).fill('2027-06-13')
    await visible(page, 'One guest more').click()
    await page.getByRole('radio', { name: 'Hotels', exact: true }).filter({ visible: true }).click()
    await page.getByRole('radio', { name: '★★★★', exact: true }).filter({ visible: true }).click()
    await page.getByRole('radio', { name: '8+', exact: true }).filter({ visible: true }).click()
    await page.getByRole('spinbutton', { name: 'Maximum price per night in euros' }).filter({ visible: true }).fill('180.5')
    await visible(page, 'Quiet zones').click()
    await page.getByTestId('quiet-threshold').filter({ visible: true }).fill('17')
    await visible(page, 'Advanced').click()
    const scroller = page.getByTestId('layers-scroll').filter({ visible: true })
    await scroller.hover()
    await page.mouse.wheel(0, 230)
    await expect.poll(() => Number(new URLSearchParams(new URL(page.url()).hash.slice(1)).get('scroll')?.split(',')[0])).toBeGreaterThan(0)
    const offset = await scroller.evaluate(element => Math.round(element.scrollTop))
    const link = page.url()
    const recipient = await browser.newContext(phone ? PHONE : {})
    try {
      const opened = await recipient.newPage()
      const remote = await fixtures(opened)
      await opened.goto(link)
      await expect(opened.getByTestId('stay-checkin').filter({ visible: true })).toHaveValue('2027-06-10')
      await expect(opened.getByTestId('stay-checkout').filter({ visible: true })).toHaveValue('2027-06-13')
      await expect(opened.getByTestId('stay-adults').filter({ visible: true })).toHaveText('3')
      await expect(opened.getByRole('spinbutton', { name: 'Maximum price per night in euros' }).filter({ visible: true })).toHaveValue('180.5')
      await expect(visible(opened, 'Advanced')).toHaveAttribute('aria-expanded', 'true')
      await expect(visible(opened, 'Quiet zones')).toHaveAttribute('aria-pressed', 'true')
      await expect(opened.getByTestId('quiet-threshold').filter({ visible: true })).toHaveValue('17')
      await expect.poll(() => remote.stays.some(p => p.get('checkin') === '2027-06-10' && p.get('checkout') === '2027-06-13'
        && p.get('adults') === '3' && p.get('type') === 'hotel' && p.get('minstars') === '4' && p.get('minscore') === '8')).toBe(true)
      await expect.poll(() => opened.getByTestId('layers-scroll').filter({ visible: true }).evaluate((element, offset) => Math.abs(Math.round(element.scrollTop) - offset), offset)).toBeLessThanOrEqual(1)
      await opened.screenshot({ path: test.info().outputPath('received-filters.png') })

      // Show the hotel pin (the sparse noise fixture deliberately has no quiet value here).
      await visible(opened, 'Quiet zones').click()
      if (phone) await visible(opened, 'Drag to close').click()
      else await visible(opened, 'Fold').click()
      const { x, y } = await canvasCenter(opened)
      await expect.poll(async () => {
        await opened.mouse.click(x, y)
        return opened.getByTestId('stay-header').filter({ visible: true }).count()
      }).toBe(1)
      await expect(opened.getByTestId('stay-header').filter({ visible: true })).toContainText('Shared hotel')
      const selected = opened.url()
      await page.goto(selected)
      await expect(page.getByTestId('stay-header').filter({ visible: true })).toContainText('Shared hotel')
      await expect(page.getByTestId('stay-price').filter({ visible: true })).toContainText('€120 a night')
      await answer(page)
      await page.screenshot({ path: test.info().outputPath('received-stay.png') })
    } finally { await recipient.close() }
  })

  test('the address carries open sources, calculation, reachable area and phone sheet size', async ({ page, browser }) => {
    if (!phone) await page.setViewportSize({ width: 1280, height: 520 })
    await fixtures(page)
    await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng}`)
    await answer(page)
    const row = popup(page).getByRole('button', { name: /Fixture street/ })
    await row.click()
    await popup(page).getByTestId('calculation-toggle').click()
    await visible(page, 'Reachable area').click()
    await page.getByRole('spinbutton', { name: 'Minutes', exact: true }).filter({ visible: true }).fill('17')
    await visible(page, 'Car').click()
    await visible(page, 'Show area').click()
    await expect(visible(page, 'Show area')).toBeEnabled()
    const detailScroller = popup(page).locator('..')
    await detailScroller.hover()
    await page.mouse.wheel(0, 160)
    if (phone) await visible(page, 'Collapse').click()
    await expect.poll(() => new URLSearchParams(new URL(page.url()).hash.slice(1)).get('scroll')).toBeTruthy()
    const offset = await popup(page).locator('..').evaluate(element => Math.round(element.scrollTop))
    const link = page.url()
    const recipient = await browser.newContext(phone ? PHONE : { viewport: { width: 1280, height: 520 } })
    try {
      const opened = await recipient.newPage()
      const remote = await fixtures(opened)
      await opened.goto(link)
      await answer(opened)
      await expect(visible(opened, 'Reachable area')).toHaveAttribute('aria-expanded', 'true')
      await expect(opened.getByRole('spinbutton', { name: 'Minutes', exact: true }).filter({ visible: true })).toHaveValue('17')
      await expect(visible(opened, 'Car')).toHaveAttribute('aria-pressed', 'false')
      await expect.poll(() => remote.areas.some(p => p.get('minutes') === '17' && p.get('modes') === 'walk'
        && Math.abs(Number(p.get('lat')) - POINT.lat) < 0.000001)).toBe(true)
      if (phone) await expect(visible(opened, 'Expand')).toBeVisible()
      await expect(popup(opened).getByRole('button', { name: /Fixture street/ })).toHaveAttribute('aria-expanded', 'true')
      await expect(popup(opened).getByTestId('calculation-toggle')).toHaveAttribute('aria-expanded', 'true')
      await expect.poll(() => popup(opened).locator('..').evaluate((element, offset) => Math.abs(Math.round(element.scrollTop) - offset), offset)).toBeLessThanOrEqual(1)
      await opened.screenshot({ path: test.info().outputPath('received-panels.png') })
    } finally { await recipient.close() }
  })
  test('a pasted point with the same scroll offset restores through a growing answer', async ({ page }) => {
    if (!phone) await page.setViewportSize({ width: 1280, height: 520 })
    await fixtures(page)
    const growingAnswer = (seq: number, partial: boolean, lat: number, lng: number, count: number) => {
      const data = popupUpdate(seq, partial, lat, lng, 60)
      return { ...data, top_contributors: Array.from({ length: count }, (_, i) => ({
        ...data.top_contributors[0], id: i.toString(16).padStart(16, '0'), name: `Street ${i}`, share: 1 / count,
      })) }
    }
    await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng}&calc=1&popup=%7B%22rest%22%3Atrue%7D&scroll=0,500`)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    await sendPopupLine(page, growingAnswer(1, false, POINT.lat, POINT.lng, 50))
    await endPopup(page)
    const scroller = popup(page).locator('..')
    await expect.poll(() => scroller.evaluate(element => Math.round(element.scrollTop))).toBe(500)
    // A gesture ends restoration; the next address must start it again even at the same offset.
    await scroller.dispatchEvent('pointerdown')
    const next = new URLSearchParams(new URL(page.url()).hash.slice(1))
    next.set('d', `${POINT.lat + 0.01},${POINT.lng}`)
    await page.evaluate(hash => { location.hash = hash }, next.toString())
    await expect.poll(() => popupRequests(page)).toHaveLength(2)
    const point = (await popupRequests(page))[1]
    await sendPopupLine(page, growingAnswer(1, true, point.lat, point.lng, 12))
    await expect(popup(page)).toBeVisible()
    await page.waitForTimeout(200)
    expect(new URLSearchParams(new URL(page.url()).hash.slice(1)).get('scroll')).toBe('0,500')
    await sendPopupLine(page, growingAnswer(2, false, point.lat, point.lng, 50))
    await endPopup(page)
    await expect.poll(() => scroller.evaluate(element => Math.round(element.scrollTop))).toBe(500)
  })
})

test('a tilted, rotated view opens at the same camera; pasted links and Back replace every public selection', async ({ page, browser }) => {
  test.setTimeout(45_000)
  await fixtures(page)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.move(x - 80, y)
  await page.mouse.down({ button: 'right' })
  await page.mouse.move(x + 45, y - 75, { steps: 10 })
  await page.mouse.up({ button: 'right' })
  await expect(page).toHaveURL(url => Math.abs(Number(new URLSearchParams(url.hash.slice(1)).get('b'))) > 10
    && Number(new URLSearchParams(url.hash.slice(1)).get('p')) > 10)
  await page.mouse.click(x - 90, y - 100)
  await answer(page)
  await popup(page).getByRole('button', { name: /Fixture street/ }).click()
  const link = page.url()
  const recipient = await browser.newContext({ viewport: { width: 1280, height: 720 } })
  try {
    const opened = await recipient.newPage()
    await fixtures(opened)
    await opened.goto(link)
    await answer(opened)
    // A second click at the same screen pixel must unproject to the same geographic point.
    await page.mouse.click(x - 150, y - 80)
    await opened.mouse.click(x - 150, y - 80)
    await expect.poll(async () => (await popupRequests(page)).length).toBe(2)
    await expect.poll(async () => (await popupRequests(opened)).length).toBe(2)
    const originalPoint = (await popupRequests(page))[1], copiedPoint = (await popupRequests(opened))[1]
    expect(copiedPoint.lat).toBeCloseTo(originalPoint.lat, 5)
    expect(copiedPoint.lng).toBeCloseTo(originalPoint.lng, 5)

    const previous = opened.url()
    const next = new URL(previous), params = new URLSearchParams(next.hash.slice(1))
    params.set('panel', 'layers'); params.set('stay', '1'); params.set('guests', '5'); params.set('qc', '1')
    params.set('qt', '2'); params.set('adv', '1'); params.set('m', 'nden'); params.delete('d'); params.delete('popup')
    await opened.evaluate(hash => { location.hash = hash }, params.toString())
    await expect(opened.getByTestId('stay-adults').filter({ visible: true })).toHaveText('5')
    await expect(visible(opened, 'Advanced')).toHaveAttribute('aria-expanded', 'true')
    await expect(popup(opened)).toHaveCount(0)
    await opened.goBack()
    await expect.poll(async () => (await popupRequests(opened)).length).toBe(3)
    await sendPopupLine(opened, popupUpdate(1, false, copiedPoint.lat, copiedPoint.lng, 60))
    await expect(visible(opened, 'Places to stay')).toHaveCount(0)
    const restored = new URLSearchParams(new URL(opened.url()).hash.slice(1))
    expect(restored.get('qc')).toBeNull()
    expect(restored.get('guests')).toBeNull()
    expect(restored.get('d')).toBe(new URLSearchParams(new URL(previous).hash.slice(1)).get('d'))
  } finally { await recipient.close() }
})

test('a shared stay without a current offer keeps its point and explains the missing offer', async ({ page }) => {
  await fixtures(page)
  await page.route('**/api/stay?**', route => route.fulfill({ json: { listings: [], nights: 2, currency: 'EUR', expiresIn: 3600, failure: null } }))
  await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng}&sid=gone&stay=1&in=2027-06-10&out=2027-06-12`)
  await answer(page)
  await expect(page.getByTestId('stay-status').filter({ visible: true })).toHaveText('No current offer was found for this stay and these dates.')
  await expect(page.getByRole('link', { name: 'View offer' }).filter({ visible: true })).toHaveCount(0)
  expect(new URLSearchParams(new URL(page.url()).hash.slice(1)).get('sid')).toBe('gone')
})

test('an About address reopens its explanations and section', async ({ page, browser }) => {
  await page.goto('/about/the-noise-map')
  await page.getByText('The limits and the last check', { exact: true }).click()
  const details = page.locator('details').filter({ has: page.getByText('The limits and the last check', { exact: true }) })
  await expect(details).toHaveAttribute('open', '')
  await expect(page).toHaveURL(/open=/)
  const recipient = await browser.newContext()
  try {
    const opened = await recipient.newPage()
    await opened.goto(page.url())
    await expect(opened.locator('details').filter({ has: opened.getByText('The limits and the last check', { exact: true }) })).toHaveAttribute('open', '')
    await expect(opened.getByText('The limits and the last check', { exact: true })).toBeInViewport()
  } finally { await recipient.close() }
})

test('past dates in a copied address stay unchanged and never show an offer for other dates', async ({ page }) => {
  const remote = await fixtures(page)
  await page.goto(`${mapUrl(POINT)}&stay=1&panel=layers&in=2020-06-10&out=2020-06-13&d=${POINT.lat},${POINT.lng}&sid=shared-hotel`)
  await expect(page.getByTestId('stay-checkin').filter({ visible: true })).toHaveValue('2020-06-10')
  await expect(page.getByTestId('stay-checkout').filter({ visible: true })).toHaveValue('2020-06-13')
  await expect(page.getByTestId('stay-status').filter({ visible: true })).toHaveText('These stay dates have passed. Choose new dates.')
  await expect(page.getByRole('link', { name: 'View offer' }).filter({ visible: true })).toHaveCount(0)
  expect(remote.stays).toHaveLength(0)
})

for (const phone of [false, true]) test.describe(phone ? 'phone search address' : 'desktop search address', () => {
  if (phone) test.use(PHONE)
  test('a copied search result preserves the padded camera, and an offscreen point stays offscreen', async ({ page, browser }) => {
    await fixtures(page)
    const target = { display_name: 'Test destination', secondary: 'Test country', lat: POINT.lat, lon: POINT.lng }
    await page.route('**/api/search?**', route => route.fulfill({ json: [target] }))
    await page.goto(mapUrl(POINT))
    await page.getByRole('combobox').fill('Test destination')
    await page.getByRole('option', { name: /Test destination/ }).click()
    await answer(page)
    if (!phone) {
      await page.mouse.move(400, 250)
      await page.mouse.down({ button: 'right' })
      await page.mouse.move(480, 190, { steps: 10 })
      await page.mouse.up({ button: 'right' })
      await expect(page).toHaveURL(url => Number(new URLSearchParams(url.hash.slice(1)).get('p')) > 10)
    }
    const recipient = await browser.newContext(phone ? PHONE : { viewport: { width: 1280, height: 720 } })
    try {
      const opened = await recipient.newPage()
      await fixtures(opened)
      await opened.goto(page.url())
      await answer(opened)
      await page.mouse.click(170, 220)
      await opened.mouse.click(170, 220)
      await expect.poll(async () => (await popupRequests(page)).length).toBe(2)
      await expect.poll(async () => (await popupRequests(opened)).length).toBe(2)
      const a = (await popupRequests(page))[1], b = (await popupRequests(opened))[1]
      expect(b.lat).toBeCloseTo(a.lat, 5)
      expect(b.lng).toBeCloseTo(a.lng, 5)
      await sendPopupLine(opened, popupUpdate(1, false, b.lat, b.lng, 60))
      await endPopup(opened)
      await expect(popup(opened)).toBeVisible()
      const url = new URL(opened.url()), params = new URLSearchParams(url.hash.slice(1))
      params.set('lat', '40'); params.set('lng', '10')
      url.hash = params.toString()
      await opened.goto(url.toString())
      await expect.poll(() => Number(new URLSearchParams(new URL(opened.url()).hash.slice(1)).get('lat'))).toBe(40)
      await expect(popup(opened)).toBeVisible()
      expect(new URLSearchParams(new URL(opened.url()).hash.slice(1)).get('d')).toBe(params.get('d'))
    } finally { await recipient.close() }
  })
})


test('old world-copy points reopen; a busy shared stay retries without changing its selection', async ({ page }) => {
  await fixtures(page)
  let calls = 0
  await page.route('**/api/stay?**', route => {
    calls++
    return calls === 1 ? route.fulfill({ status: 503, headers: { 'Retry-After': '1' }, json: { error: 'busy' } })
      : route.fulfill({ json: { listings: [HOTEL], nights: 3, currency: 'EUR', expiresIn: 3600, failure: null } })
  })
  await page.goto(`${mapUrl(POINT)}&d=${POINT.lat},${POINT.lng + 360}&sid=${HOTEL.id}&in=2027-06-10&out=2027-06-13`)
  await answer(page)
  await expect(page.getByTestId('stay-header').filter({ visible: true })).toContainText(HOTEL.name)
  expect(calls).toBe(2)
  const [point] = await popupRequests(page)
  expect(point.lng).toBeCloseTo(POINT.lng, 6)
})

// The standard base map comes from the map's own server, and OpenStreetMap's credit on it folds
// away, leaving a phone's bottom strip to the map; the satellite map keeps Esri's credit in sight.
import { expect, test } from '@playwright/test'
import { PHONE, POINT, canvasCenter, installHermeticMap, mapUrl } from './support'

const STANDARD = mapUrl(POINT).replace('bm=terrain', 'bm=standard')

for (const phone of [false, true]) test.describe(phone ? 'phone' : 'desktop', () => {
  if (phone) test.use(PHONE)

  test('the credit shows at load, folds at the first move of the map and stays on the satellite map', async ({ page }) => {
    const requested: string[] = []
    page.on('request', request => requested.push(request.url()))
    await installHermeticMap(page, POINT)
    await page.route('**/World_Imagery/MapServer/tile/**', route => route.fulfill({ status: 204 }))
    const ownTile = page.waitForRequest(/\/api\/basemap\/\d+\/\d+\/\d+\.mvt$/)
    await page.goto(STANDARD)
    await ownTile
    const credits = page.locator('.maplibregl-ctrl-attrib')
    await expect(credits).toHaveText('© OpenStreetMap contributors')
    await expect(credits).toBeInViewport({ ratio: 1 })

    const { x, y } = await canvasCenter(page)
    await page.mouse.move(x, y)
    await page.mouse.down()
    await page.mouse.move(x, y - 80, { steps: 4 })
    await page.mouse.up()
    // At once, well before the five seconds that would fold it anyway.
    await expect(credits).toBeHidden({ timeout: 1500 })
    // Folded, the credit is found through the ⓘ; a phone's control row drops to the bottom edge.
    const info = page.getByRole('link', { name: 'About quietmap.org' })
    await expect(info).toBeVisible()
    await expect(info).toHaveAttribute('href', '/about')
    if (phone) {
      const bottomGap = async (name: string) => {
        const box = (await page.getByRole('button', { name, exact: true }).filter({ visible: true }).boundingBox())!
        return page.viewportSize()!.height - (box.y + box.height)
      }
      await expect.poll(() => bottomGap('Standard')).toBe(16)
      await expect.poll(() => bottomGap('Toggle layers panel')).toBe(16)
      const infoBox = (await info.boundingBox())!
      expect(page.viewportSize()!.height - (infoBox.y + infoBox.height)).toBe(16)
    }

    await page.getByRole('button', { name: 'Satellite', exact: true }).filter({ visible: true }).click()
    await expect(credits).toContainText('Powered by Esri')
    await expect(credits).toContainText('© OpenStreetMap contributors')
    await expect(credits).toBeInViewport({ ratio: 1 })
    await page.getByRole('button', { name: 'Standard', exact: true }).filter({ visible: true }).click()
    await expect(credits).toBeHidden()
    expect(requested.filter(url => url.includes('cartocdn.com'))).toEqual([])
  })
})

test('a map left alone folds the credit after five seconds', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(STANDARD)
  const credits = page.locator('.maplibregl-ctrl-attrib')
  await expect(credits).toBeVisible()
  const shown = Date.now()
  await expect(credits).toBeHidden()
  expect(Date.now() - shown).toBeGreaterThan(2000)
})

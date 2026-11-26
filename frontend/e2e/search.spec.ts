// A search, its address results and the selected place form one column on desktop and phone.
import { expect, test } from '@playwright/test'
import { popupUpdate } from './answers'
import { PHONE, POINT, canvasCenter, installHermeticMap, mapUrl, popupRequests, sendPopupLine } from './support'

for (const phone of [false, true]) test.describe(phone ? 'phone' : 'desktop', () => {
  if (phone) test.use(PHONE)
  test('history keeps the level of the exact point it reopens', async ({ page }) => {
    await installHermeticMap(page, POINT)
    const url = mapUrl(POINT) + '&m=lden'
    await page.goto(url)
    const search = page.getByRole('combobox')
    const option = page.getByRole('option')
    const open = page.getByTestId('open-place')
    const first = { lat: 50.554111, lng: 16.798389 }, second = { lat: 50.554111, lng: 16.798194 }
    async function select(at: typeof first, count: number) {
      await search.fill(`${at.lat}, ${at.lng}`)
      await option.click()
      await expect.poll(() => popupRequests(page)).toHaveLength(count)
      expect((await popupRequests(page)).at(-1)).toEqual(at)
    }
    async function answer(at: typeof first, db: number) {
      await sendPopupLine(page, popupUpdate(1, false, at.lat, at.lng, db))
      await expect(open).toContainText(`${db.toFixed(1)} dB`)
    }
    await select(first, 1)
    await answer(first, 59)
    await select(second, 2)
    await expect(open).not.toContainText('59.0 dB')
    await answer(second, 75.8)
    await search.click()
    await expect(option).toHaveCount(1)
    await expect(option).toContainText('59.0 dB')
    await search.press('Escape')

    const field = open.getByTestId('place-title')
    await field.fill('Beside the road')
    await field.press('Enter')
    await select(first, 3)
    await answer(first, 59)
    await page.goto('/?reload=history'+url.slice(1))
    await search.click()
    await expect(option).toContainText('Beside the road')
    await expect(option).toContainText('75.8 dB')
    await option.getByRole('button', { name: 'Beside the road', exact: true }).click()
    await expect.poll(() => popupRequests(page)).toEqual([second])
    await answer(second, 75.8)
  })

  test('search results join the field; selecting and opening elsewhere never reopen the list', async ({ page }) => {
    await installHermeticMap(page, POINT)
    const at = { lat: POINT.lat + 0.001, lng: POINT.lng + 0.001 }
    await page.route('**/api/search?**', route => route.fulfill({ json: [
      { display_name: 'Selected address', secondary: 'District, Country', lat: at.lat, lon: at.lng },
      { display_name: 'Another address', secondary: 'District, Country', lat: at.lat + 0.001, lon: at.lng },
    ] }))
    await page.goto(mapUrl(POINT))
    const search = page.getByRole('combobox')
    await search.fill('address')
    const list = page.getByRole('listbox')
    await expect(page.getByRole('option')).toHaveCount(2)
    const fieldBox = (await search.boundingBox())!, listBox = (await list.boundingBox())!
    expect(Math.abs(fieldBox.x - listBox.x)).toBeLessThanOrEqual(1)
    expect(Math.abs(fieldBox.width - listBox.width)).toBeLessThanOrEqual(1)
    expect(listBox.y - fieldBox.y - fieldBox.height).toBeLessThanOrEqual(1)
    await page.screenshot({ path: test.info().outputPath('search-results.png') })
    await page.getByRole('option').first().click()
    await expect.poll(() => popupRequests(page)).toEqual([at])
    await expect(list).toHaveCount(0)
    await expect(search).not.toBeFocused()
    await expect(search).toHaveAttribute('placeholder', 'Search another place…')
    const chosen = page.getByTestId('open-place'), chosenBox = (await chosen.boundingBox())!
    expect(Math.abs(chosenBox.width - fieldBox.width)).toBeLessThanOrEqual(1)
    expect(chosenBox.y - fieldBox.y - fieldBox.height).toBeLessThanOrEqual(1)
    await chosen.locator('svg').first().click()
    await expect(list).toHaveCount(0)
    await expect(search).not.toBeFocused()
    const map = await canvasCenter(page)
    await page.mouse.click(map.x - 70, map.y - 150)
    await expect.poll(() => popupRequests(page)).toHaveLength(2)
    await expect(list).toHaveCount(0)
    await page.screenshot({ path: test.info().outputPath('selected-place.png') })
  })
})

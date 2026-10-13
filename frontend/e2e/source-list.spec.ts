// The click's list uses precise shares and the same ten-row limit on desktop and phone.
import { expect, test } from '@playwright/test'
import { popupUpdate, roadContributor } from './answers'
import { canvasCenter, endPopup, installHermeticMap, mapUrl, PHONE, POINT, sendPopupLine, SOURCE_DB } from './support'

for (const display of [
  { name: 'desktop', options: { viewport: { width: 1280, height: 720 } } },
  { name: 'phone', options: PHONE },
]) {
  test.describe(display.name, () => {
    test.use(display.options)
    test('sources below 1% fold into Rest and ten qualifying rows remain', async ({ page }) => {
      await installHermeticMap(page, POINT)
      await page.goto(mapUrl(POINT))
      const { x, y } = await canvasCenter(page)
      await page.mouse.click(x, y)

      const data = popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB)
      const shares = [0.4, 0.25, 0.15, 0.0099, ...Array<number>(8).fill(0.02)]
      data.top_contributors = shares.map((share, index) => ({
        ...roadContributor(SOURCE_DB),
        id: index.toString(16).padStart(16, '0'),
        name: `Fixture source ${index}`,
        nden_sone: shares.length - index,
        share,
      }))
      data.rest_share = 1 - shares.reduce((sum, share) => sum + share, 0)
      await sendPopupLine(page, data)
      await endPopup(page)

      const popup = page.locator('[data-testid="detail-popup"]:visible')
      const rows = popup.getByRole('button').filter({ has: page.getByTestId('row-share') })
      await expect(rows.locator('span.font-medium')).toHaveText([
        ...[0, 1, 2, 4, 5, 6, 7, 8, 9, 10].map(index => `Fixture source ${index}`),
        'Rest (2)',
      ])
      await popup.getByRole('button', { name: /^Rest/ }).click()
      await expect(popup.locator('span.font-medium').filter({ hasText: /^Fixture source 3$/ })).toBeVisible()
      await expect(popup.locator('span.font-medium').filter({ hasText: /^Fixture source 11$/ })).toBeVisible()
    })
  })
}

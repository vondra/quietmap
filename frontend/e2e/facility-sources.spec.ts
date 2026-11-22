// A visitor checks historical industrial uncertainty, follows the dated sources and returns.
import { readFileSync } from 'node:fs'
import { expect, test } from '@playwright/test'
import { PHONE } from './support'

const years = JSON.parse(readFileSync(new URL('../.generated/years.json', import.meta.url), 'utf8')) as { name: string; manifest: { built: boolean } }[]

for (const device of ['desktop', 'phone'] as const) {
  test.describe(device, () => {
    if (device === 'phone') test.use(PHONE)
    test('industrial history distinguishes operation from mapped estimates', async ({ page }, info) => {
      await page.goto('/about/data-preparation#industry')
      const history = page.locator('main p').filter({ hasText: 'A dated operating interval or an earlier observed status' })
      await expect(history).toContainText('operation remains unknown')
      await expect(history).toContainText('Names can suggest a sector')
      await history.scrollIntoViewIfNeeded()
      await page.screenshot({ path: info.outputPath(`facility-history-${device}.png`) })
      await history.getByRole('link', { name: 'sources by year' }).click()
      await expect(page).toHaveURL(/\/about\/data-sources#sources-by-year$/)
      await expect(page.locator('main')).not.toContainText('Facility identities, geometry, activity, capacity units')
      for (const year of years) {
        await expect(page.getByRole('heading', { name: year.name, exact: true })).toHaveCount(year.manifest.built ? 1 : 0)
      }
      expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
      await page.goBack()
      await expect(history).toBeVisible()
    })
  })
}

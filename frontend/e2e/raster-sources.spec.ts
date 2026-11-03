// A visitor follows the dated forest sources and returns without seeing duplicate mosaic inputs.
import { expect, test } from '@playwright/test'
import { PHONE } from './support'

for (const device of ['desktop', 'phone'] as const) {
  test.describe(device, () => {
    if (device === 'phone') test.use(PHONE)
    test('forest sources distinguish the survey from annual losses', async ({ page }, info) => {
      await page.goto('/about/data-sources/europe#sources-by-year')
      const survey = page.locator('main li').filter({ hasText: 'HRL tree-cover density survey of 2023' })
      await expect(survey).toHaveCount(1)
      await expect(survey.locator('strong')).toHaveText('2023')
      await survey.scrollIntoViewIfNeeded()
      await page.screenshot({ path: info.outputPath(`forest-survey-${device}.png`) })
      await page.getByRole('link', { name: 'world sources', exact: true }).click()
      await expect(page).toHaveURL(/\/about\/data-sources#sources-by-year$/)
      const baseline = page.locator('main li').filter({ hasText: 'Tree cover in 2000; annual losses are a separate input.' })
      const losses = page.locator('main li').filter({ hasText: 'Annual tree-cover loss codes, applied only through the map year.' })
      await expect(baseline.locator('strong')).toHaveText('2000')
      await expect(losses.locator('strong')).toHaveText('2001 to 2024')
      await expect(page.locator('main')).not.toContainText('Native pixels of the same observation')
      await expect(page.getByRole('heading', { name: '2024', exact: true })).toHaveCount(0)
      await losses.scrollIntoViewIfNeeded()
      await page.screenshot({ path: info.outputPath(`forest-loss-${device}.png`) })
      expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
      await page.goBack()
      await expect(survey).toBeVisible()
    })
  })
}

// A visitor follows the dated forest sources and returns without seeing duplicate mosaic inputs.
import { readFileSync } from 'node:fs'
import { expect, test } from '@playwright/test'
import { PHONE } from './support'

const years = JSON.parse(readFileSync(new URL('../.generated/years.json', import.meta.url), 'utf8')) as { name: string; manifest: { built: boolean } }[]
const builtCount = years.filter(year => year.manifest.built).length

for (const device of ['desktop', 'phone'] as const) {
  test.describe(device, () => {
    if (device === 'phone') test.use(PHONE)
    test('forest sources distinguish the survey from annual losses', async ({ page }, info) => {
      await page.goto('/about/data-sources/europe#sources-by-year')
      const survey = page.locator('main li').filter({ hasText: 'HRL tree-cover density survey of 2023' })
      await expect(survey).toHaveCount(builtCount)
      await expect(survey.first().locator('strong')).toHaveText('2023')
      await survey.first().scrollIntoViewIfNeeded()
      await page.screenshot({ path: info.outputPath(`forest-survey-${device}.png`) })
      await page.getByRole('link', { name: 'world sources', exact: true }).click()
      await expect(page).toHaveURL(/\/about\/data-sources#sources-by-year$/)
      const baseline = page.locator('main li').filter({ hasText: 'Tree cover in 2000; annual losses are a separate input.' })
      const losses = page.locator('main li').filter({ hasText: 'Annual tree-cover loss codes, applied only through the map year.' })
      await expect(baseline).toHaveCount(builtCount)
      await expect(losses).toHaveCount(builtCount)
      await expect(baseline.first().locator('strong')).toHaveText('2000')
      await expect(losses.first().locator('strong')).toHaveText('2001 to 2024')
      await expect(page.locator('main')).not.toContainText('Native pixels of the same observation')
      for (const year of years) {
        await expect(page.getByRole('heading', { name: year.name, exact: true })).toHaveCount(year.manifest.built ? 1 : 0)
      }
      await losses.first().scrollIntoViewIfNeeded()
      await page.screenshot({ path: info.outputPath(`forest-loss-${device}.png`) })
      expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
      await page.goBack()
      await expect(survey.first()).toBeVisible()
    })
  })
}

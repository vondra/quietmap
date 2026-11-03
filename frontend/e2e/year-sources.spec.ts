// A visitor follows a region to a country, reads both years, opens the shared sources, and returns.
import { expect, test } from '@playwright/test'
import { PHONE } from './support'

for (const device of ['desktop', 'phone'] as const) {
  test.describe(device, () => {
    if (device === 'phone') test.use(PHONE)
    for (const [region, country, code, publisher] of [
      ['asia', 'Thailand', 'th', 'Department of Rural Roads'],
      ['north-america', 'United States', 'us', 'FHWA'],
    ]) {
      test(`${country} lists its inputs and gaps by year`, async ({ page }, info) => {
        const errors: string[] = []
        page.on('pageerror', error => errors.push(error.message))
        await page.goto(`/about/data-sources/${region}`)
        await page.getByRole('link', { name: country, exact: true }).click()
        await expect(page).toHaveURL(new RegExp(`/about/data-sources/${region}/${code}$`))
        await expect(page.getByRole('heading', { name: country, exact: true })).toBeVisible()
        await expect(page.getByRole('heading', { name: '2026', exact: true })).toBeVisible()
        await expect(page.getByRole('link', { name: publisher, exact: false }).first()).toBeVisible()
        const details = page.locator('details').first()
        await expect(details).not.toHaveAttribute('open', '')
        await details.locator('summary').click()
        await expect(details).toHaveAttribute('open', '')
        await expect(details.getByText('OpenStreetMap contributors', { exact: true }).first()).toBeVisible()
        await details.locator('summary').click()
        await page.getByRole('heading', { name: '2024', exact: true }).scrollIntoViewIfNeeded()
        await expect(page.getByText('This year’s inputs are being prepared.', { exact: false })).toBeVisible()
        await expect(page.getByText('Where an earlier source is missing', { exact: true }).first()).toBeVisible()
        await page.screenshot({ path: info.outputPath(`${code}-${device}-earlier.png`) })
        expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
        await page.locator('main').evaluate(element => { element.scrollTop = 0 })
        await page.screenshot({ path: info.outputPath(`${code}-${device}.png`) })
        expect(errors).toEqual([])
        await page.goBack()
        await expect(page.getByRole('link', { name: country, exact: true })).toBeVisible()
      })
    }
  })
}

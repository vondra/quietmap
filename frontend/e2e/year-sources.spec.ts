// A visitor reads a country's method, follows its shared sources and returns; only built maps appear.
import { readFileSync } from 'node:fs'
import { expect, test } from '@playwright/test'
import { canvasCenter, endPopup, installHermeticMap, mapUrl, PHONE, POINT, sendPopupLine, SOURCE_DB } from './support'
import { popupUpdate } from './answers'

const years = JSON.parse(readFileSync(new URL('../.generated/years.json', import.meta.url), 'utf8')) as { name: string; manifest: { built: boolean } }[]
const citations = JSON.parse(readFileSync(new URL('../.generated/datasets.json', import.meta.url), 'utf8')) as Record<string, [string, string, string, string]>
for (const device of ['desktop', 'phone'] as const) {
  test.describe(device, () => {
    if (device === 'phone') test.use(PHONE)
    test('a country without admitted road counts explains its model and retains the source journey', async ({ page }, info) => {
      await page.goto('/about/data-sources/asia')
      await page.getByRole('link', { name: 'India', exact: true }).click()
      const explanation = page.getByText('No road count at or before this year is used for this country.', { exact: false })
      await explanation.scrollIntoViewIfNeeded()
      await expect(explanation).toBeVisible()
      await expect(explanation).toContainText('global model')
      await page.screenshot({ path: info.outputPath(`missing-road-counts-${device}.png`) })
      expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
      await page.getByRole('link', { name: 'world sources', exact: true }).click()
      await expect(page).toHaveURL(/\/about\/data-sources#sources-by-year$/)
      await page.goBack()
      await expect(page).toHaveURL(/\/about\/data-sources\/asia\/in$/)
      await expect(explanation).toBeVisible()
    })
    for (const [region, country, code, method, publisher] of [
      ['asia', 'Thailand', 'th', 'vehicle-kilometres', 'Department of Rural Roads'],
      ['asia', 'Japan', 'jp', 'it is not a measured zero', 'MLIT'],
      ['europe', 'Netherlands', 'nl', 'The published all-vehicle total', 'Rijkswaterstaat'],
      ['europe', 'Italy', 'it', 'daily averages describe November 2015', 'Anas'],
      ['north-america', 'United States', 'us', 'vehicle mix comes from FHWA', 'FHWA'],
    ]) {
      test(`${country} preserves its method and publishes only built years`, async ({ page }, info) => {
        const errors: string[] = []
        page.on('pageerror', error => errors.push(error.message))
        await page.goto(`/about/data-sources/${region}`)
        await page.getByRole('link', { name: country, exact: true }).click()
        const countryURL = `/about/data-sources/${region}/${code}`
        await expect(page).toHaveURL(new RegExp(`${countryURL}$`))
        await expect(page.getByRole('heading', { name: 'Roads', exact: true })).toBeVisible()
        await expect(page.locator('main')).toContainText(method)
        await page.getByText(method, { exact: false }).scrollIntoViewIfNeeded()
        await page.screenshot({ path: info.outputPath(`${code}-${device}-method.png`) })
        for (const year of years) {
          await expect(page.getByRole('heading', { name: year.name, exact: true })).toHaveCount(year.manifest.built ? 1 : 0)
        }
        const sources = page.getByRole('heading', { name: 'Sources by year', exact: true })
        await sources.scrollIntoViewIfNeeded()
        await expect(page.getByRole('link', { name: publisher, exact: false }).last()).toBeAttached()
        await expect(page.getByRole('link', { name: 'OpenStreetMap contributors', exact: true })).toHaveCount(0)
        await expect(page.locator('main')).not.toContainText(/enrichment|[\w-]+-cache|paged|keeps its behaviour/)
        await page.screenshot({ path: info.outputPath(`${code}-${device}-sources.png`) })
        expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
        await page.getByRole('link', { name: 'world sources', exact: true }).click()
        await expect(page).toHaveURL(/\/about\/data-sources#sources-by-year$/)
        await expect(page.locator('main a[href="https://planet.openstreetmap.org/"]')).toHaveCount(years.filter(year => year.manifest.built).length)
        await page.goBack()
        await expect(page).toHaveURL(new RegExp(`${countryURL}$`))
        await expect(sources).toBeVisible()
        expect(errors).toEqual([])
      })
    }
    test('a road cites the manifest once in its traffic detail', async ({ page }, info) => {
      await installHermeticMap(page, POINT)
      await page.goto(mapUrl(POINT))
      const { x, y } = await canvasCenter(page)
      await page.mouse.click(x, y)
      const answer = popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB)
      answer.top_contributors[0].metadata!.source_id = 10
      await sendPopupLine(page, answer)
      await endPopup(page)
      const popup = page.locator('[data-testid="detail-popup"]:visible')
      await popup.getByRole('button', { name: /^Fixture street/ }).click()
      const traffic = popup.getByText('9.6k/day', { exact: true })
      if (device === 'phone') await traffic.tap()
      else await traffic.hover()
      const tooltip = page.getByRole('tooltip')
      const [publisher, observed] = citations['10']
      await expect(tooltip).toContainText(`Data: ${publisher}, ${observed}`)
      expect((await tooltip.innerText()).split(publisher)).toHaveLength(2)
      const bounds = await tooltip.boundingBox()
      expect(bounds!.height).toBeLessThan(400)
      await page.screenshot({ path: info.outputPath(`citation-${device}.png`) })
    })
  })
}

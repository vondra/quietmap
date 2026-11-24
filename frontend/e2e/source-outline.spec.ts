// Opening a site's row draws its boundary; late responses cannot draw a site after it is closed.
import { expect, test } from '@playwright/test'
import { MAP_YEAR } from '../src/lib/map-year'
import { popupUpdate, roadContributor } from './answers'
import { canvasCenter, endPopup, installHermeticMap, mapUrl, pngCenterPixel, PHONE, POINT, sendPopupLine, SOURCE_DB } from './support'

for (const display of [
  { name: 'desktop', options: { viewport: { width: 1280, height: 720 } } },
  { name: 'phone', options: PHONE },
]) {
  test.describe(display.name, () => {
    test.use(display.options)
    test('a mapped site loads on opening, paints its area and drops a late response after closing', async ({ page }) => {
      await installHermeticMap(page, POINT)
      const id = 'fedcba9876543210'
      const { lat, lng } = POINT
      const outline = {
        type: 'FeatureCollection',
        features: [{ type: 'Feature', properties: {}, geometry: {
          type: 'Polygon', coordinates: [[[lng - 0.05, lat - 0.05], [lng + 0.05, lat - 0.05],
            [lng + 0.05, lat + 0.05], [lng - 0.05, lat + 0.05], [lng - 0.05, lat - 0.05]]],
        } }],
      }
      let requests = 0
      let release!: () => void
      let held = Promise.resolve()
      await page.route('**/api/source-outline/**', async route => {
        expect(new URL(route.request().url()).searchParams.get('year')).toBe(MAP_YEAR)
        requests++
        await held
        await route.fulfill({ json: outline }).catch(() => {})
      })
      await page.goto(mapUrl(POINT))
      const { x, y } = await canvasCenter(page)
      await page.mouse.click(x, y)
      const data = popupUpdate(1, true, lat, lng, SOURCE_DB)
      data.top_contributors = [{
        ...roadContributor(SOURCE_DB), id, name: 'Fixture works', source_type: 'industrial',
        metadata: { source_type: 'factory', area_m2: 10_000 }, geometry: [[[lat, lng]]],
      }]
      await sendPopupLine(page, data)
      const row = page.getByRole('button', { name: /^Fixture works/ }).filter({ visible: true })
      await expect(row).toBeVisible()
      expect(requests).toBe(0)
      const shade = async () => (await pngCenterPixel(page,
        await page.screenshot({ clip: { x: x + 60, y: y - 60, width: 1, height: 1 } })))[0]
      await expect.poll(shade).toBeLessThan(10)
      await row.click()
      await expect.poll(() => requests).toBe(1)
      await expect.poll(shade).toBeGreaterThan(50)
      await row.click()
      await expect.poll(shade).toBeLessThan(10)

      // A new site has no previous outline to reuse while its request is outstanding.
      const final = popupUpdate(2, false, lat, lng, SOURCE_DB)
      final.top_contributors = [{ ...data.top_contributors[0], id: 'fedcba9876543211' }]
      await sendPopupLine(page, final)
      await endPopup(page)
      held = new Promise<void>(resolve => { release = resolve })
      await row.click()
      await expect.poll(() => requests).toBe(2)
      await expect.poll(shade).toBeLessThan(10)
      await row.click()
      release()
      await expect.poll(shade).toBeLessThan(10)
      await expect(page.getByTestId('noise-badge').filter({ visible: true })).toHaveText('15 sone Nden')
    })

    test('nearby unmodelled ranges show with null and non-null levels and draw their outline on open', async ({ page }) => {
      await installHermeticMap(page, POINT)
      const rangeId = '1234567890abcdef'
      const { lat, lng } = POINT
      const outline = {
        type: 'FeatureCollection',
        features: [{ type: 'Feature', properties: {}, geometry: {
          type: 'Polygon', coordinates: [[[lng - 0.05, lat - 0.05], [lng + 0.05, lat - 0.05],
            [lng + 0.05, lat + 0.05], [lng - 0.05, lat + 0.05], [lng - 0.05, lat - 0.05]]],
        } }],
      }
      let requests = 0
      await page.route('**/api/source-outline/**', async route => {
        expect(new URL(route.request().url()).pathname).toBe(`/api/source-outline/${rangeId}`)
        requests++
        await route.fulfill({ json: outline }).catch(() => {})
      })
      await page.goto(mapUrl(POINT))
      const { x, y } = await canvasCenter(page)
      await page.mouse.click(x, y)

      // First: a non-null modelled update keeps the unmodelled range beside the level and shares.
      const unmodelled = [
        {
          id: rangeId,
          source_type: 'industrial' as const,
          kind: 'military_training_area' as const,
          name: 'Fixture Northern Artillery Training Area',
          inside: false,
          distance_m: 80,
          area_m2: 34_200_000,
        },
        {
          id: 'abcdef1234567890',
          source_type: 'industrial' as const,
          kind: 'military_range' as const,
          name: '',
          inside: true,
          distance_m: 0,
          area_m2: 120_000,
        },
      ]
      const modelled = popupUpdate(1, true, lat, lng, SOURCE_DB)
      modelled.unmodelled = unmodelled
      await sendPopupLine(page, modelled)
      const section = page.getByTestId('unmodelled-section').filter({ visible: true })
      await expect(page.getByTestId('noise-badge').filter({ visible: true })).toContainText('sone')
      await expect(section).toBeVisible()
      await expect(section).toContainText('Not in the level')
      await expect(section.getByRole('button', { name: /Fixture Northern Artillery Training Area/ })).toContainText('80 m')

      // Second: a null-level update (including a building with no exposed facade) still displays
      // the unmodelled ranges and draws the opened range's outline.
      const silent = popupUpdate(2, false, lat, lng, SOURCE_DB)
      silent.total_lden = null
      silent.total = { ld: null, le: null, ln: null, lden: null }
      silent.loudness = null
      silent.building = { id: 'b1', height_m: 12, floor: 2, facade: null }
      silent.top_contributors = []
      silent.unmodelled = unmodelled
      await sendPopupLine(page, silent)
      await endPopup(page)

      await expect(section).toBeVisible()
      await expect(section).toContainText('Not in the level')
      const rangeRow = section.getByRole('button', { name: /Fixture Northern Artillery Training Area/ })
      await expect(rangeRow).toContainText('80 m')
      const insideRow = section.getByRole('button', { name: /Military firing range/ })
      await expect(insideRow).toContainText('Inside')
      const shade = async () => (await pngCenterPixel(page,
        await page.screenshot({ clip: { x: x + 60, y: y - 60, width: 1, height: 1 } })))[0]
      await expect.poll(shade).toBeLessThan(10)
      await rangeRow.click()
      await expect.poll(() => requests).toBe(1)
      await expect.poll(shade).toBeGreaterThan(50)
      await expect(section).toContainText('Military training area')
      await expect(section).toContainText('34.2 km²')
      await expect(section).toContainText('Firing and blast noise is not included in the level.')
      await rangeRow.click()
      await expect.poll(shade).toBeLessThan(10)
    })
  })
}

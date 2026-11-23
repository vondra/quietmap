// Visitors can inspect the missing-calendar estimate and the annual railway method on desktop and phone.
import { expect, test } from '@playwright/test'
import { popupUpdate } from './answers'
import { canvasCenter, endPopup, installHermeticMap, mapUrl, PHONE, POINT, popupRequests, sendPopupLine, SOURCE_DB } from './support'

for (const device of ['desktop', 'phone'] as const) {
  test.describe(device, () => {
    if (device === 'phone') test.use(PHONE)
    test('a railway count distinguishes timetable trains from missing-calendar estimates', async ({ page }, info) => {
      await installHermeticMap(page, POINT)
      await page.goto(mapUrl(POINT, 'railway'))
      const { x, y } = await canvasCenter(page)
      if (device === 'phone') await page.touchscreen.tap(x, y)
      else await page.mouse.click(x, y)
      await expect.poll(() => popupRequests(page)).toHaveLength(1)
      const answer = popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB)
      answer.sources[0].source_type = 'railway'
      answer.top_contributors = [{
        ...answer.top_contributors[0], source_type: 'railway', name: 'Fixture railway', subtype: 'rail',
        metadata: {
          rail_type: 'rail', usage: 'main', speed_kmh: 100, speed_source: 'osm_posted',
          passenger_status: 2, freight_status: 2, passenger_source_id: 100, freight_source_id: 0,
          trains_passenger_day: 40, trains_passenger_evening: 10, trains_passenger_night: 5,
          trains_freight_day: 2, trains_freight_evening: 1, trains_freight_night: 1,
          passenger_missing_calendar_estimate_share: 0.75, freight_missing_calendar_estimate_share: 0,
        },
      }]
      await sendPopupLine(page, answer)
      await endPopup(page)
      const popup = page.locator('[data-testid="detail-popup"]:visible')
      const railway = popup.getByRole('button', { name: /^Fixture railway/ })
      if (device === 'phone') await railway.tap()
      else await railway.click()
      const trains = popup.getByText('55 passenger · 4 freight/day', { exact: true })
      if (device === 'phone') await trains.tap()
      else await trains.hover()
      const tooltip = page.getByRole('tooltip')
      await expect(tooltip).toContainText('75.0% estimated for dates without timetable data')
      await expect(tooltip).toContainText('Day / evening / night: 40 / 10 / 5')
      const bounds = await tooltip.boundingBox()
      const viewport = page.viewportSize()!
      expect(bounds!.x).toBeGreaterThanOrEqual(0)
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport.width)
      expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height)
      await page.screenshot({ path: info.outputPath(`rail-count-${device}.png`) })
    })
    test('the annual method explains both map years and calendar coverage', async ({ page }, info) => {
      await page.goto('/about/data-preparation#railways')
      const summary = page.locator('summary').filter({ hasText: 'How timetables become annual train counts' })
      const detail = page.locator('details').filter({ has: summary })
      await expect(detail).not.toHaveAttribute('open', '')
      await summary.click()
      await expect(detail).toContainText('including weekends')
      await expect(detail).toContainText('busiest Wednesday')
      await expect(detail).toContainText('Each operator keeps its own observation horizon')
      await expect(detail).toContainText('On a matched line, a covered day with no scheduled service counts as zero.')
      await expect(detail).toContainText('A service with no departure on an incomplete calendar has no annual timetable estimate')
      await summary.evaluate(element => element.scrollIntoView({ block: 'start' }))
      await page.evaluate(() => window.scrollBy(0, -100))
      await page.screenshot({ path: info.outputPath(`rail-method-${device}.png`) })
      expect(await page.locator('main').evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true)
      await page.goto('/about/data-sources/europe/fr')
      await expect(page.locator('main')).toContainText('one busy Wednesday')
      await expect(page.locator('main')).toContainText('four archived 2024 feeds')
      await expect(page.locator('main')).toContainText('Bruitparif identified train noise at 23 sites')
      await expect(page.locator('main')).not.toContainText('contribute no scheduled services')
      for (const [country, text] of [
        ['europe/fi', '42 of 366 days in 2024'],
        ['europe/be', '195 of 366 days covered in 2024'],
        ['europe/de', 'archived 2024 long-distance, regional and local feeds'],
        ['europe/cz', 'no 2024 CIS JR archive is available'],
        ['europe/dk', 'no 2024 Danish archive was recovered'],
        ['asia/il', '31 of 366 days covered'],
        ['north-america/mx', 'Metro, Tren Ligero and Tren Suburbano'],
      ]) {
        await page.goto(`/about/data-sources/${country}`)
        await expect(page.locator('main')).toContainText(text)
        await expect(page.locator('main')).not.toContainText('contributes no timetable counts')
      }
    })
  })
}

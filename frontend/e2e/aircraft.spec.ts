// The aircraft layer of the popup in a real browser, without a backend: a loudest flight's track on
// the map while its row is hovered (tapped on a phone), redrawn by every streamed update, and airport
// ground operations as rows of their own.
import { expect, test, type Page } from '@playwright/test'
import type { TrackPiece } from '../src/types/noise'
import { FIXTURE_FLIGHTS, airportContributor, popupUpdate, withAircraft } from './answers'
import {
  PHONE,
  POINT,
  SOURCE_DB,
  canvasCenter,
  endPopup,
  installHermeticMap,
  mapPixels,
  mapUrl,
  popupRequests,
  sendPopupLine,
  TILE_Z,
} from './support'

/** The middle of a track piece, [lat, lon]. */
const middle = ([start, end]: TrackPiece) => [(start[0] + end[0]) / 2, (start[1] + end[1]) / 2]

// The highlight is dev1's white line on a black casing; the hermetic map is black, so a place is
// drawn when a pixel within 3 px of it is near white.
async function trackDrawnAt(page: Page, places: number[][]): Promise<boolean[]> {
  const degrees = 3 * 360 / (512 * 2 ** TILE_Z)
  const offsets = [[0, 0], [degrees, 0], [-degrees, 0], [0, degrees], [0, -degrees]]
  const around = places.flatMap(([lat, lng]) => offsets.map(([dLat, dLng]) => [lat + dLat, lng + dLng]))
  const white = (await mapPixels(page, POINT, around)).map(pixel => Math.min(...pixel.slice(0, 3)) > 200)
  return places.map((_, index) => white.slice(index * offsets.length, (index + 1) * offsets.length).some(Boolean))
}

const [AIRBUS, HELICOPTER] = FIXTURE_FLIGHTS
const [AIRBUS_EAST, AIRBUS_WEST, AIRBUS_BETWEEN] = AIRBUS.track.map(middle)
const HELICOPTER_MIDDLE = middle(HELICOPTER.track[0])

test('desktop: a hovered flight draws its track on the map, redrawn by every streamed update', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  // The first ring computed the Airbus's pieces east and west of the point, not yet the one between.
  const firstRing = { ...AIRBUS, track: AIRBUS.track.slice(0, 2) }
  await sendPopupLine(page, withAircraft(popupUpdate(1, true, POINT.lat, POINT.lng, SOURCE_DB), [firstRing, HELICOPTER]))
  const popup = page.locator('[data-testid="detail-popup"]:visible')
  await popup.getByRole('button', { name: /^Aircraft/ }).click()
  const rows = popup.getByRole('table', { name: 'Loudest flights' }).locator('tbody tr')
  const lmaxCell = (row: number) => rows.nth(row).locator('td').first()
  const airbus = [AIRBUS_EAST, AIRBUS_WEST, AIRBUS_BETWEEN]
  await expect.poll(() => trackDrawnAt(page, [...airbus, HELICOPTER_MIDDLE])).toEqual([false, false, false, false])

  // A hovered row draws its flight's pieces where they are, apart where nothing was computed.
  await lmaxCell(0).hover()
  await expect.poll(() => trackDrawnAt(page, airbus)).toEqual([true, true, false])
  await sendPopupLine(page, withAircraft(popupUpdate(2, true, POINT.lat, POINT.lng, SOURCE_DB), [AIRBUS, HELICOPTER]))
  await expect.poll(() => trackDrawnAt(page, airbus)).toEqual([true, true, true])
  // Another row draws its own track; leaving the table draws none.
  await lmaxCell(1).hover()
  await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST, HELICOPTER_MIDDLE])).toEqual([false, true])
  await page.mouse.move(x, y + 100)
  await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST, HELICOPTER_MIDDLE])).toEqual([false, false])

  // A flight that leaves the list takes its track along, though the pointer stays on its row.
  await lmaxCell(0).hover()
  await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST])).toEqual([true])
  await sendPopupLine(page, withAircraft(popupUpdate(3, false, POINT.lat, POINT.lng, SOURCE_DB), [HELICOPTER]))
  await endPopup(page)
  await expect(rows).toHaveCount(1)
  await expect.poll(() => trackDrawnAt(page, airbus)).toEqual([false, false, false])

  // Closing the popup ends the highlight though the pointer never left the row: the next click's
  // answer lists the same flight and draws no track.
  await page.mouse.move(x, y + 100)
  await lmaxCell(0).hover()
  await expect.poll(() => trackDrawnAt(page, [HELICOPTER_MIDDLE])).toEqual([true])
  await page.locator('button[aria-label="Close"]:visible').press('Enter')
  await expect(popup).toHaveCount(0)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(2)
  await sendPopupLine(page, withAircraft(popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB), FIXTURE_FLIGHTS))
  await expect(popup.getByTestId('noise-badge')).toBeVisible()
  await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST, HELICOPTER_MIDDLE])).toEqual([false, false])
})

test('desktop: airport ground operations read as their airport and its traffic', async ({ page }) => {
  await installHermeticMap(page, POINT)
  await page.goto(mapUrl(POINT))
  const { x, y } = await canvasCenter(page)
  await page.mouse.click(x, y)
  await expect.poll(() => popupRequests(page)).toHaveLength(1)
  const update = withAircraft(popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB), FIXTURE_FLIGHTS)
  await sendPopupLine(page, { ...update, top_contributors: [...update.top_contributors, airportContributor(52)] })
  // A row of its own beside the Aircraft layer's row, whose level includes it.
  const popup = page.locator('[data-testid="detail-popup"]:visible')
  await expect(popup.getByRole('button')).toHaveText([/^Fixture street/, /^Aircraft/, /^LKPR ground operations/, /Segments/])
  const airport = popup.getByRole('button', { name: /^LKPR ground operations/ })
  await airport.click()
  // Its class in words, then its fields by name: the subtype is the class, not a line of its own.
  await expect(airport.locator('xpath=following-sibling::div[1]').locator(':scope > div')).toHaveText([
    'Airport ground operations',
    /^airport\s*Letiště Václava Havla Praha$/,
    /^arrivals per day\s*180\.4$/,
    /^departures per day\s*181\.2$/,
    /^ground vehicles per day\s*36\.5$/,
    /^Day\/Evening\/Night/,
  ])
})

test.describe('mobile', () => {
  test.use(PHONE)

  test('a tapped flight draws its track above the sheet, until another row or a new click', async ({ page }) => {
    await installHermeticMap(page, POINT)
    await page.goto(mapUrl(POINT))
    const { x, y } = await canvasCenter(page)
    await page.touchscreen.tap(x, y)
    await expect.poll(() => popupRequests(page)).toHaveLength(1)
    await sendPopupLine(page, withAircraft(popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB), FIXTURE_FLIGHTS))
    const sheet = page.getByTestId('mobile-detail-sheet')
    await sheet.getByRole('button', { name: /^Aircraft/ }).tap()
    const rows = sheet.getByRole('table', { name: 'Loudest flights' }).locator('tbody tr')
    await rows.nth(0).locator('td').first().tap()
    await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST, AIRBUS_WEST, AIRBUS_BETWEEN])).toEqual([true, true, true])
    await rows.nth(1).locator('td').first().tap()
    await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST, HELICOPTER_MIDDLE])).toEqual([false, true])
    // A new click draws none, though its answer lists the same flights.
    await page.touchscreen.tap(x, y - 120)
    await expect.poll(() => popupRequests(page)).toHaveLength(2)
    await sendPopupLine(page, withAircraft(popupUpdate(1, false, POINT.lat, POINT.lng, SOURCE_DB), FIXTURE_FLIGHTS))
    await expect(sheet.getByTestId('noise-badge')).toBeVisible()
    await expect.poll(() => trackDrawnAt(page, [AIRBUS_EAST, HELICOPTER_MIDDLE])).toEqual([false, false])
  })
})

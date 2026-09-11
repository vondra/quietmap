import { expect, test } from '@playwright/test'
import { canvasCenter, hm3PixelCenter, mockTerrainBasemap } from './support'

// HM3 stores 0.5 dB steps (≤0.25 dB quantisation error) and both surfaces
// display one decimal. One dB leaves a conservative numeric/projection margin
// while still catching a user-visible map/popup disagreement.
const REAL_PARITY_TOLERANCE_DB = 1

const SCENARIOS = [
  { name: 'D4 49.8486,14.1639', lat: 49.8486, lng: 14.1639 },
  { name: 'LKPR mid-runway', lat: 50.114, lng: 14.27 },
] as const

test('real data: D4 and LKPR map hover match the clicked popup', async ({ page, request }, testInfo) => {
  test.skip(!process.env.E2E_REAL_BASE_URL, 'set E2E_REAL_BASE_URL to run the real-data smoke')

  // A deployment without its prepared data (`/api/ready` 503 whose only
  // failed component is `prepared-data`) or without a published heatmap
  // generation (manifest 404) has nothing to compare: skip with the reason,
  // never fail or pass. Any other readiness failure is a broken deployment.
  const ready = await request.get('/api/ready')
  const readiness = await ready.text()
  const failed = ready.status() === 503 ? (JSON.parse(readiness) as { failed?: string[] }).failed ?? [] : []
  test.skip(failed.length > 0 && failed.every(component => component === 'prepared-data'), `E2E_REAL_BASE_URL has no prepared data: ${readiness}`)
  expect(ready.status(), readiness).toBe(200)
  const manifestResponse = await request.get('/api/tiles-manifest')
  test.skip(manifestResponse.status() === 404, 'no heatmap generation is published at E2E_REAL_BASE_URL')
  expect(manifestResponse.status()).toBe(200)
  const manifest = await manifestResponse.json() as { build: string; zoom: number; layers: Record<string, { build?: string }> }
  const totalBuild = manifest.layers.total?.build ?? manifest.build
  expect(totalBuild).toMatch(/^b\d+$/)
  // Hover at the published base zoom: the readout then quotes the exact
  // receiver cell the popup computes, not a coarser pyramid cell.
  const tileZoom = manifest.zoom
  expect(Number.isInteger(tileZoom)).toBe(true)

  await mockTerrainBasemap(page)
  await page.route('**/api/reverse?**', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ place: 'E2E real-data point' }),
  }))

  for (const [scenarioIndex, scenario] of SCENARIOS.entries()) {
    await test.step(scenario.name, async () => {
      const point = hm3PixelCenter(scenario.lat, scenario.lng, tileZoom)
      const tilePath = `/api/tiles/${totalBuild}/total/${tileZoom}/${point.tx}/${point.ty}.bin`
      const targetTile = page.waitForResponse(response => new URL(response.url()).pathname === tilePath)
      // A changed, ignored query forces a fresh MapApp for the second point;
      // changing only the hash would leave useUrlState's initial view intact.
      await page.goto(
        `/?e2e-real=${scenarioIndex}#lat=${point.lat}&lng=${point.lng}&z=${tileZoom}&bm=terrain`,
      )
      expect((await targetTile).status()).toBe(200)

      const { x, y } = await canvasCenter(page)
      await page.mouse.move(x, y)
      const hover = page.getByTestId('heatmap-hover')
      await expect(hover).toHaveText(/^Lden: \d+\.\d dB$/)
      const hoverText = await hover.textContent()
      const hoverDb = Number(hoverText!.match(/([0-9]+\.[0-9])/)![1])

      const popupResponse = page.waitForResponse(response =>
        new URL(response.url()).pathname === '/api/noise-onfly-v2',
      )
      await page.mouse.click(x, y)
      const response = await popupResponse
      expect(response.status()).toBe(200)
      const api = await response.json() as { total_lden: number | null }
      expect(typeof api.total_lden).toBe('number')
      const popupDb = api.total_lden!
      await expect(page.locator('[data-testid="noise-badge"]:visible'))
        .toHaveText(`${popupDb.toFixed(1)} dB`)

      const requestUrl = new URL(response.url())
      expect(Math.abs(Number(requestUrl.searchParams.get('lat')) - point.lat)).toBeLessThan(1e-6)
      expect(Math.abs(Number(requestUrl.searchParams.get('lng')) - point.lng)).toBeLessThan(1e-6)
      const deltaDb = Math.abs(hoverDb - popupDb)
      const values = { scenario: scenario.name, hover_db: hoverDb, popup_db: popupDb, delta_db: deltaDb }
      console.log(`real-data ${scenario.name}: HM3 ${hoverDb.toFixed(1)} dB, popup ${popupDb.toFixed(1)} dB, |Δ| ${deltaDb.toFixed(2)} dB`)
      await testInfo.attach(`${scenario.name}-values`, {
        contentType: 'application/json',
        body: Buffer.from(JSON.stringify(values, null, 2)),
      })
      expect(deltaDb).toBeLessThanOrEqual(REAL_PARITY_TOLERANCE_DB)
    })
  }
})

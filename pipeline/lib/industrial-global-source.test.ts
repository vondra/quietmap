/** Original registry classifications and fail-closed admission of all requested sources. */

import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { GLOBAL_INDUSTRIAL_SOURCES, loadGlobalIndustrialSources, parseGlobalIndustrialSource } from './industrial-global-source.js'

const csv = (fuels: string[]) => 'latitude,longitude,primary_fuel\n' + fuels.map(fuel => `50,14,${fuel}`).join('\n')
const gem = (status: string, latitude: unknown = 50, longitude: unknown = 14) => ({
  type: 'Feature', geometry: { type: 'Point', coordinates: [15, 51] },
  properties: { status, Latitude: latitude, Longitude: longitude },
})

test('observed fuel and Annex sector select original profiles; wind/unknown never guess thermal', () => {
  const gppd = parseGlobalIndustrialSource(csv(['Coal', 'Hydro', 'Solar', 'Geothermal', 'Wind', '', 'Other']), GLOBAL_INDUSTRIAL_SOURCES[0])
  assert.deepEqual(gppd.facilities.map(f => f.nace4), [3511, 3512, 3599, 3512])
  assert.equal(gppd.census.unclassified, 3)
  const results = ['1(a)', '2(a)', '3(a)', '4(a)(viii)', '5(a)', '6(a)', '7(a)', '8(a)', '9(a)', 'unknown'].map(activity =>
    ({ y_4326: 50, x_4326: 14, EPRTRAnnexIMainActivity: activity }))
  const eprtr = parseGlobalIndustrialSource(JSON.stringify({ results }), GLOBAL_INDUSTRIAL_SOURCES[1])
  assert.deepEqual(eprtr.facilities.map(f => f.nace4), [3511, 2410, 2351, 2011, 3821, 1711, 146, 1011, 1310])
  assert.equal(eprtr.census.unclassified, 1)
})

test('GEM lifecycle and observed coordinate precedence; finite equator/meridian coordinates remain observations', () => {
  const features = [gem('operating'), gem('operating-pre-retirement', 0, 0), gem('retired'),
    gem('proposed'), gem('operating', 'NaN'), gem('operating', 91),
    { ...gem('operating'), properties: { status: 'operating' } }]
  for (const source of GLOBAL_INDUSTRIAL_SOURCES.slice(2)) {
    const parsed = parseGlobalIndustrialSource(JSON.stringify({ type: 'FeatureCollection', features }), source)
    assert.deepEqual(parsed.facilities.map(f => [f.lat, f.lon]), [[50, 14], [0, 0], [51, 15]])
    assert.deepEqual(parsed.census, { raw: 7, invalidCoordinates: 2, inactive: 2, unclassified: 0, classified: 3 })
  }
  const invalid = parseGlobalIndustrialSource('latitude,longitude,primary_fuel\nInfinity,14,Coal\n50,190,Coal', GLOBAL_INDUSTRIAL_SOURCES[0])
  assert.equal(invalid.census.invalidCoordinates, 2)
  assert.throws(() => parseGlobalIndustrialSource('{"features":[]}', GLOBAL_INDUSTRIAL_SOURCES[2]), /FeatureCollection/)
  assert.throws(() => parseGlobalIndustrialSource('{"results":[]}', GLOBAL_INDUSTRIAL_SOURCES[1]), /empty/)
})

test('full requested feed set must be present and above original source floors before admission succeeds', () => {
  const work = mkdtempSync(resolve(tmpdir(), 'industrial-source-'))
  try {
    writeFileSync(resolve(work, 'gppd.csv'), csv(new Array<string>(1000).fill('Solar')))
    assert.throws(() => loadGlobalIndustrialSources(work), /eprtr-facilities/)
    writeFileSync(resolve(work, 'eprtr-facilities.json'), JSON.stringify({ results: Array.from({ length: 10_000 }, () =>
      ({ y_4326: 50, x_4326: 14, EPRTRAnnexIMainActivity: '2(a)' })) }))
    for (const source of GLOBAL_INDUSTRIAL_SOURCES.slice(2)) writeFileSync(resolve(work, source.file),
      JSON.stringify({ type: 'FeatureCollection', features: Array.from({ length: 100 }, () => gem('operating')) }))
    const admitted = loadGlobalIndustrialSources(work)
    assert.equal(admitted.receipts.length, 5)
    assert.equal(admitted.facilities.length, 11_300)
    assert.equal(admitted.receipts.reduce((sum, r) => sum + r.raw, 0), 11_300)
    writeFileSync(resolve(work, 'gem-cement.geojson'), JSON.stringify({ type: 'FeatureCollection', features: [gem('operating')] }))
    assert.throws(() => loadGlobalIndustrialSources(work), /gem-cement.*below 100/)
  } finally { rmSync(work, { recursive: true, force: true }) }
})

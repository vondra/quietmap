/** Common GEM source admission and literal territory-policy regressions. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { GEM_COUNTRIES, gemAreaContains, gemCountryOwns } from './industrial-gem-countries.js'
import { parseGemPoints, gemFuelNace } from './industrial-gem-source.js'
import { iso2Code } from './prepared-grid.js'

const policy = (country: string) => GEM_COUNTRIES.find(p => p.country === country)!
const collection = (features: unknown[]) => JSON.stringify({ type: 'FeatureCollection', features })
const point = { geometry: { type: 'Point', coordinates: [-2, 12] }, properties: { Status: 'operating', Type: 'Geothermal' } }

test('documented empty is no-write testimony; malformed, absent and arbitrary empty are not admitted', () => {
  assert.equal(GEM_COUNTRIES.length, 102)
  assert.deepEqual(GEM_COUNTRIES.filter(p => p.knownEmpty).map(p => p.country), ['HT', 'LI', 'MC', 'SM', 'ST'])
  assert.deepEqual(parseGemPoints(collection([]), policy('HT')), [])
  for (const input of ['', '{}', collection([]), collection([{}]), collection([{ ...point, properties: {} }]),
    collection([{ ...point, geometry: { type: 'Point', coordinates: [null, 12] } }])]) {
    assert.throws(() => parseGemPoints(input, policy('BF')))
  }
  const retired = parseGemPoints(collection([{ ...point, properties: { Status: 'retired', Type: 'coal' } }]), policy('BF'))
  assert.equal(retired.length, 1); assert.equal(retired[0].status, 'retired')
  assert.deepEqual(['', 'unknown', 'wind', 'solar', 'hydropower', 'geothermal', 'nuclear', 'coal'].map(gemFuelNace),
    [null, null, null, 3599, 3512, 3511, 3511, 3511])
})

test('literal masks preserve MA/EH, isolated NC, Andorra cuts and country-only source gates', () => {
  assert.equal(gemCountryOwns(policy('MA'), 27, -13, iso2Code('EH')), true)
  assert.equal(gemCountryOwns(policy('MA'), 27, -13, iso2Code('DZ')), false)
  assert.equal(gemCountryOwns(policy('NC'), -22, 166, 0), true)
  assert.equal(gemCountryOwns(policy('NC'), 48, 2, iso2Code('FR')), false)
  assert.equal(gemAreaContains(policy('AD'), 42.42, 1.5, iso2Code('AD')), false)
  assert.equal(gemAreaContains(policy('AD'), 42.5, 1.5, iso2Code('AD')), true)
  assert.equal(gemAreaContains(policy('MX'), 31, -110, iso2Code('US')), false)
  assert.equal(gemAreaContains(policy('BF'), 12, -2, iso2Code('ML')), true, 'original source bbox differs from destructive row ownership')
})

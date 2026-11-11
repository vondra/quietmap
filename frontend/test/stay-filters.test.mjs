// Quiet-stay selection: exact quoted prices and visible world copies.
import assert from 'node:assert/strict'
import test from 'node:test'
import { belowPrice, stayInView } from '../src/lib/stay-filters.ts'

test('the exact EUR quote must meet the nightly ceiling, even when both displayed prices round alike', () => {
  const quote = { total: 300, nights: 2, currency: 'EUR' }
  assert.equal(belowPrice(quote, 150), true)
  assert.equal(belowPrice({ ...quote, total: 300.01 }, 150), false)
  assert.equal(belowPrice({ ...quote, total: null }, 150), false)
  assert.equal(belowPrice({ ...quote, currency: 'USD' }, 150), false)
  assert.equal(belowPrice({ ...quote, total: 0 }, 0), true)
  assert.equal(belowPrice({ ...quote, total: null }, null), true)
})

test('visible stays wrap into a view across the antimeridian', () => {
  const view = { west: 179, east: 181, south: -20, north: -16, zoom: 10 }
  assert.equal(stayInView({ lng: -179.5, lat: -18 }, view), true)
  assert.equal(stayInView({ lng: 179.5, lat: -21 }, view), false)
  assert.equal(stayInView({ lng: 0, lat: -18 }, view), false)
})

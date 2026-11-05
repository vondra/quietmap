// Quiet-stay selection: exact quoted prices and reachable polygons, including holes and world copies.
import assert from 'node:assert/strict'
import test from 'node:test'
import { belowPrice, insideReach, stayInView } from '../src/lib/stay-filters.ts'

const ring = (w, s, e, n) => [[w, s], [e, s], [e, n], [w, n], [w, s]]
const area = (...geometries) => ({ type: 'FeatureCollection', features: geometries.map(geometry => ({ type: 'Feature', geometry, properties: {} })) })

test('the exact EUR quote must meet the nightly ceiling, even when both displayed prices round alike', () => {
  const quote = { total: 300, nights: 2, currency: 'EUR' }
  assert.equal(belowPrice(quote, 150), true)
  assert.equal(belowPrice({ ...quote, total: 300.01 }, 150), false)
  assert.equal(belowPrice({ ...quote, total: null }, 150), false)
  assert.equal(belowPrice({ ...quote, currency: 'USD' }, 150), false)
  assert.equal(belowPrice({ ...quote, total: 0 }, 0), true)
  assert.equal(belowPrice({ ...quote, total: null }, null), true)
})

test('reach is a union of modes and islands, with holes excluded and antimeridian copies equivalent', () => {
  const reach = area(
    { type: 'Polygon', coordinates: [ring(179, -20, -179, -16), ring(179.5, -19, -179.5, -18)] },
    { type: 'MultiPolygon', coordinates: [[ring(176, -20, 177, -19)], [ring(174, -20, 175, -19)]] },
  )
  for (const lng of [179.2, -180.8, 539.2, -179.2]) assert.equal(insideReach({ lng, lat: -18 }, reach), true)
  assert.equal(insideReach({ lng: 176.5, lat: -19.5 }, reach), true)
  assert.equal(insideReach({ lng: 174.5, lat: -19.5 }, reach), true)
  assert.equal(insideReach({ lng: 180, lat: -18.5 }, reach), false)
  assert.equal(insideReach({ lng: 0, lat: -18 }, reach), false)
  assert.equal(insideReach({ lng: 179, lat: -20 }, reach), true)
  assert.equal(insideReach({ lng: 179, lat: -20 }, area()), false)
})

test('visible stays wrap into a view across the antimeridian', () => {
  const view = { west: 179, east: 181, south: -20, north: -16, zoom: 10 }
  assert.equal(stayInView({ lng: -179.5, lat: -18 }, view), true)
  assert.equal(stayInView({ lng: 179.5, lat: -21 }, view), false)
  assert.equal(stayInView({ lng: 0, lat: -18 }, view), false)
})

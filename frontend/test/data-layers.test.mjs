// The data layers a link names, in the panel's order, and the one the heatmap draws beneath.
import assert from 'node:assert/strict'
import test from 'node:test'

import { lowestDataLayerId, parseDataLayers } from '../src/lib/data-layers.ts'

test('a link names its data layers in the panel order; unknown names and an absent list name none', () => {
  assert.deepEqual(parseDataLayers('barriers,elevation'), ['elevation', 'barriers'])
  assert.deepEqual(parseDataLayers('roads,,constructor,forest'), ['forest'])
  assert.deepEqual(parseDataLayers(''), [])
  assert.deepEqual(parseDataLayers(null), [])
})

test('the heatmap draws beneath the lowest data layer, or beneath the labels when none is on', () => {
  const style = ['background', 'data-hard', 'data-barriers', '_label-place'].map(id => ({ id }))
  assert.equal(lowestDataLayerId(style), 'data-hard')
  assert.equal(lowestDataLayerId([{ id: 'background' }, { id: 'data-other' }]), undefined)
  assert.equal(lowestDataLayerId(undefined), undefined)
})

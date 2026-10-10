// An opened row's segments on the map: the colours of the list, the selected segment's rays, and what
// the map frames first.
import assert from 'node:assert/strict'
import test from 'node:test'

import { belowColor, rayColors, segmentFan } from '../src/components/noise/segments/fan.ts'

const terms = [0, 0, 0, 0, 0.1, 100]
const piece = (lden, rays, ends = [[50, 14], [50, 14.001]]) => ({ received: { lden }, ends, rays })

test('the loudest is red, 10 dB under it violet, 20 blue, 30 and more gray', () => {
  assert.deepEqual([0, -10, -20, -30, -45].map(belowColor), ['#dc2626', '#7c3aed', '#2563eb', '#94a3b8', '#94a3b8'])
})

test('a silent ray has no colour and is not drawn; the selected ray is marked', () => {
  const pieces = [piece(60, [[50, 14, 0.1, 50, terms], [50, 14.0005, 0.1, null, terms], [50, 14.001, 0.1, 40, terms]])]
  assert.deepEqual(rayColors(pieces[0]), ['#dc2626', null, '#7c3aed'])
  const fan = segmentFan('aa', pieces, [50.001, 14], { selected: 0, selectedRay: 2, opened: 0 })
  assert.deepEqual(fan.rays, [
    { from: [50, 14], color: '#dc2626', selected: false },
    { from: [50, 14.001], color: '#7c3aed', selected: true },
  ])
  assert.deepEqual(fan.opened, { index: 0, points: [[50, 14], [50, 14.001], [50, 14], [50, 14.0005], [50, 14.001]] })
})

test('the map first frames the segments within 20 dB of the loudest; none selected draws no rays', () => {
  const pieces = [piece(60, [], [[50, 14]]), piece(41, [], [[51, 14]]), piece(39, [], [[52, 14]])]
  const fan = segmentFan('aa', pieces, [50, 14], { selected: null, selectedRay: null, opened: null })
  assert.deepEqual(fan.overview, [[50, 14], [51, 14]])
  assert.deepEqual(fan.rays, [])
  assert.equal(fan.opened, null)
  assert.deepEqual(fan.pieces.map(drawn => drawn.selected), [false, false, false])
})

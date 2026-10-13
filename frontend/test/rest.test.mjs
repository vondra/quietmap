// The list's last row: how many sources it holds, every ground layer's unlisted contributors and the
// rows the list hides (a phone shows fewer, an aircraft layer under 0 dB); no row without sound.
import assert from 'node:assert/strict'
import test from 'node:test'

import { restSources, shownRows } from '../src/components/noise/rest.ts'

const silent = { ld: null, le: null, ln: null, lden: null }
const layer = (source_type, unlisted, unlisted_sources) => ({
  source_type, ld: 40, le: 38, ln: 30, lden: 42, unlisted, unlisted_sources, lden_upper: 42, evaluated: 1, candidates: 1,
})

test('the last row counts the popup\'s unlisted sources and the rows the list hides', () => {
  // Meloneras, Gran Canaria: 368 roads, 35 sites, 849 buildings and places and 157 ship cells.
  const layers = [
    layer('road', { ld: 21.2, le: 21.4, ln: 16.8, lden: 24.8 }, 368),
    layer('railway', silent, 0),
    layer('industrial', { ld: 0.2, le: -2.8, ln: -8.5, lden: 1.0 }, 35),
    layer('building', { ld: 17.4, le: 16.6, ln: 10.9, lden: 19.7 }, 849),
    layer('ship', { ld: 10.3, le: 11.4, ln: 11.6, lden: 17.8 }, 157),
    { ...layer('aircraft'), unlisted: undefined },
  ]
  assert.equal(restSources(layers, []), 1409)
  const hiddenRow = { contributor: { received: { ld: 30, le: 30, ln: 30, lden: 36 } } }
  assert.equal(restSources(layers, [hiddenRow, { layer: layers[5] }]), 1411)
})

test('no row when what is left out makes no sound', () => {
  assert.equal(restSources([layer('road', silent, 0)], []), null)
})

test('the list shows its loudest three and every other row of 2 % or more, ten at most', () => {
  // La Palma (28.7022, -17.9355): thirteen roads, most of them 1-2 % of the place's loudness.
  const laPalma = [33, 30, 8, 5, 4, 2, 2, 1, 2, 1, 1, 1, 1]
  assert.deepEqual(shownRows(laPalma).map((shown, row) => shown ? row : null).filter(row => row !== null),
    [0, 1, 2, 3, 4, 5, 6, 8])
  // The loudest three stay whatever their share; a phone's limit caps the rest.
  assert.deepEqual(shownRows([1, 1, 1, 1]), [true, true, true, false])
  assert.equal(shownRows(Array(14).fill(7)).filter(Boolean).length, 10)
  assert.equal(shownRows(Array(14).fill(7), 9).filter(Boolean).length, 9)
})

test('before the shares arrive the list shows its first ten', () => {
  assert.equal(shownRows(Array(13).fill(undefined)).filter(Boolean).length, 10)
  assert.deepEqual(shownRows([undefined, undefined]), [true, true])
})

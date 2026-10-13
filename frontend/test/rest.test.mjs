// The click's visible rows and Rest: shares, row limits and unlisted sources.
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

test('the list uses unrounded shares: its loudest three and rows of at least 1 %, ten at most', () => {
  const rowShares = [0.33, 0.3, 0.08, 0.05, 0.04, 0.02, 0.02, 0.0099, 0.0101, 0.01, 0.019, 0.018, 0.011]
  assert.deepEqual(shownRows(rowShares).map((shown, row) => shown ? row : null).filter(row => row !== null),
    [0, 1, 2, 3, 4, 5, 6, 8, 9, 10])
  assert.deepEqual(shownRows([0.001, 0.001, 0.001, 0.0099]), [true, true, true, false])
  assert.equal(shownRows(Array(14).fill(0.07)).filter(Boolean).length, 10)
})

test('before the shares arrive the list shows its first ten', () => {
  assert.equal(shownRows(Array(13).fill(undefined)).filter(Boolean).length, 10)
  assert.deepEqual(shownRows([undefined, undefined]), [true, true])
})

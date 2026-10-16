// Several layers on the map: their cells combined, their source and tile addresses.
import assert from 'node:assert/strict'
import test from 'node:test'

import { combineLayerCells } from '../src/lib/hm3-combine.ts'
import { HM3_NO_LEVEL, HM3_SILENCE } from '../src/lib/hm3-decoder.ts'
import { HEATMAP_LAYERS, LAYER_URL_SEPARATOR, buildKey, sourceOf, tileUrl } from '../src/lib/tile-urls.ts'

const cell = (...bytes) => combineLayerCells(bytes.map(byte => Uint8Array.of(byte)))[0]

test('a layer alone among silent ones keeps its own level; no level anywhere is no level', () => {
  for (const byte of [0, 47, 80, 133, 200, 253]) {
    assert.equal(cell(byte, HM3_SILENCE, HM3_SILENCE), byte)
  }
  assert.equal(cell(HM3_SILENCE, HM3_SILENCE), HM3_SILENCE)
  assert.equal(cell(120, HM3_NO_LEVEL), HM3_NO_LEVEL)
})

test('two equal layers add as loudness when quiet and nearly as energy when loud', () => {
  // 1 sone each (40 phon, p 1.30): 2^(1/p) sone, 7.7 phon up; 64 sone each (80 phon, p 2.72):
  // 3.7 phon up, near energy's 3.
  assert.equal(cell(80, 80), 95)
  assert.equal(cell(160, 160), 167)
  // A layer 20 phon under the loudest adds almost nothing when loud.
  assert.equal(cell(160, 120), 160)
})

test('the layers on name one source: every layer the total, one itself, several joined', () => {
  assert.equal(sourceOf([]), null)
  assert.equal(sourceOf(['road']), 'road')
  assert.equal(sourceOf(['road', 'aircraft']), 'road+aircraft')
  assert.equal(sourceOf([...HEATMAP_LAYERS]), 'total')
  const builds = { latest: 'b2', byLayer: { road: 'b1', aircraft: 'b2' }, zoom: 13 }
  assert.equal(
    tileUrl(builds, 'road+aircraft', 12, 2209, 1391),
    ['/api/tiles/b1/road/12/2209/1391.bin', '/api/tiles/b2/aircraft/12/2209/1391.bin'].join(LAYER_URL_SEPARATOR),
  )
  assert.equal(buildKey(builds, 'road+aircraft'), 'road+aircraft:b1,b2|z13')
  assert.equal(buildKey(builds, 'road'), 'road:b1|z13')
})

// The heatmap's place in the map's stack: beneath the labels and the app's own layers on every basemap.
import assert from 'node:assert/strict'
import test from 'node:test'

import { overlayAnchorId } from '../src/utils/overlay-anchor.ts'

const clickedPoint = { id: 'clicked-point-marker', type: 'circle', source: 'clicked-point' }
const highlight = { id: 'flight-track-line', type: 'line', source: 'flight-track' }
const geojson = { type: 'geojson', data: { type: 'FeatureCollection', features: [] } }

test('on the terrain basemap, whose labels are in its raster tiles, the heatmap goes beneath the clicked point and the highlighted source', () => {
  const style = {
    sources: { opentopomap: { type: 'raster', tiles: [] }, 'clicked-point': geojson, 'flight-track': geojson },
    layers: [{ id: 'opentopomap', type: 'raster', source: 'opentopomap' }, clickedPoint, highlight],
  }
  assert.equal(overlayAnchorId(style), 'clicked-point-marker')
})

test('on a basemap with labels the heatmap goes beneath the first label, below the app\'s layers too', () => {
  const style = {
    sources: { carto: { type: 'vector', url: '' }, 'clicked-point': geojson },
    layers: [{ id: 'water', type: 'fill', source: 'carto' }, { id: '_label-place', type: 'symbol', source: 'carto' }, clickedPoint],
  }
  assert.equal(overlayAnchorId(style), '_label-place')
})

test('with neither labels nor the app\'s layers the heatmap goes on top', () => {
  const style = { sources: { opentopomap: { type: 'raster', tiles: [] } }, layers: [{ id: 'opentopomap', type: 'raster', source: 'opentopomap' }] }
  assert.equal(overlayAnchorId(style), undefined)
})

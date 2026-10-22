// The basemaps' styles from CARTO's Positron: every layer's source in the style, its labels last.
import assert from 'node:assert/strict'
import test from 'node:test'

import { basemapStyle } from '../src/utils/basemaps.ts'

// Positron as CARTO serves it since October 2026: one source named by its Protomaps schema, a label
// among the ground's layers.
const positron = {
  version: 8,
  glyphs: 'https://example.org/fonts/{fontstack}/{range}.pbf',
  sprite: 'https://example.org/sprite',
  sources: { protomaps: { type: 'vector', tiles: ['https://example.org/{z}/{x}/{y}.mvt'] } },
  layers: [
    { id: 'background', type: 'background', paint: { 'background-color': '#fafaf8' } },
    { id: 'water', type: 'fill', source: 'protomaps', 'source-layer': 'water' },
    { id: 'waterway_label', type: 'symbol', source: 'protomaps', 'source-layer': 'water', paint: { 'text-color': '#7a96a0' } },
    { id: 'road_minor_fill', type: 'line', source: 'protomaps', 'source-layer': 'roads' },
    { id: 'place_town', type: 'symbol', source: 'protomaps', 'source-layer': 'places', paint: { 'text-color': '#697b89' } },
  ],
}

function sourcesMissing(style) {
  return style.layers.filter(layer => layer.source && !(layer.source in style.sources)).map(layer => layer.id)
}

test('the standard map is Positron with its labels over its ground, every layer on its own source', () => {
  const style = basemapStyle('standard', positron)
  assert.deepEqual(sourcesMissing(style), [])
  assert.deepEqual(style.layers.map(layer => layer.id),
    ['background', 'water', 'road_minor_fill', 'waterway_label', 'place_town'])
  assert.equal(style.layers.at(-1).paint['text-color'], '#1a1a1a')
  assert.equal(positron.layers[4].paint['text-color'], '#697b89')
})

test('the satellite map is the imagery under Positron\'s labels alone', () => {
  const style = basemapStyle('satellite', positron)
  assert.deepEqual(sourcesMissing(style), [])
  assert.deepEqual(style.layers.map(layer => layer.id), ['esri-imagery', 'waterway_label', 'place_town'])
  assert.equal(style.layers.at(-1).paint['text-color'], '#ffffff')
  assert.equal(style.glyphs, positron.glyphs)
})

test('without Positron the maps still draw: the heatmap on paper, the imagery without labels', () => {
  assert.deepEqual(basemapStyle('standard', null).layers.map(layer => layer.type), ['background'])
  assert.deepEqual(basemapStyle('satellite', null).layers.map(layer => layer.id), ['esri-imagery'])
})

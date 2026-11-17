// The basemaps' styles: the standard map from the map's own origin alone, the labels last and
// recoloured, every layer on a source of its style.
import assert from 'node:assert/strict'
import test from 'node:test'

import { basemapStyle } from '../src/utils/basemaps.ts'

const ORIGIN = 'https://map.example'

function sourcesMissing(style) {
  return style.layers.filter(layer => layer.source && !(layer.source in style.sources)).map(layer => layer.id)
}

function hosts(style) {
  return [...new Set(JSON.stringify(style).match(/https?:\/\/[^/"]+/g))]
}

test('the standard map loads from the map\'s own origin alone, its dark labels over its ground', () => {
  const style = basemapStyle('standard', ORIGIN)
  assert.deepEqual(sourcesMissing(style), [])
  assert.deepEqual(hosts(style), [ORIGIN])
  const types = style.layers.map(layer => layer.type)
  const firstLabel = types.indexOf('symbol')
  assert.ok(firstLabel > 0)
  assert.ok(types.slice(firstLabel).every(type => type === 'symbol'))
  const town = style.layers.find(layer => layer.id === 'places_locality')
  assert.equal(town.paint['text-color'], '#1a1a1a')
  assert.equal(town.paint['text-halo-color'], 'rgba(255,255,255,0.9)')
  // Shops and sights would crowd the noise colours.
  assert.equal(style.layers.find(layer => layer.id === 'pois'), undefined)
})

test('the satellite map is the imagery under the same labels alone, white', () => {
  const style = basemapStyle('satellite', ORIGIN)
  assert.deepEqual(sourcesMissing(style), [])
  assert.equal(style.layers[0].id, 'esri-imagery')
  assert.ok(style.layers.slice(1).every(layer => layer.type === 'symbol'))
  assert.deepEqual(style.layers.slice(1).map(layer => layer.id),
    basemapStyle('standard', ORIGIN).layers.filter(layer => layer.type === 'symbol').map(layer => layer.id))
  assert.equal(style.layers.find(layer => layer.id === 'places_locality').paint['text-color'], '#ffffff')
})

test('the terrain map has its labels in its tiles, and the map\'s fonts for the app\'s own labels', () => {
  const style = basemapStyle('terrain', ORIGIN)
  assert.deepEqual(style.layers.map(layer => layer.type), ['raster'])
  assert.equal(style.glyphs, `${ORIGIN}/api/basemap/fonts/{fontstack}/{range}.pbf`)
})

// Auto-extracted from CartoDB Positron GL style.
// The symbol-layer definitions live in ../assets/positron-labels.json — keeping
// 1.7 kLoC of static style JSON out of the TS source tree.
import type { BasemapId } from './basemaps'
import POSITRON_SYMBOL_LAYERS_JSON from '../assets/positron-labels.json'

export const CARTO_VECTOR_SOURCE = {
  type: 'vector' as const,
  url: 'https://tiles.basemaps.cartocdn.com/vector/carto.streets/v1/tiles.json',
}

export const CARTO_GLYPHS = 'https://tiles.basemaps.cartocdn.com/fonts/{fontstack}/{range}.pbf'
export const CARTO_SPRITE = 'https://tiles.basemaps.cartocdn.com/gl/positron-gl-style/sprite'

const POSITRON_SYMBOL_LAYERS = POSITRON_SYMBOL_LAYERS_JSON as any[]

function adaptPaint(paint: Record<string, any>, basemapId: BasemapId): Record<string, any> {
  const p = { ...paint }
  if (basemapId === 'standard') {
    // Dark text + white halo — bolder than default Positron grey-blue
    if (p['text-color'] && typeof p['text-color'] === 'string') {
      p['text-color'] = '#1a1a1a'
    }
    p['text-halo-color'] = 'rgba(255,255,255,0.9)'
    p['text-halo-width'] = 1.5
    if (p['icon-color']) p['icon-color'] = '#1a1a1a'
    return p
  }
  if (basemapId === 'satellite') {
    // White text + dark halo for readability over dark imagery
    if (p['text-color'] && typeof p['text-color'] === 'string' && !p['text-color'].includes('fff')) {
      p['text-color'] = '#ffffff'
    }
    p['text-halo-color'] = 'rgba(0,0,0,0.75)'
    p['text-halo-width'] = 2
    p['text-halo-blur'] = 0
    if (p['icon-color']) p['icon-color'] = '#ffffff'
    return p
  }
  return p
}

/** Get label layers for basemaps that need them (standard + satellite). */
export function getLabelLayers(basemapId: BasemapId): any[] {
  // Terrain has labels baked into raster tiles — no overlay needed
  if (basemapId === 'terrain') return []
  return POSITRON_SYMBOL_LAYERS.map(layer => ({
    ...layer,
    id: '_label-' + layer.id,
    source: 'carto',
    paint: adaptPaint(layer.paint || {}, basemapId),
  }))
}

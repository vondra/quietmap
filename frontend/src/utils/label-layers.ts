// Place labels over the standard and satellite basemaps: the CartoDB Positron symbol layers,
// auto-extracted into ../assets/positron-labels.json (1.7 kLoC of static style JSON kept out of
// the TS source tree), recoloured per basemap.
import type { SymbolLayerSpecification } from 'maplibre-gl'
import type { BasemapId } from './basemaps'
import POSITRON_SYMBOL_LAYERS_JSON from '../assets/positron-labels.json'
import { STAY_DOT_LAYER } from '../lib/stays'

/** The basemap's vector tiles' source in every style: the Positron styles name it so. */
export const CARTO_SOURCE = 'carto'

export const CARTO_VECTOR_SOURCE = {
  type: 'vector' as const,
  url: 'https://tiles.basemaps.cartocdn.com/vector/carto.streets/v1/tiles.json',
}

export const CARTO_GLYPHS = 'https://tiles.basemaps.cartocdn.com/fonts/{fontstack}/{range}.pbf'
export const CARTO_SPRITE = 'https://tiles.basemaps.cartocdn.com/gl/positron-gl-style/sprite'

type Paint = Record<string, unknown>

const POSITRON_SYMBOL_LAYERS = POSITRON_SYMBOL_LAYERS_JSON as unknown as SymbolLayerSpecification[]

function adaptPaint(paint: Paint, basemapId: BasemapId): Paint {
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
export function getLabelLayers(basemapId: BasemapId): SymbolLayerSpecification[] {
  // Terrain has labels baked into raster tiles — no overlay needed
  if (basemapId === 'terrain') return []
  return POSITRON_SYMBOL_LAYERS.map(layer => ({
    ...layer,
    id: '_label-' + layer.id,
    source: CARTO_SOURCE,
    paint: adaptPaint(layer.paint ?? {}, basemapId) as SymbolLayerSpecification['paint'],
  }))
}

/** The layer the map's overlays draw beneath, so labels stay on top: the first label of the
 *  basemap's tiles (the `_label` layers of standard and satellite, the Positron fallback's own), else,
 *  on a basemap without labels (terrain), the dots of the places to stay. Those draw over everything
 *  and their prices are no basemap label. */
export function labelAnchorId(layers: readonly { id: string; type: string; source?: unknown }[] | undefined): string | undefined {
  return layers?.find(layer => layer.type === 'symbol' && layer.source === CARTO_SOURCE)?.id
    ?? layers?.find(layer => layer.id === STAY_DOT_LAYER)?.id
}

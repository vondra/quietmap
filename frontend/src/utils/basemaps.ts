// The three basemaps (standard, terrain, satellite) and their MapLibre styles.
import type { LayerSpecification, SourceSpecification, StyleSpecification } from 'maplibre-gl'

export type BasemapId = 'standard' | 'terrain' | 'satellite'
export const DEFAULT_BASEMAP: BasemapId = 'standard'

export interface BasemapDef {
  id: BasemapId
  label: string
}

export const BASEMAPS: BasemapDef[] = [
  {
    id: 'standard',
    label: 'Standard',
  },
  {
    id: 'terrain',
    label: 'Terrain',
  },
  {
    id: 'satellite',
    label: 'Satellite',
  },
]

/** CARTO's Positron: the standard map, and the labels over the satellite imagery, both taken from
 *  the style as CARTO serves it, so the labels name its own source and tile schema. Labels kept
 *  apart for the OpenMapTiles tiles named a source its Protomaps style (October 2026) lacks, and
 *  MapLibre drew no standard map at all. */
const POSITRON_STYLE_URL = 'https://basemaps.cartocdn.com/gl/positron-gl-style/style.json'

/** Glyphs for the places to stay's prices on the terrain map, whose labels are in its tiles. */
const CARTO_GLYPHS = 'https://tiles.basemaps.cartocdn.com/fonts/{fontstack}/{range}.pbf'

const TERRAIN_STYLE: StyleSpecification = {
  version: 8,
  glyphs: CARTO_GLYPHS,
  sources: {
    opentopomap: {
      type: 'raster',
      tiles: [
        'https://a.tile.opentopomap.org/{z}/{x}/{y}.png',
        'https://b.tile.opentopomap.org/{z}/{x}/{y}.png',
        'https://c.tile.opentopomap.org/{z}/{x}/{y}.png',
      ],
      tileSize: 256,
      attribution: '&copy; OpenStreetMap, &copy; OpenTopoMap',
    },
  },
  layers: [{ id: 'opentopomap', type: 'raster', source: 'opentopomap' }],
}

const ESRI_IMAGERY: SourceSpecification = {
  type: 'raster',
  tiles: [
    'https://services.arcgisonline.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
  ],
  tileSize: 256,
  attribution: '&copy; Esri, Maxar, Earthstar Geographics',
}

/** Without Positron (CARTO unreachable) the heatmap is drawn on Positron's paper. */
const BLANK_STYLE: StyleSpecification = {
  version: 8,
  glyphs: CARTO_GLYPHS,
  sources: {},
  layers: [{ id: 'background', type: 'background', paint: { 'background-color': '#fafaf8' } }],
}

type Paint = Record<string, unknown>

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

/** A basemap's style from Positron's (null: none): its labels last, recoloured for the basemap, so
 *  the heatmap goes beneath the first of them and over all the rest (`overlayAnchorId`). */
export function basemapStyle(id: BasemapId, positron: StyleSpecification | null): StyleSpecification {
  if (id === 'terrain') return structuredClone(TERRAIN_STYLE)
  const layers = positron?.layers ?? []
  const labels = layers
    .filter(layer => layer.type === 'symbol')
    .map(layer => ({ ...layer, paint: adaptPaint(layer.paint ?? {}, id) }) as LayerSpecification)
  if (id === 'satellite') {
    return structuredClone({
      version: 8,
      glyphs: positron?.glyphs ?? CARTO_GLYPHS,
      ...(positron?.sprite ? { sprite: positron.sprite } : {}),
      sources: { esri: ESRI_IMAGERY, ...positron?.sources },
      layers: [{ id: 'esri-imagery', type: 'raster', source: 'esri' }, ...labels],
    })
  }
  if (!positron) return structuredClone(BLANK_STYLE)
  return structuredClone({ ...positron, layers: [...layers.filter(layer => layer.type !== 'symbol'), ...labels] })
}

let positron: Promise<StyleSpecification | null> | undefined

/** Positron's style, fetched once a page; a failed fetch is tried again at the next basemap. */
function loadPositron(): Promise<StyleSpecification | null> {
  positron ??= fetch(POSITRON_STYLE_URL)
    .then(response => (response.ok ? response.json() as Promise<StyleSpecification> : null))
    .catch(() => null)
    .then(style => {
      if (!style) positron = undefined
      return style
    })
  return positron
}

export async function loadBasemapStyle(id: BasemapId): Promise<StyleSpecification> {
  return basemapStyle(id, id === 'terrain' ? null : await loadPositron())
}

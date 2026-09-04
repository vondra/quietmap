import type { StyleSpecification } from 'maplibre-gl'
import { CARTO_VECTOR_SOURCE, CARTO_GLYPHS, CARTO_SPRITE, getLabelLayers } from './label-layers'

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

const POSITRON_NOLABELS_STYLE_URL = 'https://basemaps.cartocdn.com/gl/positron-nolabels-gl-style/style.json'
const POSITRON_FALLBACK_STYLE_URL = 'https://basemaps.cartocdn.com/gl/positron-gl-style/style.json'

const TERRAIN_STYLE: StyleSpecification = {
  version: 8,
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

const SATELLITE_STYLE: StyleSpecification = {
  version: 8,
  glyphs: CARTO_GLYPHS,
  sprite: CARTO_SPRITE,
  sources: {
    esri: {
      type: 'raster',
      tiles: [
        'https://services.arcgisonline.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
      ],
      tileSize: 256,
      attribution: '&copy; Esri, Maxar, Earthstar Geographics, &copy; CARTO',
    },
    carto: CARTO_VECTOR_SOURCE,
  },
  layers: [{ id: 'esri-imagery', type: 'raster', source: 'esri' }],
}

const styleCache = new Map<BasemapId, Promise<StyleSpecification | string>>()

function cloneStyle<T>(style: T): T {
  return JSON.parse(JSON.stringify(style))
}

function withEmbeddedLabels(
  baseStyle: StyleSpecification,
  basemapId: BasemapId,
): StyleSpecification {
  if (basemapId === 'terrain') return cloneStyle(baseStyle)

  const style = cloneStyle(baseStyle)
  const labelLayers = getLabelLayers(basemapId)
  style.layers = [...(style.layers ?? []), ...labelLayers]
  return style
}

export async function loadBasemapStyle(id: BasemapId): Promise<StyleSpecification | string> {
  const cached = styleCache.get(id)
  if (cached) return await cached

  const promise = (async () => {
    if (id === 'terrain') return cloneStyle(TERRAIN_STYLE)
    if (id === 'satellite') return withEmbeddedLabels(SATELLITE_STYLE, 'satellite')

    try {
      const res = await fetch(POSITRON_NOLABELS_STYLE_URL)
      if (!res.ok) throw new Error(`style fetch failed: ${res.status}`)
      const style = await res.json() as StyleSpecification
      return withEmbeddedLabels(style, 'standard')
    } catch {
      return POSITRON_FALLBACK_STYLE_URL
    }
  })()

  styleCache.set(id, promise)
  return await promise
}

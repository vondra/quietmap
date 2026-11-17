// The three basemaps (standard, terrain, satellite) and their MapLibre styles.
import { LIGHT, layers, type Flavor } from '@protomaps/basemaps'
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

/** OpenStreetMap's credit: the standard map's tiles, the satellite map's labels and the noise model
 *  all owe it (MapCredits), and MapLibre leaves out a credit another contains, so it shows once on
 *  every base map. */
export const OSM_CREDIT = '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'

/** The font of the labels the app draws itself, one of the three the base map is served with. */
export const LABEL_FONT = 'Noto Sans Medium'

/** The standard map's ground as it has looked under the noise colours (sampled from the map of
 *  2026-11-17): paper, also what a server without a base map leaves under the heatmap. */
const PAPER = '#fafaf8'
const WATER = '#d4dadc'
const GREEN_FAR = '#f2f6f1'
const GREEN_NEAR = '#eef4ed'
const USED_GROUND = '#f5f5f4'

/** The names on the standard map: dark, bolder than the flavor's greys. */
const INK = '#1a1a1a'

/** Protomaps' light flavor, made quiet under the noise colours: its white roads and grey buildings
 *  on paper, water, greens and used ground pale, borders and railways faint, no land cover and no
 *  icons of shops and sights. */
const QUIET_LIGHT: Flavor = {
  ...LIGHT,
  background: PAPER, earth: PAPER, water: WATER,
  park_a: GREEN_FAR, wood_a: GREEN_FAR, scrub_a: GREEN_FAR,
  park_b: GREEN_NEAR, wood_b: GREEN_NEAR, scrub_b: GREEN_NEAR, zoo: GREEN_NEAR,
  hospital: USED_GROUND, school: USED_GROUND, industrial: USED_GROUND, pedestrian: USED_GROUND,
  aerodrome: USED_GROUND, military: USED_GROUND, sand: USED_GROUND, beach: USED_GROUND,
  boundaries: '#c4c4c4',
  railway: '#c9cdce',
  // Countries and regions stay pale: their names lie across whole lands of noise.
  city_label: INK, subplace_label: INK, roads_label_minor: INK, roads_label_major: INK,
  ocean_label: INK, address_label: INK,
  country_label: '#8c8c8c', state_label: '#9e9e9e',
  pois: undefined,
  landcover: undefined,
}

/** The standard map's tiles, fonts and sprites, which the map's own server keeps (`/api/basemap`):
 *  OpenStreetMap in Protomaps' schema. MapLibre takes absolute addresses only. */
function ownBasemap(origin: string): Pick<StyleSpecification, 'glyphs' | 'sprite' | 'sources'> {
  return {
    glyphs: `${origin}/api/basemap/fonts/{fontstack}/{range}.pbf`,
    sprite: `${origin}/api/basemap/sprites/light`,
    // The deepest zoom of a Protomaps planet; the map draws its tiles larger beyond it.
    sources: { protomaps: { type: 'vector', tiles: [`${origin}/api/basemap/{z}/{x}/{y}.mvt`], maxzoom: 15 } },
  }
}

const TERRAIN: SourceSpecification = {
  type: 'raster',
  tiles: [
    'https://a.tile.opentopomap.org/{z}/{x}/{y}.png',
    'https://b.tile.opentopomap.org/{z}/{x}/{y}.png',
    'https://c.tile.opentopomap.org/{z}/{x}/{y}.png',
  ],
  tileSize: 256,
  attribution: `${OSM_CREDIT}, SRTM | Map style: &copy; <a href="https://opentopomap.org/">OpenTopoMap</a> (<a href="https://creativecommons.org/licenses/by-sa/3.0/">CC-BY-SA</a>)`,
}

const ESRI_IMAGERY: SourceSpecification = {
  type: 'raster',
  tiles: [
    'https://services.arcgisonline.com/arcgis/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
  ],
  tileSize: 256,
  // The service's own credit (its `copyrightText`, October 2026), after the "Powered by Esri" Esri asks for.
  attribution: 'Powered by <a href="https://www.esri.com/">Esri</a> | Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community',
}

/** What the labels' own paint gives way to: a halo that holds over the noise colours, and white
 *  names over the dark imagery. */
const LABEL_PAINT = {
  standard: { 'text-halo-color': 'rgba(255,255,255,0.9)', 'text-halo-width': 1.5 },
  satellite: { 'text-color': '#ffffff', 'text-halo-color': 'rgba(0,0,0,0.75)', 'text-halo-width': 2, 'text-halo-blur': 0 },
}

/** Protomaps' labels (its symbol layers), painted for the basemap and thinned to what reads over
 *  the noise colours: no road numbers and one-way arrows, which are for drivers, and the names of
 *  main roads from zoom 14, not 11. */
function labelLayers(id: Exclude<BasemapId, 'terrain'>): LayerSpecification[] {
  return layers('protomaps', QUIET_LIGHT, { labelsOnly: true, lang: 'en' })
    .filter(layer => layer.id !== 'roads_shields' && layer.id !== 'roads_oneway')
    .map(layer => ({
      ...layer,
      ...(layer.id === 'roads_labels_major' && { minzoom: 14 }),
      paint: { ...layer.paint, ...LABEL_PAINT[id] },
    }) as LayerSpecification)
}

/** A basemap's style, its own tiles from `origin`: the labels come last, so the heatmap goes
 *  beneath the first of them and over all the rest (`overlayAnchorId`). The terrain map has its
 *  labels in its tiles; its glyphs are for the labels the app draws. */
export function basemapStyle(id: BasemapId, origin: string): StyleSpecification {
  const own = ownBasemap(origin)
  if (id === 'terrain') {
    return { version: 8, glyphs: own.glyphs, sources: { opentopomap: TERRAIN }, layers: [{ id: 'opentopomap', type: 'raster', source: 'opentopomap' }] }
  }
  if (id === 'satellite') {
    return {
      version: 8,
      ...own,
      sources: { esri: ESRI_IMAGERY, ...own.sources },
      layers: [{ id: 'esri-imagery', type: 'raster', source: 'esri' }, ...labelLayers(id)],
    }
  }
  return { version: 8, ...own, layers: [...layers('protomaps', QUIET_LIGHT), ...labelLayers(id)] }
}

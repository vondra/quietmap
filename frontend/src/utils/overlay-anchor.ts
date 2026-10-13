// Where the heatmap and the data layers go in the map's stack: beneath the labels and everything
// the app draws itself.
import type { StyleSpecification } from 'maplibre-gl'

/** The layer the heatmap and the data layers draw beneath: the lowest label (a symbol layer: the
 *  basemap's place names, the places to stay's prices) or layer the app draws from GeoJSON (the
 *  clicked point, the highlighted source, the opened row's segments). The terrain basemap has its
 *  labels in its raster tiles, so there only the app's layers anchor; with neither, the overlays go
 *  on top. */
export function overlayAnchorId(style: Pick<StyleSpecification, 'layers' | 'sources'> | undefined): string | undefined {
  return style?.layers.find(layer =>
    layer.type === 'symbol'
    || ('source' in layer && typeof layer.source === 'string' && style.sources[layer.source]?.type === 'geojson'))?.id
}

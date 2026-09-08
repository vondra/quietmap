import { useEffect, useState } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { GeoJsonLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'
import { FAN_SLICE_RGBA, type FanSliceKind } from './noise/fanGeometry'

interface HighlightLayerProps {
  geometry?: GeoJSON.Geometry | GeoJSON.FeatureCollection | null
}

/**
 * Contributor highlight: the clicked noise-source geometry drawn as a
 * black-cased + white-core outline pair (readable on any basemap) on its
 * own deck.gl canvas so it always draws above the basemap.
 */
export default function HighlightLayer({ geometry }: HighlightLayerProps): null {
  const { current: mapRef } = useMap()
  const [overlay, setOverlay] = useState<MapboxOverlay | null>(null)

  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const next = new MapboxOverlay({ interleaved: false, layers: [] })
    map.addControl(next)
    setOverlay(next)
    return () => {
      map.removeControl(next)
      setOverlay(null)
    }
  }, [mapRef])

  useEffect(() => {
    if (!overlay) return
    overlay.setProps({ layers: makeHighlightLayers(geometry) })
  }, [overlay, geometry])

  return null
}

/**
 * Contributor-highlight casing + core pair: a wider black stroke under a thinner
 * white one (readable on any basemap). No `beforeId` → drawn last, above labels.
 *
 * A road/railway segment with an engine screening fan arrives as a
 * FeatureCollection: one triangle per fan slice (properties.fanKind colors the
 * slice by its cause — open / building / terrain / both) plus the
 * characteristic ray and segment LineStrings. Anything else is a single
 * geometry as before.
 */
function makeHighlightLayers(geometry: GeoJSON.Geometry | GeoJSON.FeatureCollection | null | undefined) {
  if (!geometry) return []
  const features: GeoJSON.Feature[] =
    geometry.type === 'FeatureCollection'
      ? geometry.features
      : [{ type: 'Feature', geometry: geometry as GeoJSON.Geometry, properties: {} }]
  // Fan slices only: plain polygons without fanKind keep the
  // old white highlight instead of a cause color.
  const slices = features.filter(f => f.geometry?.type === 'Polygon' && f.properties?.fanKind != null)
  const rest = features.filter(f => !(f.geometry?.type === 'Polygon' && f.properties?.fanKind != null))
  const restData: GeoJSON.FeatureCollection = { type: 'FeatureCollection', features: rest }
  const sliceFill = (f: { properties?: Record<string, unknown> }): [number, number, number, number] => {
    const c = FAN_SLICE_RGBA[(f.properties?.fanKind ?? 'clear') as FanSliceKind]
    return c ?? FAN_SLICE_RGBA.clear
  }
  return [
    ...(slices.length > 0
      ? [
          new GeoJsonLayer({
            id: 'segment-fan-slices',
            data: slices,
            stroked: true, filled: true,
            getLineColor: (f: { properties?: Record<string, unknown> }): [number, number, number, number] => {
              const c = sliceFill(f)
              return [c[0], c[1], c[2], 220]
            },
            getLineWidth: 1, lineWidthMinPixels: 1, lineWidthUnits: 'pixels',
            getFillColor: sliceFill,
            pickable: false,
          }),
        ]
      : []),
    ...(rest.length > 0
      ? [
          new GeoJsonLayer({
            id: 'contributor-highlight-casing',
            data: restData,
            stroked: true, filled: true,
            getLineColor: [0, 0, 0, 255],
            getLineWidth: 5, lineWidthMinPixels: 5, lineWidthUnits: 'pixels',
            getFillColor: [255, 255, 255, 76],
            getPointRadius: 5, pointRadiusUnits: 'pixels',
            pickable: false,
          }),
          new GeoJsonLayer({
            id: 'contributor-highlight-core',
            data: restData,
            stroked: true, filled: false,
            getLineColor: [255, 255, 255, 255],
            getLineWidth: 2, lineWidthMinPixels: 2, lineWidthUnits: 'pixels',
            getPointRadius: 3, pointRadiusUnits: 'pixels',
            pickable: false,
          }),
        ]
      : []),
  ]
}

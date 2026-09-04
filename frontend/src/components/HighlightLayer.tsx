import { useEffect, useState } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { GeoJsonLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'

interface HighlightLayerProps {
  geometry?: GeoJSON.Geometry | null
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
 */
function makeHighlightLayers(geometry: GeoJSON.Geometry | null | undefined) {
  if (!geometry) return []
  const data: GeoJSON.Feature = { type: 'Feature', geometry, properties: {} }
  return [
    new GeoJsonLayer({
      id: 'contributor-highlight-casing',
      data,
      stroked: true, filled: true,
      getLineColor: [0, 0, 0, 255],
      getLineWidth: 5, lineWidthMinPixels: 5, lineWidthUnits: 'pixels',
      getFillColor: [255, 255, 255, 76],
      getPointRadius: 5, pointRadiusUnits: 'pixels',
      pickable: false,
    }),
    new GeoJsonLayer({
      id: 'contributor-highlight-core',
      data,
      stroked: true, filled: false,
      getLineColor: [255, 255, 255, 255],
      getLineWidth: 2, lineWidthMinPixels: 2, lineWidthUnits: 'pixels',
      getPointRadius: 3, pointRadiusUnits: 'pixels',
      pickable: false,
    }),
  ]
}

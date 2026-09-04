import { useEffect, useRef } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import type { LngLatBoundsLike } from 'maplibre-gl'

interface IsochronLayerProps {
  geojson: GeoJSON.Feature | null
}

function getBounds(feature: GeoJSON.Feature): LngLatBoundsLike | null {
  let minLng = Infinity, minLat = Infinity, maxLng = -Infinity, maxLat = -Infinity
  const walk = (coords: unknown) => {
    if (!Array.isArray(coords)) return
    if (typeof coords[0] === 'number' && typeof coords[1] === 'number') {
      const [lng, lat] = coords as [number, number]
      if (lng < minLng) minLng = lng
      if (lng > maxLng) maxLng = lng
      if (lat < minLat) minLat = lat
      if (lat > maxLat) maxLat = lat
    } else {
      for (const c of coords) walk(c)
    }
  }
  if (feature.geometry && 'coordinates' in feature.geometry) walk(feature.geometry.coordinates)
  if (minLng === Infinity) return null
  return [[minLng, minLat], [maxLng, maxLat]]
}

export default function IsochronLayer({ geojson }: IsochronLayerProps) {
  const { current: map } = useMap()
  const prevRef = useRef<GeoJSON.Feature | null>(null)

  useEffect(() => {
    if (!map || !geojson || geojson === prevRef.current) return
    prevRef.current = geojson
    const bounds = getBounds(geojson)
    if (bounds) map.fitBounds(bounds, { padding: 60, maxZoom: 14 })
  }, [map, geojson])

  if (!geojson) return null

  return (
    <Source id="isochron" type="geojson" data={{ type: 'FeatureCollection', features: [geojson] }}>
      <Layer id="isochron-fill" type="fill" paint={{ 'fill-color': '#2563eb', 'fill-opacity': 0.15 }} />
      <Layer id="isochron-line" type="line" paint={{ 'line-color': '#2563eb', 'line-width': 2.5, 'line-opacity': 0.9 }} />
    </Source>
  )
}

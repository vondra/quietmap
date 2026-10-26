// The reachable area on the map: each mode's polygon, the car's lighter under the walk's, and the map
// fitted to them once drawn, in the part the controls leave in sight.
import { useEffect } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import { framePadding } from './map-frame'

/** The box of every coordinate of `area`. */
function bounds(area: GeoJSON.FeatureCollection): [[number, number], [number, number]] | null {
  let [west, south, east, north] = [Infinity, Infinity, -Infinity, -Infinity]
  const visit = (coordinates: unknown): void => {
    if (!Array.isArray(coordinates)) return
    if (typeof coordinates[0] === 'number') {
      const [lng, lat] = coordinates as [number, number]
      ;[west, south, east, north] = [Math.min(west, lng), Math.min(south, lat), Math.max(east, lng), Math.max(north, lat)]
    } else coordinates.forEach(visit)
  }
  for (const feature of area.features) if (feature.geometry && 'coordinates' in feature.geometry) visit(feature.geometry.coordinates)
  return west === Infinity ? null : [[west, south], [east, north]]
}

export default function IsochronLayer({ area }: { area: GeoJSON.FeatureCollection | null }) {
  const { current: map } = useMap()
  useEffect(() => {
    const box = area && bounds(area)
    if (map && box) map.fitBounds(box, { padding: framePadding(map.getContainer()), maxZoom: 15 })
  }, [map, area])
  if (!area) return null
  return (
    <Source id="isochron" type="geojson" data={area}>
      <Layer id="isochron-fill" type="fill" paint={{ 'fill-color': '#2563eb', 'fill-opacity': ['match', ['get', 'mode'], 'walk', 0.18, 0.08] }} />
      <Layer id="isochron-line" type="line" paint={{ 'line-color': '#2563eb', 'line-width': ['match', ['get', 'mode'], 'walk', 2.5, 1.5], 'line-opacity': 0.9 }} />
    </Source>
  )
}

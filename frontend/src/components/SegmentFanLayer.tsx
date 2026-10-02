// The segments view's rays on the map: a thin line from each computed piece to the point the level
// is computed at, darker and redder the louder it arrives, the opened piece's ray white on a black
// casing, and that point a dot.
import type { FilterSpecification } from 'maplibre-gl'
import { Layer, Source } from 'react-map-gl/maplibre'
import type { SegmentFan } from '../types/noise'

const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const
const RAYS: FilterSpecification = ['==', ['get', 'kind'], 'ray']
const SELECTED: FilterSpecification = ['==', ['get', 'kind'], 'selected']
const RECEIVER: FilterSpecification = ['==', ['get', 'kind'], 'receiver']

export function fanGeoJson(fan: SegmentFan): GeoJSON.FeatureCollection {
  const line = (ray: [[number, number], [number, number]]): GeoJSON.LineString => ({
    type: 'LineString',
    coordinates: ray.map(([lat, lon]) => [lon, lat]),
  })
  return {
    type: 'FeatureCollection',
    features: [
      ...fan.rays.map(({ ray, lden, selected }) => ({
        type: 'Feature' as const,
        properties: { kind: selected ? 'selected' : 'ray', lden },
        geometry: line(ray),
      })),
      {
        type: 'Feature' as const,
        properties: { kind: 'receiver' },
        geometry: { type: 'Point', coordinates: [fan.receiver[1], fan.receiver[0]] },
      },
    ],
  }
}

export default function SegmentFanLayer({ fan }: { fan: SegmentFan | null }) {
  if (!fan) return null
  return (
    <Source id="segment-fan" type="geojson" data={fanGeoJson(fan)}>
      <Layer
        id="segment-fan-ray"
        type="line"
        filter={RAYS}
        layout={LINE_LAYOUT}
        paint={{
          'line-color': ['interpolate', ['linear'], ['get', 'lden'], 0, '#94a3b8', 30, '#2563eb', 45, '#7c3aed', 60, '#dc2626'],
          'line-width': 1.5,
          'line-opacity': 0.85,
        }}
      />
      <Layer id="segment-fan-selected-casing" type="line" filter={SELECTED} layout={LINE_LAYOUT} paint={{ 'line-color': '#000000', 'line-width': 5 }} />
      <Layer id="segment-fan-selected" type="line" filter={SELECTED} layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 2 }} />
      <Layer
        id="segment-fan-receiver"
        type="circle"
        filter={RECEIVER}
        paint={{ 'circle-color': '#ffffff', 'circle-radius': 5, 'circle-stroke-color': '#000000', 'circle-stroke-width': 2 }}
      />
    </Source>
  )
}

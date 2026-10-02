// The segments view's pieces on the map: each listed piece drawn thick and a thin ray from its
// nearest point to the point the level is computed at, both coloured by the Lden the piece
// delivers (the colour of its dot in the list), the selected piece white on a black casing, and
// that point a dot.
import type { FilterSpecification } from 'maplibre-gl'
import { Layer, Source } from 'react-map-gl/maplibre'
import type { SegmentFan } from '../types/noise'

const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const
const kind = (name: string): FilterSpecification => ['==', ['get', 'kind'], name]

/** The colour of a piece by the Lden it delivers: grey, blue, violet, red from 0 to 60 dB. */
const STOPS: [number, [number, number, number]][] = [
  [0, [0x94, 0xa3, 0xb8]],
  [30, [0x25, 0x63, 0xeb]],
  [45, [0x7c, 0x3a, 0xed]],
  [60, [0xdc, 0x26, 0x26]],
]

export function pieceColor(lden: number): string {
  const k = STOPS.findIndex(([at]) => lden < at)
  const [from, to] = k <= 0 ? (k === 0 ? [STOPS[0], STOPS[0]] : [STOPS.at(-1)!, STOPS.at(-1)!]) : [STOPS[k - 1], STOPS[k]]
  const t = to[0] > from[0] ? (lden - from[0]) / (to[0] - from[0]) : 0
  const channel = (c: number) => Math.round(from[1][c] + t * (to[1][c] - from[1][c])).toString(16).padStart(2, '0')
  return `#${channel(0)}${channel(1)}${channel(2)}`
}

export function fanGeoJson(fan: SegmentFan): GeoJSON.FeatureCollection {
  const lonLat = ([lat, lon]: [number, number]) => [lon, lat]
  return {
    type: 'FeatureCollection',
    features: [
      ...fan.pieces.flatMap(({ ray, ends, lden, selected }) => {
        const properties = (name: string) => ({ kind: selected ? `${name}-selected` : name, color: pieceColor(lden) })
        const point = ends.length < 2 || (ends[0][0] === ends[1][0] && ends[0][1] === ends[1][1])
        return [
          {
            type: 'Feature' as const,
            properties: properties('ray'),
            geometry: { type: 'LineString' as const, coordinates: ray.map(lonLat) },
          },
          {
            type: 'Feature' as const,
            properties: properties(point ? 'point' : 'piece'),
            geometry: point
              ? { type: 'Point' as const, coordinates: lonLat(ends[0]) }
              : { type: 'LineString' as const, coordinates: ends.map(lonLat) },
          },
        ]
      }),
      {
        type: 'Feature' as const,
        properties: { kind: 'receiver' },
        geometry: { type: 'Point', coordinates: lonLat(fan.receiver) },
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
        filter={kind('ray')}
        layout={LINE_LAYOUT}
        paint={{ 'line-color': ['get', 'color'], 'line-width': 1.25, 'line-opacity': 0.8 }}
      />
      <Layer
        id="segment-fan-piece"
        type="line"
        filter={kind('piece')}
        layout={LINE_LAYOUT}
        paint={{ 'line-color': ['get', 'color'], 'line-width': 5, 'line-opacity': 0.9 }}
      />
      <Layer
        id="segment-fan-point"
        type="circle"
        filter={kind('point')}
        paint={{ 'circle-color': ['get', 'color'], 'circle-radius': 4.5, 'circle-opacity': 0.9 }}
      />
      <Layer id="segment-fan-ray-selected-casing" type="line" filter={kind('ray-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#000000', 'line-width': 4.5 }} />
      <Layer id="segment-fan-ray-selected" type="line" filter={kind('ray-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 2 }} />
      <Layer id="segment-fan-piece-selected-casing" type="line" filter={kind('piece-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#000000', 'line-width': 9 }} />
      <Layer id="segment-fan-piece-selected" type="line" filter={kind('piece-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 5 }} />
      <Layer
        id="segment-fan-point-selected"
        type="circle"
        filter={kind('point-selected')}
        paint={{ 'circle-color': '#ffffff', 'circle-radius': 5.5, 'circle-stroke-color': '#000000', 'circle-stroke-width': 2.5 }}
      />
      <Layer
        id="segment-fan-receiver"
        type="circle"
        filter={kind('receiver')}
        paint={{ 'circle-color': '#ffffff', 'circle-radius': 5, 'circle-stroke-color': '#000000', 'circle-stroke-width': 2 }}
      />
    </Source>
  )
}

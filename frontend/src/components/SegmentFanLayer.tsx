// The segments view's pieces on the map: each listed piece drawn thick and a thin ray from its
// nearest point to the point the level is computed at, both in the colour of the piece's row, the
// selected piece white on a black casing, and that point a dot. When the pieces of a click first
// appear, the map moves out just enough to show them all beside the popup.
import type { FilterSpecification } from 'maplibre-gl'
import { useEffect, useRef } from 'react'
import { Layer, Source, useMap } from 'react-map-gl/maplibre'
import type { SegmentFan } from '../types/noise'

const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const
const kind = (name: string): FilterSpecification => ['==', ['get', 'kind'], name]
/** Below this width the popup is the bottom sheet over the lower half of the map. */
const PHONE_WIDTH_PX = 768
/** The desktop popup's column on the right: its width and two gutters. */
const CARD_COLUMN_PX = 320 + 2 * 12

export function fanGeoJson(fan: SegmentFan): GeoJSON.FeatureCollection {
  const lonLat = ([lat, lon]: [number, number]) => [lon, lat]
  return {
    type: 'FeatureCollection',
    features: [
      ...fan.pieces.flatMap(({ ray, ends, color, selected }) => {
        const properties = (name: string) => ({ kind: selected ? `${name}-selected` : name, color })
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

/** Moves the map out to show every piece of a click's fan beside the popup, once per click and
 *  only when some piece lies outside; never zooms in. */
function useFitFan(fan: SegmentFan | null) {
  const { current: map } = useMap()
  const fitted = useRef<string | null>(null)
  useEffect(() => {
    if (!fan || !map) {
      if (!fan) fitted.current = null
      return
    }
    const click = fan.receiver.join(',')
    if (fitted.current === click) return
    fitted.current = click
    const points = [fan.receiver, ...fan.pieces.flatMap(({ ends, ray }) => [...ends, ray[0]])]
    const phone = window.innerWidth < PHONE_WIDTH_PX
    const padding = phone
      ? { top: 72, bottom: Math.round(window.innerHeight / 2) + 24, left: 24, right: 24 }
      : { top: 72, bottom: 48, left: 48, right: CARD_COLUMN_PX + 24 }
    const canvas = map.getCanvas()
    const [width, height] = [canvas.clientWidth, canvas.clientHeight]
    const shown = points.every(([lat, lon]) => {
      const { x, y } = map.project([lon, lat])
      return x >= padding.left && x <= width - padding.right && y >= padding.top && y <= height - padding.bottom
    })
    if (shown) return
    const lats = points.map(([lat]) => lat)
    const lons = points.map(([, lon]) => lon)
    map.fitBounds(
      [[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]],
      { padding, maxZoom: map.getZoom(), duration: 600 },
    )
  }, [fan, map])
}

export default function SegmentFanLayer({ fan }: { fan: SegmentFan | null }) {
  useFitFan(fan)
  if (!fan) return null
  return (
    <Source id="segment-fan" type="geojson" data={fanGeoJson(fan)}>
      <Layer
        id="segment-fan-ray"
        type="line"
        filter={kind('ray')}
        layout={LINE_LAYOUT}
        paint={{ 'line-color': ['get', 'color'], 'line-width': 1.25, 'line-opacity': 0.85 }}
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

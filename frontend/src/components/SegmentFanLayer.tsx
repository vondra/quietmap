// An opened row's segments on the map: each listed segment drawn thick in the colour of its line in
// the list, the selected one (hovered or opened) white on a black casing with every ray it was
// summed over, each in the colour of what it delivers, the selected ray thicker on a casing; and the
// point the level is computed at, a dot. The map frames the segments that make the level when a
// row's first appear, and a segment when it is opened.
import type { FilterSpecification } from 'maplibre-gl'
import { useEffect, useRef } from 'react'
import { Layer, Source, useMap } from 'react-map-gl/maplibre'
import type { SegmentFan } from '../types/noise'

const LINE_LAYOUT = { 'line-cap': 'round', 'line-join': 'round' } as const
const kind = (name: string): FilterSpecification => ['==', ['get', 'kind'], name]
/** Below this width the popup is the bottom sheet over the lower half of the map. */
const PHONE_WIDTH_PX = 768
/** The desktop popup's column on the right: its width and two gutters. */
const CARD_COLUMN_PX = 360 + 2 * 12
/** The closest the map comes to frame an opened piece: a street's width stays readable. */
const OPENED_PIECE_MAX_ZOOM = 18

export function fanGeoJson(fan: SegmentFan): GeoJSON.FeatureCollection {
  const lonLat = ([lat, lon]: [number, number]) => [lon, lat]
  return {
    type: 'FeatureCollection',
    features: [
      ...fan.pieces.map(({ ends, color, selected }) => {
        const point = ends.length < 2 || (ends[0][0] === ends[1][0] && ends[0][1] === ends[1][1])
        const shape = point ? 'point' : 'piece'
        return {
          type: 'Feature' as const,
          properties: { kind: selected ? `${shape}-selected` : shape, color },
          geometry: point
            ? { type: 'Point' as const, coordinates: lonLat(ends[0]) }
            : { type: 'LineString' as const, coordinates: ends.map(lonLat) },
        }
      }),
      ...fan.rays.map(({ from, color, selected }) => ({
        type: 'Feature' as const,
        properties: { kind: selected ? 'ray-selected' : 'ray', color },
        geometry: { type: 'LineString' as const, coordinates: [lonLat(from), lonLat(fan.receiver)] },
      })),
      {
        type: 'Feature' as const,
        properties: { kind: 'receiver' },
        geometry: { type: 'Point', coordinates: lonLat(fan.receiver) },
      },
    ],
  }
}

/** The map's padding around what it frames: clear of the search bar, and of the popup's card
 *  column on a desktop or of the bottom sheet on a phone. */
function framePadding() {
  return window.innerWidth < PHONE_WIDTH_PX
    ? { top: 72, bottom: Math.round(window.innerHeight / 2) + 24, left: 24, right: 24 }
    : { top: 72, bottom: 48, left: 48, right: CARD_COLUMN_PX + 24 }
}

/** Frames the fan: when a row's segments first appear, the map moves out (never in) only as far as
 *  needed to show the segments that make the level with the receiver; when a segment is opened,
 *  the map frames that segment, its rays and the receiver. */
function useFrameFan(fan: SegmentFan | null) {
  const { current: map } = useMap()
  const framed = useRef<{ row: string, opened: number | null } | null>(null)
  useEffect(() => {
    if (!fan || !map) {
      if (!fan) framed.current = null
      return
    }
    const row = `${fan.source}@${fan.receiver.join(',')}`
    const opened = fan.opened?.index ?? null
    const before = framed.current?.row === row ? framed.current : null
    framed.current = { row, opened }
    const padding = framePadding()
    const frame = (points: [number, number][], maxZoom: number) => {
      const lats = points.map(([lat]) => lat)
      const lons = points.map(([, lon]) => lon)
      map.fitBounds(
        [[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]],
        { padding, maxZoom, duration: 600 },
      )
    }
    if (!before) {
      const points = [fan.receiver, ...fan.overview]
      const canvas = map.getCanvas()
      const [width, height] = [canvas.clientWidth, canvas.clientHeight]
      const shown = points.every(([lat, lon]) => {
        const { x, y } = map.project([lon, lat])
        return x >= padding.left && x <= width - padding.right && y >= padding.top && y <= height - padding.bottom
      })
      if (!shown) frame(points, map.getZoom())
    } else if (fan.opened && opened !== before.opened) {
      frame([fan.receiver, ...fan.opened.points], OPENED_PIECE_MAX_ZOOM)
    }
  }, [fan, map])
}

export default function SegmentFanLayer({ fan }: { fan: SegmentFan | null }) {
  useFrameFan(fan)
  if (!fan) return null
  return (
    <Source id="segment-fan" type="geojson" data={fanGeoJson(fan)}>
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
      <Layer id="segment-fan-piece-selected-casing" type="line" filter={kind('piece-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#000000', 'line-width': 9 }} />
      <Layer id="segment-fan-piece-selected" type="line" filter={kind('piece-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#ffffff', 'line-width': 5 }} />
      <Layer
        id="segment-fan-ray"
        type="line"
        filter={kind('ray')}
        layout={LINE_LAYOUT}
        paint={{ 'line-color': ['get', 'color'], 'line-width': 2, 'line-opacity': 0.95 }}
      />
      <Layer id="segment-fan-ray-selected-casing" type="line" filter={kind('ray-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': '#000000', 'line-width': 6 }} />
      <Layer id="segment-fan-ray-selected" type="line" filter={kind('ray-selected')} layout={LINE_LAYOUT} paint={{ 'line-color': ['get', 'color'], 'line-width': 3 }} />
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

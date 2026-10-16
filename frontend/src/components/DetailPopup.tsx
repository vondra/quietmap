// The map click that opens the popup (a click on a place to stay opens the place instead, at its
// point), the clicked-point marker, and the streamed request for it.
import { useEffect } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import type { Map as MaplibreMap, MapMouseEvent } from 'maplibre-gl'
import type { PopupUpdate } from '../types/noise'
import { streamPopup } from '../lib/popup-stream'
import { DESKTOP_QUERY } from '../hooks/useIsDesktop'
import { sheetTop } from './MobileDetailSheet'
import { stayAt } from './StayLayer'
import type { Stay } from '../lib/stays'

/** On a phone the popup's sheet rises over the lower half of the map: a point it would cover (at `y`
 *  px from the top) moves up to the middle of the map left between the search bar and the sheet,
 *  where the opened rows draw their roads, segments and rays. */
function keepAboveSheet(map: MaplibreMap, y: number) {
  if (window.matchMedia(DESKTOP_QUERY).matches) return
  const top = document.querySelector('.map-top-bar')?.getBoundingClientRect().bottom ?? 0
  const bottom = sheetTop(map.getContainer().clientHeight)
  if (y < bottom - 24) return
  map.panBy([0, y - (top + bottom) / 2], { duration: 400 })
}

export interface DetailPopupProps {
  isCurrentDetailPosition: (position: { lat: number; lng: number }) => boolean
  detailPosition: { lat: number; lng: number } | null
  triggerPosition: { lat: number; lng: number } | null
  onDetailData?: (data: PopupUpdate) => void
  onDetailPositionChange?: (pos: { lat: number; lng: number } | null) => void
  onDetailError?: (message: string) => void
  /** A click on a place to stay opens the place (at its point) instead. */
  onStaySelect?: (stay: Stay) => void
}

export default function DetailPopup({ isCurrentDetailPosition, detailPosition, triggerPosition, onDetailData, onDetailPositionChange, onDetailError, onStaySelect }: DetailPopupProps) {
  const { current: map } = useMap()

  useEffect(() => {
    if (!map) return
    let dragStart: { x: number; y: number } | null = null

    const onMouseDown = (e: MapMouseEvent) => {
      dragStart = { x: e.originalEvent.clientX, y: e.originalEvent.clientY }
    }

    const onClick = (e: MapMouseEvent) => {
      if (dragStart) {
        const dx = e.originalEvent.clientX - dragStart.x
        const dy = e.originalEvent.clientY - dragStart.y
        if (Math.sqrt(dx * dx + dy * dy) > 5) return
      }
      const stay = stayAt(map.getMap(), e.point)
      if (stay) {
        onStaySelect?.(stay)
        return
      }
      const { lat, lng } = e.lngLat
      onDetailPositionChange?.({ lat, lng })
    }

    map.on('mousedown', onMouseDown)
    map.on('click', onClick)
    return () => {
      map.off('mousedown', onMouseDown)
      map.off('click', onClick)
    }
  }, [map, onDetailPositionChange, onStaySelect])

  useEffect(() => {
    if (triggerPosition) onDetailPositionChange?.(triggerPosition)
  }, [triggerPosition, onDetailPositionChange])

  // Whatever opened the popup (a tap, a link's `d=`, a recent place), its point stays in sight.
  useEffect(() => {
    if (!detailPosition || !map) return
    keepAboveSheet(map.getMap(), map.project([detailPosition.lng, detailPosition.lat]).y)
  }, [detailPosition, map])

  // One streamed request per clicked point: every line redraws the popup. A new click (or closing
  // the popup) aborts it, which also stops the computation on the server.
  useEffect(() => {
    if (!detailPosition || !map) return
    const controller = new AbortController()
    const isCurrent = () => isCurrentDetailPosition(detailPosition)
    void streamPopup(detailPosition, controller.signal, {
      onUpdate: update => { if (isCurrent()) onDetailData?.(update) },
      onError: message => { if (isCurrent()) onDetailError?.(message) },
    })
    return () => controller.abort()
  }, [detailPosition, map, onDetailData, onDetailError, isCurrentDetailPosition])

  return (
    <>
      {detailPosition && (
        <Source id="clicked-point" type="geojson" data={{
          type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: [detailPosition.lng, detailPosition.lat] }
        }}>
          <Layer id="clicked-point-marker" type="circle" paint={{
            'circle-radius': 6, 'circle-color': '#3b82f6', 'circle-stroke-color': '#ffffff', 'circle-stroke-width': 2,
          }} />
        </Source>
      )}
    </>
  )
}

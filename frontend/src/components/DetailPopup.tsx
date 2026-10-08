// The map click that opens the popup (a click on a place to stay opens the place instead, at its
// point), the clicked-point marker, and the streamed request for it.
import { useEffect } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import type { MapMouseEvent } from 'maplibre-gl'
import type { PopupUpdate } from '../types/noise'
import { streamPopup } from '../lib/popup-stream'
import { stayAt } from './StayLayer'
import type { Stay } from '../lib/stays'

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

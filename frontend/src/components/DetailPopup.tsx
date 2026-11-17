// The map click that opens the popup (a click on a place to stay opens the place instead, at its
// point), the clicked-point marker, and the streamed request for it.
import { useEffect, useRef, useState } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import type { Map as MaplibreMap, MapMouseEvent } from 'maplibre-gl'
import type { PopupUpdate } from '../types/noise'
import { streamPopup } from '../lib/popup-stream'
import { DESKTOP_QUERY } from '../hooks/useIsDesktop'
import { sameDetailPosition } from '../lib/url-state'
import { sheetTop } from './MobileDetailSheet'
import { stayAt } from './StayLayer'
import type { Stay } from '../lib/stays'

/** Keep selected points clear of the search and result panels. Only a local choice may bring an
 *  offscreen point into view: a clipped stay pin can be tapped while its centre lies outside. */
function keepInSight(map: MaplibreMap, at: { lat: number; lng: number }, revealOffscreen: boolean) {
  const phone = !window.matchMedia(DESKTOP_QUERY).matches
  // A search crossing the antimeridian can leave the camera in another copy of the world.
  const longitude = at.lng + 360 * Math.round((map.getCenter().lng - at.lng) / 360)
  const { x, y } = map.project([longitude, at.lat])
  const { clientWidth: width, clientHeight: height } = map.getContainer()
  const outside = x < 0 || x > width || y < 0 || y > height
  if (outside && !revealOffscreen) return
  const search = document.querySelector('.map-top-bar')?.getBoundingClientRect()
  // Reserve the cards' column: the skeleton grows as the answer streams in.
  const column = document.querySelector('.map-card-column')?.getBoundingClientRect()
  const right = phone ? width : column?.left ?? width
  const coveredByCard = !phone && column && x >= right - 8 && y >= column.top - 8 && y <= column.bottom + 8
  const top = search?.bottom ?? 0
  const bottom = phone ? sheetTop(height) : height
  const coveredBySearch = search && (phone || (x >= search.left - 8 && x <= search.right + 8)) && y <= top + 8
  if (outside || coveredByCard || coveredBySearch || (phone && y >= bottom - 24)) {
    const targetX = outside || x >= right - 8 ? right / 2 : x
    const targetY = outside || y < top + 24 || y >= bottom - 24 ? (top + bottom) / 2 : y
    const center = map.project(map.getCenter())
    // Position the geographic point at the target pixel; panBy is inexact away from a tilted map's centre.
    map.easeTo({ center: [longitude, at.lat], offset: [targetX - center.x, targetY - center.y], duration: 400 })
  }
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
  const [visibilityCheck, requestVisibilityCheck] = useState(0)
  const localSelection = useRef<{ lat: number; lng: number } | null>(null)

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
        localSelection.current = { lat: stay.lat, lng: stay.lng }
        onStaySelect?.(stay)
        return
      }
      const { lat, lng } = e.lngLat.wrap()
      localSelection.current = { lat, lng }
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
    if (triggerPosition) {
      localSelection.current = triggerPosition
      onDetailPositionChange?.(triggerPosition)
    }
  }, [triggerPosition, onDetailPositionChange])

  // Commit selection/resize with MapStateSync before revealing, or its padding update can undo
  // the move. Ordinary panning requests no check, so the visitor remains free to leave the point.
  useEffect(() => {
    if (!detailPosition) { localSelection.current = null; return }
    if (!map) return
    const check = () => requestVisibilityCheck(count => count + 1)
    map.on('resize', check)
    const frame = requestAnimationFrame(() => {
      if (!isCurrentDetailPosition(detailPosition)) return
      // A resize must not cut short a search's arrival or a reveal already in progress.
      if (map.isMoving()) map.once('moveend', check)
      else {
        const revealOffscreen = sameDetailPosition(localSelection.current, detailPosition)
        localSelection.current = null
        keepInSight(map.getMap(), detailPosition, revealOffscreen)
      }
    })
    return () => {
      cancelAnimationFrame(frame)
      map.off('resize', check)
      map.off('moveend', check)
    }
  }, [detailPosition, map, visibilityCheck, isCurrentDetailPosition])

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

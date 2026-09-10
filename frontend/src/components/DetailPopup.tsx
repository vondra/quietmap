import { useEffect } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import { propertyJustClicked } from '../lib/property-click-guard'
import type { NoiseComputeData } from '../types/noise'
import { fetchNoiseDetail, type SurfacePreview } from '../lib/fetch-noise-detail'

export interface DetailPopupProps {
  isCurrentDetailPosition: (position: { lat: number; lng: number }) => boolean
  detailPosition: { lat: number; lng: number } | null
  triggerPosition: { lat: number; lng: number } | null
  onDetailData?: (data: NoiseComputeData | null) => void
  onDetailPreview?: (preview: SurfacePreview | null) => void
  onDetailPositionChange?: (pos: { lat: number; lng: number } | null) => void
  onDetailError?: (message: string | null) => void
}

export default function DetailPopup({ isCurrentDetailPosition, detailPosition, triggerPosition, onDetailData, onDetailPreview, onDetailPositionChange, onDetailError }: DetailPopupProps) {
  const { current: map } = useMap()

  useEffect(() => {
    if (!map) return
    let dragStart: { x: number; y: number } | null = null

    const onMouseDown = (e: any) => {
      dragStart = { x: e.originalEvent.clientX, y: e.originalEvent.clientY }
    }

    const onClick = (e: any) => {
      if (dragStart) {
        const dx = e.originalEvent.clientX - dragStart.x
        const dy = e.originalEvent.clientY - dragStart.y
        if (Math.sqrt(dx * dx + dy * dy) > 5) return
      }
      if ((e.originalEvent.target as HTMLElement).closest('.maplibregl-popup')) return

      const { lat, lng } = e.lngLat
      // Pin layers stamp the guard at pointerup (attachPinTapGuard), which
      // always precedes this click handler — the deferred tick is belt and
      // braces for any handler-ordering edge, not the primary mechanism.
      setTimeout(() => {
        if (!propertyJustClicked()) onDetailPositionChange?.({ lat, lng })
      }, 0)
    }

    map.on('mousedown', onMouseDown)
    map.on('click', onClick)
    return () => {
      map.off('mousedown', onMouseDown)
      map.off('click', onClick)
    }
  }, [map, onDetailPositionChange])

  useEffect(() => {
    if (triggerPosition) onDetailPositionChange?.(triggerPosition)
  }, [triggerPosition, onDetailPositionChange])

  useEffect(() => {
    if (!detailPosition || !map) return

    const controller = new AbortController()
    void fetchNoiseDetail(detailPosition, controller.signal, {
      isCurrent: () => isCurrentDetailPosition(detailPosition),
      onData: data => onDetailData?.(data),
      onPreview: preview => onDetailPreview?.(preview),
      onError: message => onDetailError?.(message),
    })

    return () => controller.abort()
  }, [detailPosition, map, onDetailData, onDetailPreview, onDetailError, isCurrentDetailPosition])

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

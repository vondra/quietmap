import { useEffect } from 'react'
import { useMap, Source, Layer } from 'react-map-gl/maplibre'
import { propertyJustClicked } from '../lib/property-click-guard'
import type { NoiseComputeData } from '../types/noise'

// The rich popup BODY lives in ./NoiseDetailContent (a lazy chunk). This file is
// just the lightweight controller MapView mounts: it owns the map click → fetch
// → onDetailData flow and draws the clicked-point marker. Keeping the noise/
// component tree out of this module keeps it off the first-paint bundle.

export interface DetailPopupProps {
  detailPosition: { lat: number; lng: number } | null
  triggerPosition: { lat: number; lng: number } | null
  onDetailData?: (data: NoiseComputeData | null) => void
  onDetailPositionChange?: (pos: { lat: number; lng: number } | null) => void
  /// Fired when the noise fetch fails (non-abort). MVP-0: card stays
  /// open showing the position skeleton with an error message instead of
  /// silently unmounting via `onDetailPositionChange(null)` — Gemini
  /// /gg 2026-05-24 CRITICAL.
  onDetailError?: (message: string | null) => void
}

export default function DetailPopup({ detailPosition, triggerPosition, onDetailData, onDetailPositionChange, onDetailError }: DetailPopupProps) {
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
    fetch(`/api/noise-onfly-v2?lat=${detailPosition.lat}&lng=${detailPosition.lng}`, { signal: controller.signal })
      .then(res => { if (!res.ok) throw new Error(`API ${res.status}`); return res.json() })
      .then((data: NoiseComputeData) => onDetailData?.(data))
      .catch(err => {
        if (err.name === 'AbortError') return
        console.error(err)
        // Keep `detailPosition` so the card stays open with a skeleton
        // → error state. Clearing position here (the pre-MVP-0 behavior)
        // unmounted the card and lost the user's clicked location.
        onDetailError?.(err instanceof Error ? err.message : String(err))
      })

    return () => controller.abort()
  }, [detailPosition, map, onDetailData, onDetailError])

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

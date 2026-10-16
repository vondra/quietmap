// The cursor position over the map for hover readouts — one listener block
// shared by every tooltip that follows the pointer.
import { useEffect, useState } from 'react'
import type { MapRef } from 'react-map-gl/maplibre'

export interface MapHover {
  lat: number
  lng: number
  clientX: number
  clientY: number
  zoom: number
}

/**
 * The pointer's map position and viewport zoom; `null` off the map, while
 * `enabled` is false, and while the camera moves (drag, wheel zoom, keyboard,
 * easing) — a readout must never quote a mid-move position or zoom. Once the
 * camera settles, the resting pointer is re-sampled under the new view.
 * The pointer is tracked on the DOM (maplibre withholds its own mousemove
 * while a drag handler is active, which would leave the pre-drag position).
 */
export function useMapHover(mapRef: MapRef | undefined, enabled: boolean): MapHover | null {
  const [hover, setHover] = useState<MapHover | null>(null)
  useEffect(() => {
    if (!mapRef || !enabled) {
      setHover(null)
      return
    }
    const map = mapRef.getMap()
    const container = map.getCanvasContainer()
    let point: { x: number; y: number } | null = null
    const sample = () => {
      if (!point || map.isMoving()) return
      const { lng, lat } = map.unproject([point.x, point.y])
      setHover({ lat, lng, clientX: point.x, clientY: point.y, zoom: map.getZoom() })
    }
    // A finger has no hover: the mouse events a browser sends after a tap would leave a readout
    // standing at the tapped point, beside the popup's own level and after it closes.
    const onPointerMove = (e: PointerEvent) => {
      if (e.pointerType === 'touch') return
      const rect = container.getBoundingClientRect()
      point = { x: e.clientX - rect.left, y: e.clientY - rect.top }
      sample()
    }
    const onPointerLeave = () => { point = null; setHover(null) }
    const onMoveStart = () => setHover(null)
    container.addEventListener('pointermove', onPointerMove)
    container.addEventListener('pointerleave', onPointerLeave)
    map.on('movestart', onMoveStart)
    map.on('moveend', sample)
    return () => {
      container.removeEventListener('pointermove', onPointerMove)
      container.removeEventListener('pointerleave', onPointerLeave)
      map.off('movestart', onMoveStart)
      map.off('moveend', sample)
    }
  }, [mapRef, enabled])
  return hover
}

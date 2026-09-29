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
    const onMouseMove = (e: MouseEvent) => {
      const rect = container.getBoundingClientRect()
      point = { x: e.clientX - rect.left, y: e.clientY - rect.top }
      sample()
    }
    const onMouseLeave = () => { point = null; setHover(null) }
    const onMoveStart = () => setHover(null)
    container.addEventListener('mousemove', onMouseMove)
    container.addEventListener('mouseleave', onMouseLeave)
    map.on('movestart', onMoveStart)
    map.on('moveend', sample)
    return () => {
      container.removeEventListener('mousemove', onMouseMove)
      container.removeEventListener('mouseleave', onMouseLeave)
      map.off('movestart', onMoveStart)
      map.off('moveend', sample)
    }
  }, [mapRef, enabled])
  return hover
}

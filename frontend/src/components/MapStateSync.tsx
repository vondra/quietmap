import { useEffect, useRef, useCallback } from 'react'
import { useMap } from 'react-map-gl/maplibre'
import { parseHash, type UrlState } from '../hooks/useUrlState'

interface MapStateSyncProps {
  onViewChange: (lat: number, lng: number, zoom: number) => void
  onHashState?: (next: UrlState) => void
}

/** URL ↔ map view: debounced moveend → hash, and an external hash change → map. */
export default function MapStateSync({ onViewChange, onHashState }: MapStateSyncProps) {
  const { current: map } = useMap()
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const handleMove = useCallback(() => {
    if (!map) return
    if (timerRef.current) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => {
      const c = map.getCenter()
      onViewChange(c.lat, c.lng, map.getZoom())
    }, 200)
  }, [map, onViewChange])

  useEffect(() => {
    if (!map) return
    map.on('moveend', handleMove)
    return () => {
      map.off('moveend', handleMove)
      if (timerRef.current) clearTimeout(timerRef.current)
    }
  }, [map, handleMove])

  // A link pasted into an already open tab (or a history step) fires
  // hashchange — our own replaceState writes never do. Jump when the hash
  // names a view the map does not already show at the hash's own precision
  // (a jump would only echo the hash back), then let App apply the other
  // tokens; before this the URL changed and the map stayed put (audit
  // 2026-09-06).
  useEffect(() => {
    if (!map) return
    const onHashChange = () => {
      const next = parseHash()
      const c = map.getCenter()
      const there = c.lat.toFixed(4) === next.lat.toFixed(4) && c.lng.toFixed(4) === next.lng.toFixed(4)
        && map.getZoom().toFixed(2) === next.zoom.toFixed(2)
      if (next.hasExplicitView && !there) map.jumpTo({ center: [next.lng, next.lat], zoom: next.zoom })
      onHashState?.(next)
    }
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [map, onHashState])

  return null
}

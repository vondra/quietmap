import { useEffect, useRef, useCallback } from 'react'
import { useMap } from 'react-map-gl/maplibre'

interface MapStateSyncProps {
  onViewChange: (lat: number, lng: number, zoom: number) => void
}

export default function MapStateSync({ onViewChange }: MapStateSyncProps) {
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

  return null
}

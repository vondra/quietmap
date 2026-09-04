import { useEffect } from 'react'
import { useMap } from 'react-map-gl/maplibre'

export interface SelectedLocation {
  display_name: string
  lat: number
  lon: number
}

interface FlyToLocationProps {
  location: SelectedLocation | null
  onArrived?: (pos: { lat: number; lng: number }) => void
}

export default function FlyToLocation({ location, onArrived }: FlyToLocationProps) {
  const { current: map } = useMap()

  useEffect(() => {
    if (!location || !map) return

    const isMobile = window.innerWidth < 768
    map.flyTo({
      center: [location.lon, location.lat],
      zoom: 14,
      duration: 1500,
      padding: { top: 120, bottom: isMobile ? 350 : 0, left: 0, right: 0 },
    })

    const onMoveEnd = () => {
      map.off('moveend', onMoveEnd)
      onArrived?.({ lat: location.lat, lng: location.lon })
    }

    map.once('moveend', onMoveEnd)

    return () => {
      map.off('moveend', onMoveEnd)
    }
  }, [location, map, onArrived])

  return null
}

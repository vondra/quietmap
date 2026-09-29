// Flies the map to a search result and opens the popup there once the flight has arrived.
import { useEffect } from 'react'
import { useMap } from 'react-map-gl/maplibre'
import { searchFlightArrived, SEARCH_FLIGHT_ZOOM } from '../lib/search-flight'

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
      zoom: SEARCH_FLIGHT_ZOOM,
      duration: 1500,
      padding: { top: 120, bottom: isMobile ? 350 : 0, left: 0, right: 0 },
    })

    // An interrupted flight (hash navigation, a drag) also ends in moveend;
    // only a real arrival opens the popup — see searchFlightArrived.
    const arrive = () => {
      const c = map.getCenter()
      if (searchFlightArrived({ lat: c.lat, lng: c.lng, zoom: map.getZoom() }, location)) {
        onArrived?.({ lat: location.lat, lng: location.lon })
      }
    }
    const onMoveEnd = () => {
      map.off('moveend', onMoveEnd)
      arrive()
    }

    // With prefers-reduced-motion MapLibre jumps instead of flying: the move
    // (and its moveend) is already over when flyTo returns.
    if (map.isMoving()) map.once('moveend', onMoveEnd)
    else arrive()

    return () => {
      map.off('moveend', onMoveEnd)
    }
  }, [location, map, onArrived])

  return null
}

// Flies the map to a search's result or a place of the history and opens the popup there once the
// flight has arrived.
import { useEffect } from 'react'
import { useMap } from 'react-map-gl/maplibre'
import { LngLat } from 'maplibre-gl'
import { searchFlightArrived } from '../lib/search-flight'

export interface SelectedLocation {
  lat: number
  lon: number
  zoom: number
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
    // A place in the history can have a zoom above this map's limit.
    const zoom = Math.min(location.zoom, map.getMaxZoom())
    const point = new LngLat(location.lon, location.lat)
    map.flyTo({
      center: point,
      zoom,
      duration: 1500,
      padding: { top: 120, bottom: isMobile ? 350 : 0, left: 0, right: 0 },
    })

    // An interrupted flight (hash navigation, a drag) also ends in moveend;
    // only a real arrival opens the popup — see searchFlightArrived.
    const arrive = () => {
      const c = map.getCenter()
      // At the Mercator edge the camera stops short to keep the screen inside the world.
      // Compare with the centre flyTo can reach, then open the visitor's original point.
      const landing = map.getMap().transform.applyConstrain(point, zoom)
      if (searchFlightArrived({ lat: c.lat, lng: c.lng, zoom: map.getZoom() },
        { lat: landing.center.lat, lon: landing.center.lng, zoom: landing.zoom })) {
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

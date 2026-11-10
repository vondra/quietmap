// The settled camera and address stay in step, including orientation and viewport padding.
import { useEffect } from 'react'
import { useMap } from 'react-map-gl/maplibre'
import { pixelPadding, type MapCamera } from '../lib/url-state'

export default function MapStateSync({ view, onViewChange }: { view: MapCamera; onViewChange: (view: MapCamera) => void }) {
  const { current: map } = useMap()
  useEffect(() => {
    if (!map) return
    const move = () => {
      const canvas = map.getCanvas()
      const center = map.getCenter(), padding = map.getPadding()
      onViewChange({ lat: center.lat, lng: center.lng, zoom: map.getZoom(), bearing: map.getBearing(), pitch: map.getPitch(),
        padding: { top: (padding.top ?? 0) / canvas.clientHeight, right: (padding.right ?? 0) / canvas.clientWidth,
          bottom: (padding.bottom ?? 0) / canvas.clientHeight, left: (padding.left ?? 0) / canvas.clientWidth } })
    }
    map.on('moveend', move)
    return () => { map.off('moveend', move) }
  }, [map, onViewChange])
  const { lat, lng, zoom, bearing, pitch } = view
  const { top, right, bottom, left } = view.padding
  useEffect(() => {
    if (!map) return
    const canvas = map.getCanvas()
    const center = map.getCenter(), currentPadding = map.getPadding()
    const padding = pixelPadding({ top, right, bottom, left }, canvas.clientWidth, canvas.clientHeight)
    const same = Math.abs(center.lat - lat) < 0.000001 && Math.abs(center.lng - lng) < 0.000001
      && Math.abs(map.getZoom() - zoom) < 0.0001 && Math.abs(map.getBearing() - bearing) < 0.0001 && Math.abs(map.getPitch() - pitch) < 0.0001
    const samePadding = (['top', 'right', 'bottom', 'left'] as const).every(side => Math.abs((currentPadding[side] ?? 0) - padding[side]) < 0.01)
    if (!same || !samePadding) map.jumpTo({ center: [lng, lat], zoom, bearing, pitch, padding })
  }, [map, lat, lng, zoom, bearing, pitch, top, right, bottom, left])
  return null
}

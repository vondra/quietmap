// Public overlays draw in the same order after clicks, asynchronous answers and copied addresses.
import { useEffect } from 'react'
import { useMap } from 'react-map-gl/maplibre'
import { overlayAnchorId } from '../utils/overlay-anchor'

// Area shading below labels; stay pins below the selected point, source highlight and rays.
const SOURCES = ['isochron', 'stays', 'clicked-point', 'flight-track', 'segment-fan']

export default function PublicOverlayOrder() {
  const { current: mapRef } = useMap()
  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const arrange = () => {
      const style = map.getStyle()
      if (!style) return
      const groups = SOURCES.map(source => style.layers
        .filter(layer => 'source' in layer && layer.source === source).map(layer => layer.id))
      const owned = new Set(groups.flat())
      const base = style.layers.filter(layer => !owned.has(layer.id))
      const anchor = overlayAnchorId({ ...style, layers: base })
      const at = anchor ? base.findIndex(layer => layer.id === anchor) : base.length
      const desired = [...base.slice(0, at).map(layer => layer.id), ...groups[0],
        ...base.slice(at).map(layer => layer.id), ...groups.slice(1).flat()]
      const current = style.layers.map(layer => layer.id)
      for (let i = desired.length - 1; i >= 0; i--) {
        const id = desired[i], before = desired[i + 1], old = current.indexOf(id)
        if (!owned.has(id) || current[old + 1] === before) continue
        map.moveLayer(id, before)
        current.splice(old, 1)
        current.splice(before ? current.indexOf(before) : current.length, 0, id)
      }
    }
    map.on('styledata', arrange)
    arrange()
    return () => { map.off('styledata', arrange) }
  }, [mapRef])
  return null
}

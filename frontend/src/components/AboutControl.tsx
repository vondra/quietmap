// The ⓘ in the map's bottom-right corner, after the base map's credits: the way to the About pages,
// which credit every source of the map (on a phone a button of the control row, beside the layers).
import { useControl } from 'react-map-gl/maplibre'

export default function AboutControl() {
  useControl(() => {
    const link = document.createElement('a')
    link.href = '/about'
    link.title = 'About quietmap.org'
    link.setAttribute('aria-label', 'About quietmap.org')
    link.className = 'maplibregl-ctrl about-control'
    return { onAdd: () => link, onRemove: () => link.remove() }
  }, { position: 'bottom-right' })
  return null
}

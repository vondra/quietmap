// "About quietmap.org" in the map's bottom-right corner, left of the attribution ⓘ (from md up;
// the phone's layers sheet carries the link instead).
import { useControl } from 'react-map-gl/maplibre'

export default function AboutControl() {
  useControl(() => {
    const link = document.createElement('a')
    link.href = '/about'
    link.textContent = 'About quietmap.org'
    link.className = 'maplibregl-ctrl about-control hidden md:block rounded-md font-sans border border-black/5 bg-white/95 px-2 py-0.5 text-xs text-muted-foreground shadow-lg backdrop-blur-md hover:text-foreground'
    return { onAdd: () => link, onRemove: () => link.remove() }
  }, { position: 'bottom-right' })
  return null
}

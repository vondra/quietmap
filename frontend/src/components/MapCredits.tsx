// Keep the basemap's required credit at the bottom, reserving its actual height below phone sheets.
import { AttributionControl } from 'maplibre-gl'
import { useControl } from 'react-map-gl/maplibre'
import { OSM_CREDIT } from '../utils/basemaps'

export default function MapCredits() {
  useControl(() => {
    // CARTO requires its credit visible on the map (carto.com/attribution); Esri names its sources.
    // Every other noise-model source is credited in About, reached through the neighbouring ⓘ.
    const control = new AttributionControl({ compact: false, customAttribution: OSM_CREDIT })
    // Different basemaps and phone widths wrap the credit into different numbers of lines.
    const observer = new ResizeObserver(([entry]) => {
      document.documentElement.style.setProperty('--map-footer-height', `${entry.target.getBoundingClientRect().height}px`)
    })
    return {
      onAdd(map) {
        const element = control.onAdd(map)
        observer.observe(element)
        return element
      },
      onRemove() {
        observer.disconnect()
        control.onRemove()
        document.documentElement.style.removeProperty('--map-footer-height')
      },
    }
  }, { position: 'bottom-right' })
  return null
}

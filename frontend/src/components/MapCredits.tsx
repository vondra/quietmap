// The base map's credit in the map's corner: OpenStreetMap's alone folds away, found again through
// the ⓘ beside it; on a phone the control row stands on the credit's measured height.
import { useEffect } from 'react'
import { AttributionControl, type Map as MapLibreMap, type MapLibreEvent } from 'maplibre-gl'
import { useControl } from 'react-map-gl/maplibre'
import { OSM_CREDIT } from '../utils/basemaps'

/** OpenStreetMap's attribution guidelines (osmfoundation.org, read 2026-11-17) let its credit
 *  collapse "automatically after five seconds" or "on map interaction such as panning, clicking,
 *  or zooming", while "the user must still be able to find the licence information": the About
 *  pages, behind the neighbouring ⓘ, credit every source. */
const FOLDS_AFTER_MS = 5000

function foldingCredits() {
  // The noise model owes OpenStreetMap's credit on every base map, also one whose tiles failed.
  const control = new AttributionControl({ compact: false, customAttribution: OSM_CREDIT })
  // Different basemaps and phone widths wrap the credit into different numbers of lines; folded, it has none.
  const observer = new ResizeObserver(([entry]) => {
    document.documentElement.style.setProperty('--map-footer-height', `${entry.target.getBoundingClientRect().height}px`)
  })
  let element: HTMLElement | undefined
  let folds = false
  let shownLongEnough = false
  let stopWaiting = () => {}
  const apply = () => { if (element) element.hidden = folds && shownLongEnough }
  return {
    setFolds(value: boolean) {
      folds = value
      apply()
    },
    onAdd(map: MapLibreMap) {
      element = control.onAdd(map)
      observer.observe(element)
      const fold = () => {
        shownLongEnough = true
        stopWaiting()
        apply()
      }
      // The visitor's own pan or zoom carries the event that caused it; the map's own moves (to a
      // place found, to an address opened) carry none.
      const foldOnVisitorsMove = (event: MapLibreEvent<unknown>) => { if (event.originalEvent) fold() }
      const timer = setTimeout(fold, FOLDS_AFTER_MS)
      map.on('movestart', foldOnVisitorsMove)
      map.on('click', fold)
      stopWaiting = () => {
        clearTimeout(timer)
        map.off('movestart', foldOnVisitorsMove)
        map.off('click', fold)
      }
      return element
    },
    onRemove() {
      stopWaiting()
      observer.disconnect()
      control.onRemove()
      document.documentElement.style.removeProperty('--map-footer-height')
    },
  }
}

/** `folds`: the base map owes OpenStreetMap's credit alone (the standard one), so it folds away once
 *  shown long enough. Esri's and OpenTopoMap's terms keep theirs on the map. */
export default function MapCredits({ folds }: { folds: boolean }) {
  const credits = useControl(foldingCredits, { position: 'bottom-right' })
  useEffect(() => credits.setFolds(folds), [credits, folds])
  return null
}

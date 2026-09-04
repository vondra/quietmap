import { useEffect, useState, useMemo } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { ScatterplotLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'
import { attachPinTapGuard } from '../lib/property-click-guard'

export interface RealEstateFilters {
  enabled: boolean
  propertyType: 'all' | 'land' | 'house'
  listingType: 'all' | 'buy' | 'rent'
  maxNoise: number
}

export interface Property {
  id: string
  title: string
  price: number
  currency: string
  lat: number
  lng: number
  area: number | null
  type: string
  listing: string
  url: string
  photo: string | null
  noise: number | null
  updated: string
}

interface RealEstateLayerProps {
  filters: RealEstateFilters
  onPropertySelect?: (property: Property | null) => void
}

const BUY: [number, number, number] = [37, 99, 235]   // blue
const RENT: [number, number, number] = [245, 158, 11] // amber

// The full CZ listing set is small (a few thousand) and static within a
// session — fetch once and share across mounts; the client filters by
// viewport + type + noise. One fetch, no per-square requests.
let allCache: Property[] | null = null
let allPromise: Promise<Property[]> | null = null
function loadAllProperties(): Promise<Property[]> {
  if (allCache) return Promise.resolve(allCache)
  if (!allPromise) {
    allPromise = fetch('/api/properties')
      .then(r => (r.ok ? r.json() : []))
      .then((data: Property[]) => { allCache = data; return data })
      .catch(() => { allPromise = null; return [] as Property[] })
  }
  return allPromise
}

/**
 * Property markers as a deck.gl ScatterplotLayer on its own overlay, so the
 * pins sit above the basemap — a MapLibre symbol/circle layer would be
 * occluded by the non-interleaved deck canvas. Coloured dots (blue = buy,
 * amber = rent) rather than pin icons:
 * map image loading proved unreliable, and property TYPE is handled by the
 * filter, so a dot reads fine.
 */
export default function RealEstateLayer({ filters, onPropertySelect }: RealEstateLayerProps) {
  const { current: mapRef } = useMap()
  const [overlay, setOverlay] = useState<MapboxOverlay | null>(null)
  const [all, setAll] = useState<Property[]>([])
  const [bounds, setBounds] = useState<{ w: number; s: number; e: number; n: number } | null>(null)

  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const o = new MapboxOverlay({ interleaved: false, layers: [] })
    map.addControl(o)
    setOverlay(o)
    return () => { map.removeControl(o); setOverlay(null) }
  }, [mapRef])

  // Fetch the full listing set once when the overlay is enabled.
  useEffect(() => {
    if (!filters.enabled) { onPropertySelect?.(null); return }
    let cancelled = false
    void loadAllProperties().then(props => { if (!cancelled) setAll(props) })
    return () => { cancelled = true }
  }, [filters.enabled, onPropertySelect])

  // Track the viewport so visible markers re-derive as the map moves.
  useEffect(() => {
    if (!mapRef || !filters.enabled) return
    const map = mapRef.getMap()
    const update = () => {
      const b = map.getBounds()
      setBounds({ w: b.getWest(), s: b.getSouth(), e: b.getEast(), n: b.getNorth() })
    }
    update()
    map.on('moveend', update)
    return () => { map.off('moveend', update) }
  }, [mapRef, filters.enabled])

  // Visible markers: viewport clip + type / listing / noise filters.
  const visible = useMemo(() => {
    if (!filters.enabled || !bounds) return [] as Property[]
    return all.filter(p =>
      p.noise != null &&
      p.lng >= bounds.w && p.lng <= bounds.e && p.lat >= bounds.s && p.lat <= bounds.n &&
      (filters.propertyType === 'all' || p.type === filters.propertyType) &&
      (filters.listingType === 'all' || p.listing === filters.listingType) &&
      p.noise <= filters.maxNoise,
    )
  }, [all, bounds, filters])

  useEffect(() => {
    if (!overlay) return
    overlay.setProps({ layers: visible.length > 0 ? [makeLayer(visible, onPropertySelect)] : [] })
  }, [overlay, visible, onPropertySelect])

  // Stamp the click guard from a cheap CPU hit-test against the visible pins —
  // see attachPinTapGuard for why this must not wait for deck's onClick pick.
  useEffect(() => {
    if (!mapRef || !filters.enabled || visible.length === 0) return
    const map = mapRef.getMap()
    return attachPinTapGuard(map.getCanvas(), (x, y) => visible.some(p => {
      const pt = map.project([p.lng, p.lat])
      return (x - pt.x) ** 2 + (y - pt.y) ** 2 <= 81 // 6 px radius + 2 stroke + slack
    }))
  }, [mapRef, filters.enabled, visible])

  return null
}

function makeLayer(data: Property[], onSelect?: (p: Property | null) => void) {
  return new ScatterplotLayer<Property>({
    id: 'properties',
    data,
    getPosition: (p) => [p.lng, p.lat],
    getFillColor: (p) => (p.listing === 'rent' ? RENT : BUY),
    getLineColor: [255, 255, 255],
    stroked: true,
    radiusUnits: 'pixels',
    getRadius: 6,
    radiusMinPixels: 5,
    radiusMaxPixels: 9,
    lineWidthUnits: 'pixels',
    getLineWidth: 2,
    pickable: true,
    autoHighlight: true,
    highlightColor: [255, 255, 255, 90],
    // No guard stamp here — attachPinTapGuard already stamped in the pointerup
    // task (deck's click pick may lag frames behind the deferred check).
    onClick: (info) => {
      if (!info.object) return
      onSelect?.(info.object as Property)
    },
  })
}

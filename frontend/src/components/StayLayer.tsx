import { useEffect, useRef, useState } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { ScatterplotLayer, TextLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'
import { attachPinTapGuard } from '../lib/property-click-guard'
import { paletteRgb } from '../lib/heatmap-palette'

export interface StayFilters {
  enabled: boolean
  hotels: boolean
  rentals: boolean
  /** YYYY-MM-DD; both set = real stay window, else the server's default. */
  checkin: string | null
  checkout: string | null
  adults: number | null
  /** € per night (forwarded as Stay22's USD-based `max` — close enough for a filter). */
  maxPrice: number | null
  minStars: number | null
  minRating: number | null
}

export interface Stay {
  id: string
  name: string
  lat: number
  lng: number
  thumbnail: string | null
  rating: { value: number | null; count: number | null; stars: number | null }
  capacity: { guests: number | null; bedrooms: number | null }
  freeCancellation: boolean
  price: { total: number; perNight: number } | null
  url: string
  /** Nights the quoted price covers (picked dates, else the server default). */
  nights: number
  /** Total Lden for the pin; null where nothing is computed. */
  noise: number | null
}

interface StayLayerProps {
  filters: StayFilters
  onStaySelect?: (stay: Stay | null) => void
}

// Below this zoom a viewport exceeds the server's bbox cap — above it the
// server scales density itself (one-per-cell sampling when zoomed out,
// the full flat list at street spans).
const MIN_ZOOM = 7
// Bucket grid for cacheable URLs — panning within a cell reuses the
// browser/server cache instead of minting new requests. Zoom-tiered so deep
// zooms get tight boxes (street spans unlock the server's flat mode) while
// zoomed-out views don't churn buckets; every tier is a multiple of the
// server's snap step (see server/src/routes/stay.ts).
const gridFor = (rawSpan: number) => (rawSpan < 0.03 ? 0.01 : rawSpan < 3 ? 0.05 : 0.5)
const NO_NOISE_GREY: [number, number, number] = [148, 163, 184]

type StayResponse = {
  listings: Omit<Stay, 'noise' | 'nights'>[]
  meta: { nights: number; partial?: boolean }
}

// Live listings are viewport-scoped; the cache holds the in-flight promise
// (not just the settled result) so overlapping moveends for the same bucket
// share one request instead of double-hitting the server's upstream budget.
const fetchCache = new Map<string, { at: number; promise: Promise<Stay[]> }>()
const FETCH_TTL_MS = 10 * 60 * 1000
const FETCH_CACHE_MAX = 40

// Zoom-stability pool (owner 2026-07-29: an offer must not vanish when the
// zoom changes): the server's buckets and density modes shift with zoom, so
// the layer renders the union of everything seen this session, keyed by id —
// fresh data wins, oldest entries evicted. Collision filtering keeps the
// pile-up readable; the pool is cleared when the type filter changes because
// pooled entries don't carry their accommodation type.
const POOL_MAX = 1200
const pool = new Map<string, Stay>()
function mergeIntoPool(incoming: Stay[]): Stay[] {
  for (const s of incoming) {
    pool.delete(s.id)
    pool.set(s.id, s)
  }
  while (pool.size > POOL_MAX) pool.delete(pool.keys().next().value!)
  return [...pool.values()]
}

// The epsilon keeps grid-boundary values in place — bare floor(50.05/0.05)
// lands on 1000.999…, snapping a whole cell too far (mirrors the server).
const snap = (v: number, up: boolean, grid: number) => {
  const q = v / grid
  return ((up ? Math.ceil(q - 1e-9) : Math.floor(q + 1e-9)) * grid).toFixed(2)
}

export function formatPerNight(amount: number): string {
  return `€${amount}`
}

/** Pill screen-space box for a pin at (x, y) — mirrors the TextLayer's
 *  pixel offset, font metrics and padding; shared with the tap guard. */
function pillBox(x: number, y: number, label: string) {
  const halfW = 7 + 3.5 * label.length
  return { x1: x - halfW, y1: y - 26, x2: x + halfW, y2: y - 6 }
}

// Overlapping prices were unreadable (owner 2026-07-29) and deck's
// CollisionFilterExtension silently blanks this TextLayer inside the
// overlaid (non-interleaved) MapboxOverlay — so pill winners are picked on
// the CPU: popularity-ranked greedy placement in screen space. Review count
// moves slowly, so the same pills keep winning and zooming/panning doesn't
// reshuffle which prices show. Runs per moveend over ≤ POOL_MAX pins.
function declutterPills(stays: Stay[], map: { project: (c: [number, number]) => { x: number; y: number } }, width: number, height: number): Stay[] {
  const priced = stays.filter(s => s.price != null)
  priced.sort((a, b) => (b.rating.count ?? 0) - (a.rating.count ?? 0))
  const placed: { x1: number; y1: number; x2: number; y2: number }[] = []
  const out: Stay[] = []
  for (const s of priced) {
    const p = map.project([s.lng, s.lat])
    if (p.x < -60 || p.x > width + 60 || p.y < -40 || p.y > height + 40) continue
    const r = pillBox(p.x, p.y, formatPerNight(s.price!.perNight))
    if (placed.some(q => q.x1 < r.x2 + 2 && r.x1 < q.x2 + 2 && q.y1 < r.y2 + 2 && r.y1 < q.y2 + 2)) continue
    placed.push(r)
    out.push(s)
  }
  return out
}

function loadListings(url: string): Promise<Stay[]> {
  let entry = fetchCache.get(url)
  if (!entry || Date.now() - entry.at >= FETCH_TTL_MS) {
    const partial = { v: false }
    const promise = (async () => {
      const res = await fetch(url)
      if (!res.ok) throw new Error(`stay ${res.status}`)
      const data: StayResponse = await res.json()
      partial.v = data.meta.partial === true
      return data.listings.map((l): Stay => ({ ...l, noise: null, nights: data.meta.nights }))
    })()
    entry = { at: Date.now(), promise }
    fetchCache.delete(url) // re-insert so a refreshed bucket is newest for eviction
    fetchCache.set(url, entry)
    const self = entry
    // A failed bucket must not poison the cache for its whole TTL; a partial
    // (server window-truncated) set shows now but retries on the next moveend.
    promise.then(
      () => { if (partial.v && fetchCache.get(url) === self) fetchCache.delete(url) },
      () => { if (fetchCache.get(url) === self) fetchCache.delete(url) },
    )
    while (fetchCache.size > FETCH_CACHE_MAX) fetchCache.delete(fetchCache.keys().next().value!)
  }
  return entry.promise
}

/**
 * Bookable stays (hotels + vacation rentals via Stay22) as price pills over
 * dB-coloured dots. Stamps the shared click guard so the noise popup skips
 * pin clicks. Unlike the static CZ property set this is a live worldwide
 * feed — listings are fetched per viewport bucket. Per-pin noise sampling
 * needs the tile raster (unavailable here), so pins render grey until the
 * server reports per-listing noise.
 */
export default function StayLayer({ filters, onStaySelect }: StayLayerProps) {
  const { current: mapRef } = useMap()
  const [overlay, setOverlay] = useState<MapboxOverlay | null>(null)
  const [stays, setStays] = useState<Stay[]>([])
  const [view, setView] = useState<{ w: number; s: number; e: number; n: number; z: number } | null>(null)
  // Pins fetched under the previous filter set must not survive a filter
  // change — "Hotels only" showing apartments, or pins priced for other
  // dates, would be silently wrong (pool included).
  const filterKey = [filters.hotels, filters.rentals, filters.checkin, filters.checkout,
    filters.adults, filters.maxPrice, filters.minStars, filters.minRating].join('|')
  const appliedTypeRef = useRef(filterKey)
  // Declutter winners — the tap guard suppresses the noise popup only for
  // pill boxes that are actually visible (a hidden pill would eat the tap).
  const pillIdsRef = useRef<Set<string>>(new Set())

  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const o = new MapboxOverlay({ interleaved: false, layers: [] })
    map.addControl(o)
    setOverlay(o)
    return () => { map.removeControl(o); setOverlay(null) }
  }, [mapRef])

  // Track the viewport so fetches re-derive as the map moves.
  useEffect(() => {
    if (!mapRef || !filters.enabled) { onStaySelect?.(null); setStays([]); return }
    const map = mapRef.getMap()
    const update = () => {
      const b = map.getBounds()
      setView({ w: b.getWest(), s: b.getSouth(), e: b.getEast(), n: b.getNorth(), z: map.getZoom() })
    }
    update()
    map.on('moveend', update)
    return () => { map.off('moveend', update) }
  }, [mapRef, filters.enabled, onStaySelect])

  // Stamp the click guard from a cheap CPU hit-test against the loaded pins —
  // see attachPinTapGuard for why this must not go through deck's pick.
  useEffect(() => {
    if (!mapRef || !filters.enabled || stays.length === 0) return
    const map = mapRef.getMap()
    return attachPinTapGuard(map.getCanvas(), (x, y) => {
      for (const s of stays) {
        const p = map.project([s.lng, s.lat])
        const dx = x - p.x
        const dy = y - p.y
        if (dx * dx + dy * dy <= 49) return true // dot: 5 px radius + 1.5 stroke + slack
        // Pill box above the dot — visible (declutter-winning) pills only.
        if (s.price != null && pillIdsRef.current.has(s.id)) {
          const r = pillBox(p.x, p.y, formatPerNight(s.price.perNight))
          if (x >= r.x1 && x <= r.x2 && y >= r.y1 && y <= r.y2) return true
        }
      }
      return false
    })
  }, [mapRef, filters.enabled, stays])

  // Fetch the viewport bucket. Pins render as soon as listings arrive.
  useEffect(() => {
    if (!filters.enabled || !view || view.z < MIN_ZOOM) { setStays([]); return }
    if (appliedTypeRef.current !== filterKey) {
      appliedTypeRef.current = filterKey
      pool.clear()
      setStays([])
    }
    if (!filters.hotels && !filters.rentals) { setStays([]); return }
    const grid = gridFor(Math.max(view.n - view.s, view.e - view.w))
    const bbox = {
      swlat: snap(view.s, false, grid), swlng: snap(view.w, false, grid),
      nelat: snap(view.n, true, grid), nelng: snap(view.e, true, grid),
    }
    // Mirror of the server's span cap — skipping beats a guaranteed 400.
    if (+bbox.nelat - +bbox.swlat > 12 || +bbox.nelng - +bbox.swlng > 12) { setStays([]); return }
    const params = new URLSearchParams(bbox)
    if (!(filters.hotels && filters.rentals)) params.set('type', filters.hotels ? 'hotel' : 'rental')
    if (filters.checkin && filters.checkout) {
      params.set('checkin', filters.checkin)
      params.set('checkout', filters.checkout)
    }
    if (filters.adults != null) params.set('adults', String(filters.adults))
    if (filters.maxPrice != null) params.set('max', String(filters.maxPrice))
    if (filters.minStars != null) params.set('minstars', String(filters.minStars))
    if (filters.minRating != null) params.set('minrating', String(filters.minRating))

    let cancelled = false
    void (async () => {
      try {
        const listings = await loadListings(`/api/stay?${params}`)
        if (cancelled) return
        setStays(mergeIntoPool(listings))
      } catch { /* keep previous pins; the next moveend retries */ }
    })()
    return () => { cancelled = true }
  }, [filters, filterKey, view])

  // Layers rebuild per pool merge and per moveend (pill winners depend on
  // screen space) — ≤ POOL_MAX points, cheap for deck.
  const gateOpen = filters.enabled && view != null && view.z >= MIN_ZOOM
  useEffect(() => {
    if (!overlay || !mapRef) return
    if (!gateOpen || stays.length === 0) { pillIdsRef.current = new Set(); overlay.setProps({ layers: [] }); return }
    const map = mapRef.getMap()
    const canvas = map.getCanvas()
    const pills = declutterPills(stays, map, canvas.clientWidth, canvas.clientHeight)
    pillIdsRef.current = new Set(pills.map(p => p.id))
    overlay.setProps({ layers: makeLayers(stays, pills, onStaySelect) })
  }, [overlay, mapRef, stays, gateOpen, onStaySelect, view])

  return null
}

function makeLayers(data: Stay[], pills: Stay[], onSelect?: (s: Stay | null) => void) {
  const dbColor = (s: Stay): [number, number, number] =>
    s.noise != null ? paletteRgb(s.noise) : NO_NOISE_GREY
  // No guard stamp here — attachPinTapGuard already stamped in the pointerup
  // task (deck's click pick may lag frames behind).
  const onClick = (info: { object?: unknown }) => {
    if (!info.object) return
    onSelect?.(info.object as Stay)
  }
  return [
    new ScatterplotLayer<Stay>({
      id: 'stays-dots',
      data,
      getPosition: (s) => [s.lng, s.lat],
      getFillColor: dbColor,
      getLineColor: [255, 255, 255],
      stroked: true,
      radiusUnits: 'pixels',
      getRadius: 5,
      radiusMinPixels: 4,
      radiusMaxPixels: 7,
      lineWidthUnits: 'pixels',
      getLineWidth: 1.5,
      pickable: true,
      autoHighlight: true,
      highlightColor: [255, 255, 255, 90],
      onClick,
    }),
    // Airbnb-style price pill above the dot; the border repeats the dot's dB
    // colour so price and noise read together at a glance. Collision-filtered
    // (owner 2026-07-29: overlapping prices were unreadable) — popular stays
    // win the spot, their dots stay visible and clickable underneath.
    new TextLayer<Stay>({
      id: 'stays-price',
      data: pills,
      getPosition: (s) => [s.lng, s.lat],
      getText: (s) => formatPerNight(s.price!.perNight),
      getPixelOffset: [0, -16],
      getSize: 12,
      fontFamily: 'system-ui, sans-serif',
      fontWeight: 600,
      characterSet: 'auto',
      getColor: [15, 23, 42, 255],
      background: true,
      getBackgroundColor: [255, 255, 255, 235],
      getBorderColor: (s) => [...dbColor(s), 255] as [number, number, number, number],
      getBorderWidth: 1.5,
      backgroundPadding: [6, 3, 6, 3],
      pickable: true,
      onClick,
    }),
  ]
}

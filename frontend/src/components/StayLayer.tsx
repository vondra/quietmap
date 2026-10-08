// Places to stay on the map: a dot for every place with a room for the visitor's stay and its price
// of a night above it wherever the labels do not collide (the most reviewed first), asked for every
// view the map settles on and kept while it moves; a click on one opens it. One search per mount:
// MapView mounts it anew for a new search.
import { useEffect, useMemo, useRef, useState } from 'react'
import { Layer, Source, useMap } from 'react-map-gl/maplibre'
import type { Map as MapLibreMap, MapMouseEvent } from 'maplibre-gl'
import {
  STAY_DOT_LAYER as DOTS, STAY_MIN_ZOOM, stayFeatures, stayRequest, viewFootprint, withStays,
  type Stay, type ViewBox, type StaySearch,
} from '../lib/stays'

const PRICES = 'stays-price'
const PINS = [DOTS, PRICES]
/** A dot's drawn radius with its ring; a finger (a coarse pointer) reaches one from further. A
 *  mouse click beside a dot opens the popup there, the map's purpose. */
const DOT_PX = 6
const FINGER_PX = 12
/** How long the map rests before its view asks: a burst of wheel zooms is one search of Stay22's 150
 *  a minute, not one per step. */
const REST_MS = 300
/** The basemap labels' font, whose glyphs every basemap serves. */
const FONT = ['Montserrat Medium', 'Open Sans Bold', 'Noto Sans Regular', 'HanWangHeiLight Regular', 'NanumBarunGothic Regular']

/** The server's answer for a view. */
interface StayAnswer {
  listings: Omit<Stay, 'nights' | 'currency'>[]
  nights: number
  currency: string
}

/** The place whose pin is under a map point, if any, as it is seen: a dot clicked, else a price
 *  label clicked (labels lie over the dots), else the nearest dot within a finger's reach. On a
 *  street of hotels the dots lie a few pixels apart. DetailPopup asks too: a click on a pin opens
 *  the place, never a popup beside it. */
export function stayIdAt(map: MapLibreMap, point: { x: number; y: number }): string | null {
  if (!map.getLayer(DOTS)) return null
  const reach = window.matchMedia('(pointer: coarse)').matches ? FINGER_PX : DOT_PX
  const dots = map.queryRenderedFeatures(
    [[point.x - reach, point.y - reach], [point.x + reach, point.y + reach]],
    { layers: [DOTS] },
  )
  let nearest: { id: string; pixels: number } | null = null
  for (const dot of dots) {
    const at = map.project((dot.geometry as GeoJSON.Point).coordinates as [number, number])
    const pixels = Math.hypot(at.x - point.x, at.y - point.y)
    if (!nearest || pixels < nearest.pixels) nearest = { id: String(dot.properties.id), pixels }
  }
  if (nearest && nearest.pixels <= DOT_PX) return nearest.id
  const [label] = map.queryRenderedFeatures([point.x, point.y], { layers: [PRICES] })
  return label ? String(label.properties.id) : nearest && nearest.pixels <= reach ? nearest.id : null
}

export default function StayLayer({ search, onSelect }: { search: StaySearch; onSelect: (stay: Stay) => void }) {
  const { current: mapRef } = useMap()
  const [view, setView] = useState<ViewBox | null>(null)
  const [pins, setPins] = useState<Stay[]>([])
  const [status, setStatus] = useState<{ loading: boolean; failure: string | null }>({ loading: true, failure: null })
  const pinsRef = useRef(pins)
  pinsRef.current = pins
  const onSelectRef = useRef(onSelect)
  onSelectRef.current = onSelect

  // The view the map settles on.
  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    let resting: ReturnType<typeof setTimeout> | undefined
    const settle = () => {
      clearTimeout(resting)
      resting = setTimeout(() => {
        const canvas = map.getCanvas()
        setView(viewFootprint(map.getCenter(), map.getZoom(), map.getBearing(), canvas.clientWidth, canvas.clientHeight))
      }, REST_MS)
    }
    // A pointer over a pin shows that it opens.
    const pointer = () => { map.getCanvas().style.cursor = 'pointer' }
    const away = () => { map.getCanvas().style.cursor = '' }
    const open = (event: MapMouseEvent) => {
      const id = stayIdAt(map, event.point)
      const stay = id == null ? undefined : pinsRef.current.find(pin => pin.id === id)
      if (stay) onSelectRef.current(stay)
    }
    settle()
    map.on('moveend', settle)
    map.on('mouseenter', PINS, pointer)
    map.on('mouseleave', PINS, away)
    map.on('click', open)
    return () => {
      clearTimeout(resting)
      map.off('moveend', settle)
      map.off('mouseenter', PINS, pointer)
      map.off('mouseleave', PINS, away)
      map.off('click', open)
      away()
    }
  }, [mapRef])

  // Every settled view asks for its places. Each answer adds to the pins, a late one too; the status
  // is the current view's. When Stay22 takes no more searches this minute, the view asks again
  // after the seconds the server names.
  const zoomedIn = view != null && view.zoom >= STAY_MIN_ZOOM
  useEffect(() => {
    if (!view || view.zoom < STAY_MIN_ZOOM) return
    let current = true
    let retry: ReturnType<typeof setTimeout> | undefined
    setStatus({ loading: true, failure: null })
    void (async () => {
      const response = await fetch(stayRequest(view, search))
      const body = await response.json().catch(() => null) as Partial<StayAnswer> & { error?: string } | null
      const wait = Number(response.headers.get('retry-after'))
      if (response.status === 503 && wait > 0 && current) retry = setTimeout(() => setView(view => view && { ...view }), wait * 1000)
      if (!response.ok || !body?.listings) throw new Error(body?.error ?? `The server answered HTTP ${response.status}.`)
      const answer = body as StayAnswer
      setPins(pins => withStays(pins, answer.listings.map(listing => ({ ...listing, nights: answer.nights, currency: answer.currency }))))
      if (current) setStatus({ loading: false, failure: null })
    })().catch((error: unknown) => {
      if (current) setStatus({ loading: false, failure: error instanceof Error ? error.message : String(error) })
    })
    return () => {
      current = false
      clearTimeout(retry)
    }
  }, [view, search])

  const data = useMemo(() => stayFeatures(pins), [pins])
  // A pin counts in the world copy nearest the view (a view across the antimeridian runs past 180°).
  const inView = view == null ? 0 : pins.filter(pin => {
    const lng = pin.lng + 360 * Math.round(((view.west + view.east) / 2 - pin.lng) / 360)
    return pin.lat >= view.south && pin.lat <= view.north && lng >= view.west && lng <= view.east
  }).length
  const note =
    view == null ? null
    : !zoomedIn ? 'Zoom in for places to stay'
    : status.failure ? status.failure
    : inView > 0 ? null
    : status.loading ? 'Finding places to stay…'
    : 'No place to stay has a room here for these dates'

  return (
    <>
      {zoomedIn && (
        <Source id="stays" type="geojson" data={data}>
          <Layer
            id={DOTS}
            type="circle"
            paint={{ 'circle-radius': 4.5, 'circle-color': '#334155', 'circle-stroke-color': '#ffffff', 'circle-stroke-width': 1.5 }}
          />
          <Layer
            id={PRICES}
            type="symbol"
            filter={['!=', ['get', 'price'], '']}
            layout={{
              'text-field': ['get', 'price'],
              'text-font': FONT,
              'text-size': 12,
              'text-anchor': 'bottom',
              'text-offset': [0, -0.55],
              'text-padding': 3,
              'symbol-sort-key': ['-', ['get', 'reviews']],
            }}
            // Haloed as the basemap's own labels are: without glyphs nothing covers the dot.
            paint={{ 'text-color': '#0f172a', 'text-halo-color': '#ffffff', 'text-halo-width': 2 }}
          />
        </Source>
      )}
      {note && (
        <div
          data-testid="stay-note"
          role="status"
          className="pointer-events-none absolute left-1/2 top-[calc(env(safe-area-inset-top,0px)+4rem)] z-[1] -translate-x-1/2 whitespace-nowrap rounded-full border border-black/5 bg-white/95 px-3 py-1 text-xs text-muted-foreground shadow-md"
        >
          {note}
        </div>
      )}
    </>
  )
}

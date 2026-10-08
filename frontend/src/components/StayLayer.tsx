// Places to stay on the map: a dot for every place with a room for the visitor's stay and its price
// of a night above it wherever the labels do not collide (the most reviewed first), asked for every
// view the map settles on, kept while it moves and until its price expires. A click on one opens it
// (DetailPopup asks `stayAt`). One search per mount: MapView mounts it anew for a new search.
import { useEffect, useMemo, useState } from 'react'
import { Layer, Source, useMap } from 'react-map-gl/maplibre'
import type { Map as MapLibreMap } from 'maplibre-gl'
import {
  STAY_DOT_LAYER as DOTS, STAY_MIN_ZOOM, stayFeatures, stayOfFeature, stayRequests, viewFootprint, withStays,
  type Stay, type StaySearch, type ViewBox,
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

/** The server's answer for a box. */
interface StayAnswer {
  listings: Omit<Stay, 'nights' | 'currency' | 'expires'>[]
  nights: number
  currency: string
  /** Seconds the places have left. */
  expiresIn: number
  /** What part of the search failed, when the rest is answered. */
  failure: string | null
}

/** The place whose pin is under a map point, if any, as it is seen: the price label clicked (labels
 *  lie over the dots), else the nearest dot within reach, in the world copy the click is in. */
export function stayAt(map: MapLibreMap, point: { x: number; y: number }): Stay | null {
  if (!map.getLayer(DOTS)) return null
  const [label] = map.queryRenderedFeatures([point.x, point.y], { layers: [PRICES] })
  if (label) return stayOfFeature(label)
  const reach = window.matchMedia('(pointer: coarse)').matches ? FINGER_PX : DOT_PX
  const clicked = map.unproject([point.x, point.y]).lng
  let nearest: { stay: Stay; pixels: number } | null = null
  for (const dot of map.queryRenderedFeatures([[point.x - reach, point.y - reach], [point.x + reach, point.y + reach]], { layers: [DOTS] })) {
    const stay = stayOfFeature(dot)
    const at = map.project([stay.lng + 360 * Math.round((clicked - stay.lng) / 360), stay.lat])
    const pixels = Math.hypot(at.x - point.x, at.y - point.y)
    if (pixels <= reach && (!nearest || pixels < nearest.pixels)) nearest = { stay, pixels }
  }
  return nearest?.stay ?? null
}

export default function StayLayer({ search }: { search: StaySearch }) {
  const { current: mapRef } = useMap()
  const [view, setView] = useState<ViewBox | null>(null)
  const [pins, setPins] = useState<Stay[]>([])
  const [status, setStatus] = useState<{ loading: boolean; failure: string | null }>({ loading: true, failure: null })

  // The view the map settles on, and a pointer over a pin that shows it opens.
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
    const pointer = () => { map.getCanvas().style.cursor = 'pointer' }
    const away = () => { map.getCanvas().style.cursor = '' }
    settle()
    map.on('moveend', settle)
    map.on('mouseenter', PINS, pointer)
    map.on('mouseleave', PINS, away)
    return () => {
      clearTimeout(resting)
      map.off('moveend', settle)
      map.off('mouseenter', PINS, pointer)
      map.off('mouseleave', PINS, away)
      away()
    }
  }, [mapRef])

  // Every settled view asks for its places (a box a side of the antimeridian). Each answer adds to
  // the pins, a late one too; the status is the current view's. When Stay22 is asked too often, the
  // view asks again after the seconds the server names.
  const zoomedIn = view != null && view.zoom >= STAY_MIN_ZOOM
  useEffect(() => {
    if (!view || view.zoom < STAY_MIN_ZOOM) return
    let current = true
    let retry: ReturnType<typeof setTimeout> | undefined
    const failures: string[] = []
    setStatus({ loading: true, failure: null })
    void Promise.all(stayRequests(view, search).map(async request => {
      try {
        const response = await fetch(request)
        const body = await response.json().catch(() => null) as Partial<StayAnswer> & { error?: string } | null
        const wait = Number(response.headers.get('retry-after'))
        if (wait > 0 && current && retry === undefined) retry = setTimeout(() => setView(view => view && { ...view }), wait * 1000)
        if (!response.ok || !body?.listings) throw new Error(body?.error ?? `The server answered HTTP ${response.status}.`)
        const answer = body as StayAnswer
        const expires = Date.now() + answer.expiresIn * 1000
        setPins(pins => withStays(pins, answer.listings.map(listing => ({ ...listing, nights: answer.nights, currency: answer.currency, expires }))))
        if (answer.failure) failures.push(answer.failure)
      } catch (error) {
        failures.push(error instanceof Error ? error.message : String(error))
      }
    })).then(() => {
      if (current) setStatus({ loading: false, failure: failures[0] ?? null })
    })
    return () => {
      current = false
      clearTimeout(retry)
    }
  }, [view, search])

  // A pin goes when its price expires (Stay22's lifetime), and the view asks again.
  useEffect(() => {
    if (!pins.length) return
    const next = Math.min(...pins.map(pin => pin.expires))
    const timer = setTimeout(() => {
      setPins(pins => withStays(pins, []))
      setView(view => view && { ...view })
    }, Math.max(0, next - Date.now()))
    return () => clearTimeout(timer)
  }, [pins])

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
              // Digits, the comma and € are all in it (its glyphs, 2026-10-08).
              'text-font': ['Montserrat Medium'],
              'text-size': 12,
              'text-anchor': 'bottom',
              'text-offset': [0, -0.55],
              'text-padding': 1,
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

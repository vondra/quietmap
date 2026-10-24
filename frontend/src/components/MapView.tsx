// The map: basemap, noise heatmap, quiet zones, places to stay, hover readout, search flight, the
// click popup and the track of the loudest flight highlighted in it.
import { useState, useCallback, useEffect, useMemo, useRef } from 'react'
import Map, { NavigationControl, GeolocateControl, useMap, type MapRef, type ViewStateChangeEvent } from 'react-map-gl/maplibre'
import type { StyleSpecification, GeolocateControl as GeolocateControlInstance } from 'maplibre-gl'
import FlyToLocation from './FlyToLocation'
import DetailPopup from './DetailPopup'
import FlightTrackLayer from './FlightTrackLayer'
import SegmentFanLayer from './SegmentFanLayer'
import QuietZonesLayer from './QuietZonesLayer'
import HeatmapOverlay from './HeatmapOverlay'
import { HEATMAP_LAYERS, sourceOf, useTileBuild, type HeatmapSource } from '../lib/tile-urls'
import HoverTooltip from './HoverTooltip'
import AboutControl from './AboutControl'
import DataLayersOverlay from './DataLayersOverlay'
import StayLayer from './StayLayer'
import type { Stay, StaySearch } from '../lib/stays'
import type { DataLayerId } from '../lib/data-layers'
import MapStateSync from './MapStateSync'
import { DEFAULT_BASEMAP, loadBasemapStyle, type BasemapId } from '../utils/basemaps'
import { QUIET_THRESHOLD_DEFAULT, type UrlState } from '../hooks/useUrlState'
import type { SelectedLocation } from './FlyToLocation'
import type { PopupUpdate, SegmentFan } from '../types/noise'
import 'maplibre-gl/dist/maplibre-gl.css'

// The noise model's data: OpenStreetMap's ODbL asks for its credit on the map (the satellite base
// map does not name it); every other source is credited on the About pages. On a phone the credits
// open from the control row's ⓘ and lead with the way to the About pages (`.attrib-about`).
const NOISE_DATA_CREDITS = '<a class="attrib-about" href="/about">About quietmap.org</a>'
  + 'Noise model: &copy; OpenStreetMap contributors and <a href="/about/data-sources">others</a>'

// OpenStreetMap's attribution guidelines let credits shown as a map opens fold away at the first
// touch of the map or after five seconds (osmfoundation.org/wiki/Licence/Attribution_Guidelines).
const CREDITS_SHOWN_MS = 5000

// A geolocate move ends with the position at the map's centre (its accuracy's box is centred in
// Mercator, a fraction of a pixel off); a move cut short leaves it farther.
const ARRIVED_PX = 2

/** The credits open with the map and fold into their ⓘ at the first touch of the map or after
 *  `CREDITS_SHOWN_MS`, once; a visitor's own tap on the ⓘ ends that, and the ⓘ opens them again. */
function CreditsFoldAtFirstTouch() {
  const { current: map } = useMap()
  useEffect(() => {
    if (!map) return
    const credits = map.getContainer().querySelector('.maplibregl-ctrl-attrib')
    const canvas = map.getCanvasContainer()
    const listening = new AbortController()
    const fold = () => {
      credits?.classList.remove('maplibregl-compact-show')
      stop()
    }
    const timer = setTimeout(fold, CREDITS_SHOWN_MS)
    const stop = () => {
      clearTimeout(timer)
      listening.abort()
    }
    canvas.addEventListener('pointerdown', fold, { signal: listening.signal })
    canvas.addEventListener('wheel', fold, { passive: true, signal: listening.signal })
    credits?.querySelector('.maplibregl-ctrl-attrib-button')?.addEventListener('click', stop, { signal: listening.signal })
    return stop
  }, [map])
  return null
}

interface MapViewProps {
  isCurrentDetailPosition: (position: { lat: number; lng: number }) => boolean
  selectedLocation?: SelectedLocation | null
  initialCenter?: [number, number]
  initialZoom?: number
  basemap?: BasemapId
  onViewChange?: (lat: number, lng: number, zoom: number) => void
  onHashState?: (next: UrlState) => void
  onDetailData?: (data: PopupUpdate) => void
  onDetailPositionChange?: (pos: { lat: number; lng: number } | null) => void
  onDetailError?: (message: string) => void
  detailPosition?: { lat: number; lng: number } | null
  /** What the popup highlights: a loudest flight's track or a contributor. */
  highlight?: GeoJSON.FeatureCollection | null
  /** An opened row's segments and the rays of the one selected. */
  segmentFan?: SegmentFan | null
  quietClustersEnabled?: boolean
  quietThreshold?: number
  heatmapLayers?: Record<string, boolean>
  dataLayers?: DataLayerId[]
  /** The places to stay for the stay searched, when shown; a click on one selects it. */
  stays?: { search: StaySearch; onSelect: (stay: Stay) => void } | null
  /** Hands the parent a function that fires the map's GeolocateControl — the
   *  mobile locate box in the BasemapBar row triggers GPS through it. */
  registerGeolocateTrigger?: (trigger: () => void) => void
  /** True while the map is actively following the user's position. */
  onGeolocateActiveChange?: (active: boolean) => void
  /** True once the GeolocateControl is mounted and the browser has a
   *  geolocation API — before that a trigger tap would silently no-op. */
  onGeolocateReadyChange?: (ready: boolean) => void
}

export default function MapView({
  isCurrentDetailPosition, selectedLocation, initialCenter, initialZoom,
  basemap, onViewChange, onHashState, onDetailData, onDetailPositionChange, onDetailError, detailPosition, highlight, segmentFan,
  quietClustersEnabled, quietThreshold, heatmapLayers, dataLayers, stays,
  registerGeolocateTrigger, onGeolocateActiveChange, onGeolocateReadyChange,
}: MapViewProps) {
  const center = initialCenter ?? [49.8, 15.5]
  const zoom = initialZoom ?? 8
  const bm = basemap ?? DEFAULT_BASEMAP
  const [flyToPos, setFlyToPos] = useState<{ lat: number; lng: number } | null>(null)
  const [mapStyle, setMapStyle] = useState<string | StyleSpecification | null>(null)

  const handleArrived = useCallback((pos: { lat: number; lng: number }) => {
    setFlyToPos(pos)
  }, [])

  // A press of the locate button opens the popup where the visitor stands, once the map has arrived
  // there: the control moves the camera for the press, to the position it finds or back to the last
  // one found, and tells the press and the position around that move (without an animation, as with
  // reduced motion, the move ends before either), so the arrival is judged once they have all run;
  // a move cut short (a finger, a newer position) leaves the map elsewhere and waits for the next.
  const mapRef = useRef<MapRef>(null)
  const located = useRef<{ lat: number; lng: number } | null>(null)
  const popupAtLocationDue = useRef(false)
  const openPopupAtLocationOnArrival = useCallback(() => queueMicrotask(() => {
    const map = mapRef.current
    const at = located.current
    if (!popupAtLocationDue.current || !at || !map || map.isMoving()) return
    const there = map.project([at.lng, at.lat])
    const centre = map.project(map.getCenter())
    if (Math.hypot(there.x - centre.x, there.y - centre.y) > ARRIVED_PX) return
    popupAtLocationDue.current = false
    setFlyToPos({ ...at })
  }), [])
  const handleMoveEnd = useCallback((e: ViewStateChangeEvent & { geolocateSource?: boolean }) => {
    if (e.geolocateSource) openPopupAtLocationOnArrival()
  }, [openPopupAtLocationOnArrival])

  const geolocateRef = useRef<GeolocateControlInstance | null>(null)
  useEffect(() => {
    registerGeolocateTrigger?.(() => geolocateRef.current?.trigger())
  }, [registerGeolocateTrigger])
  // Stable ref callback: an inline arrow would re-fire the control's
  // useImperativeHandle layout effect on EVERY MapView render (React appends
  // the ref itself to the deps), cascading an extra render of the whole map
  // tree per unrelated state change.
  const setGeolocateRef = useCallback((instance: GeolocateControlInstance | null) => {
    geolocateRef.current = instance
    onGeolocateReadyChange?.(instance !== null && 'geolocation' in navigator)
  }, [onGeolocateReadyChange])

  const tileBuild = useTileBuild()
  // The layers on, as one source: every layer the painted total, one its own tiles, several their
  // combination (`hm3-combine`). A layer the manifest does not publish (no archive painted yet) has
  // no tiles to fetch.
  const heatmapSource = useMemo((): HeatmapSource | null => {
    const active = HEATMAP_LAYERS.filter(s => !!heatmapLayers?.[s])
    if (active.length === HEATMAP_LAYERS.length) return 'total'
    return sourceOf(active.filter(s => tileBuild === null || s in tileBuild.byLayer))
  }, [heatmapLayers, tileBuild])

  useEffect(() => {
    let cancelled = false
    void loadBasemapStyle(bm).then((style) => {
      if (!cancelled) setMapStyle(style)
    })
    return () => {
      cancelled = true
    }
  }, [bm])

  if (!mapStyle) {
    return <div className="h-full w-full bg-[#fafaf8]" />
  }

  return (
    <Map
      ref={mapRef}
      initialViewState={{
        latitude: center[0],
        longitude: center[1],
        zoom,
      }}
      style={{ width: '100%', height: '100%' }}
      mapStyle={mapStyle}
      onMoveEnd={handleMoveEnd}
      fadeDuration={0}
      maxZoom={16}
      // Compact ⓘ: expands to the basemap sources' own credits (OSM/Carto —
      // their licenses require on-map attribution) and the noise model's data.
      // AboutControl sits left of it.
      attributionControl={{ compact: true, customAttribution: NOISE_DATA_CREDITS }}
      // Defaults (deceleration 2500, maxSpeed 1400) give ~1.25 s inertia on a medium
      // flick — too sluggish. 4000 / 1100 lands around ~780 ms, between the default
      // and a Google-Maps-snappy feel.
      dragPan={{ deceleration: 4000, maxSpeed: 1100 }}
    >
      <NavigationControl position="bottom-left" showCompass={false} />
      <AboutControl />
      <CreditsFoldAtFirstTouch />
      {/* Precise location is user-triggered (Google pattern): the initial view
          only approximates from browser language (utils/initial-view.ts); GPS
          fires on this button's click, when the permission prompt is expected. */}
      {/* trackUserLocation is what makes the button STATEFUL (blue while
          following, outline when the map pans away) — without it maplibre
          never applies the -active class at all. */}
      <GeolocateControl
        ref={setGeolocateRef}
        position="bottom-left"
        trackUserLocation
        showUserLocation
        fitBoundsOptions={{ maxZoom: 14 }}
        onGeolocate={(e) => {
          located.current = { lat: e.coords.latitude, lng: e.coords.longitude }
          openPopupAtLocationOnArrival()
        }}
        onTrackUserLocationStart={() => {
          popupAtLocationDue.current = true
          onGeolocateActiveChange?.(true)
        }}
        // Fires for BACKGROUND (user panned away while tracking) as well as
        // OFF. The row's icon deliberately shows blue only while actively
        // FOLLOWING — distinguishing the two would need maplibre's private
        // _watchState, and the location dot stays visible either way.
        onTrackUserLocationEnd={() => {
          popupAtLocationDue.current = false
          onGeolocateActiveChange?.(false)
        }}
        // PERMISSION_DENIED goes straight to OFF without a
        // trackuserlocationend — without this the row's icon stays blue.
        onError={() => onGeolocateActiveChange?.(false)}
      />
      {/* Noise heatmap (HM3 tiles out of the pmtiles archives, decoded +
          energy-summed + palette-mapped in the browser) below the labels,
          above the basemap, on its own interleaved deck.gl canvas. */}
      <HeatmapOverlay source={heatmapSource} />
      <DataLayersOverlay layers={dataLayers ?? []} />
      <QuietZonesLayer enabled={quietClustersEnabled ?? false} threshold={quietThreshold ?? QUIET_THRESHOLD_DEFAULT} />
      <FlyToLocation location={selectedLocation ?? null} onArrived={handleArrived} />
      <HoverTooltip source={heatmapSource} />
      <DetailPopup
        isCurrentDetailPosition={isCurrentDetailPosition}
        detailPosition={detailPosition ?? null}
        triggerPosition={flyToPos}
        onDetailData={onDetailData}
        onDetailPositionChange={onDetailPositionChange}
        onDetailError={onDetailError}
        onStaySelect={stays?.onSelect}
      />
      {/* A new search starts from no pins. */}
      {stays && <StayLayer key={JSON.stringify(stays.search)} search={stays.search} />}
      {/* The segments above the opened row's highlight: a style rebuild adds the layers in this
          order. */}
      <FlightTrackLayer highlight={highlight ?? null} />
      <SegmentFanLayer fan={segmentFan ?? null} />
      {onViewChange && onHashState && <MapStateSync onViewChange={onViewChange} onHashState={onHashState} />}
    </Map>
  )
}

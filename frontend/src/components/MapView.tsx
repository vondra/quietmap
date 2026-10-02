// The map: basemap, noise heatmap, quiet zones, hover readout, search flight, the click popup and the
// track of the loudest flight highlighted in it.
import { useState, useCallback, useEffect, useMemo, useRef } from 'react'
import Map, { NavigationControl, GeolocateControl } from 'react-map-gl/maplibre'
import type { StyleSpecification, GeolocateControl as GeolocateControlInstance } from 'maplibre-gl'
import FlyToLocation from './FlyToLocation'
import DetailPopup from './DetailPopup'
import FlightTrackLayer from './FlightTrackLayer'
import SegmentFanLayer from './SegmentFanLayer'
import QuietZonesLayer from './QuietZonesLayer'
import HeatmapOverlay from './HeatmapOverlay'
import { HEATMAP_LAYERS, useTileBuild, type HeatmapSource } from '../lib/tile-urls'
import HoverTooltip from './HoverTooltip'
import MapStateSync from './MapStateSync'
import { DEFAULT_BASEMAP, loadBasemapStyle, type BasemapId } from '../utils/basemaps'
import { QUIET_THRESHOLD_DEFAULT, type UrlState } from '../hooks/useUrlState'
import type { SelectedLocation } from './FlyToLocation'
import type { PopupUpdate, SegmentFan } from '../types/noise'
import 'maplibre-gl/dist/maplibre-gl.css'

// The noise model's inputs whose licences ask for credit (OpenStreetMap's ODbL above all).
const NOISE_DATA_CREDITS =
  'Noise model: &copy; OpenStreetMap contributors (ODbL), Copernicus ERA5, WorldClim, IEA, Eurostat, EU TEN-T'

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
  /** The track of the loudest flight highlighted in the popup. */
  /** The highlighted flight track or contributor pieces ([lat, lon, ..] ends). */
  flightTrack?: number[][][] | null
  /** The segments view's rays from the computed pieces to the receiver. */
  segmentFan?: SegmentFan | null
  quietClustersEnabled?: boolean
  quietThreshold?: number
  heatmapLayers?: Record<string, boolean>
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
  basemap, onViewChange, onHashState, onDetailData, onDetailPositionChange, onDetailError, detailPosition, flightTrack, segmentFan,
  quietClustersEnabled, quietThreshold, heatmapLayers,
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
  const activeHeatmapSources = useMemo((): readonly HeatmapSource[] => {
    const active = HEATMAP_LAYERS.filter(s => !!heatmapLayers?.[s])
    // All on → the single precomputed `total` tile (one fetch, no client-side sum);
    // any subset → fetch + energy-sum those layers. A layer the manifest does not
    // publish (no archive painted yet) has no tiles to fetch.
    if (active.length === HEATMAP_LAYERS.length) return ['total']
    return active.filter(s => tileBuild === null || s in tileBuild.byLayer)
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
      initialViewState={{
        latitude: center[0],
        longitude: center[1],
        zoom,
      }}
      style={{ width: '100%', height: '100%' }}
      mapStyle={mapStyle}
      // maplibre mounts the compact attribution EXPANDED (covering the mobile
      // control row) and only collapses it on its own toggle click — collapse
      // immediately; the corner ⓘ is the resting state, tap to read credits.
      onLoad={(e) => {
        e.target.getContainer().querySelector('.maplibregl-ctrl-attrib')?.classList.remove('maplibregl-compact-show')
      }}
      fadeDuration={0}
      maxZoom={16}
      // Compact ⓘ: expands to the basemap sources' own credits (OSM/Carto —
      // their licenses require on-map attribution) and the noise model's data.
      attributionControl={{ compact: true, customAttribution: NOISE_DATA_CREDITS }}
      // Defaults (deceleration 2500, maxSpeed 1400) give ~1.25 s inertia on a medium
      // flick — too sluggish. 4000 / 1100 lands around ~780 ms, between the default
      // and a Google-Maps-snappy feel.
      dragPan={{ deceleration: 4000, maxSpeed: 1100 }}
    >
      <NavigationControl position="bottom-left" showCompass={false} />
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
        onTrackUserLocationStart={() => onGeolocateActiveChange?.(true)}
        // Fires for BACKGROUND (user panned away while tracking) as well as
        // OFF. The row's icon deliberately shows blue only while actively
        // FOLLOWING — distinguishing the two would need maplibre's private
        // _watchState, and the location dot stays visible either way.
        onTrackUserLocationEnd={() => onGeolocateActiveChange?.(false)}
        // PERMISSION_DENIED goes straight to OFF without a
        // trackuserlocationend — without this the row's icon stays blue.
        onError={() => onGeolocateActiveChange?.(false)}
      />
      {/* Noise heatmap (HM3 tiles out of the pmtiles archives, decoded +
          energy-summed + palette-mapped in the browser) below the labels,
          above the basemap, on its own interleaved deck.gl canvas. */}
      <HeatmapOverlay sources={activeHeatmapSources} />
      <QuietZonesLayer enabled={quietClustersEnabled ?? false} threshold={quietThreshold ?? QUIET_THRESHOLD_DEFAULT} />
      <FlyToLocation location={selectedLocation ?? null} onArrived={handleArrived} />
      <HoverTooltip sources={activeHeatmapSources} />
      <DetailPopup
        isCurrentDetailPosition={isCurrentDetailPosition}
        detailPosition={detailPosition ?? null}
        triggerPosition={flyToPos}
        onDetailData={onDetailData}
        onDetailPositionChange={onDetailPositionChange}
        onDetailError={onDetailError}
      />
      <SegmentFanLayer fan={segmentFan ?? null} />
      <FlightTrackLayer track={flightTrack ?? null} />
      {onViewChange && onHashState && <MapStateSync onViewChange={onViewChange} onHashState={onHashState} />}
    </Map>
  )
}

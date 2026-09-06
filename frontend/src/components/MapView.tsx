import { useState, useCallback, useEffect, useMemo, useRef } from 'react'
import Map, { NavigationControl, GeolocateControl } from 'react-map-gl/maplibre'
import type { StyleSpecification, GeolocateControl as GeolocateControlInstance } from 'maplibre-gl'
import FlyToLocation from './FlyToLocation'
import DetailPopup from './DetailPopup'
import QuietZonesLayer from './QuietZonesLayer'
import RealEstateLayer from './RealEstateLayer'
import StayLayer from './StayLayer'
import ValidationLayer, { type ValidationPayload, type ValidationSelection } from './ValidationLayer'
import IsochronLayer from './IsochronLayer'
import RasterOverlayLayer from './RasterOverlayLayer'
import HeatmapOverlay, { HEATMAP_LAYERS } from './HeatmapOverlay'
import HoverTooltip from './HoverTooltip'
import CellInspectorLayer from './CellInspectorLayer'
import MapStateSync from './MapStateSync'
import { DEFAULT_BASEMAP, loadBasemapStyle, type BasemapId } from '../utils/basemaps'
import { QUIET_THRESHOLD_DEFAULT, type UrlState } from '../hooks/useUrlState'
import type { SelectedLocation } from './FlyToLocation'
import type { NoiseComputeData } from '../types/noise'
import 'maplibre-gl/dist/maplibre-gl.css'

interface MapViewProps {
  selectedLocation?: SelectedLocation | null
  initialCenter?: [number, number]
  initialZoom?: number
  basemap?: BasemapId
  onViewChange?: (lat: number, lng: number, zoom: number) => void
  onHashState?: (next: UrlState) => void
  onDetailData?: (data: NoiseComputeData | null) => void
  onDetailPositionChange?: (pos: { lat: number; lng: number } | null) => void
  onDetailError?: (message: string | null) => void
  detailPosition?: { lat: number; lng: number } | null
  quietClustersEnabled?: boolean
  quietThreshold?: number
  highlightGeometry?: any | null
  isochronGeojson?: GeoJSON.Feature | null
  realEstateFilters?: import('./RealEstateLayer').RealEstateFilters
  onPropertySelect?: (property: import('./RealEstateLayer').Property | null) => void
  stayFilters?: import('./StayLayer').StayFilters
  onStaySelect?: (stay: import('./StayLayer').Stay | null) => void
  rasterOverlays?: Record<string, boolean>
  validationEnabled?: boolean
  validationPayload?: ValidationPayload | null
  onValidationSelect?: (selection: ValidationSelection) => void
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
  selectedLocation, initialCenter, initialZoom,
  basemap, onViewChange, onHashState, onDetailData, onDetailPositionChange, onDetailError, detailPosition,
  quietClustersEnabled, quietThreshold, highlightGeometry, isochronGeojson, realEstateFilters, onPropertySelect, stayFilters, onStaySelect, rasterOverlays,
  validationEnabled, validationPayload, onValidationSelect,
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

  const activeHeatmapSources = useMemo(() => {
    const active = HEATMAP_LAYERS.filter(s => !!rasterOverlays?.[s])
    // All seven on → fetch the single precomputed `total` tile (one fetch, no
    // client-side sum); any subset → fetch + energy-sum those layers.
    return active.length === HEATMAP_LAYERS.length ? (['total'] as const) : active
  }, [rasterOverlays])

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
      // their licenses require on-map attribution) plus the DB-IP CC-BY
      // credit for the IP-country initial view (/api/initial-view).
      attributionControl={{ compact: true, customAttribution: '<a href="https://db-ip.com" target="_blank" rel="noopener">IP Geolocation by DB-IP</a>' }}
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
      <RasterOverlayLayer visibleLayers={rasterOverlays ?? {}} />
      {/* Highlight rides on the same deck.gl canvas as the heatmap so
          it always draws above the HM3 tiles. A separate MapLibre
          Source/Layer would sit under the non-interleaved deck canvas
          and disappear whenever any heatmap was active. */}
      <HeatmapOverlay
        sources={activeHeatmapSources}
        highlightGeometry={highlightGeometry ?? null}
      />
      <QuietZonesLayer enabled={quietClustersEnabled ?? false} threshold={quietThreshold ?? QUIET_THRESHOLD_DEFAULT} />
      {/* After the heatmap + quiet overlays so the property markers (their own
          deck overlay) stack on top rather than being hidden under the heatmap. */}
      {realEstateFilters && <RealEstateLayer filters={realEstateFilters} onPropertySelect={onPropertySelect} />}
      {stayFilters && <StayLayer filters={stayFilters} onStaySelect={onStaySelect} />}
      {/* Validation anchors above every data overlay — a dot click also lands
          the ordinary map click, so the noise popup opens for the same spot
          (measured next to modelled is the point). Mounted only with `val=1`
          so ordinary visitors never pay for the extra overlay. */}
      {validationEnabled && <ValidationLayer payload={validationPayload ?? null} onSelect={onValidationSelect} />}
      <HoverTooltip sources={activeHeatmapSources} />
      <CellInspectorLayer rasterOverlays={rasterOverlays ?? {}} />
      <IsochronLayer geojson={isochronGeojson ?? null} />
      <FlyToLocation location={selectedLocation ?? null} onArrived={handleArrived} />
      <DetailPopup
        detailPosition={detailPosition ?? null}
        triggerPosition={flyToPos}
        onDetailData={onDetailData}
        onDetailPositionChange={onDetailPositionChange}
        onDetailError={onDetailError}
      />
      {onViewChange && <MapStateSync onViewChange={onViewChange} onHashState={onHashState} />}
    </Map>
  )
}

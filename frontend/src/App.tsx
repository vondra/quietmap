import { useState, useCallback, useRef, useEffect, lazy, Suspense } from 'react'
import { Tooltip } from '@base-ui/react/tooltip'
import MapView from './components/MapView'
import SearchBar from './components/SearchBar'
import IsochronPanel from './components/IsochronPanel'
import ControlCard from './components/ControlCard'
import DetailCard from './components/DetailCard'
import LayersPanel from './components/LayersPanel'
import MobileDetailSheet from './components/MobileDetailSheet'
import BasemapBar from './components/BasemapBar'
import PropertyCard from './components/PropertyCard'
import StayCard from './components/StayCard'
import FloatingCard from './components/FloatingCard'
import ValidationCard, { ValidationStatusCard } from './components/ValidationCard'
import { useUrlState, EMPTY_RASTER_OVERLAYS, QUIET_THRESHOLD_DEFAULT } from './hooks/useUrlState'
import type { SelectedLocation } from './components/FlyToLocation'
import type { RealEstateFilters, Property } from './components/RealEstateLayer'
import type { StayFilters, Stay } from './components/StayLayer'

function localStayDate(offsetDays: number): string {
  const d = new Date()
  d.setDate(d.getDate() + offsetDays)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

/** StayFilters → the `stay` URL token (null = layer off, omitted from the hash). */
function stayParam(f: StayFilters): 'all' | 'hotel' | 'rental' | 'none' | null {
  if (!f.enabled) return null
  if (f.hotels && f.rentals) return 'all'
  if (f.hotels) return 'hotel'
  if (f.rentals) return 'rental'
  return 'none'
}
import { useValidationPayload, type ValidationSelection } from './components/ValidationLayer'
import type { NoiseComputeData } from './types/noise'
import { DEFAULT_BASEMAP, type BasemapId } from './utils/basemaps'
import { fetchIpCountry, resolveInitialView, type InitialView } from './utils/initial-view'
import { setDocumentTitle } from './utils/page-title'

const AboutPage = lazy(() => import('./components/AboutPage'))

export default function App() {
  // Route split: /about and the map app must not share one component instance,
  // or the early return would skip MapApp's hooks on the next render and break
  // React's hook order (Rules-of-Hooks). Keep all map state inside MapApp.
  if (window.location.pathname === '/about' || window.location.pathname.startsWith('/about/')) {
    return (
      <Suspense fallback={<div className="h-screen w-screen bg-[#fafaf8]" />}>
        <AboutPage />
      </Suspense>
    )
  }
  return <MapApp />
}

function MapApp() {
  const { initial, updateUrl } = useUrlState()

  // First-visit view: race the server's IP-country guess (bounded 500 ms)
  // BEFORE the map mounts, so the first paint already shows the visitor's
  // country — never a visible Europe→country jump. A #hash with explicit
  // coordinates (a shared link) resolves immediately and skips the fetch;
  // any fetch failure falls back to the language heuristic in `initial`.
  const [initialView, setInitialView] = useState<InitialView | null>(
    initial.hasExplicitView ? { lat: initial.lat, lng: initial.lng, zoom: initial.zoom } : null,
  )
  // While the view is unresolved the map isn't mounted, but the layer/basemap
  // controls ARE — a toggle in that ≤500 ms window must not serialize the
  // language-fallback coordinates into the shared URL (syncUrl guards on
  // this; the next sync after resolution carries the toggle anyway).
  const initialViewPendingRef = useRef(initialView === null)
  useEffect(() => {
    if (initialView !== null) return
    let cancelled = false
    void fetchIpCountry(500).then((ipCountry) => {
      if (cancelled) return
      // IP country wins; on a miss (null / timeout) keep the hash-seeded
      // `initial` so a partial hash's zoom/state survives — its lat/lng/zoom
      // already carry the language fallback for the no-hash first visit.
      const view = ipCountry
        ? resolveInitialView(navigator.languages ?? [], ipCountry)
        : { lat: initial.lat, lng: initial.lng, zoom: initial.zoom }
      // Seed the URL-composition ref too: a layer toggle before the first
      // moveend must serialize THIS view, not the language fallback.
      mapViewRef.current = view
      initialViewPendingRef.current = false
      setInitialView(view)
    })
    return () => { cancelled = true }
  }, [initialView, initial])

  const [selectedLocation, setSelectedLocation] = useState<SelectedLocation | null>(null)
  const [layersOpen, setLayersOpen] = useState(false)
  const [isochronActive, setIsochronActive] = useState(false)
  const [isochronGeojson, setIsochronGeojson] = useState<GeoJSON.Feature | null>(null)
  const [detailPosition, setDetailPosition] = useState<{ lat: number; lng: number } | null>(initial.detailPosition)
  const [noiseDetailData, setNoiseDetailData] = useState<NoiseComputeData | null>(null)
  // MVP-0: card opens skeleton-immediate-on-click; error path keeps the
  // position so user sees the failure context instead of card vanishing.
  const [noiseDetailError, setNoiseDetailError] = useState<string | null>(null)
  const [highlightGeometry, setHighlightGeometry] = useState<any | null>(null)
  const [quietClustersEnabled, setQuietClustersEnabled] = useState(initial.quietClusters)
  const [quietThreshold, setQuietThreshold] = useState(initial.quietThreshold ?? QUIET_THRESHOLD_DEFAULT)
  const [basemap, setBasemap] = useState<BasemapId>(initial.basemap ?? DEFAULT_BASEMAP)
  const [realEstateFilters, setRealEstateFilters] = useState<RealEstateFilters>({
    enabled: false, propertyType: 'all', listingType: 'all', maxNoise: 60,
  })
  const [selectedProperty, setSelectedProperty] = useState<Property | null>(null)
  // Bookable-stays overlay (Stay22 pilot): no panel UI yet — enabled only via
  // Bookable-stays overlay: the panel toggle drives it; a shared link's
  // `stay=1|hotel|rental|none` URL param preselects it. Both families are on
  // by default so enabling the layer shows everything.
  const [stayFilters, setStayFilters] = useState<StayFilters>(() => ({
    enabled: initial.stay != null,
    hotels: initial.stay == null || initial.stay === 'all' || initial.stay === 'hotel',
    rentals: initial.stay == null || initial.stay === 'all' || initial.stay === 'rental',
    // Prefilled stay window (owner 2026-07-29, Booking/Google convention:
    // an empty date field reads as broken): today + 2 nights, local calendar.
    checkin: localStayDate(0),
    checkout: localStayDate(2),
    adults: null,
    maxPrice: null,
    minStars: null,
    minRating: null,
  }))
  const stayFiltersRef = useRef(stayFilters)
  const [selectedStay, setSelectedStay] = useState<Stay | null>(null)
  // Mobile locate box lives in the BasemapBar row (one container = one
  // baseline); it fires the map's GeolocateControl through this ref.
  const geolocateTrigger = useRef<() => void>(() => {})
  const [geolocateActive, setGeolocateActive] = useState(false)
  const [geolocateReady, setGeolocateReady] = useState(false)
  const registerGeolocateTrigger = useCallback((trigger: () => void) => {
    geolocateTrigger.current = trigger
  }, [])
  const handleLocate = useCallback(() => geolocateTrigger.current(), [])
  const [rasterOverlays, setRasterOverlays] = useState<Record<string, boolean>>(
    initial.rasterOverlays ?? { ...EMPTY_RASTER_OVERLAYS },
  )
  // Validation-anchor overlay (owner/QA tool): enabled only via `val=1` in
  // the URL; a clicked anchor renders its card in the right column while the
  // ordinary noise popup opens for the same spot underneath.
  const validationEnabled = initial.validation
  const [validationSelection, setValidationSelection] = useState<ValidationSelection | null>(null)
  const validationPayload = useValidationPayload(validationEnabled)
  const rasterOverlaysRef = useRef(rasterOverlays)
  rasterOverlaysRef.current = rasterOverlays

  const mapViewRef = useRef({ lat: initial.lat, lng: initial.lng, zoom: initial.zoom })
  const quietClustersRef = useRef(quietClustersEnabled)
  const quietThresholdRef = useRef(quietThreshold)
  const basemapRef = useRef(basemap)

  // Pre-warm the lazy popup-body chunk the instant a point is clicked, so it
  // downloads concurrently with the ~1.5 s noise compute instead of after it
  // (gg: codex+agy — React.lazy only fetches when the component first renders,
  // which is after data arrives; this overlaps the network with the compute).
  useEffect(() => {
    if (detailPosition) void import('./components/NoiseDetailContent')
  }, [detailPosition])

  // Tab/share title tracks the open popup: reverse-geocode the position
  // (place-level, server-cached) and compose "Dejvice, Praha - 62 dB -
  // quietmap.org" — place first, never the number (owner spec 2026-07-10).
  const [detailPlaceName, setDetailPlaceName] = useState<string | null>(null)
  useEffect(() => {
    // Clear synchronously so a moved popup never shows the previous place
    // next to the new position's dB while the new lookup is in flight.
    setDetailPlaceName(null)
    if (!detailPosition) return
    const controller = new AbortController()
    fetch(`/api/reverse?lat=${detailPosition.lat}&lon=${detailPosition.lng}`, { signal: controller.signal })
      .then(res => (res.ok ? res.json() : null))
      .then(json => setDetailPlaceName(json?.place ?? null))
      .catch(() => { if (!controller.signal.aborted) setDetailPlaceName(null) })
    return () => controller.abort()
  }, [detailPosition])

  const detailTotalLden = noiseDetailData?.total_lden ?? null
  useEffect(() => {
    setDocumentTitle(detailPosition
      ? [detailPlaceName, detailTotalLden != null ? `${Math.round(detailTotalLden)} dB` : null]
      : [])
  }, [detailPosition, detailPlaceName, detailTotalLden])

  const syncUrl = useCallback((overrides?: Partial<{
    quietClusters: boolean
    quietThreshold: number
    lat: number
    lng: number
    zoom: number
    detailPosition: { lat: number; lng: number } | null
    basemap: BasemapId
    rasterOverlays: Record<string, boolean>
    stay: 'all' | 'hotel' | 'rental' | 'none' | null
  }>) => {
    // Never serialize the pre-resolution language fallback (see
    // initialViewPendingRef above) — first visits carry no hash anyway.
    if (initialViewPendingRef.current) return
    const v = mapViewRef.current
    updateUrl({
      lat: overrides?.lat ?? v.lat,
      lng: overrides?.lng ?? v.lng,
      zoom: overrides?.zoom ?? v.zoom,
      quietClusters: overrides?.quietClusters ?? quietClustersRef.current,
      quietThreshold: overrides?.quietThreshold ?? quietThresholdRef.current,
      detailPosition: overrides?.detailPosition ?? null,
      basemap: overrides?.basemap ?? basemapRef.current,
      rasterOverlays: overrides?.rasterOverlays ?? rasterOverlaysRef.current,
      validation: validationEnabled,
      stay: overrides?.stay !== undefined ? overrides.stay : stayParam(stayFiltersRef.current),
    })
  }, [updateUrl, validationEnabled])

  const handleRasterOverlaysChange = useCallback((next: Record<string, boolean>) => {
    setRasterOverlays(next)
    rasterOverlaysRef.current = next
    syncUrl({ rasterOverlays: next })
  }, [syncUrl])

  const handleViewChange = useCallback((lat: number, lng: number, zoom: number) => {
    mapViewRef.current = { lat, lng, zoom }
    syncUrl({ lat, lng, zoom })
  }, [syncUrl])

  const handleQuietClustersChange = useCallback((enabled: boolean) => {
    setQuietClustersEnabled(enabled)
    quietClustersRef.current = enabled
    syncUrl({ quietClusters: enabled })
  }, [syncUrl])

  const handleQuietThresholdChange = useCallback((threshold: number) => {
    setQuietThreshold(threshold)
    quietThresholdRef.current = threshold
    syncUrl({ quietThreshold: threshold })
  }, [syncUrl])

  // Everything the open noise detail owns — data, error, its map highlight.
  // One closer keeps the pin-card exclusivity paths from diverging (a pin
  // click once closed the card but left the highlight geometry behind).
  const closeNoiseDetail = useCallback(() => {
    setNoiseDetailData(null)
    setNoiseDetailError(null)
    setHighlightGeometry(null)
  }, [])

  const handleDetailPositionChange = useCallback((pos: { lat: number; lng: number } | null) => {
    setDetailPosition(pos)
    // Fresh click: clear stale data + error so the new skeleton renders
    // for the new position (Codex /gg 2026-05-24 WARNING — position-match
    // gating in the card is the back-stop). The right column shows ONE card
    // at a time (owner 2026-07-29), so a noise popup also closes any pin card.
    if (pos) {
      closeNoiseDetail()
      setSelectedStay(null)
      setSelectedProperty(null)
    }
    syncUrl({ detailPosition: pos })
  }, [syncUrl, closeNoiseDetail])

  const handleDetailData = useCallback((d: NoiseComputeData | null) => {
    setNoiseDetailData(d)
    if (d) {
      setNoiseDetailError(null)
    } else {
      // Backend returned valid-but-empty (no R4 coverage, all-silent point).
      // Surface as an error so the skeleton stops spinning forever —
      // Gemini /gg #82 WARNING. NoiseDetailContent's own
      // "No noise data computed" path renders only after `data` is set
      // with total_lden=null; an outright null payload bypasses it.
      setNoiseDetailError('No data available for this location')
    }
  }, [])

  const handleDetailError = useCallback((msg: string | null) => {
    setNoiseDetailError(msg)
  }, [])

  const handleNoiseClose = useCallback(() => {
    closeNoiseDetail()
    handleDetailPositionChange(null)
  }, [closeNoiseDetail, handleDetailPositionChange])

  const handleStayChange = useCallback((next: StayFilters) => {
    // Any filter change invalidates the open card — the pins it came from
    // (type, dates, price band…) are about to change under it.
    setSelectedStay(null)
    setStayFilters(next)
    stayFiltersRef.current = next
    syncUrl({ stay: stayParam(next) })
  }, [syncUrl])

  // ONE card at a time (owner 2026-07-29): picking a pin closes the noise
  // popup and any other pin card, and vice versa.
  const handleStaySelect = useCallback((s: Stay | null) => {
    setSelectedStay(s)
    if (s) {
      setSelectedProperty(null)
      closeNoiseDetail()
      handleDetailPositionChange(null)
    }
  }, [closeNoiseDetail, handleDetailPositionChange])

  const handlePropertySelect = useCallback((p: Property | null) => {
    setSelectedProperty(p)
    if (p) {
      setSelectedStay(null)
      closeNoiseDetail()
      handleDetailPositionChange(null)
    }
  }, [closeNoiseDetail, handleDetailPositionChange])

  const handleIsochronGo = useCallback(async (req: { lat: number; lng: number; time: number; modes: string[] }) => {
    // A previously drawn area goes first: the map must never show an older
    // request's polygon next to this request's error line.
    setIsochronGeojson(null)
    const url = `/api/isochron?lat=${req.lat}&lng=${req.lng}&time=${req.time}&modes=${req.modes.join(',')}`
    const res = await fetch(url).catch(() => null)
    if (res?.status === 429) throw new Error('Too many requests. Try again in a minute.')
    const geojson = res?.ok ? await res.json().catch(() => null) : null
    if (!geojson) throw new Error('Could not draw the area. The routing service is down.')
    setIsochronGeojson(geojson)
  }, [])

  const handleBasemapChange = useCallback((id: BasemapId) => {
    setBasemap(id)
    basemapRef.current = id
    syncUrl({ basemap: id })
  }, [syncUrl])

  return (
    <Tooltip.Provider>
    <div className="relative h-screen w-screen overflow-hidden">
      <SearchBar
        onSelect={setSelectedLocation}
        onIsochronToggle={() => setIsochronActive(prev => !prev)}
        isochronActive={isochronActive}
        onSearchInput={() => setIsochronActive(false)}
        mapCenter={mapViewRef.current}
      />

      <IsochronPanel
        location={selectedLocation ? { lat: Number(selectedLocation.lat), lng: Number(selectedLocation.lon) } : null}
        onGo={handleIsochronGo}
        active={isochronActive}
      />

      {/* UI overlays */}
      <div className="absolute inset-0 z-[1002] pointer-events-none">
        {/* bottom-14 + clip: the card column must never run under the About
            footer chip (owner report 2026-07-10); DetailCard scrolls its own
            content, anything beyond the cap is clipped, not overlaid. */}
        <div className="hidden md:flex absolute top-3 right-3 bottom-14 flex-col gap-2 w-[320px] overflow-hidden">
          <div className="pointer-events-auto">
            <ControlCard
              quietClustersEnabled={quietClustersEnabled}
              onQuietClustersChange={handleQuietClustersChange}
              quietThreshold={quietThreshold}
              onQuietThresholdChange={handleQuietThresholdChange}
              realEstateFilters={realEstateFilters}
              onRealEstateChange={setRealEstateFilters}
              stayFilters={stayFilters}
              onStayChange={handleStayChange}
              rasterOverlays={rasterOverlays}
              onRasterOverlayChange={handleRasterOverlaysChange}
            />
          </div>
          {validationEnabled && (
            <div className="pointer-events-auto">
              <ValidationStatusCard payload={validationPayload} />
            </div>
          )}
          {validationSelection && (
            <div className="pointer-events-auto">
              <ValidationCard
                selection={validationSelection}
                onClose={() => setValidationSelection(null)}
              />
            </div>
          )}
          <div className="pointer-events-auto">
            <DetailCard
              noiseData={noiseDetailData}
              position={detailPosition}
              error={noiseDetailError}
              onNoiseClose={handleNoiseClose}
              onHighlight={setHighlightGeometry}
            />
          </div>
          {selectedProperty && (
            <div className="pointer-events-auto">
              <FloatingCard>
                <PropertyCard
                  property={selectedProperty}
                  onClose={() => setSelectedProperty(null)}
                />
              </FloatingCard>
            </div>
          )}
          {selectedStay && (
            <div className="pointer-events-auto">
              <FloatingCard>
                <StayCard
                  stay={selectedStay}
                  onClose={() => setSelectedStay(null)}
                />
              </FloatingCard>
            </div>
          )}
        </div>

        <div className="pointer-events-auto">
          <BasemapBar
            basemap={basemap}
            onBasemapChange={handleBasemapChange}
            onLocate={handleLocate}
            locating={geolocateActive}
            locateReady={geolocateReady}
          />
        </div>
      </div>

      {initialView === null ? (
        <div className="h-full w-full bg-[#fafaf8]" />
      ) : (
      <MapView
        selectedLocation={selectedLocation}
        initialCenter={[initialView.lat, initialView.lng]}
        initialZoom={initialView.zoom}
        basemap={basemap}
        isochronGeojson={isochronGeojson}
        onViewChange={handleViewChange}
        onDetailData={handleDetailData}
        onDetailPositionChange={handleDetailPositionChange}
        onDetailError={handleDetailError}
        detailPosition={detailPosition}
        quietClustersEnabled={quietClustersEnabled}
        quietThreshold={quietThreshold}
        highlightGeometry={highlightGeometry}
        realEstateFilters={realEstateFilters}
        onPropertySelect={handlePropertySelect}
        stayFilters={stayFilters}
        onStaySelect={handleStaySelect}
        rasterOverlays={rasterOverlays}
        validationEnabled={validationEnabled}
        validationPayload={validationPayload}
        onValidationSelect={setValidationSelection}
        registerGeolocateTrigger={registerGeolocateTrigger}
        onGeolocateActiveChange={setGeolocateActive}
        onGeolocateReadyChange={setGeolocateReady}
      />
      )}

      {/* Mobile: layers toggle button */}
      {!layersOpen && (
        <button
          onClick={() => { setLayersOpen(true); setNoiseDetailData(null); handleDetailPositionChange(null) }}
          className="fixed bottom-[16px] right-[10px] z-[1003] flex h-11 w-11 items-center justify-center rounded-lg bg-white md:hidden"
          style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.1)' }}
          aria-label="Toggle layers panel"
        >
          <svg xmlns="http://www.w3.org/2000/svg" className="h-4 w-4 text-muted-foreground" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polygon points="12 2 2 7 12 12 22 7 12 2" />
            <polyline points="2 17 12 22 22 17" />
            <polyline points="2 12 12 17 22 12" />
          </svg>
        </button>
      )}

      {/* Mobile: layers panel (bottom sheet) */}
      <div className="md:hidden">
        <LayersPanel
          open={layersOpen}
          onClose={() => setLayersOpen(false)}
          quietClustersEnabled={quietClustersEnabled}
          onQuietClustersChange={handleQuietClustersChange}
          quietThreshold={quietThreshold}
          onQuietThresholdChange={handleQuietThresholdChange}
          realEstateFilters={realEstateFilters}
          onRealEstateChange={setRealEstateFilters}
          stayFilters={stayFilters}
          onStayChange={handleStayChange}
          rasterOverlays={rasterOverlays}
          onRasterOverlayChange={handleRasterOverlaysChange}
        />
      </div>

      {/* Mobile: detail sheet */}
      <a
        href="/about"
        className="hidden md:block fixed bottom-2 right-2 z-[1003] rounded-md border border-black/5 bg-white/95 px-2 py-0.5 text-xs text-muted-foreground shadow-lg backdrop-blur-md hover:text-foreground"
      >
        About quietmap.org
      </a>

      <MobileDetailSheet
        data={noiseDetailData}
        position={detailPosition}
        error={noiseDetailError}
        onClose={() => { setNoiseDetailData(null); setNoiseDetailError(null); handleDetailPositionChange(null) }}
        onHighlight={setHighlightGeometry}
      />

      {/* Mobile: stay card as a bottom sheet — the desktop right column that
          hosts it is hidden below md, and a guarded pin tap must never end
          in nothing. */}
      {selectedStay && (
        <div className="fixed bottom-0 left-0 right-0 z-[2000] md:hidden">
          <div className="bg-background rounded-t-xl shadow-2xl">
            <StayCard
              stay={selectedStay}
              onClose={() => setSelectedStay(null)}
            />
          </div>
        </div>
      )}
    </div>
    </Tooltip.Provider>
  )
}

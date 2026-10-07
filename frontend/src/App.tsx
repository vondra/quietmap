// The map application: search, layer controls, the map, and the popup card or sheet, with every
// piece of state mirrored into the shareable URL hash.
import { useState, useCallback, useRef, useEffect } from 'react'
import MapView from './components/MapView'
import SearchBar from './components/SearchBar'
import ControlCard from './components/ControlCard'
import DetailCard from './components/DetailCard'
import LayersPanel from './components/LayersPanel'
import MobileDetailSheet from './components/MobileDetailSheet'
import BasemapBar from './components/BasemapBar'
import { sameDetailPosition, useUrlState, QUIET_THRESHOLD_DEFAULT, type UrlState } from './hooks/useUrlState'
import type { SelectedLocation } from './components/FlyToLocation'
import type { PopupUpdate, SegmentFan } from './types/noise'
import { topFlightKey } from './components/noise/top-flights'
import { DEFAULT_BASEMAP, type BasemapId } from './utils/basemaps'
import { setDocumentTitle } from './utils/page-title'
import RecentPlaces from './components/RecentPlaces'
import { loadRecentPlaces, saveRecentPlaces, withPlace, withoutPlace, type RecentPlace } from './lib/recent-places'
import { useIsDesktop } from './hooks/useIsDesktop'


export default function App() {
  const { initial, updateUrl } = useUrlState()

  const [selectedLocation, setSelectedLocation] = useState<SelectedLocation | null>(null)
  const [layersOpen, setLayersOpen] = useState(false)
  const [detailPosition, setDetailPosition] = useState<{ lat: number; lng: number } | null>(initial.detailPosition)
  const activeDetailPosition = useRef(detailPosition)
  const isCurrentDetailPosition = useCallback((position: { lat: number; lng: number }) =>
    activeDetailPosition.current === position, [])
  // The latest streamed update of the open point (each replaces the previous one); the card opens
  // with a skeleton at the click and keeps the position on an error, so the visitor sees where.
  const [noiseDetailData, setNoiseDetailData] = useState<PopupUpdate | null>(null)
  const [noiseDetailError, setNoiseDetailError] = useState<string | null>(null)
  // What the map highlights: a loudest flight's track (its row hovered, or tapped on a phone), by
  // key, or a contributor's lines (its row opened), by `source:<id>`; read from the latest
  // update, so it follows the stream and goes when it leaves the list.
  const [highlighted, setHighlighted] = useState<string | null>(null)
  // The detailed calculation's pieces and rays on the map, while it is open.
  const [fan, setFan] = useState<SegmentFan | null>(null)
  // The detailed calculation, open under the popup's list.
  const [calculationOpen, setCalculationOpen] = useState(false)
  const toggleCalculation = useCallback(() => setCalculationOpen(open => !open), [])
  const desktop = useIsDesktop()
  // The last places opened, newest first, kept in this browser.
  const [recentPlaces, setRecentPlaces] = useState<RecentPlace[]>(loadRecentPlaces)
  const highlightedTrack = highlighted?.startsWith('source:')
    ? noiseDetailData?.top_contributors.find(c => `source:${c.id}` === highlighted)?.geometry ?? null
    : noiseDetailData?.top_flights.find(f => topFlightKey(f) === highlighted)?.track ?? null
  const [quietClustersEnabled, setQuietClustersEnabled] = useState(initial.quietClusters)
  const [quietThreshold, setQuietThreshold] = useState(initial.quietThreshold ?? QUIET_THRESHOLD_DEFAULT)
  const [basemap, setBasemap] = useState<BasemapId>(initial.basemap ?? DEFAULT_BASEMAP)
  // Mobile locate box lives in the BasemapBar row (one container = one
  // baseline); it fires the map's GeolocateControl through this ref.
  const geolocateTrigger = useRef<() => void>(() => {})
  const [geolocateActive, setGeolocateActive] = useState(false)
  const [geolocateReady, setGeolocateReady] = useState(false)
  const registerGeolocateTrigger = useCallback((trigger: () => void) => {
    geolocateTrigger.current = trigger
  }, [])
  const handleLocate = useCallback(() => geolocateTrigger.current(), [])
  const [heatmapLayers, setHeatmapLayers] = useState<Record<string, boolean>>(initial.heatmapLayers)

  // Refs are the post-event truth for the URL: each handler updates its ref before syncUrl.
  const mapViewRef = useRef({ lat: initial.lat, lng: initial.lng, zoom: initial.zoom })
  const getMapView = useCallback(() => mapViewRef.current, [])
  // A pan must keep `d=` in the shared URL while the card is open.
  const detailPositionRef = useRef(detailPosition)
  detailPositionRef.current = detailPosition
  const quietClustersRef = useRef(quietClustersEnabled)
  const quietThresholdRef = useRef(quietThreshold)
  const basemapRef = useRef(basemap)
  const heatmapLayersRef = useRef(heatmapLayers)

  // Pre-warm the lazy popup-body chunk the instant a point is clicked, so it
  // downloads while the first answer is computed instead of after it.
  useEffect(() => {
    if (detailPosition) void import('./components/NoiseDetailContent')
  }, [detailPosition])

  // Tab/share title tracks the open popup: reverse-geocode the position
  // (place-level, server-cached) and compose "Dejvice, Praha - 62 dB -
  // quietmap.org" — place first, never the number.
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

  // An answered point joins the recent places (again first), with its name once it is known.
  useEffect(() => {
    if (!detailPosition || !noiseDetailData || noiseDetailData.partial) return
    const entry: RecentPlace = {
      lat: detailPosition.lat,
      lng: detailPosition.lng,
      place: detailPlaceName,
      sone: noiseDetailData.loudness?.n5_den_sone ?? null,
      lden: noiseDetailData.total_lden,
    }
    setRecentPlaces(places => {
      const next = withPlace(places, entry)
      saveRecentPlaces(next)
      return next
    })
  }, [detailPosition, noiseDetailData, detailPlaceName])
  const forgetPlace = useCallback((place: RecentPlace) => {
    setRecentPlaces(places => {
      const next = withoutPlace(places, place)
      saveRecentPlaces(next)
      return next
    })
  }, [])
  // A tab flies to its place and opens its popup, as a search result does.
  const openPlace = useCallback((place: RecentPlace) => {
    setSelectedLocation({ display_name: place.place ?? '', lat: place.lat, lon: place.lng })
  }, [])

  const detailTotalLden = noiseDetailData?.total_lden ?? null
  useEffect(() => {
    setDocumentTitle(detailPosition
      ? [detailPlaceName, detailTotalLden != null ? `${Math.round(detailTotalLden)} dB` : null]
      : [])
  }, [detailPosition, detailPlaceName, detailTotalLden])

  const syncUrl = useCallback((overrides?: Partial<Omit<UrlState, 'hasExplicitView'>>) => {
    // Refs are the post-event truth; an override carries a value the state
    // has not committed yet (an explicit null included).
    updateUrl({
      ...mapViewRef.current,
      quietClusters: quietClustersRef.current,
      quietThreshold: quietThresholdRef.current,
      detailPosition: detailPositionRef.current,
      basemap: basemapRef.current,
      heatmapLayers: heatmapLayersRef.current,
      ...overrides,
    })
  }, [updateUrl])

  const handleHeatmapLayersChange = useCallback((next: Record<string, boolean>) => {
    setHeatmapLayers(next)
    heatmapLayersRef.current = next
    syncUrl({ heatmapLayers: next })
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

  const closeNoiseDetail = useCallback(() => {
    setNoiseDetailData(null)
    setNoiseDetailError(null)
    setHighlighted(null)
    setFan(null)
  }, [])
  // The calculation goes with the popup; a new point keeps it open for that point.
  useEffect(() => {
    if (!detailPosition) setCalculationOpen(false)
  }, [detailPosition])

  const handleDetailPositionChange = useCallback((pos: { lat: number; lng: number } | null) => {
    activeDetailPosition.current = pos
    detailPositionRef.current = pos
    setDetailPosition(pos)
    // Fresh click: clear the previous point's answer, error and flight so the new skeleton renders.
    closeNoiseDetail()
    syncUrl({ detailPosition: pos })
  }, [syncUrl, closeNoiseDetail])

  const handleDetailData = useCallback((update: PopupUpdate) => {
    setNoiseDetailData(update)
    setNoiseDetailError(null)
  }, [])

  // An error replaces whatever the point showed: a partial answer followed by an error is
  // incomplete and must not stay on screen as if it were the level.
  const handleDetailError = useCallback((message: string) => {
    setNoiseDetailData(null)
    setNoiseDetailError(message)
  }, [])

  const handleNoiseClose = useCallback(() => {
    handleDetailPositionChange(null)
  }, [handleDetailPositionChange])

  const handleBasemapChange = useCallback((id: BasemapId) => {
    setBasemap(id)
    basemapRef.current = id
    syncUrl({ basemap: id })
  }, [syncUrl])

  // A hash change in the open tab (pasted link, history step): MapStateSync
  // moves the map and hands over the parsed hash; every other token is
  // applied here through the same handlers a click would use. The detail is
  // re-requested only when its point changed.
  const handleHashState = useCallback((next: UrlState) => {
    handleQuietClustersChange(next.quietClusters)
    handleQuietThresholdChange(next.quietThreshold)
    handleHeatmapLayersChange(next.heatmapLayers)
    handleBasemapChange(next.basemap)
    if (sameDetailPosition(detailPositionRef.current, next.detailPosition)) return
    handleDetailPositionChange(next.detailPosition)
  }, [handleQuietClustersChange, handleQuietThresholdChange, handleHeatmapLayersChange, handleBasemapChange, handleDetailPositionChange])

  return (
    <div className="relative h-screen w-screen overflow-hidden">
      <SearchBar onSelect={setSelectedLocation} getMapView={getMapView} />

      {/* UI overlays */}
      <div className="absolute inset-0 z-[1002] pointer-events-none">
        {/* The layers card and the detail card are shrinkable flex items (min-h-0)
            that scroll their own content, so on a short viewport both stay reachable. */}
        <div className="hidden md:flex absolute top-3 right-(--map-gutter) bottom-3 flex-col gap-2 w-(--map-card-column) overflow-hidden">
          <ControlCard
            popupOpen={detailPosition != null}
            quietClustersEnabled={quietClustersEnabled}
            onQuietClustersChange={handleQuietClustersChange}
            quietThreshold={quietThreshold}
            onQuietThresholdChange={handleQuietThresholdChange}
            heatmapLayers={heatmapLayers}
            onHeatmapLayersChange={handleHeatmapLayersChange}
          />
          {detailPosition && (
            <RecentPlaces places={recentPlaces} current={detailPosition} onOpen={openPlace} onRemove={forgetPlace} />
          )}
          <DetailCard
            noiseData={noiseDetailData}
            position={detailPosition}
            error={noiseDetailError}
            onNoiseClose={handleNoiseClose}
            onHighlight={setHighlighted}
            calculationOpen={calculationOpen && desktop}
            onCalculationToggle={toggleCalculation}
            onFan={setFan}
          />
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

      <MapView
        isCurrentDetailPosition={isCurrentDetailPosition}
        selectedLocation={selectedLocation}
        initialCenter={[initial.lat, initial.lng]}
        initialZoom={initial.zoom}
        basemap={basemap}
        onViewChange={handleViewChange}
        onDetailData={handleDetailData}
        onDetailPositionChange={handleDetailPositionChange}
        onHashState={handleHashState}
        onDetailError={handleDetailError}
        detailPosition={detailPosition}
        flightTrack={highlightedTrack}
        segmentFan={fan}
        quietClustersEnabled={quietClustersEnabled}
        quietThreshold={quietThreshold}
        heatmapLayers={heatmapLayers}
        registerGeolocateTrigger={registerGeolocateTrigger}
        onGeolocateActiveChange={setGeolocateActive}
        onGeolocateReadyChange={setGeolocateReady}
      />

      {/* Mobile: layers toggle button */}
      {!layersOpen && (
        <button
          onClick={() => { setLayersOpen(true); handleNoiseClose() }}
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
          heatmapLayers={heatmapLayers}
          onHeatmapLayersChange={handleHeatmapLayersChange}
        />
      </div>

      {/* Mobile: detail sheet */}
      <MobileDetailSheet
        data={noiseDetailData}
        position={detailPosition}
        error={noiseDetailError}
        onClose={handleNoiseClose}
        onHighlight={setHighlighted}
        calculationOpen={calculationOpen && !desktop}
        onCalculationToggle={toggleCalculation}
        onFan={setFan}
        recentPlaces={<RecentPlaces places={recentPlaces} current={detailPosition} onOpen={openPlace} onRemove={forgetPlace} />}
      />

    </div>
  )
}

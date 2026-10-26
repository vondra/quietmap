// The map application: search, layer controls, the map, and the popup card or sheet, with every
// piece of state mirrored into the shareable URL hash.
import { useState, useCallback, useRef, useEffect, useMemo } from 'react'
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
import { contributorHighlight, highlightGeoJson, topFlightKey } from './components/noise/top-flights'
import { DEFAULT_BASEMAP, type BasemapId } from './utils/basemaps'
import { setDocumentTitle } from './utils/page-title'
import AccountButton from './components/AccountButton'
import PlaceMedia from './components/PlaceMedia'
import PlaceTabs from './components/PlaceTabs'
import { samePlace, titleAt, withPlace, type RecentPlace } from './lib/place-rules'
import { usePlaces } from './hooks/usePlaces'
import { createMediaKeeper } from './lib/media-keeper'
import { isSignedIn } from './lib/places-keeper'
import { startRecording } from './lib/recorder'
import { useIsDesktop } from './hooks/useIsDesktop'
import type { DataLayerId } from './lib/data-layers'
import { defaultStaySearch, freshSearch, type Stay, type StaySearch } from './lib/stays'


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
  // An opened row's segments and rays on the map, while its segments are open: the row that drew
  // them last; closing another row's leaves them.
  const [fan, setFan] = useState<SegmentFan | null>(null)
  const showFan = useCallback((source: string, next: SegmentFan | null) =>
    setFan(current => next ?? (current?.source === source ? null : current)), [])
  // The detailed calculation, open under the popup's list.
  const [calculationOpen, setCalculationOpen] = useState(false)
  const toggleCalculation = useCallback(() => setCalculationOpen(open => !open), [])
  const desktop = useIsDesktop()
  // The last places opened, newest first, and the names given places: this browser's, or the
  // signed-in visitor's account's.
  const {
    account, recent: recentPlaces, titles: placeTitles, notSaved, visit, nameLate, forget: forgetPlace, rename, signOut, deleteAccount,
  } = usePlaces()
  const highlightedSource = highlighted?.startsWith('source:')
    ? noiseDetailData?.top_contributors.find(c => `source:${c.id}` === highlighted)
    : undefined
  const highlightedTrack = noiseDetailData?.top_flights.find(f => topFlightKey(f) === highlighted)?.track
  const highlight = highlightedSource?.geometry
    ? contributorHighlight(highlightedSource)
    : highlightedTrack ? highlightGeoJson(highlightedTrack) : null
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
  const [dataLayers, setDataLayers] = useState<DataLayerId[]>(initial.dataLayers)
  const [staysOn, setStaysOn] = useState(initial.stays)
  const [staySearch, setStaySearch] = useState<StaySearch>(() => defaultStaySearch())
  // The place to stay whose pin opened the popup: shown above the popup until the popup moves.
  const [selectedStay, setSelectedStay] = useState<Stay | null>(null)

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
  const dataLayersRef = useRef(dataLayers)
  const staysRef = useRef(staysOn)

  // Pre-warm the lazy popup-body chunk the instant a point is clicked, so it
  // downloads while the first answer is computed instead of after it.
  useEffect(() => {
    if (detailPosition) void import('./components/NoiseDetailContent')
  }, [detailPosition])

  // Tab/share title tracks the open popup: reverse-geocode the position
  // (place-level, server-cached) and compose "Camden, London - 62 dB -
  // quietmap.org" — place first, never the number. The lookup is not cancelled
  // by the next click: a name that comes back late still names its recent tab. The name is kept
  // with the point it is for, so a moved popup never shows the previous place, not for a frame.
  const [geocoded, setGeocoded] = useState<{ lat: number, lng: number, place: string } | null>(null)
  const detailPlaceName = geocoded && detailPosition
    && geocoded.lat === detailPosition.lat && geocoded.lng === detailPosition.lng ? geocoded.place : null
  useEffect(() => {
    if (!detailPosition) return
    const position = detailPosition
    let open = true
    fetch(`/api/reverse?lat=${position.lat}&lon=${position.lng}`)
      .then(res => (res.ok ? res.json() : null))
      .then(json => {
        const place = typeof json?.place === 'string' && json.place ? json.place : null
        if (!place) return
        if (open) setGeocoded({ lat: position.lat, lng: position.lng, place })
        nameLate(position, place)
      })
      .catch(() => {})
    return () => { open = false }
  }, [detailPosition, nameLate])

  // An answered point joins the recent places (first, or in its tab), with its name once it is known.
  useEffect(() => {
    if (!detailPosition || !noiseDetailData || noiseDetailData.partial) return
    const entry: RecentPlace = {
      lat: detailPosition.lat,
      lng: detailPosition.lng,
      place: detailPlaceName,
      sone: noiseDetailData.loudness?.nden_sone ?? null,
      lden: noiseDetailData.total_lden,
    }
    visit(entry)
  }, [detailPosition, noiseDetailData, detailPlaceName, visit])
  // The open place's own name, given or taken back in its tab.
  const detailTitle = detailPosition ? titleAt(placeTitles, detailPosition) : null
  // The account's form at the top right, opened there or from the tabs.
  const [accountOpen, setAccountOpen] = useState(false)
  // The signed-in visitor's recordings and photos of the open place.
  const [media] = useState(() => createMediaKeeper(startRecording))
  useEffect(() => media.open(isSignedIn(account) ? account.email : null, detailPosition), [media, account, detailPosition])

  const detailTotalLden = noiseDetailData?.total_lden ?? null
  useEffect(() => {
    setDocumentTitle(detailPosition
      ? [detailTitle ?? detailPlaceName, detailTotalLden != null ? `${Math.round(detailTotalLden)} dB` : null]
      : [])
  }, [detailPosition, detailTitle, detailPlaceName, detailTotalLden])

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
      dataLayers: dataLayersRef.current,
      stays: staysRef.current,
      ...overrides,
    })
  }, [updateUrl])

  const handleHeatmapLayersChange = useCallback((next: Record<string, boolean>) => {
    setHeatmapLayers(next)
    heatmapLayersRef.current = next
    syncUrl({ heatmapLayers: next })
  }, [syncUrl])

  const handleDataLayersChange = useCallback((next: DataLayerId[]) => {
    setDataLayers(next)
    dataLayersRef.current = next
    syncUrl({ dataLayers: next })
  }, [syncUrl])

  const handleStaysChange = useCallback((on: boolean) => {
    setStaysOn(on)
    staysRef.current = on
    setSelectedStay(null)
    syncUrl({ stays: on })
  }, [syncUrl])

  // The place shown was priced for the stay searched before.
  const handleStaySearchChange = useCallback((search: StaySearch) => {
    setStaySearch(search)
    setSelectedStay(null)
  }, [])
  // Past midnight a search from the day before is one Stay22 no longer takes: it moves on as a change
  // of dates does, the open place with it; checked when the tab comes back, and every minute.
  const staySearchRef = useRef(staySearch)
  staySearchRef.current = staySearch
  useEffect(() => {
    const roll = () => {
      const fresh = freshSearch(staySearchRef.current)
      if (fresh !== staySearchRef.current) handleStaySearchChange(fresh)
    }
    const minute = setInterval(roll, 60_000)
    document.addEventListener('visibilitychange', roll)
    return () => {
      clearInterval(minute)
      document.removeEventListener('visibilitychange', roll)
    }
  }, [handleStaySearchChange])
  // The open place goes with its price, at the end of Stay22's lifetime.
  useEffect(() => {
    if (!selectedStay) return
    const timer = setTimeout(() => setSelectedStay(null), selectedStay.expires - Date.now())
    return () => clearTimeout(timer)
  }, [selectedStay])

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
    setSelectedStay(null)
    syncUrl({ detailPosition: pos })
  }, [syncUrl, closeNoiseDetail])

  // A place to stay opens the popup at its point, with the place above it: one card, not two.
  const handleStaySelect = useCallback((stay: Stay) => {
    handleDetailPositionChange({ lat: stay.lat, lng: stay.lng })
    setSelectedStay(stay)
  }, [handleDetailPositionChange])
  const stays = useMemo(() => (staysOn ? { search: staySearch, onSelect: handleStaySelect } : null),
    [staysOn, staySearch, handleStaySelect])

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
    handleDataLayersChange(next.dataLayers)
    handleBasemapChange(next.basemap)
    if (next.stays !== staysRef.current) handleStaysChange(next.stays)
    if (sameDetailPosition(detailPositionRef.current, next.detailPosition)) return
    handleDetailPositionChange(next.detailPosition)
  }, [handleQuietClustersChange, handleQuietThresholdChange, handleHeatmapLayersChange, handleDataLayersChange, handleBasemapChange, handleStaysChange, handleDetailPositionChange])
  // The open place is a tab from the click on, with its last numbers until its answer comes; the recent
  // places keep it once it is answered. A tab opens its place at once, the map moving only to keep it
  // in sight (DetailPopup).
  const opened = detailPosition && recentPlaces.find(place => samePlace(place, detailPosition))
  const tabs = detailPosition && (
    <PlaceTabs
      places={withPlace(recentPlaces, { ...detailPosition, place: detailPlaceName, sone: opened?.sone ?? null, lden: opened?.lden ?? null })}
      titles={placeTitles}
      current={detailPosition}
      notSaved={notSaved}
      onOpen={place => handleDetailPositionChange({ lat: place.lat, lng: place.lng })}
      onForget={place => {
        forgetPlace(place)
        if (samePlace(place, detailPosition)) handleNoiseClose()
      }}
      onRename={rename}
      onSignIn={account === null ? () => setAccountOpen(true) : undefined}
    />
  )
  // Made anew for each place under its key: siblings under one key once left a name field behind at
  // every new place.
  const placeMedia = detailPosition && <PlaceMedia key={`${detailPosition.lat},${detailPosition.lng}`} media={media} />
  const stayControls = { staysOn, onStaysChange: handleStaysChange, staySearch, onStaySearchChange: handleStaySearchChange }

  return (
    // The app is the box the fixed controls stand in: a phone browser's 100vh reaches under its
    // toolbar, which put the map's own corner controls (the ⓘ) below the control row.
    <div className="fixed inset-0 overflow-hidden">
      <SearchBar onSelect={setSelectedLocation} getMapView={getMapView} />
      <AccountButton account={account} onSignOut={signOut} onDeleteAccount={deleteAccount} open={accountOpen} onOpenChange={setAccountOpen} />

      {/* UI overlays */}
      <div className="absolute inset-0 z-[1002] pointer-events-none">
        {/* The layers card and the detail card are shrinkable flex items (min-h-0)
            that scroll their own content, so on a short viewport both stay reachable. The column
            starts under the account (36 px from 12 px) and ends over the map's corner, whose credits
            and ⓘ stay in sight with every card open. */}
        <div className="hidden md:flex absolute top-14 right-(--map-gutter) bottom-11 flex-col gap-2 w-(--map-card-column) overflow-hidden">
          <ControlCard
            popupOpen={detailPosition != null}
            quietClustersEnabled={quietClustersEnabled}
            onQuietClustersChange={handleQuietClustersChange}
            quietThreshold={quietThreshold}
            onQuietThresholdChange={handleQuietThresholdChange}
            heatmapLayers={heatmapLayers}
            onHeatmapLayersChange={handleHeatmapLayersChange}
            dataLayers={dataLayers}
            onDataLayersChange={handleDataLayersChange}
            {...stayControls}
          />
          <DetailCard
            noiseData={noiseDetailData}
            position={detailPosition}
            error={noiseDetailError}
            onNoiseClose={handleNoiseClose}
            onHighlight={setHighlighted}
            calculationOpen={calculationOpen && desktop}
            onCalculationToggle={toggleCalculation}
            onFan={showFan}
            stay={selectedStay}
            tabs={tabs}
            placeMedia={placeMedia}
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
        highlight={highlight}
        segmentFan={fan}
        quietClustersEnabled={quietClustersEnabled}
        quietThreshold={quietThreshold}
        heatmapLayers={heatmapLayers}
        dataLayers={dataLayers}
        stays={stays}
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
          dataLayers={dataLayers}
          onDataLayersChange={handleDataLayersChange}
          {...stayControls}
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
        onFan={showFan}
        tabs={tabs}
        stay={selectedStay}
        placeMedia={placeMedia}
      />

    </div>
  )
}

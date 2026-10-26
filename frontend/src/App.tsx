// The map application: search, layer controls, the map, and the popup card or sheet, with every
// piece of state mirrored into the shareable URL hash.
import { useState, useCallback, useRef, useEffect, useMemo } from 'react'
import MapView from './components/MapView'
import SearchBar from './components/SearchBar'
import ControlCard, { type CardTab } from './components/ControlCard'
import IsochronPanel, { type IsochronRequest } from './components/IsochronPanel'
import { ICON_BUTTON } from './components/ui/button'
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
import PlaceMedia, { MediaButtons } from './components/PlaceMedia'
import { samePlace, titleAt } from './lib/place-rules'
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
  // A search's result on its way: its name, for the history when its popup opens there.
  const searchedName = useRef<{ at: { lat: number; lng: number }; name: string } | null>(null)
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
  // The history of places opened, in its order, and the names given places: this browser's, or the
  // signed-in visitor's account's.
  const {
    account, recent: recentPlaces, titles: placeTitles, notSaved, open: countOpening, answered, nameLate, forget: forgetPlace,
    rename, signOut, deleteAccount,
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

  // An answered point's numbers join its place in the history, with its name once it is known.
  useEffect(() => {
    if (!detailPosition || !noiseDetailData || noiseDetailData.partial) return
    answered({
      lat: detailPosition.lat,
      lng: detailPosition.lng,
      place: detailPlaceName,
      sone: noiseDetailData.loudness?.nden_sone ?? null,
      lden: noiseDetailData.total_lden,
    })
  }, [detailPosition, noiseDetailData, detailPlaceName, answered])
  // The open place's own name, given or taken back under the search field, and its entry of the history,
  // which knows its name before the geocoder answers (a search's result, a place opened before).
  const detailTitle = detailPosition ? titleAt(placeTitles, detailPosition) : null
  const openEntry = detailPosition && recentPlaces.find(place => samePlace(place, detailPosition))
  // A popup a link opened counts as an opening, once.
  const linkCounted = useRef(false)
  useEffect(() => {
    if (linkCounted.current || !initial.detailPosition) return
    linkCounted.current = true
    countOpening(initial.detailPosition)
  }, [initial.detailPosition, countOpening])
  // The phone's account under its button at the top right; the desktop's corner card, its layers or
  // its account, folded while a popup is open (the popup has the column) and on its layers again when
  // it closes. Both open from the history's sign-in line too.
  const [accountOpen, setAccountOpen] = useState(false)
  const popupOpen = detailPosition != null
  const [cardTab, setCardTab] = useState<CardTab | null>(popupOpen ? null : 'layers')
  useEffect(() => setCardTab(popupOpen ? null : 'layers'), [popupOpen])
  // The area reachable from the open place: whether its panel is open, and the area drawn for a point,
  // shown while that point is open. A new request drops the one before, whose answer would otherwise
  // land after it.
  const [isochronOpen, setIsochronOpen] = useState(false)
  const [isochron, setIsochron] = useState<{ at: { lat: number; lng: number }; area: GeoJSON.FeatureCollection } | null>(null)
  const isochronRequest = useRef<AbortController | null>(null)
  const showIsochron = useCallback(async ({ lat, lng, minutes, modes }: IsochronRequest) => {
    isochronRequest.current?.abort()
    const request = new AbortController()
    isochronRequest.current = request
    setIsochron(null)
    const url = `/api/isochron?lat=${lat}&lng=${lng}&minutes=${minutes}&modes=${modes.join(',')}`
    const response = await fetch(url, { signal: request.signal }).catch(() => null)
    const answer = await response?.json().catch(() => null) as GeoJSON.FeatureCollection & { error?: string } | null
    if (request.signal.aborted) return
    if (response?.status === 429) throw new Error('Too many areas asked from here. Try again in a minute.')
    if (!response?.ok || !answer?.features) throw new Error(answer?.error ?? 'The map could not be reached. Try again in a minute.')
    setIsochron({ at: { lat, lng }, area: answer })
    const missing = modes.find(mode => !answer.features.some(feature => feature.properties?.mode === mode))
    if (missing) throw new Error(`The area ${missing === 'car' ? 'by car' : 'on foot'} could not be drawn. Try again in a minute.`)
  }, [])
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
    // Each opening counts in the history; a search's result keeps its name there.
    const searched = searchedName.current
    searchedName.current = null
    if (pos) countOpening(pos, searched && samePlace(searched.at, pos) ? searched.name : null)
  }, [syncUrl, closeNoiseDetail, countOpening])

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
  // The area reachable from the open place: its button at the search field's end, where dev1 had it,
  // its panel under the place's row.
  const isochronButton = (
    <button
      type="button"
      aria-label="Reachable area"
      aria-expanded={isochronOpen}
      title="The area reachable from here"
      className={`${ICON_BUTTON} aria-expanded:bg-accent aria-expanded:text-foreground`}
      onClick={() => setIsochronOpen(open => !open)}
    >
      {/* The place and the rings reached from it (dev1's). */}
      <svg viewBox="0 0 24 24" className="size-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
        <circle cx="12" cy="12" r="2.5" />
        <circle cx="12" cy="12" r="6.5" strokeDasharray="4 3" />
        <circle cx="12" cy="12" r="10.5" strokeDasharray="4 5" />
      </svg>
    </button>
  )
  const isochronPanel = isochronOpen && detailPosition && <IsochronPanel at={detailPosition} onShow={showIsochron} />
  const stayControls = { staysOn, onStaysChange: handleStaysChange, staySearch, onStaySearchChange: handleStaySearchChange }

  return (
    // The app is the box the fixed controls stand in: a phone browser's 100vh reaches under its
    // toolbar, which put the map's own corner controls (the ⓘ) below the control row.
    <div className="fixed inset-0 overflow-hidden">
      <SearchBar
        onSelect={result => {
          searchedName.current = {
            at: { lat: result.lat, lng: result.lon },
            name: result.secondary ? `${result.display_name}, ${result.secondary}` : result.display_name,
          }
          setSelectedLocation(result)
        }}
        getMapView={getMapView}
        history={recentPlaces}
        titles={placeTitles}
        open={detailPosition && {
          at: detailPosition,
          place: openEntry?.place ?? detailPlaceName,
          sone: noiseDetailData?.loudness?.nden_sone ?? openEntry?.sone ?? null,
        }}
        notSaved={notSaved}
        // A place of the history opens at once, the map moving only to keep it in sight (DetailPopup).
        onOpenPlace={place => handleDetailPositionChange({ lat: place.lat, lng: place.lng })}
        onRename={rename}
        onForget={forgetPlace}
        onSignIn={account === null ? () => (desktop ? setCardTab('account') : setAccountOpen(true)) : undefined}
        // The microphone and the camera where a visitor records, on a touch screen, by the place's name;
        // under the row the take, the how-to before it, and the place's recordings and photos. Made anew
        // for each place and account under its key (what was open belongs to them): siblings under one
        // key once left a name field behind.
        openAction={<MediaButtons media={media} />}
        fieldAction={isochronButton}
      >
        {isochronPanel}
        {detailPosition && <PlaceMedia key={`${isSignedIn(account) ? account.email : ''}@${detailPosition.lat},${detailPosition.lng}`} media={media} />}
      </SearchBar>
      {/* The phone's account beside the search field (a desktop has it as the corner card's tab). */}
      <div className="fixed top-[calc(env(safe-area-inset-top,0px)+0.75rem)] right-(--map-gutter) z-[2004] md:hidden">
        <AccountButton account={account} onSignOut={signOut} onDeleteAccount={deleteAccount} open={accountOpen} onOpenChange={setAccountOpen} />
      </div>

      {/* UI overlays */}
      <div className="absolute inset-0 z-[1002] pointer-events-none">
        {/* The layers card and the detail card are shrinkable flex items (min-h-0)
            that scroll their own content, so on a short viewport both stay reachable. The column ends
            over the map's corner, whose credits and ⓘ stay in sight with every card open. */}
        <div className="hidden md:flex absolute top-3 right-(--map-gutter) bottom-11 flex-col gap-2 w-(--map-card-column) overflow-hidden">
          <ControlCard
            tab={cardTab}
            onTabChange={setCardTab}
            account={account}
            onSignOut={signOut}
            onDeleteAccount={deleteAccount}
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
        isochron={isochron && sameDetailPosition(isochron.at, detailPosition) ? isochron.area : null}
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
          className="fixed bottom-[16px] right-[10px] z-[1003] flex h-11 w-11 items-center justify-center rounded-lg bg-background md:hidden"
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
        stay={selectedStay}
      />

    </div>
  )
}

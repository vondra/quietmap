// The map application: search, layer controls, the map, and the popup card or sheet, with every
// piece of state mirrored into the shareable URL hash.
import { useState, useCallback, useRef, useEffect, useMemo } from 'react'
import MapView from './components/MapView'
import SearchBar from './components/SearchBar'
import ControlCard from './components/ControlCard'
import IsochronPanel from './components/IsochronPanel'
import { ICON_BUTTON } from './components/ui/button'
import DetailCard from './components/DetailCard'
import LayersPanel from './components/LayersPanel'
import MobileDetailSheet from './components/MobileDetailSheet'
import BasemapBar from './components/BasemapBar'
import { useUrlState } from './hooks/useUrlState'
import { QUIET_THRESHOLD_DEFAULT, sameDetailPosition } from './lib/url-state'
import type { Metric } from './lib/hm3-decoder'
import type { SelectedLocation } from './components/FlyToLocation'
import { SEARCH_FLIGHT_ZOOM } from './lib/search-flight'
import type { PopupUpdate, SegmentFan } from './types/noise'
import { contributorHighlight, highlightGeoJson, topFlightKey } from './components/noise/top-flights'
import { type BasemapId } from './utils/basemaps'
import { setDocumentTitle } from './utils/page-title'
import AccountButton from './components/AccountButton'
import PlaceMedia, { MediaButtons } from './components/PlaceMedia'
import { samePlace, titleAt } from './lib/place-rules'
import { usePlaces } from './hooks/usePlaces'
import { createMediaKeeper } from './lib/media-keeper'
import { isSignedIn } from './lib/places-keeper'
import { startRecording } from './lib/recorder'
import { DESKTOP_QUERY, useIsDesktop } from './hooks/useIsDesktop'
import { useSourceOutline } from './hooks/useSourceOutline'
import type { DataLayerId } from './lib/data-layers'
import { firstCheckin, type Stay, type StaySearch } from './lib/stays'

import { PopupStateContext } from './hooks/usePopupState'
import { useReachableArea } from './hooks/useReachableArea'
import { useSelectedStay } from './hooks/useSelectedStay'
import type { MapCamera, Position, ReachRequest } from './lib/url-state'

export default function App() {
  const { state, updateUrl, getShareUrl } = useUrlState()
  const { detailPosition, metric, quietThreshold, basemap, heatmapLayers, dataLayers,
    stays: staysOn, quietClusters: quietClustersEnabled, staySearch, stayMaxPrice,
    controls: cardTab, calculation: calculationOpen, isochronOpen } = state
  const layersOpen = cardTab === 'layers'
  const setCardTab = (controls: typeof cardTab) => updateUrl({ controls })
  const setAccountOpen = (open: boolean) => updateUrl({ controls: open ? 'account' : null })
  const setLayersOpen = (open: boolean) => updateUrl({ controls: open ? 'layers' : null })
  const toggleCalculation = () => updateUrl(s => ({ calculation: !s.calculation }))
  const [staySeed, setStaySeed] = useState<{ stay: Stay; search: StaySearch } | null>(null)
  const { stay: selectedStay, message: stayMessage } = useSelectedStay(state.stayId, detailPosition, staySearch, staySeed)
  const isochron = useReachableArea(state.reach)
  const isochronBusy = useRef(isochron.busy)
  isochronBusy.current = isochron.busy
  const showIsochron = (reach: ReachRequest) => { isochron.fitNext.current = true; updateUrl({ reach }) }
  const clearIsochron = () => updateUrl({ reach: null })
  const latestState = useRef(state)
  latestState.current = state

  const [selectedLocation, setSelectedLocation] = useState<SelectedLocation | null>(null)
  // A search's result on its way: its name, for the history when its popup opens there.
  const searchedName = useRef<{ at: { lat: number; lng: number }; name: string } | null>(null)
  const activeDetailPosition = useRef(detailPosition)
  activeDetailPosition.current = detailPosition
  const isCurrentDetailPosition = useCallback((position: { lat: number; lng: number }) =>
    activeDetailPosition.current === position, [])
  // The latest streamed update of the open point (each replaces the previous one); the card opens
  // with a skeleton at the click and keeps the position on an error, so the visitor sees where.
  const [noiseAnswer, setNoiseDetailData] = useState<{ position: Position | null; data: PopupUpdate } | null>(null)
  const noiseDetailData = noiseAnswer?.position === detailPosition ? noiseAnswer?.data ?? null : null
  const [noiseError, setNoiseDetailError] = useState<{ position: Position | null; message: string } | null>(null)
  const noiseDetailError = noiseError?.position === detailPosition ? noiseError?.message ?? null : null
  // What the map highlights: a loudest flight's track (its row hovered, or tapped on a phone), by
  // key, or a contributor's lines (its row opened), by `source:<id>`; read from the latest
  // update, so it follows the stream and goes when it leaves the list.
  const highlighted = typeof state.popup.highlight === 'string' ? state.popup.highlight : null
  const setHighlighted = (key: string | null) => updateUrl(s => {
    const popup = { ...s.popup }; delete popup.highlight
    if (key) popup.highlight = key
    return { popup }
  })
  // An opened row's segments and rays on the map, while its segments are open: the row that drew
  // them last; closing another row's leaves them.
  const [fan, setFan] = useState<SegmentFan | null>(null)
  const showFan = useCallback((source: string, next: SegmentFan | null) =>
    setFan(current => next ?? (current?.source === source ? null : current)), [])
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
  const sourceOutline = useSourceOutline(highlightedSource)
  const highlight = sourceOutline ?? (highlightedSource?.geometry
    ? contributorHighlight(highlightedSource)
    : highlightedTrack ? highlightGeoJson(highlightedTrack) : null)
  // Mobile locate box lives in the BasemapBar row (one container = one
  // baseline); it fires the map's GeolocateControl through this ref.
  const geolocateTrigger = useRef<() => void>(() => {})
  const [geolocateActive, setGeolocateActive] = useState(false)
  const [geolocateReady, setGeolocateReady] = useState(false)
  const registerGeolocateTrigger = useCallback((trigger: () => void) => {
    geolocateTrigger.current = trigger
  }, [])
  const handleLocate = useCallback(() => geolocateTrigger.current(), [])
  const getMapView = useCallback(() => latestState.current, [])

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
  const detailTitle = detailPosition ? titleAt(placeTitles, detailPosition)?.title ?? null : null
  const openEntry = detailPosition && recentPlaces.find(place => samePlace(place, detailPosition))
  // Every opened point joins the history, including pasted addresses and browser Back.
  useEffect(() => {
    const searched = searchedName.current
    searchedName.current = null
    if (detailPosition) countOpening(detailPosition, searched && samePlace(searched.at, detailPosition) ? searched.name : null)
  }, [detailPosition, countOpening])
  // The signed-in visitor's recordings and photos of the open place.
  const [media] = useState(() => createMediaKeeper(startRecording))
  useEffect(() => media.open(isSignedIn(account) ? account.email : null, detailPosition), [media, account, detailPosition])

  const detailTotalLden = noiseDetailData?.total_lden ?? null
  useEffect(() => {
    setDocumentTitle(detailPosition
      ? [detailTitle ?? detailPlaceName, detailTotalLden != null ? `${Math.round(detailTotalLden)} dB` : null]
      : [])
  }, [detailPosition, detailTitle, detailPlaceName, detailTotalLden])

  const handleHeatmapLayersChange = (heatmapLayers: Record<string, boolean>) => updateUrl({ heatmapLayers })
  const handleDataLayersChange = (dataLayers: DataLayerId[]) => updateUrl({ dataLayers })
  const handleStaysChange = (stays: boolean) => { setStaySeed(null); updateUrl({ stays, stayId: null }) }
  const handleStaySearchChange = useCallback((staySearch: StaySearch) => {
    setStaySeed(null)
    updateUrl({ staySearch, stayId: null })
  }, [updateUrl])
  useEffect(() => {
    let day = firstCheckin()
    const roll = () => {
      const today = firstCheckin()
      if (today === day) return
      day = today
      if (latestState.current.staySearch.checkin >= today) return
      setStaySeed(null)
      // A passed date remains the selected date, with no old price or silent date substitution.
      updateUrl(s => ({ staySearch: { ...s.staySearch } }))
    }
    const minute = setInterval(roll, 60_000)
    document.addEventListener('visibilitychange', roll)
    return () => { clearInterval(minute); document.removeEventListener('visibilitychange', roll) }
  }, [updateUrl])
  const handleViewChange = useCallback((view: MapCamera) => updateUrl(view), [updateUrl])
  const handleQuietClustersChange = (quietClusters: boolean) => updateUrl({ quietClusters })
  const handleQuietThresholdChange = (quietThreshold: number) => updateUrl({ quietThreshold })
  const handleMetricChange = (metric: Metric) => updateUrl(s => s.metric === metric ? {} : { metric, quietThreshold: QUIET_THRESHOLD_DEFAULT[metric] })

  // A different point discards its answer and map overlays, including when a pasted link changes it.
  useEffect(() => {
    setFan(null)
  }, [detailPosition])
  const handleDetailPositionChange = useCallback((pos: { lat: number; lng: number } | null) => {
    activeDetailPosition.current = pos
    if (isochronBusy.current) updateUrl({ reach: null })
    setStaySeed(null)
    updateUrl(s => ({ detailPosition: pos, stayId: null, popup: {}, scroll: { ...s.scroll, detail: 0 }, controls: pos ? (s.detailPosition ? s.controls : null) : window.matchMedia(DESKTOP_QUERY).matches ? 'layers' : null,
      sheetExpanded: true, ...(pos ? {} : { calculation: false }) }))
  }, [updateUrl])
  const handleStaySelect = useCallback((stay: Stay) => {
    handleDetailPositionChange({ lat: stay.lat, lng: stay.lng })
    setStaySeed({ stay, search: latestState.current.staySearch })
    updateUrl({ stayId: stay.id })
  }, [handleDetailPositionChange, updateUrl])
  const stays = useMemo(() => (staysOn ? { search: staySearch, maxPrice: stayMaxPrice, onSelect: handleStaySelect } : null),
    [staysOn, staySearch, stayMaxPrice, handleStaySelect])

  const handleDetailData = useCallback((update: PopupUpdate) => {
    setNoiseDetailData({ position: activeDetailPosition.current, data: update })
    setNoiseDetailError(null)
  }, [])

  // An error replaces whatever the point showed: a partial answer followed by an error is
  // incomplete and must not stay on screen as if it were the level.
  const handleDetailError = useCallback((message: string) => {
    setNoiseDetailData(null)
    setNoiseDetailError({ position: activeDetailPosition.current, message })
  }, [])

  const handleNoiseClose = useCallback(() => {
    handleDetailPositionChange(null)
  }, [handleDetailPositionChange])

  const handleBasemapChange = (basemap: BasemapId) => updateUrl({ basemap })
  // The area reachable from the open place: its button at the search field's end, where dev1 had it,
  // its panel under the place's row.
  const isochronButton = (
    <button
      type="button"
      aria-label="Reachable area"
      aria-expanded={isochronOpen}
      title="The area reachable from here"
      className={`${ICON_BUTTON} aria-expanded:bg-accent aria-expanded:text-foreground`}
      onClick={() => updateUrl(s => ({ isochronOpen: !s.isochronOpen }))}
    >
      {/* The place and the rings reached from it (dev1's). */}
      <svg viewBox="0 0 24 24" className="size-5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
        <circle cx="12" cy="12" r="2.5" />
        <circle cx="12" cy="12" r="6.5" strokeDasharray="4 3" />
        <circle cx="12" cy="12" r="10.5" strokeDasharray="4 5" />
      </svg>
    </button>
  )
  // Folded, the panel keeps what it holds: its minutes, a request on its way, what went wrong.
  const isochronPanel = detailPosition && <div hidden={!isochronOpen}><IsochronPanel at={detailPosition} options={state.reachOptions} onChange={reachOptions => updateUrl({ reachOptions })} onShow={showIsochron} busy={isochron.busy} error={isochron.error} /></div>
  const reachName = state.reach && (titleAt(placeTitles, state.reach)?.title
    ?? (sameDetailPosition(state.reach, detailPosition) ? detailPlaceName : null)
    ?? `${state.reach.lat.toFixed(4)}, ${state.reach.lng.toFixed(4)}`)
  const stayControls = { staysOn, onStaysChange: handleStaysChange, staySearch, onStaySearchChange: handleStaySearchChange,
    stayMaxPrice, onStayMaxPriceChange: (stayMaxPrice: number | null) => updateUrl({ stayMaxPrice }),
    reachLabel: state.reach ? `${state.reach.minutes} min from ${reachName}` : null, onClearReach: clearIsochron,
    advanced: state.advanced, onAdvancedChange: (advanced: boolean) => updateUrl({ advanced }) }

  return (
    // The app is the box the fixed controls stand in: a phone browser's 100vh reaches under its
    // toolbar, which put the map's own corner controls (the ⓘ) below the control row.
    <PopupStateContext.Provider value={{ popup: state.popup, scroll: state.scroll, update: updateUrl }}>
    <div className="fixed inset-0 overflow-hidden">
      <SearchBar
        metric={metric}
        onSelect={result => {
          searchedName.current = {
            at: { lat: result.lat, lng: result.lon },
            name: result.secondary ? `${result.display_name}, ${result.secondary}` : result.display_name,
          }
          setSelectedLocation({ lat: result.lat, lon: result.lon, zoom: SEARCH_FLIGHT_ZOOM })
        }}
        getMapView={getMapView}
        history={recentPlaces}
        titles={placeTitles}
        open={detailPosition && {
          at: detailPosition,
          place: openEntry?.place ?? detailPlaceName,
          sone: noiseDetailData?.loudness?.nden_sone ?? openEntry?.sone ?? null,
          lden: (noiseDetailData?.partial ? null : noiseDetailData?.total_lden) ?? openEntry?.lden ?? null,
        }}
        notSaved={notSaved}
        // A place of the history flies in as a search's result does, out, across and in, to at least the
        // search's zoom, and opens on arrival. A named place opens at the point it was named at, not where
        // its entry was first opened up to 30 m away (owner 2026-10-30: a house named, reopened 20 m
        // across the street; the history's move kept the zoom, far out, without the way across).
        onOpenPlace={place => {
          // Opened at once on its own entry: the arrival at its name's point, which may lie within 30 m
          // of a neighbour's entry too, then finds this one first.
          countOpening(place)
          const at = titleAt(placeTitles, place) ?? place
          setSelectedLocation({ lat: at.lat, lon: at.lng, zoom: Math.max(latestState.current.zoom, SEARCH_FLIGHT_ZOOM) })
        }}
        onRename={rename}
        onForget={forgetPlace}
        onSignIn={account === null ? () => setCardTab('account') : undefined}
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
        <AccountButton account={account} onSignOut={signOut} onDeleteAccount={deleteAccount} open={cardTab === 'account' && !desktop} onOpenChange={setAccountOpen} />
      </div>

      {/* UI overlays */}
      <div className="absolute inset-0 z-[1002] pointer-events-none">
        {/* The layers card and the detail card are shrinkable flex items (min-h-0)
            that scroll their own content, so on a short viewport both stay reachable. The column ends
            over the map's corner, whose credits and ⓘ stay in sight with every card open. */}
        <div className="hidden md:flex absolute top-3 right-(--map-gutter) bottom-11 flex-col gap-2 w-(--map-card-column) overflow-hidden">
          <ControlCard
            tab={desktop ? cardTab : null}
            onTabChange={setCardTab}
            account={account}
            onSignOut={signOut}
            onDeleteAccount={deleteAccount}
            quietClustersEnabled={quietClustersEnabled}
            onQuietClustersChange={handleQuietClustersChange}
            metric={metric}
            onMetricChange={handleMetricChange}
            quietThreshold={quietThreshold}
            onQuietThresholdChange={handleQuietThresholdChange}
            heatmapLayers={heatmapLayers}
            onHeatmapLayersChange={handleHeatmapLayersChange}
            dataLayers={dataLayers}
            onDataLayersChange={handleDataLayersChange}
            {...stayControls}
          />
          <DetailCard
            metric={metric}
            noiseData={noiseDetailData}
            position={detailPosition}
            error={noiseDetailError}
            onNoiseClose={handleNoiseClose}
            getShareUrl={getShareUrl}
            onHighlight={setHighlighted}
            calculationOpen={calculationOpen && desktop}
            onCalculationToggle={toggleCalculation}
            onFan={showFan}
            stay={selectedStay}
            stayMessage={stayMessage}
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
        view={state}
        basemap={basemap}
        onViewChange={handleViewChange}
        onDetailData={handleDetailData}
        onDetailPositionChange={handleDetailPositionChange}
        onDetailError={handleDetailError}
        detailPosition={detailPosition}
        highlight={highlight}
        segmentFan={fan}
        isochron={isochron.area}
        fitIsochron={isochron.fit}
        quietClustersEnabled={quietClustersEnabled}
        metric={metric}
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
          onClick={() => { handleNoiseClose(); setLayersOpen(true) }}
          className="fixed bottom-[calc(16px+var(--map-footer-height))] right-[10px] z-[1003] flex h-11 w-11 items-center justify-center rounded-lg bg-background md:hidden"
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
          open={layersOpen && !desktop}
          onClose={() => setLayersOpen(false)}
          quietClustersEnabled={quietClustersEnabled}
          onQuietClustersChange={handleQuietClustersChange}
          metric={metric}
          onMetricChange={handleMetricChange}
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
        metric={metric}
        data={noiseDetailData}
        expanded={state.sheetExpanded}
        onExpandedChange={sheetExpanded => updateUrl({ sheetExpanded })}
        position={detailPosition}
        error={noiseDetailError}
        onClose={handleNoiseClose}
        getShareUrl={getShareUrl}
        onHighlight={setHighlighted}
        calculationOpen={calculationOpen && !desktop}
        onCalculationToggle={toggleCalculation}
        onFan={showFan}
        stay={selectedStay}
        stayMessage={stayMessage}
      />

    </div>
    </PopupStateContext.Provider>
  )
}

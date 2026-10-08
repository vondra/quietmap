// The shareable map state in the URL hash: view, open popup point, basemap, layers, quiet zones,
// places to stay, data layers.
import { useCallback, useRef, useMemo } from 'react'
import { DEFAULT_BASEMAP, type BasemapId } from '../utils/basemaps'
import { resolveInitialView } from '../utils/initial-view'
import { HEATMAP_LAYERS } from '../lib/tile-urls'
import { parseDataLayers, type DataLayerId } from '../lib/data-layers'
// Quiet-zone slider spec — highlight areas whose total Lden is ≤ this.
// Range targets genuinely quiet places: 20 dB (rural quiet) to 45 dB (calm
// suburb). Step 0.5 dB matches the model's native resolution; finer would
// be false precision.
export const QUIET_THRESHOLD_MIN = 20
export const QUIET_THRESHOLD_MAX = 45
export const QUIET_THRESHOLD_DEFAULT = 35
export const QUIET_THRESHOLD_STEP = 0.5

/** The popup point's decimals in the URL: 0.1 m, so a shared link answers at the very point
 *  clicked (at four decimals, 11 m, a click inside a building could open outside it). */
const DETAIL_DECIMALS = 6

/** The point a popup link names, as written into the URL. */
const detailText = (position: { lat: number; lng: number }) =>
  `${position.lat.toFixed(DETAIL_DECIMALS)},${position.lng.toFixed(DETAIL_DECIMALS)}`

/** Whether two popup points are the same point of a link. */
export function sameDetailPosition(a: { lat: number; lng: number } | null, b: { lat: number; lng: number } | null): boolean {
  return a === b || (a != null && b != null && detailText(a) === detailText(b))
}

export interface UrlState {
  lat: number
  lng: number
  zoom: number
  /** True when the #hash carried explicit coordinates (a shared link) —
   *  false means lat/lng/zoom are the browser-language fallback. */
  hasExplicitView: boolean
  quietClusters: boolean
  quietThreshold: number
  detailPosition: { lat: number; lng: number } | null
  basemap: BasemapId
  heatmapLayers: Record<string, boolean>
  /** The data layers switched on (`data=`, none when absent). */
  dataLayers: DataLayerId[]
  /** The places to stay shown (`stay=1`); the stay searched is the visitor's own, never linked. */
  stays: boolean
}

// Default view: every noise layer on (the overlay then fetches the precomputed `total` tile).
const DEFAULT_HEATMAP_LAYERS: Record<string, boolean> = Object.fromEntries(HEATMAP_LAYERS.map(id => [id, true]))

// Quiet-zone threshold, clamped to the slider's range. parseFloat (not parseInt)
// so the 0.5 dB step survives the URL round-trip. A malformed `qt` (NaN) would
// otherwise make `byte > maxByte` always false downstream and paint every
// assessed pixel as quiet.
function parseQuietThreshold(raw: string | null): number {
  const n = raw == null ? NaN : parseFloat(raw)
  return Number.isFinite(n) ? Math.min(QUIET_THRESHOLD_MAX, Math.max(QUIET_THRESHOLD_MIN, n)) : QUIET_THRESHOLD_DEFAULT
}

export function parseHash(): UrlState {
  const hash = window.location.hash.slice(1)
  // First visit (no shared link) — and the missing-coordinate fallback for a
  // partial hash: approximate the visitor's country from browser languages,
  // else whole Europe — see utils/initial-view.ts.
  const view = resolveInitialView()
  if (!hash) {
    return {
      lat: view.lat,
      lng: view.lng,
      zoom: view.zoom,
      hasExplicitView: false,
      quietClusters: false,
      quietThreshold: QUIET_THRESHOLD_DEFAULT,
      detailPosition: null,
      basemap: DEFAULT_BASEMAP,
      heatmapLayers: { ...DEFAULT_HEATMAP_LAYERS },
      dataLayers: [],
      stays: false,
    }
  }

  const params = new URLSearchParams(hash)

  let detailPosition: { lat: number; lng: number } | null = null
  const dParam = params.get('d')
  if (dParam) {
    const parts = dParam.split(',')
    if (parts.length === 2) {
      const dlat = parseFloat(parts[0])
      const dlng = parseFloat(parts[1])
      if (Number.isFinite(dlat) && Number.isFinite(dlng)) {
        detailPosition = { lat: dlat, lng: dlng }
      }
    }
  }

  // `ro` lists exactly the active layers; its absence means the default view
  // (every layer on). An explicit empty `ro=` therefore means "all off".
  const ro = params.get('ro')
  const heatmapLayers: Record<string, boolean> = ro === null
    ? { ...DEFAULT_HEATMAP_LAYERS }
    : Object.fromEntries(HEATMAP_LAYERS.map(id => [id, ro.split(',').includes(id)]))

  const parsedLat = parseFloat(params.get('lat') || '')
  const parsedLng = parseFloat(params.get('lng') || '')
  const parsedZoom = parseFloat(params.get('z') || '')

  return {
    lat: Number.isFinite(parsedLat) ? parsedLat : view.lat,
    lng: Number.isFinite(parsedLng) ? parsedLng : view.lng,
    zoom: Number.isFinite(parsedZoom) ? parsedZoom : view.zoom,
    hasExplicitView: Number.isFinite(parsedLat) && Number.isFinite(parsedLng),
    quietClusters: params.get('qc') === '1',
    quietThreshold: parseQuietThreshold(params.get('qt')),
    detailPosition,
    basemap: (params.get('bm') as BasemapId) || DEFAULT_BASEMAP,
    heatmapLayers,
    dataLayers: parseDataLayers(params.get('data')),
    stays: params.has('stay'),
  }
}

type UrlWrite = Omit<UrlState, 'hasExplicitView'>

function buildHash(state: UrlWrite): string {
  const parts: string[] = [
    `lat=${state.lat.toFixed(4)}`,
    `lng=${state.lng.toFixed(4)}`,
    `z=${state.zoom.toFixed(2)}`,
  ]

  if (state.quietClusters) {
    parts.push('qc=1')
    if (state.quietThreshold !== QUIET_THRESHOLD_DEFAULT) parts.push(`qt=${state.quietThreshold}`)
  }

  if (state.detailPosition) {
    parts.push(`d=${detailText(state.detailPosition)}`)
  }

  if (state.basemap !== DEFAULT_BASEMAP) {
    parts.push(`bm=${state.basemap}`)
  }

  // Omit `ro` for the default view (every layer on) so a bare link opens the
  // default; serialize the exact set otherwise (including the empty "all off" set as `ro=`).
  if (!HEATMAP_LAYERS.every(id => state.heatmapLayers[id])) {
    parts.push(`ro=${HEATMAP_LAYERS.filter(id => state.heatmapLayers[id]).join(',')}`)
  }

  if (state.dataLayers.length) {
    parts.push(`data=${state.dataLayers.join(',')}`)
  }

  if (state.stays) parts.push('stay=1')

  return '#' + parts.join('&')
}

export function useUrlState() {
  const initial = useMemo(() => parseHash(), [])
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const updateUrl = useCallback((state: UrlWrite) => {
    if (timerRef.current) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => {
      window.history.replaceState(null, '', buildHash(state))
    }, 300)
  }, [])

  return { initial, updateUrl }
}

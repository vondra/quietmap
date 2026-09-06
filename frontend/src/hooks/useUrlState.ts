import { useCallback, useRef, useMemo } from 'react'
import { DEFAULT_BASEMAP, type BasemapId } from '../utils/basemaps'
import { HEATMAP_LAYERS } from '../components/HeatmapOverlay'
import { resolveInitialView } from '../utils/initial-view'
// Quiet-zone slider spec — highlight pixels whose total Lden is ≤ this.
// Range targets genuinely quiet places: 20 dB (rural quiet) to 45 dB (calm
// suburb). Step 0.5 dB matches the HM3 raster's native resolution (1 byte =
// 0.5 dB); finer would be false precision.
export const QUIET_THRESHOLD_MIN = 20
export const QUIET_THRESHOLD_MAX = 45
export const QUIET_THRESHOLD_DEFAULT = 35
export const QUIET_THRESHOLD_STEP = 0.5
const ALL_RASTER_OVERLAY_IDS = [
  ...HEATMAP_LAYERS,
  'dem',
  'building-height', // Overture building-height raster — distinct from the `building` noise layer
  'forest',
  'barriers',
]

export interface UrlState {
  lat: number
  lng: number
  zoom: number
  /** True when the #hash carried explicit coordinates (a shared link) —
   *  false means lat/lng/zoom are the language-heuristic fallback and the
   *  server's IP-country guess may improve on them before the map mounts. */
  hasExplicitView: boolean
  quietClusters: boolean
  quietThreshold: number
  detailPosition: { lat: number; lng: number } | null
  basemap: BasemapId
  rasterOverlays: Record<string, boolean>
  /** `val=1` — validation-anchor overlay (owner/QA tool, off by default). */
  validation: boolean
  /** `stay=1|hotel|rental|none` — bookable-stays overlay (off by default). */
  stay: 'all' | 'hotel' | 'rental' | 'none' | null
}

export const EMPTY_RASTER_OVERLAYS: Record<string, boolean> = Object.fromEntries(
  ALL_RASTER_OVERLAY_IDS.map(id => [id, false]),
)

// Default view: all seven noise layers on (advanced rasters off). With every
// layer on, MapView fetches the single precomputed `total` tile rather than
// summing seven — the fast path ~80% of visitors get without touching the UI.
const DEFAULT_RASTER_OVERLAYS: Record<string, boolean> = {
  ...EMPTY_RASTER_OVERLAYS,
  ...Object.fromEntries(HEATMAP_LAYERS.map(id => [id, true])),
}

// Quiet-zone threshold, clamped to the slider's range. parseFloat (not parseInt)
// so the 0.5 dB step survives the URL round-trip. A malformed `qt` (NaN) would
// otherwise make `byte > maxByte` always false downstream and paint every
// non-NO_DATA pixel as quiet.
function parseQuietThreshold(raw: string | null): number {
  const n = raw == null ? NaN : parseFloat(raw)
  return Number.isFinite(n) ? Math.min(QUIET_THRESHOLD_MAX, Math.max(QUIET_THRESHOLD_MIN, n)) : QUIET_THRESHOLD_DEFAULT
}

export function parseHash(): UrlState {
  const hash = window.location.hash.slice(1)
  // First visit (no shared link) — and the missing-coordinate fallback for a
  // partial hash like `#val=1`: approximate the visitor's country from
  // browser languages, else whole Europe — see utils/initial-view.ts.
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
      rasterOverlays: { ...DEFAULT_RASTER_OVERLAYS },
      validation: false,
      stay: null,
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

  // `ro` lists exactly the active overlays; its absence means the default view
  // (all seven layers on). An explicit empty `ro=` therefore means "all off".
  const rasterOverlays: Record<string, boolean> = params.has('ro')
    ? { ...EMPTY_RASTER_OVERLAYS }
    : { ...DEFAULT_RASTER_OVERLAYS }
  if (params.has('ro')) {
    for (const id of params.get('ro')!.split(',')) {
      if (ALL_RASTER_OVERLAY_IDS.includes(id)) rasterOverlays[id] = true
    }
  }

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
    rasterOverlays,
    validation: params.get('val') === '1',
    stay: parseStay(params.get('stay')),
  }
}

function parseStay(raw: string | null): 'all' | 'hotel' | 'rental' | 'none' | null {
  if (raw === '1' || raw === 'all') return 'all'
  if (raw === 'hotel' || raw === 'rental' || raw === 'none') return raw
  return null
}

function buildHash(state: {
  lat: number
  lng: number
  zoom: number
  quietClusters: boolean
  quietThreshold?: number
  detailPosition?: { lat: number; lng: number } | null
  basemap?: BasemapId
  rasterOverlays?: Record<string, boolean>
  validation?: boolean
  stay?: 'all' | 'hotel' | 'rental' | 'none' | null
}): string {
  const parts: string[] = [
    `lat=${state.lat.toFixed(4)}`,
    `lng=${state.lng.toFixed(4)}`,
    `z=${state.zoom.toFixed(2)}`,
  ]

  if (state.quietClusters) {
    parts.push('qc=1')
    if (state.quietThreshold != null && state.quietThreshold !== QUIET_THRESHOLD_DEFAULT) parts.push(`qt=${state.quietThreshold}`)
  }

  if (state.detailPosition) {
    parts.push(`d=${state.detailPosition.lat.toFixed(4)},${state.detailPosition.lng.toFixed(4)}`)
  }

  if (state.validation) {
    parts.push('val=1')
  }

  if (state.stay) {
    parts.push(`stay=${state.stay === 'all' ? '1' : state.stay}`)
  }

  if (state.basemap && state.basemap !== DEFAULT_BASEMAP) {
    parts.push(`bm=${state.basemap}`)
  }

  if (state.rasterOverlays) {
    const active = ALL_RASTER_OVERLAY_IDS.filter(id => state.rasterOverlays![id])
    // Omit `ro` for the default view (all seven layers on, advanced off) so a
    // bare link opens the default; serialize the exact set otherwise (including
    // the empty "all off" set as `ro=`).
    const isDefault = ALL_RASTER_OVERLAY_IDS.every(id => !!state.rasterOverlays![id] === !!DEFAULT_RASTER_OVERLAYS[id])
    if (!isDefault) {
      parts.push(`ro=${active.join(',')}`)
    }
  }

  return '#' + parts.join('&')
}

export function useUrlState() {
  const initial = useMemo(() => parseHash(), [])
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const updateUrl = useCallback((state: {
    lat: number
    lng: number
    zoom: number
    quietClusters: boolean
    quietThreshold?: number
    detailPosition?: { lat: number; lng: number } | null
    basemap?: BasemapId
    rasterOverlays?: Record<string, boolean>
    validation?: boolean
    stay?: 'all' | 'hotel' | 'rental' | 'none' | null
  }) => {
    if (timerRef.current) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => {
      if (window.location.pathname.startsWith('/about')) return
      const hash = buildHash(state)
      window.history.replaceState(null, '', hash)
    }, 300)
  }, [])

  return { initial, updateUrl }
}

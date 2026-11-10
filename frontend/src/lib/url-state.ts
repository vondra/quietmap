// Public map state encoded in the address; account data and time-limited offers never enter it.
import { DEFAULT_BASEMAP, type BasemapId } from '../utils/basemaps'
import { resolveInitialView } from '../utils/initial-view'
import { HEATMAP_LAYERS } from './tile-urls'
import { parseDataLayers, type DataLayerId } from './data-layers'
import type { Metric } from './hm3-decoder'
import { DESKTOP_QUERY } from '../hooks/useIsDesktop'
import { addDays, defaultStaySearch, type StaySearch } from './stays'

export const QUIET_THRESHOLDS: Record<Metric, readonly number[]> = {
  nden: [0.25, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1, 1.2, 1.4, 1.6, 1.8, 2, 2.5, 3, 3.5, 4],
  lden: [20, 25, 30, 35, 40, 45, 50, 55],
}
export const QUIET_THRESHOLD_DEFAULT: Record<Metric, number> = { nden: 1, lden: 35 }
export interface Position { lat: number; lng: number }
export interface CameraPadding { top: number; right: number; bottom: number; left: number }
// Padding is a fraction of canvas height/width, so the perspective survives other screen sizes.
export interface MapCamera extends Position { zoom: number; bearing: number; pitch: number; padding: CameraPadding }
export function pixelPadding(padding: CameraPadding, width: number, height: number): CameraPadding {
  return { top: padding.top * height, right: padding.right * width, bottom: padding.bottom * height, left: padding.left * width }
}
export interface ReachOptions { minutes: number; modes: ('walk' | 'car')[] }
export interface ReachRequest extends Position, ReachOptions {}
export type PopupState = Record<string, boolean | number | string>
export interface UrlState extends MapCamera {
  metric: Metric
  quietClusters: boolean
  quietThreshold: number
  detailPosition: Position | null
  basemap: BasemapId
  heatmapLayers: Record<string, boolean>
  dataLayers: DataLayerId[]
  stays: boolean
  staySearch: StaySearch
  stayMaxPrice: number | null
  stayId: string | null
  controls: 'layers' | 'account' | null
  advanced: boolean
  calculation: boolean
  sheetExpanded: boolean
  isochronOpen: boolean
  reachOptions: ReachOptions
  reach: ReachRequest | null
  popup: PopupState
  scroll: Partial<Record<'layers' | 'detail', number>>
}

// Six decimals keep the clicked point within 0.1 m: four could move it out of its building.
const positionText = (p: Position) => `${p.lat.toFixed(6)},${p.lng.toFixed(6)}`
export function sameDetailPosition(a: Position | null, b: Position | null): boolean {
  return a === b || (a != null && b != null && positionText(a) === positionText(b))
}
function number(raw: string | null | undefined, fallback: number, min: number, max: number): number {
  const n = raw == null || raw.trim() === '' ? NaN : Number(raw)
  return Number.isFinite(n) && n >= min && n <= max ? n : fallback
}
function position(raw: string | null): Position | null {
  const parts = raw?.split(',')
  if (parts?.length !== 2) return null
  const lat = number(parts[0], NaN, -85.051129, 85.051129), lng = number(parts[1], NaN, -Infinity, Infinity)
  return Number.isFinite(lat) && Number.isFinite(lng) ? { lat, lng: lng >= -180 && lng <= 180 ? lng : ((lng + 180) % 360 + 360) % 360 - 180 } : null
}
function date(raw: string | null, fallback: string): string {
  return raw && /^\d{4}-\d\d-\d\d$/.test(raw) && Number.isFinite(Date.parse(raw))
    && new Date(raw).toISOString().slice(0, 10) === raw ? raw : fallback
}
function reachOptions(raw: string | null): ReachOptions {
  const [minutes, ...modes] = (raw ?? '60,walk,car').split(',')
  return { minutes: Math.round(number(minutes, 60, 0, 120)), modes: (['walk', 'car'] as const).filter(mode => modes.includes(mode)) }
}
function popupState(raw: string | null): PopupState {
  try {
    const value: unknown = JSON.parse(raw ?? '{}')
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    return Object.fromEntries(Object.entries(value).filter(([key, item]) => {
      if (key === 'highlight') return typeof item === 'string' && item.length <= 256
      if (/^(source:|segments:)/.test(key) || key === 'aircraft' || key === 'rest') return item === true
      return /^(piece:|ray:)/.test(key) && typeof item === 'number' && Number.isInteger(item) && item >= 0 && item < 100000
    }))
  } catch { return {} }
}

export function parseHash(hash = window.location.hash): UrlState {
  const p = new URLSearchParams(hash.replace(/^#/, '')), view = resolveInitialView()
  const metric: Metric = p.get('m') === 'lden' ? 'lden' : 'nden'
  const threshold = number(p.get('qt'), QUIET_THRESHOLD_DEFAULT[metric], Number.MIN_VALUE, 1000)
  const distance = (t: number) => Math.abs(metric === 'lden' ? t - threshold : Math.log2(t / threshold))
  const detailPosition = position(p.get('d')), dataLayers = parseDataLayers(p.get('data'))
  const defaults = defaultStaySearch()
  const checkin = date(p.get('in'), defaults.checkin), checkout = date(p.get('out'), defaults.checkout)
  const staySearch: StaySearch = {
    kind: p.get('kind') === 'hotel' ? 'hotel' : p.get('kind') === 'rental' ? 'rental' : 'all',
    checkin, checkout: checkout > checkin ? checkout : checkin,
    adults: Math.round(number(p.get('guests'), 2, 1, 99)),
    minStars: [3, 4, 5].includes(Number(p.get('stars'))) ? Number(p.get('stars')) : null,
    minScore: [7, 8, 9].includes(Number(p.get('score'))) ? Number(p.get('score')) : null,
  }
  if (staySearch.checkout <= staySearch.checkin) staySearch.checkout = addDays(staySearch.checkin, 1)
  const ro = p.get('ro'), reachParts = p.get('reach')?.split(',')
  const origin = reachParts && position(reachParts.slice(0, 2).join(','))
  const reached = reachParts && reachOptions(reachParts.slice(2).join(','))
  const padding = (p.get('pad') ?? '').split(',')
  const panel = p.get('panel'), scroll = p.get('scroll')?.split(',') ?? []
  return {
    lat: number(p.get('lat'), view.lat, -85.051129, 85.051129),
    lng: number(p.get('lng'), view.lng, -100000, 100000),
    zoom: Math.max(0, Math.min(16, number(p.get('z'), view.zoom, -Infinity, Infinity))),
    bearing: number(p.get('b'), 0, -180, 180), pitch: number(p.get('p'), 0, 0, 60),
    padding: { top: number(padding[0], 0, 0, 1), right: number(padding[1], 0, 0, 1), bottom: number(padding[2], 0, 0, 1), left: number(padding[3], 0, 0, 1) },
    metric, quietClusters: p.get('qc') === '1',
    quietThreshold: QUIET_THRESHOLDS[metric].reduce((best, t) => distance(t) < distance(best) ? t : best),
    detailPosition,
    basemap: p.get('bm') === 'terrain' ? 'terrain' : p.get('bm') === 'satellite' ? 'satellite' : DEFAULT_BASEMAP,
    heatmapLayers: Object.fromEntries(HEATMAP_LAYERS.map(id => [id, ro === null || ro.split(',').includes(id)])),
    dataLayers, stays: p.has('stay'), staySearch,
    stayMaxPrice: p.has('price') && Number.isFinite(number(p.get('price'), NaN, 0, Number.MAX_SAFE_INTEGER)) ? Number(p.get('price')) : null,
    stayId: detailPosition && p.get('sid') ? p.get('sid')!.slice(0, 256) : null,
    controls: panel === 'layers' || panel === 'account' ? panel : panel === 'none' || detailPosition || !window.matchMedia(DESKTOP_QUERY).matches ? null : 'layers',
    advanced: p.has('adv') ? p.get('adv') === '1' : metric === 'lden' || dataLayers.length > 0,
    calculation: !!detailPosition && p.get('calc') === '1', sheetExpanded: p.get('sheet') !== 'peek',
    isochronOpen: p.get('iso') === '1', reachOptions: reachOptions(p.get('travel')),
    reach: origin && reached && reached.minutes > 0 && reached.modes.length ? { ...origin, ...reached } : null,
    popup: detailPosition ? popupState(p.get('popup')) : {},
    scroll: { layers: number(scroll[0], 0, 0, 1000000), detail: detailPosition ? number(scroll[1], 0, 0, 1000000) : 0 },
  }
}

export function buildHash(s: UrlState): string {
  const parts = [`lat=${s.lat.toFixed(6)}`, `lng=${s.lng.toFixed(6)}`, `z=${s.zoom.toFixed(4)}`]
  const add = (key: string, value: string | number) => parts.push(`${key}=${encodeURIComponent(value)}`)
  if (s.bearing) add('b', s.bearing.toFixed(4))
  if (s.pitch) add('p', s.pitch.toFixed(4))
  if (Object.values(s.padding).some(value => value !== 0)) parts.push(`pad=${[s.padding.top, s.padding.right, s.padding.bottom, s.padding.left].map(value => value.toFixed(6)).join(',')}`)
  if (s.metric === 'lden') add('m', 'lden')
  if (s.quietClusters) add('qc', 1)
  if (s.quietThreshold !== QUIET_THRESHOLD_DEFAULT[s.metric]) add('qt', s.quietThreshold)
  if (s.detailPosition) parts.push(`d=${positionText(s.detailPosition)}`)
  if (s.basemap !== DEFAULT_BASEMAP) add('bm', s.basemap)
  if (!HEATMAP_LAYERS.every(id => s.heatmapLayers[id])) parts.push(`ro=${HEATMAP_LAYERS.filter(id => s.heatmapLayers[id]).join(',')}`)
  if (s.dataLayers.length) parts.push(`data=${s.dataLayers.join(',')}`)
  if (s.stays) add('stay', 1)
  const search = s.staySearch
  // Explicit dates survive midnight and recipient time zones, even for an otherwise default search.
  add('in', search.checkin); add('out', search.checkout)
  if (search.kind !== 'all') add('kind', search.kind)
  if (search.adults !== 2) add('guests', search.adults)
  if (search.minStars != null) add('stars', search.minStars)
  if (search.minScore != null) add('score', search.minScore)
  if (s.stayMaxPrice != null && Number.isFinite(s.stayMaxPrice)) add('price', s.stayMaxPrice)
  if (s.stayId && s.detailPosition) add('sid', s.stayId)
  add('panel', s.controls ?? 'none'); add('adv', s.advanced ? 1 : 0)
  if (s.calculation && s.detailPosition) add('calc', 1)
  if (!s.sheetExpanded) add('sheet', 'peek')
  if (s.isochronOpen) add('iso', 1)
  const travel = (o: ReachOptions) => [o.minutes, ...o.modes].join(',')
  if (travel(s.reachOptions) !== '60,walk,car') parts.push(`travel=${travel(s.reachOptions)}`)
  if (s.reach) parts.push(`reach=${positionText(s.reach)},${travel(s.reach)}`)
  if (s.detailPosition && Object.keys(s.popup).length) add('popup', JSON.stringify(s.popup))
  if (s.scroll.layers || (s.detailPosition && s.scroll.detail)) parts.push(`scroll=${Math.round(s.scroll.layers ?? 0)},${s.detailPosition ? Math.round(s.scroll.detail ?? 0) : 0}`)
  return '#' + parts.join('&')
}

// The places of a visitor who is not signed in, kept in this browser's storage: the recent places and
// the names given places (rules in place-rules.ts), read defensively, as storage may hold anything or
// be blocked. A signed-in visitor's places live on the server instead (hooks/usePlaces.ts).
import { PLACE_TITLES_MAX, RECENT_PLACES_MAX, type PlaceTitle, type RecentPlace } from './place-rules.ts'

const RECENT_PLACES_KEY = 'quietmap.recent-places'
/** The names' storage key: another tab's change of it reaches this one as a `storage` event. */
export const PLACE_TITLES_KEY = 'quietmap.place-titles'

const numberOrNull = (value: unknown) => (typeof value === 'number' && Number.isFinite(value) ? value : null)

/** The stored entries of `key`, or null when the browser keeps no storage. */
function storedList(key: string): Record<string, unknown>[] | null {
  let stored: string | null
  try {
    stored = localStorage.getItem(key)
  } catch {
    return null
  }
  try {
    const parsed: unknown = JSON.parse(stored ?? '[]')
    return Array.isArray(parsed) ? parsed.filter(entry => typeof entry === 'object' && entry !== null) : []
  } catch {
    return []
  }
}

function store(key: string, list: unknown[]): void {
  try {
    localStorage.setItem(key, JSON.stringify(list))
  } catch {
    // A private window or blocked storage keeps the places for this visit only.
  }
}

/** The stored places; none when the browser keeps no storage or holds something else. A place kept
 *  before the history counted was opened once, at no known time. */
export function loadRecentPlaces(): RecentPlace[] {
  return (storedList(RECENT_PLACES_KEY) ?? []).flatMap(entry => {
    const [lat, lng] = [numberOrNull(entry.lat), numberOrNull(entry.lng)]
    if (lat == null || lng == null) return []
    const place = typeof entry.place === 'string' ? entry.place : null
    return [{
      lat, lng, place, sone: numberOrNull(entry.sone), lden: numberOrNull(entry.lden),
      count: Math.max(1, Math.round(numberOrNull(entry.count) ?? 1)), openedAt: numberOrNull(entry.openedAt) ?? 0,
    }]
  }).slice(0, RECENT_PLACES_MAX)
}

export function saveRecentPlaces(list: RecentPlace[]): void {
  store(RECENT_PLACES_KEY, list)
}

/** The stored names (none when storage holds something else), or null when the browser keeps no
 *  storage. */
export function loadPlaceTitles(): PlaceTitle[] | null {
  return storedList(PLACE_TITLES_KEY)?.flatMap(entry => {
    const [lat, lng] = [numberOrNull(entry.lat), numberOrNull(entry.lng)]
    const title = typeof entry.title === 'string' ? entry.title.trim() : ''
    return lat == null || lng == null || !title ? [] : [{ lat, lng, title, updatedAt: numberOrNull(entry.updatedAt) ?? 0 }]
  }).slice(0, PLACE_TITLES_MAX) ?? null
}

export function savePlaceTitles(titles: PlaceTitle[]): void {
  store(PLACE_TITLES_KEY, titles)
}

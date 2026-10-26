// A signed-in visitor's places as the browser sends them, checked: the names and the recent places
// under the rules the map keeps in the browser too (frontend/src/lib/place-rules.ts, this very file
// run here). A longitude from a repeated world copy of the map is wrapped, as the popup wraps it.
import {
  PLACE_TITLES_MAX, RECENT_PLACES_MAX, TITLE_MAX, samePlace, type PlaceTitle, type RecentPlace,
} from '../../../frontend/src/lib/place-rules.ts'
import { wrappedLongitude } from '../routes/popup.ts'

/** The longest geocoded place name kept with a recent place. */
const PLACE_NAME_MAX = 500

const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value)
const entry = (value: unknown) => (typeof value === 'object' && value !== null ? value : {}) as Record<string, unknown>

/** A point as sent, or null. */
export function point({ lat, lng }: Record<string, unknown>): { lat: number, lng: number } | null {
  return finite(lat) && Math.abs(lat) <= 90 && finite(lng) ? { lat, lng: wrappedLongitude(lng) } : null
}

/** A name as sent (null removes it), its time capped at `now` (a missing one is the oldest), or
 *  what is wrong with it. */
export function parseTitle(value: unknown, now: number): { at: { lat: number, lng: number }, title: string | null, updatedAt: number } | string {
  const sent = entry(value)
  const at = point(sent)
  if (!at) return 'lat must be within ±90 and lng a number'
  const title = typeof sent.title === 'string' ? sent.title.trim() : sent.title
  if (title !== null && (typeof title !== 'string' || title.length === 0 || title.length > TITLE_MAX)) {
    return `title must be 1 to ${TITLE_MAX} characters, or null to remove it`
  }
  const { updatedAt } = sent
  return { at, title, updatedAt: finite(updatedAt) && updatedAt > 0 ? Math.min(Math.round(updatedAt), now) : 0 }
}

/** The names a browser sends at its sign-in, or what is wrong with them. */
export function parseTitles(value: unknown, now: number): PlaceTitle[] | string {
  if (!Array.isArray(value) || value.length > PLACE_TITLES_MAX) return `titles must be at most ${PLACE_TITLES_MAX} names`
  const titles: PlaceTitle[] = []
  for (const sent of value) {
    const parsed = parseTitle(sent, now)
    if (typeof parsed === 'string') return parsed
    if (parsed.title != null) titles.push({ ...parsed.at, title: parsed.title, updatedAt: parsed.updatedAt })
  }
  return titles
}

/** The recent places as sent, one per place (the first of two within 30 m), or what is wrong with
 *  them. */
export function parseRecent(value: unknown): RecentPlace[] | string {
  if (!Array.isArray(value) || value.length > RECENT_PLACES_MAX) return `recent must be at most ${RECENT_PLACES_MAX} places`
  const places: RecentPlace[] = []
  for (const sent of value.map(entry)) {
    const at = point(sent)
    if (!at) return 'a recent place needs lat within ±90 and lng a number'
    if (places.some(place => samePlace(place, at))) continue
    const { place, sone, lden } = sent
    places.push({
      ...at,
      place: typeof place === 'string' ? place.slice(0, PLACE_NAME_MAX) : null,
      sone: finite(sone) ? sone : null,
      lden: finite(lden) ? lden : null,
    })
  }
  return places
}
